use forge_config::{load_run_config, CliOverrides, ResumeMode};
use std::fs;
use tempfile::tempdir;

#[test]
fn cli_overrides_file_values() {
    let dir = tempdir().expect("tempdir");
    fs::write(
        dir.path().join(".forgerc"),
        "max_calls_per_hour = 10\ntimeout_minutes = 2\n",
    )
    .expect("forgerc write");

    let cfg = load_run_config(
        dir.path(),
        &CliOverrides {
            codex_pre_args: Some(vec![
                "--sandbox".to_string(),
                "danger-full-access".to_string(),
            ]),
            codex_exec_args: None,
            max_calls_per_hour: Some(77),
            timeout_minutes: Some(22),
            resume: None,
            resume_last: true,
        },
    )
    .expect("load_run_config");

    assert_eq!(cfg.max_calls_per_hour, 77);
    assert_eq!(cfg.timeout_minutes, 22);
    assert_eq!(
        cfg.codex_pre_args,
        vec!["--sandbox".to_string(), "danger-full-access".to_string()]
    );
    assert!(matches!(cfg.resume_mode, ResumeMode::Last));
}
