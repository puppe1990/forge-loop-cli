use assert_cmd::Command;
use predicates::str::contains;

fn forge_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_forge"))
}

#[test]
fn help_shows_subcommands() {
    let mut cmd = forge_cmd();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(contains("interactive assistant mode"))
        .stdout(contains("run"))
        .stdout(contains("status"))
        .stdout(contains("monitor"));
}

#[test]
fn run_help_shows_required_flags() {
    let mut cmd = forge_cmd();
    cmd.args(["run", "--help"])
        .assert()
        .success()
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
