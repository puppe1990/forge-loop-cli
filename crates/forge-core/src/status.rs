use anyhow::{bail, Context, Result};
use forge_types::{ProgressSnapshot, RunStatus};
use serde::de::DeserializeOwned;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

pub fn read_progress(runtime_dir: &Path) -> ProgressSnapshot {
    read_json_or_default(&runtime_dir.join("progress.json"))
}

pub fn write_status(runtime_dir: &Path, status: &RunStatus) -> Result<()> {
    write_json(&runtime_dir.join("status.json"), status)
}

pub fn write_progress(runtime_dir: &Path, progress: &ProgressSnapshot) -> Result<()> {
    write_json(&runtime_dir.join("progress.json"), progress)
}

pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let body = serde_json::to_string_pretty(value).context("failed to serialize json")?;
    fs::write(path, body).with_context(|| format!("failed to write {}", path.display()))
}

pub fn read_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => T::default(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use forge_types::CircuitState;
    use tempfile::tempdir;

    fn make_status(state: &str) -> RunStatus {
        RunStatus {
            state: state.to_string(),
            thinking_mode: "summary".to_string(),
            run_started_at_epoch: 1000,
            current_loop: 1,
            total_loops_executed: 5,
            last_error: None,
            completion_indicators: 0,
            exit_signal_seen: false,
            session_id: Some("test-session".to_string()),
            circuit_state: CircuitState::Closed,
            current_loop_started_at_epoch: 1100,
            last_heartbeat_at_epoch: 1150,
            updated_at_epoch: 1200,
        }
    }

    #[test]
    #[allow(clippy::zombie_processes, unused_mut)]
    fn write_and_read_status() {
        let dir = tempdir().expect("tempdir");
        let status = make_status("idle"); // Use "idle" to avoid stale check

        write_status(dir.path(), &status).expect("write");

        let read = read_status(dir.path()).expect("read");
        assert_eq!(read.state, "idle");
        assert_eq!(read.current_loop, 1);
        assert_eq!(read.session_id, Some("test-session".to_string()));
    }

    #[test]
    fn read_status_fails_when_missing() {
        let dir = tempdir().expect("tempdir");
        let result = read_status(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn read_json_or_default_returns_default_when_missing() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("nonexistent.json");

        let result: ProgressSnapshot = read_json_or_default(&path);
        assert_eq!(result.loops_with_progress, 0);
        assert_eq!(result.loops_without_progress, 0);
    }

    #[test]
    fn read_json_or_default_returns_default_on_invalid_json() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("invalid.json");
        fs::write(&path, "not valid json").expect("write");

        let result: ProgressSnapshot = read_json_or_default(&path);
        assert_eq!(result.loops_with_progress, 0);
    }

    #[test]
    fn write_and_read_progress() {
        let dir = tempdir().expect("tempdir");
        let progress = ProgressSnapshot {
            loops_with_progress: 10,
            loops_without_progress: 2,
            last_summary: "completed task".to_string(),
            updated_at_epoch: 5000,
        };

        write_progress(dir.path(), &progress).expect("write");

        let read = read_progress(dir.path());
        assert_eq!(read.loops_with_progress, 10);
        assert_eq!(read.loops_without_progress, 2);
        assert_eq!(read.last_summary, "completed task");
    }
}
