use anyhow::{bail, Context, Result};
use chrono::Local;
use forge_config::RunConfig;
use forge_engine::{create_engine, epoch_now, execute_with_engine, EngineExecParams};
use forge_types::{CircuitBreakerState, CircuitState, ProgressSnapshot, RunStatus};
use serde::de::DeserializeOwned;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Completed,
    CircuitOpened,
    RateLimited,
    MaxLoopsReached,
}

#[derive(Debug)]
pub struct RunRequest {
    pub cwd: PathBuf,
    pub config: RunConfig,
    pub max_loops: u64,
}

#[derive(Debug)]
pub struct RunOutcome {
    pub reason: ExitReason,
    pub loops_executed: u64,
    pub status: RunStatus,
}

pub fn run_loop(req: RunRequest) -> Result<RunOutcome> {
    let runtime_dir = req.cwd.join(&req.config.runtime_dir);
    fs::create_dir_all(&runtime_dir)
        .with_context(|| format!("failed to create {}", runtime_dir.display()))?;
    let _runner_pid_guard = RunnerPidGuard::create(&runtime_dir)?;

    let engine = create_engine(req.config.engine);
    let previous_status: RunStatus = read_json_or_default(&runtime_dir.join("status.json"));
    let mut status = RunStatus {
        state: "running".to_string(),
        thinking_mode: req.config.thinking_mode.as_str().to_string(),
        run_started_at_epoch: epoch_now(),
        current_loop: 0,
        total_loops_executed: 0,
        last_error: None,
        completion_indicators: 0,
        exit_signal_seen: false,
        session_id: previous_status.session_id,
        circuit_state: CircuitState::Closed,
        current_loop_started_at_epoch: 0,
        last_heartbeat_at_epoch: 0,
        updated_at_epoch: epoch_now(),
    };
    let mut progress = ProgressSnapshot {
        updated_at_epoch: epoch_now(),
        ..ProgressSnapshot::default()
    };
    let mut circuit = CircuitBreakerState::default();

    status.circuit_state = circuit.state.clone();
    write_json(&runtime_dir.join("status.json"), &status)?;
    write_json(&runtime_dir.join("progress.json"), &progress)?;
    write_json(&runtime_dir.join(".circuit_breaker_state"), &circuit)?;

    let mut loop_count = 0_u64;

    while loop_count < req.max_loops {
        loop_count += 1;
        status.current_loop = loop_count;
        status.current_loop_started_at_epoch = epoch_now();
        status.updated_at_epoch = epoch_now();
        write_json(&runtime_dir.join("status.json"), &status)?;
        progress.last_summary = format!("loop {} started: invoking {}", loop_count, engine.name());
        progress.updated_at_epoch = epoch_now();
        write_json(&runtime_dir.join("progress.json"), &progress)?;
        append_live_activity(
            &runtime_dir.join("live.log"),
            &format!("loop {}: {} exec started", loop_count, engine.name()),
        )?;

        let rate = check_and_increment_call_count(&runtime_dir, req.config.max_calls_per_hour)?;
        if !rate.allowed {
            finalize_run_status(&mut status, "rate_limited");
            write_json(&runtime_dir.join("status.json"), &status)?;
            if req.config.auto_wait_on_rate_limit {
                std::thread::sleep(Duration::from_secs(req.config.sleep_on_rate_limit_secs));
                continue;
            }
            return Ok(RunOutcome {
                reason: ExitReason::RateLimited,
                loops_executed: loop_count,
                status,
            });
        }

        status.last_heartbeat_at_epoch = epoch_now();
        write_json(&runtime_dir.join("status.json"), &status)?;

        let prompt = build_plan_prompt(&req.cwd);
        let result = execute_with_engine(
            engine.as_ref(),
            EngineExecParams {
                cwd: &req.cwd,
                config: &req.config,
                prompt,
                live_log_path: &runtime_dir.join("live.log"),
            },
            || {
                status.last_heartbeat_at_epoch = epoch_now();
                status.updated_at_epoch = epoch_now();
                write_json(&runtime_dir.join("status.json"), &status)
            },
        )?;

        let end_state = if result.timed_out {
            "timed_out"
        } else if result.exit_ok {
            "completed"
        } else {
            "failed"
        };
        append_live_activity(
            &runtime_dir.join("live.log"),
            &format!("loop {}: {} exec {}", loop_count, engine.name(), end_state),
        )?;

        let analysis = engine.parse_output(
            &result.stdout,
            &result.stderr,
            &req.config.completion_indicators,
        );

        if let Some(session_id) = analysis.session_id.clone() {
            status.session_id = Some(session_id.clone());
            fs::write(runtime_dir.join(".session_id"), session_id)
                .context("failed to write session id")?;
        }

        let has_progress =
            analysis.has_progress_hint || (result.exit_ok && (!result.stdout.trim().is_empty()));
        if has_progress {
            progress.loops_with_progress += 1;
            circuit.consecutive_no_progress = 0;
            circuit.state = CircuitState::Closed;
        } else {
            progress.loops_without_progress += 1;
            circuit.consecutive_no_progress += 1;
            circuit.state = if circuit.consecutive_no_progress >= req.config.no_progress_limit {
                CircuitState::Open
            } else {
                CircuitState::HalfOpen
            };
        }

        progress.last_summary = summarize_output(&result.stdout, &result.stderr);
        progress.updated_at_epoch = epoch_now();

        status.total_loops_executed += 1;
        status.exit_signal_seen = analysis.exit_signal_true;
        status.completion_indicators = analysis.completion_indicators;
        status.last_error = if result.timed_out {
            Some("iteration timed out".to_string())
        } else if analysis.has_error {
            Some("error marker found in output".to_string())
        } else {
            None
        };
        status.circuit_state = circuit.state.clone();
        status.updated_at_epoch = epoch_now();

        write_json(&runtime_dir.join("progress.json"), &progress)?;
        write_json(&runtime_dir.join("status.json"), &status)?;
        write_json(&runtime_dir.join(".circuit_breaker_state"), &circuit)?;
        append_history(
            &runtime_dir.join(".circuit_breaker_history"),
            &format!(
                "{} loop={} state={:?} no_progress={}\n",
                epoch_now(),
                loop_count,
                circuit.state,
                circuit.consecutive_no_progress
            ),
        )?;

        if analysis.exit_signal_true && analysis.completion_indicators > 0 {
            finalize_run_status(&mut status, "completed");
            write_json(&runtime_dir.join("status.json"), &status)?;
            return Ok(RunOutcome {
                reason: ExitReason::Completed,
                loops_executed: loop_count,
                status,
            });
        }

        if matches!(circuit.state, CircuitState::Open) {
            finalize_run_status(&mut status, "circuit_open");
            write_json(&runtime_dir.join("status.json"), &status)?;
            return Ok(RunOutcome {
                reason: ExitReason::CircuitOpened,
                loops_executed: loop_count,
                status,
            });
        }
    }

    finalize_run_status(&mut status, "max_loops_reached");
    write_json(&runtime_dir.join("status.json"), &status)?;

    Ok(RunOutcome {
        reason: ExitReason::MaxLoopsReached,
        loops_executed: loop_count,
        status,
    })
}

