use forge_config::{load_run_config, CliOverrides};
use forge_core::{run_loop, ExitReason, RunRequest};
use std::fs;
use tempfile::tempdir;

#[cfg(unix)]
#[test]
fn run_completes_when_dual_gate_is_satisfied() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().expect("tempdir");
    let script_path = dir.path().join("fake-codex.sh");
    fs::write(
        &script_path,
        "#!/usr/bin/env bash\necho 'STATUS: COMPLETE'\necho 'EXIT_SIGNAL: true'\n",
    )
    .expect("script write");

    let mut perms = fs::metadata(&script_path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).expect("chmod");

    fs::write(
        dir.path().join(".forgerc"),
        format!("codex_cmd = \"{}\"\n", script_path.display()),
    )
    .expect("forgerc write");

    let cfg = load_run_config(dir.path(), &CliOverrides::default()).expect("config");
    let outcome = run_loop(RunRequest {
        cwd: dir.path().to_path_buf(),
        config: cfg,
        max_loops: 3,
    })
    .expect("run_loop");

    assert_eq!(outcome.reason, ExitReason::Completed);
    assert_eq!(outcome.status.state, "completed");
}
