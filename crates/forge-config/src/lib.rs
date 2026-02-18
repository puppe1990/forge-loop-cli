use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeMode {
    New,
    Explicit(String),
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingMode {
    Off,
    Summary,
    Raw,
}

impl ThinkingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ThinkingMode::Off => "off",
            ThinkingMode::Summary => "summary",
            ThinkingMode::Raw => "raw",
        }
    }

    pub fn codex_config_args(self) -> Vec<String> {
        match self {
            ThinkingMode::Off => vec![
                "--config".to_string(),
                "hide_agent_reasoning=true".to_string(),
                "--config".to_string(),
                "show_raw_agent_reasoning=false".to_string(),
                "--config".to_string(),
                "model_reasoning_summary=\"none\"".to_string(),
            ],
            ThinkingMode::Summary => vec![
                "--config".to_string(),
                "hide_agent_reasoning=false".to_string(),
                "--config".to_string(),
                "show_raw_agent_reasoning=false".to_string(),
                "--config".to_string(),
                "model_reasoning_summary=\"concise\"".to_string(),
            ],
            ThinkingMode::Raw => vec![
                "--config".to_string(),
                "hide_agent_reasoning=false".to_string(),
                "--config".to_string(),
                "show_raw_agent_reasoning=true".to_string(),
                "--config".to_string(),
                "model_reasoning_summary=\"detailed\"".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub codex_cmd: String,
    pub codex_pre_args: Vec<String>,
    pub codex_exec_args: Vec<String>,
    pub thinking_mode: ThinkingMode,
    pub max_calls_per_hour: u32,
    pub timeout_minutes: u64,
    pub runtime_dir: PathBuf,
    pub completion_indicators: Vec<String>,
    pub auto_wait_on_rate_limit: bool,
    pub sleep_on_rate_limit_secs: u64,
    pub no_progress_limit: u32,
    pub resume_mode: ResumeMode,
}

#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub codex_pre_args: Option<Vec<String>>,
    pub codex_exec_args: Option<Vec<String>>,
    pub thinking_mode: Option<ThinkingMode>,
    pub max_calls_per_hour: Option<u32>,
    pub timeout_minutes: Option<u64>,
    pub resume: Option<String>,
    pub resume_last: bool,
}

#[derive(Debug, Deserialize, Default)]
struct Forgerc {
    codex_cmd: Option<String>,
    codex_pre_args: Option<Vec<String>>,
    codex_exec_args: Option<Vec<String>>,
    thinking_mode: Option<ThinkingMode>,
    max_calls_per_hour: Option<u32>,
    timeout_minutes: Option<u64>,
    runtime_dir: Option<String>,
    completion_indicators: Option<Vec<String>>,
    auto_wait_on_rate_limit: Option<bool>,
    sleep_on_rate_limit_secs: Option<u64>,
    no_progress_limit: Option<u32>,
}

pub fn load_run_config(cwd: &Path, overrides: &CliOverrides) -> Result<RunConfig> {
    let mut file_cfg = Forgerc::default();
    let forgerc_path = cwd.join(".forgerc");
    if forgerc_path.exists() {
        let raw = fs::read_to_string(&forgerc_path)
            .with_context(|| format!("failed to read {}", forgerc_path.display()))?;
        file_cfg = toml::from_str(&raw)
            .with_context(|| format!("failed to parse {}", forgerc_path.display()))?;
    }

    let resume_mode = if let Some(id) = &overrides.resume {
        ResumeMode::Explicit(id.clone())
    } else if overrides.resume_last {
        ResumeMode::Last
    } else {
        ResumeMode::New
    };

    let codex_cmd = first_some(
        env::var("FORGE_CODEX_CMD").ok(),
        file_cfg.codex_cmd,
        Some("codex".to_string()),
    )
    .unwrap_or_else(|| "codex".to_string());

    let thinking_mode = first_some(
        overrides.thinking_mode,
        env_thinking_mode("FORGE_THINKING_MODE"),
        file_cfg.thinking_mode,
    )
    .unwrap_or(ThinkingMode::Summary);

    let mut codex_pre_args = first_some(
        overrides.codex_pre_args.clone(),
        env_whitespace_args("FORGE_CODEX_PRE_ARGS"),
        file_cfg.codex_pre_args,
    )
    .unwrap_or_default();
    codex_pre_args.extend(thinking_mode.codex_config_args());

    let codex_exec_args = first_some(
        overrides.codex_exec_args.clone(),
        env_whitespace_args("FORGE_CODEX_EXEC_ARGS"),
        file_cfg.codex_exec_args,
    )
    .unwrap_or_default();

    let max_calls_per_hour = first_some(
        overrides.max_calls_per_hour,
        env_u32("FORGE_MAX_CALLS_PER_HOUR"),
        file_cfg.max_calls_per_hour,
    )
    .unwrap_or(100);

    let timeout_minutes = first_some(
        overrides.timeout_minutes,
        env_u64("FORGE_TIMEOUT_MINUTES"),
        file_cfg.timeout_minutes,
    )
    .unwrap_or(15);

    let runtime_dir = first_some(
        env::var("FORGE_RUNTIME_DIR").ok(),
        file_cfg.runtime_dir,
        Some(".forge".to_string()),
    )
    .map(PathBuf::from)
    .unwrap_or_else(|| PathBuf::from(".forge"));

    let completion_indicators = first_some(
        env_csv("FORGE_COMPLETION_INDICATORS"),
        file_cfg.completion_indicators,
        Some(vec![
            "STATUS: COMPLETE".to_string(),
            "TASK_COMPLETE".to_string(),
            "NO_MORE_WORK".to_string(),
            "ALL_TASKS_DONE".to_string(),
        ]),
    )
    .unwrap_or_default();

    let auto_wait_on_rate_limit = first_some(
        env_bool("FORGE_AUTO_WAIT_ON_RATE_LIMIT"),
        file_cfg.auto_wait_on_rate_limit,
        Some(false),
    )
    .unwrap_or(false);

    let sleep_on_rate_limit_secs = first_some(
        env_u64("FORGE_RATE_LIMIT_WAIT_SECS"),
        file_cfg.sleep_on_rate_limit_secs,
        Some(60),
    )
    .unwrap_or(60);

    let no_progress_limit = first_some(
        env_u32("FORGE_NO_PROGRESS_LIMIT"),
        file_cfg.no_progress_limit,
        Some(3),
    )
    .unwrap_or(3);

    if max_calls_per_hour == 0 {
        bail!("max_calls_per_hour must be greater than 0");
    }

    Ok(RunConfig {
        codex_cmd,
        codex_pre_args,
        codex_exec_args,
        thinking_mode,
        max_calls_per_hour,
        timeout_minutes,
        runtime_dir,
        completion_indicators,
        auto_wait_on_rate_limit,
        sleep_on_rate_limit_secs,
        no_progress_limit,
        resume_mode,
    })
}

fn first_some<T>(a: Option<T>, b: Option<T>, c: Option<T>) -> Option<T> {
    a.or(b).or(c)
}

fn env_u32(key: &str) -> Option<u32> {
    env::var(key).ok()?.parse().ok()
}

fn env_u64(key: &str) -> Option<u64> {
    env::var(key).ok()?.parse().ok()
}

fn env_bool(key: &str) -> Option<bool> {
    let value = env::var(key).ok()?;
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn env_csv(key: &str) -> Option<Vec<String>> {
    let value = env::var(key).ok()?;
    let parts = value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts)
    }
}

fn env_whitespace_args(key: &str) -> Option<Vec<String>> {
    let value = env::var(key).ok()?;
    let args = value
        .split_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if args.is_empty() {
        None
    } else {
        Some(args)
    }
}

fn env_thinking_mode(key: &str) -> Option<ThinkingMode> {
    let value = env::var(key).ok()?;
    match value.to_ascii_lowercase().as_str() {
        "off" => Some(ThinkingMode::Off),
        "summary" => Some(ThinkingMode::Summary),
        "raw" => Some(ThinkingMode::Raw),
        _ => None,
    }
}
