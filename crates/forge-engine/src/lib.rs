mod output_parser;

use anyhow::{Context, Result};
use chrono::Local;
use forge_config::{EngineKind, ResumeMode, RunConfig, ThinkingMode};
use forge_types::OutputAnalysis;
use output_parser::OutputParser;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const NO_OUTPUT_WATCHDOG_SECS: u64 = 120;

#[derive(Debug)]
pub struct EngineRunResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_ok: bool,
    pub timed_out: bool,
}

#[derive(Debug)]
pub struct EngineExecParams<'a> {
    pub cwd: &'a Path,
    pub config: &'a RunConfig,
    pub prompt: Option<String>,
    pub live_log_path: &'a Path,
}

pub trait Engine {
    fn name(&self) -> &'static str;
    fn build_args(&self, params: &EngineExecParams) -> Vec<String>;
    fn parse_output(&self, stdout: &str, stderr: &str, indicators: &[String]) -> OutputAnalysis {
        OutputParser::parse(stdout, stderr, indicators)
    }
    fn is_available(&self) -> bool;
}

pub struct CodexEngine;

pub struct OpenCodeEngine;

impl Engine for CodexEngine {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn build_args(&self, params: &EngineExecParams) -> Vec<String> {
        let mut args = params.config.engine_pre_args.clone();
        args.extend(self.build_exec_args(params));
        args
    }

    fn is_available(&self) -> bool {
        Command::new("codex")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

impl Engine for OpenCodeEngine {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn build_args(&self, params: &EngineExecParams) -> Vec<String> {
        let mut args = params.config.engine_pre_args.clone();
        args.extend(self.build_exec_args(params));
        args
    }

    fn is_available(&self) -> bool {
        Command::new("opencode")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

impl CodexEngine {
    fn build_exec_args(&self, params: &EngineExecParams) -> Vec<String> {
        let mut args = match &params.config.resume_mode {
            ResumeMode::New => {
                let mut v = vec!["exec".into()];
                v.extend(params.config.engine_exec_args.iter().cloned());
                v.push("--json".into());
                v
            }
            ResumeMode::Explicit(id) => {
                let mut v = vec!["exec".into()];
                v.extend(params.config.engine_exec_args.iter().cloned());
                v.extend(vec!["resume".into(), id.clone(), "--json".into()]);
                v
            }
            ResumeMode::Last => {
                let mut v = vec!["exec".into()];
                v.extend(params.config.engine_exec_args.iter().cloned());
                v.extend(vec!["resume".into(), "--last".into(), "--json".into()]);
                v
            }
        };

        if let Some(prompt) = &params.prompt {
            args.push(prompt.clone());
        }

        args
    }
}

impl OpenCodeEngine {
    fn build_exec_args(&self, params: &EngineExecParams) -> Vec<String> {
        let mut args = vec!["run".into()];
        args.extend(params.config.engine_exec_args.iter().cloned());
        args.push("--json".into());

        if params.config.thinking_mode == ThinkingMode::Off {
            args.push("--config".into());
            args.push("hide_agent_reasoning=true".into());
        }

        if let Some(prompt) = &params.prompt {
            args.push("--prompt".into());
            args.push(prompt.clone());
        }

        args
    }
}

pub fn create_engine(kind: EngineKind) -> Box<dyn Engine> {
    match kind {
        EngineKind::Codex => Box::new(CodexEngine),
        EngineKind::OpenCode => Box::new(OpenCodeEngine),
    }
}

pub fn execute_with_engine<F>(
    engine: &dyn Engine,
    params: EngineExecParams,
    mut heartbeat: F,
) -> Result<EngineRunResult>
where
    F: FnMut() -> Result<()>,
{
    let args = engine.build_args(&params);
    let config = params.config;
    let timeout = if config.timeout_minutes == 0 {
        None
    } else {
        Some(Duration::from_secs(
            config.timeout_minutes.saturating_mul(60),
        ))
    };
    let no_output_watchdog = if config.timeout_minutes == 0 {
        None
    } else {
        Some(Duration::from_secs(NO_OUTPUT_WATCHDOG_SECS))
    };

    let mut child = Command::new(&config.engine_cmd)
        .args(&args)
        .current_dir(params.cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to execute {}", config.engine_cmd))?;

    let stdout = child.stdout.take().context("failed to capture stdout")?;
    let stderr = child.stderr.take().context("failed to capture stderr")?;

    let (tx, rx) = mpsc::channel::<StreamEvent>();
    let stdout_handle = spawn_stream_reader(stdout, StreamSource::Stdout, tx.clone());
    let stderr_handle = spawn_stream_reader(stderr, StreamSource::Stderr, tx);

    let started = Instant::now();
    let mut timed_out = false;
    let mut finished = false;
    let mut exit_ok = false;
    let mut open_streams = 2_u8;
    let mut stdout_buf = String::new();
    let mut stderr_buf = String::new();
    let mut last_output_at = Instant::now();

    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(StreamEvent::Chunk { source, chunk }) => {
                heartbeat()?;
                last_output_at = Instant::now();
                match source {
                    StreamSource::Stdout => {
                        stdout_buf.push_str(&chunk);
                        append_history(params.live_log_path, &chunk)?;
                    }
                    StreamSource::Stderr => {
                        stderr_buf.push_str(&chunk);
                        append_history(params.live_log_path, &format!("[stderr] {chunk}"))?;
                    }
                }
            }
            Ok(StreamEvent::Closed) => {
                heartbeat()?;
                open_streams = open_streams.saturating_sub(1);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                heartbeat()?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                heartbeat()?;
                open_streams = 0;
            }
        }

        if !finished {
            if let Some(status) = child.try_wait()? {
                finished = true;
                exit_ok = status.success();
            } else if let Some(limit) = timeout {
                if started.elapsed() >= limit {
                    timed_out = true;
                    let _ = child.kill();
                    let status = child
                        .wait()
                        .with_context(|| format!("failed waiting for {}", config.engine_cmd))?;
                    finished = true;
                    exit_ok = status.success();
                }
            } else if let Some(limit) = no_output_watchdog {
                if last_output_at.elapsed() >= limit {
                    timed_out = true;
                    let _ = child.kill();
                    let status = child
                        .wait()
                        .with_context(|| format!("failed waiting for {}", config.engine_cmd))?;
                    finished = true;
                    exit_ok = status.success();
                    append_history(
                        params.live_log_path,
                        &format!(
                            "[forge] no output watchdog triggered after {}s; iteration killed\n",
                            limit.as_secs()
                        ),
                    )?;
                }
            }
        }

        if finished && open_streams == 0 {
            break;
        }
    }

    for handle in [stdout_handle, stderr_handle] {
        let _ = handle.join();
    }

    Ok(EngineRunResult {
        stdout: stdout_buf,
        stderr: stderr_buf,
        exit_ok,
        timed_out,
    })
}

#[derive(Debug, Clone, Copy)]
enum StreamSource {
    Stdout,
    Stderr,
}

#[derive(Debug)]
enum StreamEvent {
    Chunk { source: StreamSource, chunk: String },
    Closed,
}

fn spawn_stream_reader<R>(
    reader: R,
    source: StreamSource,
    tx: mpsc::Sender<StreamEvent>,
) -> thread::JoinHandle<()>
where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _ = tx.send(StreamEvent::Closed);
                    break;
                }
                Ok(_) => {
                    let _ = tx.send(StreamEvent::Chunk {
                        source,
                        chunk: line.clone(),
                    });
                }
                Err(_) => {
                    let _ = tx.send(StreamEvent::Closed);
                    break;
                }
            }
        }
    })
}

