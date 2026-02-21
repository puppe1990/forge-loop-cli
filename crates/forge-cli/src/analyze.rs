use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct AnalyzeOptions {
    pub engine: String,
    pub engine_pre_args: Vec<String>,
    pub engine_exec_args: Vec<String>,
    pub thinking_mode: Option<String>,
    pub timeout_minutes: u64,
}

#[derive(Debug)]
pub struct AnalyzeResult {
    pub modified_files: Vec<String>,
    pub chunks: usize,
    pub chunk_size: usize,
    pub timed_out_chunks: u64,
    pub failed_chunks: u64,
    pub report: String,
    pub latest_path: String,
    pub history_path: String,
}

#[derive(Debug)]
struct EngineExecRun {
    report: String,
    exit_code: Option<i32>,
    timed_out: bool,
}

pub fn list_modified_files(cwd: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["diff", "--name-only"])
        .current_dir(cwd)
        .output()
        .context("failed to list modified files with git")?;
    if !output.status.success() {
        bail!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let files = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    Ok(files)
}

pub fn build_analyze_prompt(files: &[String], scope_label: &str) -> String {
    let mut out = String::from(
        "Analyze ONLY these modified files and report exactly:\n1) Critical risks\n2) High risks\n3) Medium risks\n4) Suggested next actions\nDo not propose edits, only analysis.\nEnd with: EXIT_SIGNAL: true\n\nScope: ",
    );
    out.push_str(scope_label);
    out.push_str("\n\nModified files:\n");
    for file in files {
        out.push_str("- ");
        out.push_str(file);
        out.push('\n');
    }
    out
}

pub fn run_analyze_chunk(
    engine_cmd: &str,
    engine_pre_args: &[String],
    engine_exec_args: &[String],
    cwd: &Path,
    prompt: &str,
    timeout_minutes: u64,
) -> Result<EngineExecRun> {
    let mut args = engine_pre_args.to_vec();
    args.push("exec".to_string());
    args.extend(engine_exec_args.iter().cloned());
    args.push("--json".to_string());
    args.push(prompt.to_string());

    let timeout = Duration::from_secs(timeout_minutes.saturating_mul(60));
    let mut child = Command::new(engine_cmd)
        .args(&args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to execute {}", engine_cmd))?;

    let started = Instant::now();
    let mut timed_out = false;

    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            let _ = child.kill();
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }

    let output = child
        .wait_with_output()
        .with_context(|| format!("failed waiting for {}", engine_cmd))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let report = extract_last_agent_message(&stdout).unwrap_or_else(|| {
        let merged = format!("{} {}", stdout.trim(), stderr.trim());
        merged.chars().take(4000).collect()
    });

    Ok(EngineExecRun {
        report,
        exit_code: output.status.code(),
        timed_out,
    })
}

pub fn persist_analyze_report(
    cwd: &Path,
    files: &[String],
    chunks: usize,
    chunk_size: usize,
    timed_out_chunks: u64,
    failed_chunks: u64,
    chunk_reports: &[String],
    report: &str,
) -> Result<AnalyzePaths> {
    let analyze_dir = cwd.join(".forge").join("analyze");
    let history_dir = analyze_dir.join("history");
    fs::create_dir_all(&history_dir)
        .with_context(|| format!("failed to create {}", history_dir.display()))?;

    let now = epoch_now();
    let payload = serde_json::json!({
        "created_at_epoch": now,
        "modified_files": files.len(),
        "chunks": chunks,
        "chunk_size": chunk_size,
        "timed_out_chunks": timed_out_chunks,
        "failed_chunks": failed_chunks,
        "files": files,
        "chunk_reports": chunk_reports,
        "report": report,
    });

    let latest_path = analyze_dir.join("latest.json");
    fs::write(&latest_path, serde_json::to_string_pretty(&payload)?)
        .with_context(|| format!("failed to write {}", latest_path.display()))?;

    let history_path = history_dir.join(format!("{}.json", now));
    fs::write(&history_path, serde_json::to_string_pretty(&payload)?)
        .with_context(|| format!("failed to write {}", history_path.display()))?;

    Ok(AnalyzePaths {
        latest_path: latest_path.display().to_string(),
        history_path: history_path.display().to_string(),
    })
}

