use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn forge_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_forge"))
}

fn setup_git_repo(path: &Path) {
    Command::new("git")
        .args(["init"])
        .current_dir(path)
        .assert()
        .success();
    Command::new("git")
        .args(["config", "user.email", "forge@test.local"])
        .current_dir(path)
        .assert()
        .success();
    Command::new("git")
        .args(["config", "user.name", "Forge Test"])
        .current_dir(path)
        .assert()
        .success();
}

fn setup_fake_codex(script: &Path, call_log: &Path) {
    let body = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
last="${{!#}}"
if [[ "$last" == *"Consolidate the following chunk analyses"* ]]; then
  echo "SYNTH" >> "{call_log}"
  msg="SYNTHESIS EXIT_SIGNAL: true"
else
  echo "CHUNK" >> "{call_log}"
  msg="CHUNK EXIT_SIGNAL: true"
fi
printf '{{"type":"item.completed","item":{{"type":"agent_message","text":"%s"}}}}\n' "$msg"
"#,
        call_log = call_log.display()
    );
    fs::write(script, body).expect("write fake codex");
    Command::new("chmod")
        .args(["+x", script.to_string_lossy().as_ref()])
        .assert()
        .success();
}

#[test]
fn analyze_chunking_persists_latest_report() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    setup_git_repo(root);

    fs::write(root.join("a.txt"), "one\n").expect("write a");
    fs::write(root.join("b.txt"), "two\n").expect("write b");
    Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .assert()
        .success();
    Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(root)
        .assert()
        .success();

    fs::write(root.join("a.txt"), "one changed\n").expect("modify a");
    fs::write(root.join("b.txt"), "two changed\n").expect("modify b");

    let fake_codex = root.join("fake-codex.sh");
    let call_log = root.join("calls.log");
    setup_fake_codex(&fake_codex, &call_log);

    let output = forge_cmd()
        .args([
            "--cwd",
            root.to_string_lossy().as_ref(),
            "analyze",
            "--modified-only",
            "--chunk-size",
            "1",
            "--json",
        ])
        .env("FORGE_ENGINE_CMD", fake_codex.to_string_lossy().as_ref())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).expect("valid json output");
    assert_eq!(value.get("chunks").and_then(Value::as_u64), Some(2));
    assert_eq!(
        value.get("timed_out_chunks").and_then(Value::as_u64),
        Some(0)
    );

    let latest_path = root.join(".forge/analyze/latest.json");
    assert!(latest_path.exists());
    let latest: Value =
        serde_json::from_str(&fs::read_to_string(&latest_path).expect("read latest json"))
            .expect("parse latest json");
    assert_eq!(latest.get("chunks").and_then(Value::as_u64), Some(2));
    assert_eq!(
        latest
            .get("chunk_reports")
            .and_then(Value::as_array)
            .map(|a| a.len()),
        Some(2)
    );

    let calls = fs::read_to_string(&call_log).expect("read calls");
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        vec!["CHUNK", "CHUNK", "SYNTH"]
    );
}

#[test]
fn analyze_resume_latest_report_runs_only_synthesis() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    setup_git_repo(root);
    fs::write(root.join("a.txt"), "one\n").expect("write a");
    Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .assert()
        .success();
    Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(root)
        .assert()
        .success();

    let analyze_dir = root.join(".forge/analyze");
    fs::create_dir_all(&analyze_dir).expect("create analyze dir");
    fs::write(
        analyze_dir.join("latest.json"),
        r#"{
  "created_at_epoch": 1,
  "modified_files": 2,
  "chunks": 2,
  "chunk_size": 1,
  "timed_out_chunks": 0,
  "failed_chunks": 0,
  "files": ["a.txt", "b.txt"],
  "chunk_reports": ["chunk-a", "chunk-b"],
  "report": "old"
}"#,
    )
    .expect("write latest");

    let fake_codex = root.join("fake-codex.sh");
    let call_log = root.join("calls.log");
    setup_fake_codex(&fake_codex, &call_log);

    let output = forge_cmd()
        .args([
            "--cwd",
            root.to_string_lossy().as_ref(),
            "analyze",
            "--resume-latest-report",
            "--json",
        ])
        .env("FORGE_ENGINE_CMD", fake_codex.to_string_lossy().as_ref())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(
        value.get("mode").and_then(Value::as_str),
        Some("resume_latest_report")
    );
    assert_eq!(value.get("chunk_reports").and_then(Value::as_u64), Some(2));

    let calls = fs::read_to_string(&call_log).expect("read calls");
    assert_eq!(calls.lines().collect::<Vec<_>>(), vec!["SYNTH"]);
}
