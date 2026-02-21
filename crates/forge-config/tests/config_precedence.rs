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
            engine: None,
            engine_pre_args: Some(vec![
                "--sandbox".to_string(),
                "danger-full-access".to_string(),
            ]),
            engine_exec_args: None,
            thinking_mode: None,
            max_calls_per_hour: Some(77),
            timeout_minutes: Some(22),
            resume: None,
            resume_last: true,
        },
    )
    .expect("load_run_config");

    assert_eq!(cfg.max_calls_per_hour, 77);
    assert_eq!(cfg.timeout_minutes, 22);
    assert!(cfg
        .engine_pre_args
        .starts_with(&["--sandbox".to_string(), "danger-full-access".to_string()]));
    assert!(cfg
        .engine_pre_args
        .windows(2)
        .any(|w| w == ["--config", "hide_agent_reasoning=false"]));
    assert!(cfg
        .engine_pre_args
        .windows(2)
        .any(|w| w == ["--config", "show_raw_agent_reasoning=false"]));
    assert!(matches!(cfg.resume_mode, ResumeMode::Last));
}

#[test]
fn thinking_mode_raw_adds_raw_reasoning_flags() {
    let dir = tempdir().expect("tempdir");
    let cfg = load_run_config(
        dir.path(),
        &CliOverrides {
            engine: None,
            engine_pre_args: None,
            engine_exec_args: None,
            thinking_mode: Some(forge_config::ThinkingMode::Raw),
            max_calls_per_hour: None,
            timeout_minutes: None,
            resume: None,
            resume_last: false,
        },
    )
    .expect("load_run_config");

    assert_eq!(cfg.thinking_mode, forge_config::ThinkingMode::Raw);
    assert!(cfg
        .engine_pre_args
        .windows(2)
        .any(|w| w == ["--config", "show_raw_agent_reasoning=true"]));
}
