use anyhow::{bail, Context, Result};
use forge_config::{ResumeMode, RunConfig};
use forge_types::{CircuitBreakerState, CircuitState, ProgressSnapshot, RunStatus};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

#[derive(Debug)]
pub struct OutputAnalysis {
    pub exit_signal_true: bool,
    pub completion_indicators: u32,
    pub has_error: bool,
    pub has_progress_hint: bool,
    pub session_id: Option<String>,
}

pub fn run_loop(req: RunRequest) -> Result<RunOutcome> {
    let runtime_dir = req.cwd.join(&req.config.runtime_dir);
    fs::create_dir_all(&runtime_dir)
        .with_context(|| format!("failed to create {}", runtime_dir.display()))?;

    let mut status: RunStatus = read_json_or_default(&runtime_dir.join("status.json"));
    let mut progress: ProgressSnapshot = read_json_or_default(&runtime_dir.join("progress.json"));
    let mut circuit: CircuitBreakerState =
        read_json_or_default(&runtime_dir.join(".circuit_breaker_state"));

    status.state = "running".to_string();
    status.updated_at_epoch = epoch_now();
    status.circuit_state = circuit.state.clone();
    write_json(&runtime_dir.join("status.json"), &status)?;

    let mut loop_count = 0_u64;

    while loop_count < req.max_loops {
        loop_count += 1;

        let rate = check_and_increment_call_count(&runtime_dir, req.config.max_calls_per_hour)?;
        if !rate.allowed {
            status.state = "rate_limited".to_string();
            status.updated_at_epoch = epoch_now();
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

        let (stdout, stderr, exit_ok) = execute_iteration(&req.cwd, &req.config)?;
        append_live_log(&runtime_dir.join("live.log"), &stdout, &stderr)?;

        let analysis = analyze_output(&stdout, &stderr, &req.config.completion_indicators);

        if let Some(session_id) = analysis.session_id.clone() {
            status.session_id = Some(session_id.clone());
            fs::write(runtime_dir.join(".session_id"), session_id)
                .context("failed to write session id")?;
        }

        let has_progress = analysis.has_progress_hint || (exit_ok && (!stdout.trim().is_empty()));
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

        progress.last_summary = summarize_output(&stdout, &stderr);
        progress.updated_at_epoch = epoch_now();

        status.current_loop = loop_count;
        status.total_loops_executed += 1;
        status.exit_signal_seen = analysis.exit_signal_true;
        status.completion_indicators = analysis.completion_indicators;
        status.last_error = if analysis.has_error {
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
            status.state = "completed".to_string();
            status.updated_at_epoch = epoch_now();
            write_json(&runtime_dir.join("status.json"), &status)?;
            return Ok(RunOutcome {
                reason: ExitReason::Completed,
                loops_executed: loop_count,
                status,
            });
        }

        if matches!(circuit.state, CircuitState::Open) {
            status.state = "circuit_open".to_string();
            status.updated_at_epoch = epoch_now();
            write_json(&runtime_dir.join("status.json"), &status)?;
            return Ok(RunOutcome {
                reason: ExitReason::CircuitOpened,
                loops_executed: loop_count,
                status,
            });
        }
    }

    status.state = "max_loops_reached".to_string();
    status.updated_at_epoch = epoch_now();
    write_json(&runtime_dir.join("status.json"), &status)?;

    Ok(RunOutcome {
        reason: ExitReason::MaxLoopsReached,
        loops_executed: loop_count,
        status,
    })
}

fn execute_iteration(cwd: &Path, config: &RunConfig) -> Result<(String, String, bool)> {
    let args = build_exec_args(&config.resume_mode, cwd);
    let _timeout_minutes = config.timeout_minutes;

    let output = Command::new(&config.codex_cmd)
        .args(&args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to execute {}", config.codex_cmd))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((stdout, stderr, output.status.success()))
}

fn build_exec_args(mode: &ResumeMode, cwd: &Path) -> Vec<String> {
    let mut args = match mode {
        ResumeMode::New => vec!["exec".into(), "--json".into()],
        ResumeMode::Explicit(id) => {
            vec!["exec".into(), "resume".into(), id.clone(), "--json".into()]
        }
        ResumeMode::Last => vec![
            "exec".into(),
            "resume".into(),
            "--last".into(),
            "--json".into(),
        ],
    };

    let plan_file = cwd.join(".forge/plan.md");
    if let Ok(plan) = fs::read_to_string(plan_file) {
        let trimmed = plan.trim();
        if !trimmed.is_empty() {
            args.push(trimmed.to_string());
        }
    }

    args
}

pub fn analyze_output(stdout: &str, stderr: &str, indicators: &[String]) -> OutputAnalysis {
    let text = format!("{stdout}\n{stderr}");
    let lowercase = text.to_ascii_lowercase();

    let mut completion_count = 0_u32;
    for item in indicators {
        if text.contains(item) {
            completion_count += 1;
        }
    }

    let exit_signal_true = lowercase.contains("exit_signal: true");
    let has_error = lowercase.contains("\"error\"") || lowercase.contains("error:");
    let has_progress_hint = lowercase.contains("apply_patch")
        || lowercase.contains("updated file")
        || lowercase.contains("wrote")
        || lowercase.contains("created")
        || lowercase.contains("modified");

    let mut session_id = None;
    for line in stdout.lines() {
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            if session_id.is_none() {
                session_id = extract_session_id(&value);
            }
            if completion_count == 0 {
                completion_count = indicators
                    .iter()
                    .filter(|needle| json_contains_string(&value, needle))
                    .count() as u32;
            }
        }
    }

    OutputAnalysis {
        exit_signal_true,
        completion_indicators: completion_count,
        has_error,
        has_progress_hint,
        session_id,
    }
}

fn json_contains_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(s) => s.contains(needle),
        Value::Array(arr) => arr.iter().any(|v| json_contains_string(v, needle)),
        Value::Object(map) => map.values().any(|v| json_contains_string(v, needle)),
        _ => false,
    }
}