struct RunnerPidGuard {
    path: PathBuf,
}

impl RunnerPidGuard {
    fn create(runtime_dir: &Path) -> Result<Self> {
        let path = runtime_dir.join(".runner_pid");
        fs::write(&path, process::id().to_string())
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(Self { path })
    }
}

impl Drop for RunnerPidGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn finalize_run_status(status: &mut RunStatus, state: &str) {
    status.state = state.to_string();
    status.current_loop = 0;
    status.current_loop_started_at_epoch = 0;
    status.last_heartbeat_at_epoch = 0;
    status.updated_at_epoch = epoch_now();
}

fn build_plan_prompt(cwd: &Path) -> Option<String> {
    let plan_file = cwd.join(".forge/plan.md");
    let plan = fs::read_to_string(plan_file).ok()?;
    let trimmed = plan.trim();
    if trimmed.is_empty() {
        return None;
    }

    let unchecked: Vec<String> = trimmed
        .lines()
        .filter(|line| line.contains("- [ ]"))
        .map(|line| line.trim().to_string())
        .take(80)
        .collect();

    let pending_block = if unchecked.is_empty() {
        "No explicit unchecked checklist items found; continue from current repo state and finalize remaining plan work.".to_string()
    } else {
        format!(
            "Unchecked checklist items (execute only what is still pending):\n{}",
            unchecked.join("\n")
        )
    };

    let last_summary =
        read_json_or_default::<ProgressSnapshot>(&cwd.join(".forge/progress.json")).last_summary;
    let continuity = if last_summary.trim().is_empty() {
        "Last loop summary: (none)".to_string()
    } else {
        format!("Last loop summary: {}", last_summary.trim())
    };

    Some(format!(
        "You are continuing an iterative execution loop.\n\
Continue from current workspace state. Do NOT redo completed checklist items.\n\
Avoid broad scans like `rg --files`; inspect only files needed for the current pending task.\n\
Apply small, verifiable steps and run only targeted validations per step.\n\
Emit `EXIT_SIGNAL: true` only when all pending checklist items are complete.\n\n\
{continuity}\n\n\
{pending_block}\n\n\
Plan source: .forge/plan.md"
    ))
}