#[derive(Debug)]
pub struct AnalyzePaths {
    pub latest_path: String,
    pub history_path: String,
}

pub fn load_latest_analyze_payload(cwd: &Path) -> Result<Value> {
    let path = cwd.join(".forge").join("analyze").join("latest.json");
    if !path.exists() {
        bail!("latest analyze report not found at {}", path.display());
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid json in {}", path.display()))?;
    Ok(value)
}

fn extract_last_agent_message(stdout: &str) -> Option<String> {
    let mut last = None;
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("item.completed") {
            continue;
        }
        let Some(item) = value.get("item") else {
            continue;
        };
        if item.get("type").and_then(|v| v.as_str()) != Some("agent_message") {
            continue;
        }
        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
            last = Some(text.to_string());
        }
    }
    last
}

fn epoch_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn list_modified_files_returns_empty_when_no_changes() {
        let dir = tempdir().expect("tempdir");

        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .expect("git init");

        let files = list_modified_files(dir.path()).expect("list files");
        assert!(files.is_empty());
    }

    #[test]
    fn build_analyze_prompt_includes_files() {
        let files = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
        let prompt = build_analyze_prompt(&files, "chunk 1/2");

        assert!(prompt.contains("src/main.rs"));
        assert!(prompt.contains("src/lib.rs"));
        assert!(prompt.contains("chunk 1/2"));
        assert!(prompt.contains("EXIT_SIGNAL: true"));
    }

    #[test]
    fn persist_analyze_report_creates_files() {
        let dir = tempdir().expect("tempdir");
        let files = vec!["test.rs".to_string()];
        let chunk_reports = vec!["Report 1".to_string()];

        let paths = persist_analyze_report(
            dir.path(),
            &files,
            1,
            25,
            0,
            0,
            &chunk_reports,
            "Final report",
        )
        .expect("persist");

        assert!(PathBuf::from(&paths.latest_path).exists());
        assert!(PathBuf::from(&paths.history_path).exists());

        let content = fs::read_to_string(PathBuf::from(&paths.latest_path)).expect("read");
        assert!(content.contains("test.rs"));
        assert!(content.contains("Final report"));
    }

    #[test]
    fn load_latest_analyze_payload_fails_when_missing() {
        let dir = tempdir().expect("tempdir");
        let result = load_latest_analyze_payload(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn load_latest_analyze_payload_returns_json() {
        let dir = tempdir().expect("tempdir");
        let analyze_dir = dir.path().join(".forge/analyze");
        fs::create_dir_all(&analyze_dir).expect("create dir");
        fs::write(analyze_dir.join("latest.json"), r#"{"test": "value"}"#).expect("write");

        let payload = load_latest_analyze_payload(dir.path()).expect("load");
        assert_eq!(payload.get("test").and_then(|v| v.as_str()), Some("value"));
    }

    #[test]
    fn extract_last_agent_message_extracts_text() {
        let stdout =
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"Hello world"}}"#;
        let result = extract_last_agent_message(stdout);
        assert_eq!(result, Some("Hello world".to_string()));
    }

    #[test]
    fn extract_last_agent_message_returns_last() {
        let stdout = r#"{"type":"item.completed","item":{"type":"agent_message","text":"First"}}
{"type":"item.completed","item":{"type":"agent_message","text":"Second"}}"#;
        let result = extract_last_agent_message(stdout);
        assert_eq!(result, Some("Second".to_string()));
    }

    #[test]
    fn extract_last_agent_message_ignores_non_agent() {
        let stdout = r#"{"type":"item.completed","item":{"type":"other","text":"Ignored"}}"#;
        let result = extract_last_agent_message(stdout);
        assert!(result.is_none());
    }
}
