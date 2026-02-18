use assert_cmd::Command;
use predicates::str::contains;
use std::fs;
use tempfile::tempdir;

fn forge_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_forge"))
}

#[test]
fn help_shows_subcommands() {
    let mut cmd = forge_cmd();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(contains("--cwd"))
        .stdout(contains("interactive assistant mode"))
        .stdout(contains("run"))
        .stdout(contains("status"))
        .stdout(contains("monitor"))
        .stdout(contains("sdd"));
}

#[test]
fn run_help_shows_required_flags() {
    let mut cmd = forge_cmd();
    cmd.args(["run", "--help"])
        .assert()
        .success()
        .stdout(contains("--codex-arg"))
        .stdout(contains("--resume"))
        .stdout(contains("--resume-last"))
        .stdout(contains("--max-calls-per-hour"))
        .stdout(contains("--timeout-minutes"));
}

#[test]
fn run_rejects_conflicting_resume_flags() {
    let mut cmd = forge_cmd();
    cmd.args(["run", "--resume", "abc", "--resume-last"])
        .assert()
        .failure();
}

#[test]
fn sdd_list_works_when_empty() {
    let dir = tempdir().expect("tempdir");
    let dir_str = dir.path().to_string_lossy().to_string();
    let mut cmd = forge_cmd();
    cmd.args(["--cwd", &dir_str, "sdd", "list"])
        .assert()
        .success()
        .stdout(contains("no sdds found"));
}

#[test]
fn sdd_load_fails_for_unknown_id() {
    let dir = tempdir().expect("tempdir");
    let dir_str = dir.path().to_string_lossy().to_string();
    let mut cmd = forge_cmd();
    cmd.args(["--cwd", &dir_str, "sdd", "load", "unknown-id"])
        .assert()
        .failure()
        .stderr(contains("sdd id not found"));
}

#[test]
fn sdd_load_activates_snapshot_files() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    let sdd_id = "1234-architecture";
    let sdd_dir = root.join(".forge/sdds").join(sdd_id);
    fs::create_dir_all(&sdd_dir).expect("create sdd dir");
    fs::write(sdd_dir.join("plan.md"), "# plan").expect("write plan");
    fs::write(sdd_dir.join("spec.md"), "# spec").expect("write spec");
    fs::write(sdd_dir.join("acceptance.md"), "# acceptance").expect("write acceptance");
    fs::write(sdd_dir.join("scenarios.md"), "# scenarios").expect("write scenarios");
    fs::write(
        sdd_dir.join("meta.json"),
        r#"{"id":"1234-architecture","project_name":"dashboard","goal":"improve architecture","created_at_epoch":1234}"#,
    )
    .expect("write meta");

    let dir_str = root.to_string_lossy().to_string();
    let mut cmd = forge_cmd();
    cmd.args(["--cwd", &dir_str, "sdd", "load", sdd_id])
        .assert()
        .success()
        .stdout(contains("loaded sdd: 1234-architecture"));

    assert_eq!(
        fs::read_to_string(root.join(".forge/plan.md")).expect("active plan"),
        "# plan"
    );
    assert_eq!(
        fs::read_to_string(root.join("docs/specs/session/spec.md")).expect("active spec"),
        "# spec"
    );
    assert_eq!(
        fs::read_to_string(root.join(".forge/current_sdd")).expect("current sdd marker"),
        "1234-architecture"
    );
}

#[test]
fn sdd_list_json_marks_current_snapshot() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    let sdd_dir = root.join(".forge/sdds/5678-refactor");
    fs::create_dir_all(&sdd_dir).expect("create sdd dir");
    fs::write(sdd_dir.join("plan.md"), "# plan").expect("write plan");
    fs::write(sdd_dir.join("spec.md"), "# spec").expect("write spec");
    fs::write(sdd_dir.join("acceptance.md"), "# acceptance").expect("write acceptance");
    fs::write(sdd_dir.join("scenarios.md"), "# scenarios").expect("write scenarios");
    fs::write(
        sdd_dir.join("meta.json"),
        r#"{"id":"5678-refactor","project_name":"ui","goal":"modularize","created_at_epoch":5678}"#,
    )
    .expect("write meta");
    fs::create_dir_all(root.join(".forge")).expect("create forge dir");
    fs::write(root.join(".forge/current_sdd"), "5678-refactor").expect("write current");

    let dir_str = root.to_string_lossy().to_string();
    let mut cmd = forge_cmd();
    cmd.args(["--cwd", &dir_str, "sdd", "list", "--json"])
        .assert()
        .success()
        .stdout(contains("\"id\": \"5678-refactor\""))
        .stdout(contains("\"current\": true"));
}