fn summarize_output(stdout: &str, stderr: &str) -> String {
    let joined = format!("{} {}", stdout.trim(), stderr.trim());
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        return "no output".to_string();
    }
    trimmed.chars().take(180).collect()
}

struct RateLimitState {
    allowed: bool,
}

fn check_and_increment_call_count(runtime_dir: &Path, max_calls: u32) -> Result<RateLimitState> {
    let now = epoch_now();
    let count_path = runtime_dir.join(".call_count");
    let reset_path = runtime_dir.join(".last_reset");

    let mut count = fs::read_to_string(&count_path)
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(0);

    let mut last_reset = fs::read_to_string(&reset_path)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(now);

    if now.saturating_sub(last_reset) >= 3600 {
        count = 0;
        last_reset = now;
    }

    if count >= max_calls {
        fs::write(&count_path, count.to_string()).context("failed to persist call count")?;
        fs::write(&reset_path, last_reset.to_string()).context("failed to persist reset time")?;
        return Ok(RateLimitState { allowed: false });
    }

    count += 1;
    fs::write(&count_path, count.to_string()).context("failed to persist call count")?;
    fs::write(&reset_path, last_reset.to_string()).context("failed to persist reset time")?;

    Ok(RateLimitState { allowed: true })
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let body = serde_json::to_string_pretty(value).context("failed to serialize json")?;
    fs::write(path, body).with_context(|| format!("failed to write {}", path.display()))
}

fn read_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => T::default(),
    }
}

fn append_live_activity(path: &Path, text: &str) -> Result<()> {
    let payload = serde_json::json!({
        "item": {
            "type": "agent_message",
            "text": text,
        }
    });
    append_history(path, &format!("{}\n", serde_json::to_string(&payload)?))
}

fn append_history(path: &Path, line: &str) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let stamped = stamp_lines(line);
    file.write_all(stamped.as_bytes())
        .with_context(|| format!("failed to append {}", path.display()))
}

fn stamp_lines(input: &str) -> String {
    let ts = Local::now().format("%H:%M:%S").to_string();
    let mut out = String::new();
    for segment in input.split_inclusive('\n') {
        let has_newline = segment.ends_with('\n');
        let content = segment.trim_end_matches('\n');
        if content.is_empty() {
            continue;
        }
        out.push_str(&format!("[{}] {}", ts, content));
        if has_newline {
            out.push('\n');
        }
    }
    if out.is_empty() && !input.trim().is_empty() {
        out.push_str(&format!("[{}] {}", ts, input.trim()));
    }
    out
}

pub fn read_status(runtime_dir: &Path) -> Result<RunStatus> {
    let path = runtime_dir.join("status.json");
    if !path.exists() {
        bail!("status file not found at {}", path.display());
    }
    let body =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut status: RunStatus = serde_json::from_str(&body)
        .with_context(|| format!("invalid json in {}", path.display()))?;
    if is_stale_running_status(runtime_dir, &status) {
        status.state = "stale_runner".to_string();
        status.current_loop = 0;
        status.current_loop_started_at_epoch = 0;
        status.last_heartbeat_at_epoch = 0;
        if status.last_error.is_none() {
            status.last_error = Some("runner process not found".to_string());
        }
        status.updated_at_epoch = epoch_now();
        let _ = write_json(&path, &status);
        let _ = fs::remove_file(runtime_dir.join(".runner_pid"));
    }
    Ok(status)
}