fn extract_session_id(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in ["session_id", "thread_id", "conversation_id"] {
                if let Some(Value::String(v)) = map.get(key) {
                    return Some(v.clone());
                }
            }
            map.values().find_map(extract_session_id)
        }
        Value::Array(arr) => arr.iter().find_map(extract_session_id),
        _ => None,
    }
}

fn summarize_output(stdout: &str, stderr: &str) -> String {
    let joined = format!("{} {}", stdout.trim(), stderr.trim());
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        return "no output".to_string();
    }
    trimmed.chars().take(180).collect()
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

struct RateLimitState {
    allowed: bool,
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

fn append_live_log(path: &Path, stdout: &str, stderr: &str) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;

    if !stdout.trim().is_empty() {
        writeln!(file, "[stdout]\n{}", stdout.trim()).context("failed to write stdout log")?;
    }
    if !stderr.trim().is_empty() {
        writeln!(file, "[stderr]\n{}", stderr.trim()).context("failed to write stderr log")?;
    }
    Ok(())
}

fn append_history(path: &Path, line: &str) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("failed to append {}", path.display()))
}

fn epoch_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
}

pub fn read_status(runtime_dir: &Path) -> Result<RunStatus> {
    let path = runtime_dir.join("status.json");
    if !path.exists() {
        bail!("status file not found at {}", path.display());
    }
    let body =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&body).with_context(|| format!("invalid json in {}", path.display()))
}

pub fn read_progress(runtime_dir: &Path) -> ProgressSnapshot {
    read_json_or_default(&runtime_dir.join("progress.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dual_gate_requires_exit_signal_true() {
        let indicators = vec!["STATUS: COMPLETE".to_string()];
        let analysis = analyze_output("STATUS: COMPLETE\nEXIT_SIGNAL: false", "", &indicators);
        assert_eq!(analysis.completion_indicators, 1);
        assert!(!analysis.exit_signal_true);
    }

    #[test]
    fn dual_gate_completes_with_indicator_and_exit_signal() {
        let indicators = vec!["STATUS: COMPLETE".to_string()];
        let analysis = analyze_output("STATUS: COMPLETE\nEXIT_SIGNAL: true", "", &indicators);
        assert_eq!(analysis.completion_indicators, 1);
        assert!(analysis.exit_signal_true);
    }
}