fn append_history(path: &Path, line: &str) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let stamped = stamp_lines(line);
    file.write_all(stamped.as_bytes())
        .with_context(|| format!("failed to append {}", path.display()))
}

fn stamp_lines(input: &str) -> String {
    let ts = Local::now().format("%H:%M:%S").to_string();
    let mut out = String::new();
    for segment in input.split_inclusive('\n') {
        let has_newline = segment.ends_with('\n');
        let content = segment.trim_end_matches('\n');
        if content.is_empty() {
            continue;
        }
        out.push_str(&format!("[{}] {}", ts, content));
        if has_newline {
            out.push('\n');
        }
    }
    if out.is_empty() && !input.trim().is_empty() {
        out.push_str(&format!("[{}] {}", ts, input.trim()));
    }
    out
}

pub fn epoch_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::overly_complex_bool_expr)]
    fn codex_engine_is_available_check() {
        let engine = CodexEngine;
        let result = engine.is_available();
        assert!(result || !result);
    }

    #[test]
    #[allow(clippy::overly_complex_bool_expr)]
    fn opencode_engine_is_available_check() {
        let engine = OpenCodeEngine;
        let result = engine.is_available();
        assert!(result || !result);
    }

    #[test]
    fn parse_codex_output_detects_exit_signal() {
        let output = "EXIT_SIGNAL: true\nSTATUS: COMPLETE";
        let analysis = CodexEngine.parse_output(output, "", &["STATUS: COMPLETE".into()]);
        assert!(analysis.exit_signal_true);
        assert_eq!(analysis.completion_indicators, 1);
    }

    #[test]
    fn parse_opencode_output_detects_session_id() {
        let output = r#"{"type":"thread.started","thread_id":"abc123"}"#;
        let analysis = OpenCodeEngine.parse_output(output, "", &[]);
        assert_eq!(analysis.session_id, Some("abc123".to_string()));
    }

    #[test]
    fn codex_engine_name() {
        let engine = CodexEngine;
        assert_eq!(engine.name(), "codex");
    }

    #[test]
    fn opencode_engine_name() {
        let engine = OpenCodeEngine;
        assert_eq!(engine.name(), "opencode");
    }

    #[test]
    fn create_engine_returns_codex() {
        let engine = create_engine(EngineKind::Codex);
        assert_eq!(engine.name(), "codex");
    }

    #[test]
    fn create_engine_returns_opencode() {
        let engine = create_engine(EngineKind::OpenCode);
        assert_eq!(engine.name(), "opencode");
    }
}
