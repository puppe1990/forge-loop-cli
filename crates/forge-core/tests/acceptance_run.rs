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
        format!("engine_cmd = \"{}\"\n", script_path.display()),
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

#[cfg(unix)]
#[test]
fn run_with_fixed_max_loops_resets_counters_per_execution() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().expect("tempdir");
    let script_path = dir.path().join("fake-codex-no-exit.sh");
    fs::write(&script_path, "#!/usr/bin/env bash\necho 'still working'\n").expect("script write");

    let mut perms = fs::metadata(&script_path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).expect("chmod");

    fs::write(
        dir.path().join(".forgerc"),
        format!("engine_cmd = \"{}\"\n", script_path.display()),
    )
    .expect("forgerc write");

    let cfg = load_run_config(dir.path(), &CliOverrides::default()).expect("config");
    let first = run_loop(RunRequest {
        cwd: dir.path().to_path_buf(),
        config: cfg.clone(),
        max_loops: 1,
    })
    .expect("first run");

    assert_eq!(first.reason, ExitReason::MaxLoopsReached);
    assert_eq!(first.status.state, "max_loops_reached");
    assert_eq!(first.status.total_loops_executed, 1);
    assert_eq!(first.status.current_loop, 0);

    let second = run_loop(RunRequest {
        cwd: dir.path().to_path_buf(),
        config: cfg,
        max_loops: 1,
    })
    .expect("second run");

    assert_eq!(second.reason, ExitReason::MaxLoopsReached);
    assert_eq!(second.status.state, "max_loops_reached");
    assert_eq!(second.status.total_loops_executed, 1);
    assert_eq!(second.status.current_loop, 0);
}
