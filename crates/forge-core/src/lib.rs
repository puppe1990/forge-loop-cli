pub use exit_reason::*;
pub use request_response::*;

pub mod circuit_breaker;
pub mod io;
pub mod prompt;
pub mod rate_limiter;
pub mod status;

pub use circuit_breaker::{CircuitBreaker, CircuitBreakerAction};
pub use io::{
    append_history, append_live_activity, ensure_dir, read_json, read_json_or_default,
    read_lines_reverse, write_json,
};
pub use prompt::{analyze_plan, build_plan_prompt, PlanSummary};
pub use rate_limiter::{RateLimitResult, RateLimitState, RateLimiter};
pub use status::{read_progress, read_status, write_progress, write_status};

use anyhow::{Context, Result};
use forge_engine::{create_engine, epoch_now, execute_with_engine, EngineExecParams};
use forge_types::{CircuitState, ProgressSnapshot, RunStatus};
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

mod exit_reason {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ExitReason {
        Completed,
        CircuitOpened,
        RateLimited,
        MaxLoopsReached,
    }
}

mod request_response {
    use super::ExitReason;
    use forge_config::RunConfig;
    use forge_types::RunStatus;
    use std::path::PathBuf;

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
}

pub fn run_loop(req: RunRequest) -> Result<RunOutcome> {
    let runtime_dir = req.cwd.join(&req.config.runtime_dir);
    ensure_dir(&runtime_dir)?;
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
    let mut circuit = CircuitBreaker::new(req.config.no_progress_limit);

    status.circuit_state = circuit.state.state.clone();
    write_json(&runtime_dir.join("status.json"), &status)?;
    write_json(&runtime_dir.join("progress.json"), &progress)?;
    write_json(&runtime_dir.join(".circuit_breaker_state"), &circuit.state)?;

    let rate_limiter = RateLimiter::new(req.config.max_calls_per_hour);
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

        let rate = rate_limiter.check_and_increment(&runtime_dir, epoch_now())?;
        if !rate.allowed {
            finalize_run_status(&mut status, "rate_limited");
            write_json(&runtime_dir.join("status.json"), &status)?;
            if req.config.auto_wait_on_rate_limit {
                std::thread::sleep(std::time::Duration::from_secs(
                    req.config.sleep_on_rate_limit_secs,
                ));
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

        // Early completion check before mutating circuit state
        let completed_condition_early = analysis.exit_signal_true
            && (analysis.completion_indicators > 0
                || result
                    .stdout
                    .to_ascii_lowercase()
                    .contains("status: complete")
                || result.stdout.to_ascii_lowercase().contains("task_complete"));
        if completed_condition_early {
            finalize_run_status(&mut status, "completed");
            write_json(&runtime_dir.join("status.json"), &status)?;
            return Ok(RunOutcome {
                reason: ExitReason::Completed,
                loops_executed: loop_count,
                status,
            });
        }

        let circuit_action = if has_progress {
            circuit.record_progress()
        } else {
            circuit.record_no_progress()
        };

        if has_progress {
            progress.loops_with_progress += 1;
        } else {
            progress.loops_without_progress += 1;
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
        status.circuit_state = circuit.state.state.clone();
        status.updated_at_epoch = epoch_now();

        write_json(&runtime_dir.join("progress.json"), &progress)?;
        write_json(&runtime_dir.join("status.json"), &status)?;
        write_json(&runtime_dir.join(".circuit_breaker_state"), &circuit.state)?;
        append_history(
            &runtime_dir.join(".circuit_breaker_history"),
            &format!(
                "{} loop={} state={:?} no_progress={}\n",
                epoch_now(),
                loop_count,
                circuit.state.state,
                circuit.consecutive_no_progress()
            ),
        )?;

        // Consider completed when EXIT_SIGNAL is true and we have explicit completion indicators
        // or when the engine outputs a clear completion marker like "STATUS: COMPLETE".
        let completed_condition = analysis.exit_signal_true
            && (analysis.completion_indicators > 0
                || result
                    .stdout
                    .to_ascii_lowercase()
                    .contains("status: complete")
                || result.stdout.to_ascii_lowercase().contains("task_complete"));
        if completed_condition {
            finalize_run_status(&mut status, "completed");
            write_json(&runtime_dir.join("status.json"), &status)?;
            return Ok(RunOutcome {
                reason: ExitReason::Completed,
                loops_executed: loop_count,
                status,
            });
        }

        if circuit_action == CircuitBreakerAction::OpenCircuit {
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

fn summarize_output(stdout: &str, stderr: &str) -> String {
    let joined = format!("{} {}", stdout.trim(), stderr.trim());
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        return "no output".to_string();
    }
    trimmed.chars().take(180).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_output_combines_stdout_stderr() {
        let result = summarize_output("hello", "world");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn summarize_output_truncates_long_output() {
        let long = "x".repeat(300);
        let result = summarize_output(&long, "");
        assert_eq!(result.len(), 180);
    }

    #[test]
    fn summarize_output_returns_no_output_for_empty() {
        let result = summarize_output("", "");
        assert_eq!(result, "no output");
    }

    #[test]
    fn finalize_run_status_sets_state() {
        let mut status = RunStatus {
            state: "running".to_string(),
            current_loop: 5,
            current_loop_started_at_epoch: 1000,
            last_heartbeat_at_epoch: 1100,
            ..RunStatus::default()
        };

        finalize_run_status(&mut status, "completed");

        assert_eq!(status.state, "completed");
        assert_eq!(status.current_loop, 0);
        assert_eq!(status.current_loop_started_at_epoch, 0);
        assert_eq!(status.last_heartbeat_at_epoch, 0);
    }
}