fn is_stale_running_status(runtime_dir: &Path, status: &RunStatus) -> bool {
    if status.state != "running" {
        return false;
    }
    let pid_path = runtime_dir.join(".runner_pid");
    let Ok(raw_pid) = fs::read_to_string(pid_path) else {
        return true;
    };
    let Ok(pid) = raw_pid.trim().parse::<i32>() else {
        return true;
    };
    if pid <= 0 {
        return true;
    }
    !is_pid_alive(pid)
}

#[cfg(unix)]
fn is_pid_alive(pid: i32) -> bool {
    unsafe {
        let rc = libc::kill(pid, 0);
        if rc == 0 {
            true
        } else {
            std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
        }
    }
}

#[cfg(not(unix))]
fn is_pid_alive(_pid: i32) -> bool {
    true
}

pub fn read_progress(runtime_dir: &Path) -> ProgressSnapshot {
    read_json_or_default(&runtime_dir.join("progress.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn dual_gate_requires_exit_signal_true() {
        use forge_engine::Engine;
        let engine = forge_engine::CodexEngine;
        let indicators = vec!["STATUS: COMPLETE".to_string()];
        let analysis = engine.parse_output("STATUS: COMPLETE\nEXIT_SIGNAL: false", "", &indicators);
        assert_eq!(analysis.completion_indicators, 1);
        assert!(!analysis.exit_signal_true);
    }

    #[test]
    fn dual_gate_completes_with_indicator_and_exit_signal() {
        use forge_engine::Engine;
        let engine = forge_engine::CodexEngine;
        let indicators = vec!["STATUS: COMPLETE".to_string()];
        let analysis = engine.parse_output("STATUS: COMPLETE\nEXIT_SIGNAL: true", "", &indicators);
        assert_eq!(analysis.completion_indicators, 1);
        assert!(analysis.exit_signal_true);
    }

    #[test]
    fn build_plan_prompt_includes_unchecked_items() {
        let dir = tempdir().expect("tempdir");
        let forge_dir = dir.path().join(".forge");
        fs::create_dir_all(&forge_dir).expect("create .forge");
        fs::write(
            forge_dir.join("plan.md"),
            "# Plan\n- [ ] Task A\n- [x] Task B\n",
        )
        .expect("write plan");
        fs::write(
            forge_dir.join("progress.json"),
            r#"{"last_summary":"finished task B"}"#,
        )
        .expect("write progress");

        let prompt = build_plan_prompt(dir.path()).expect("prompt");

        assert!(prompt.contains("continuing an iterative execution loop"));
        assert!(prompt.contains("Do NOT redo completed checklist items"));
        assert!(prompt.contains("Task A"));
        assert!(!prompt.contains("Task B"));
        assert!(prompt.contains("finished task B"));
    }

    #[test]
    fn build_plan_prompt_ignores_empty_plan_file() {
        let dir = tempdir().expect("tempdir");
        let forge_dir = dir.path().join(".forge");
        fs::create_dir_all(&forge_dir).expect("create .forge");
        fs::write(forge_dir.join("plan.md"), "   \n").expect("write empty plan");

        let prompt = build_plan_prompt(dir.path());

        assert!(prompt.is_none());
    }

    #[test]
    fn read_status_marks_stale_runner_when_pid_missing() {
        let dir = tempdir().expect("tempdir");
        let runtime = dir.path().join(".forge");
        fs::create_dir_all(&runtime).expect("create runtime");

        let status = RunStatus {
            state: "running".to_string(),
            current_loop: 1,
            current_loop_started_at_epoch: 10,
            last_heartbeat_at_epoch: 10,
            ..RunStatus::default()
        };
        write_json(&runtime.join("status.json"), &status).expect("write status");

        let observed = read_status(&runtime).expect("read status");
        assert_eq!(observed.state, "stale_runner");
        assert_eq!(observed.current_loop, 0);
        assert_eq!(observed.current_loop_started_at_epoch, 0);
    }
}
