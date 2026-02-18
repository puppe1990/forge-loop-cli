use anyhow::Result;
use chrono::{DateTime, Datelike, Local, TimeZone, Utc};
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use forge_core::{read_progress, read_status};
use forge_types::{ProgressSnapshot, RunStatus};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_STALL_THRESHOLD_SECS: u64 = 15;
const LIMIT_BAR_WIDTH: usize = 20;

static SESSION_PATH_CACHE: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
static SESSION_USAGE_CACHE: OnceLock<Mutex<HashMap<String, CachedSessionUsage>>> = OnceLock::new();

#[derive(Debug, Clone)]
struct CodexUsageSnapshot {
    context_left_percent: Option<i64>,
    context_used_tokens: Option<i64>,
    context_window_tokens: Option<i64>,
    five_hour_left_percent: Option<i64>,
    five_hour_resets_at: Option<String>,
    seven_day_left_percent: Option<i64>,
    seven_day_resets_at: Option<String>,
}

#[derive(Debug, Clone)]
struct CachedSessionUsage {
    modified_key: Option<u128>,
    snapshot: Option<CodexUsageSnapshot>,
}

pub fn run_monitor(runtime_dir: &Path, refresh_ms: u64, stall_threshold_secs: u64) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = monitor_loop(&mut terminal, runtime_dir, refresh_ms, stall_threshold_secs);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn monitor_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    runtime_dir: &Path,
    refresh_ms: u64,
    stall_threshold_secs: u64,
) -> Result<()> {
    loop {
        let status = read_status(runtime_dir).unwrap_or_else(|_| RunStatus::default());
        let progress = read_progress(runtime_dir);

        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(26),
                    Constraint::Percentage(14),
                    Constraint::Percentage(24),
                    Constraint::Percentage(36),
                ])
                .split(f.area());

            let top = render_status(&status, runtime_dir, stall_threshold_secs);
            let bottom = render_progress(&progress, runtime_dir);
            let plan = render_plan(runtime_dir);
            let activity = render_activity_and_logs(runtime_dir);

            f.render_widget(top, chunks[0]);
            f.render_widget(bottom, chunks[1]);
            f.render_widget(plan, chunks[2]);
            f.render_widget(activity, chunks[3]);
        })?;

        if event::poll(Duration::from_millis(refresh_ms))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn render_status(
    status: &RunStatus,
    runtime_dir: &Path,
    stall_threshold_secs: u64,
) -> Paragraph<'static> {
    let now = epoch_now();
    let run_timer = if status.run_started_at_epoch == 0 {
        "-".to_string()
    } else {
        format_elapsed(now.saturating_sub(status.run_started_at_epoch))
    };
    let command_timer = if status.current_loop_started_at_epoch == 0 {
        "-".to_string()
    } else {
        format_elapsed(now.saturating_sub(status.current_loop_started_at_epoch))
    };
    let stalled_for = stalled_for_secs(status, now, stall_threshold_secs);
    let stalled = stalled_for.is_some();
    let runner_dead = is_runner_process_dead(runtime_dir);
    let heartbeat_age = heartbeat_age_secs(status, now);
    let heartbeat_age_text = heartbeat_age
        .map(format_elapsed)
        .unwrap_or_else(|| "-".to_string());
    let stalled_text = stalled_for
        .map(format_elapsed)
        .unwrap_or_else(|| "-".to_string());
    let session_id = infer_session_id(runtime_dir, status);
    let usage = session_id
        .as_deref()
        .and_then(read_codex_usage_for_session_id);

    let mut lines = vec![
        Line::from(format!("state: {}", status.state)),
        Line::from(format!("thinking_mode: {}", status.thinking_mode)),
        Line::from(format!("run_timer: {}", run_timer)),
        Line::from(format!("current_loop: {}", status.current_loop)),
        Line::from(format!("command_timer: {}", command_timer)),
        Line::from(format!("heartbeat_age: {}", heartbeat_age_text)),
        Line::from(format!("stalled_threshold: {}s", stall_threshold_secs)),
        Line::from(format!("stalled: {}", stalled)),
        Line::from(format!("stalled_for: {}", stalled_text)),
        Line::from(format!(
            "total_loops_executed: {}",
            status.total_loops_executed
        )),
        Line::from(format!(
            "completion_indicators: {}",
            status.completion_indicators
        )),
        Line::from(format!("exit_signal_seen: {}", status.exit_signal_seen)),
        Line::from(format!("circuit_state: {:?}", status.circuit_state)),
        Line::from(format!(
            "session_id: {}",
            session_id.unwrap_or_else(|| "-".to_string())
        )),
        Line::from(format!("context: {}", format_context_line(usage.as_ref()))),
        Line::from(format!(
            "5h limit: {}",
            format_limit_line(
                usage.as_ref().and_then(|u| u.five_hour_left_percent),
                usage
                    .as_ref()
                    .and_then(|u| u.five_hour_resets_at.as_deref())
            )
        )),
        Line::from(format!(
            "7d limit: {}",
            format_limit_line(
                usage.as_ref().and_then(|u| u.seven_day_left_percent),
                usage
                    .as_ref()
                    .and_then(|u| u.seven_day_resets_at.as_deref())
            )
        )),
        Line::from(format!(
            "last_error: {}",
            status.last_error.clone().unwrap_or_else(|| "-".to_string())
        )),
        Line::from(format!("updated_at_epoch: {}", status.updated_at_epoch)),
        Line::from(""),
    ];

    if runner_dead {
        lines.push(Line::from(vec![Span::styled(
            "ALERT: runner process not found (stale status).",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )]));
    } else if stalled {
        lines.push(Line::from(vec![Span::styled(
            "ALERT: heartbeat stale (no recent events).",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )]));
    }
    lines.push(Line::from("press 'q' to quit"));

    let mut block = Block::default().title("forge status").borders(Borders::ALL);
    if stalled || runner_dead {
        block = block.border_style(Style::default().fg(Color::Red));
    }

    Paragraph::new(lines).block(block)
}

fn is_runner_process_dead(runtime_dir: &Path) -> bool {
    if !runtime_dir.join("status.json").exists() {
        return false;
    }
    let status = read_status(runtime_dir).unwrap_or_default();
    if status.state != "running" {
        return false;
    }
    let pid_path = runtime_dir.join(".runner_pid");
    let Ok(raw_pid) = fs::read_to_string(pid_path) else {
        return true;
    };
    let Ok(pid) = raw_pid.trim().parse::<i32>() else {
        return true;
    };
    if pid <= 0 {
        return true;
    }
    is_pid_dead_unix(pid)
}

#[cfg(unix)]
fn is_pid_dead_unix(pid: i32) -> bool {
    unsafe {
        let rc = libc::kill(pid, 0);
        if rc == 0 {
            false
        } else {
            std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        }
    }
}

#[cfg(not(unix))]
fn is_pid_dead_unix(_pid: i32) -> bool {
    false
}

fn heartbeat_age_secs(status: &RunStatus, now: u64) -> Option<u64> {
    if status.state != "running" || status.last_heartbeat_at_epoch == 0 {
        return None;
    }
    Some(now.saturating_sub(status.last_heartbeat_at_epoch))
}

fn stalled_for_secs(status: &RunStatus, now: u64, stall_threshold_secs: u64) -> Option<u64> {
    if status.state != "running" || status.last_heartbeat_at_epoch == 0 {
        return None;
    }
    let elapsed = now.saturating_sub(status.last_heartbeat_at_epoch);
    let threshold = if stall_threshold_secs == 0 {
        DEFAULT_STALL_THRESHOLD_SECS
    } else {
        stall_threshold_secs
    };
    if elapsed >= threshold {
        Some(elapsed)
    } else {
        None
    }
}

fn render_progress(progress: &ProgressSnapshot, runtime_dir: &Path) -> Paragraph<'static> {
    let plan_path = runtime_dir.join("plan.md");
    let body = format!(
        "loops_with_progress: {}\nloops_without_progress: {}\nlast_summary: {}\nupdated_at_epoch: {}\nplan_path: {}",
        progress.loops_with_progress,
        progress.loops_without_progress,
        progress.last_summary,
        progress.updated_at_epoch,
        plan_path.display(),
    );

    Paragraph::new(body).block(
        Block::default()
            .title("forge progress")
            .borders(Borders::ALL),
    )
}

fn render_plan(runtime_dir: &Path) -> Paragraph<'static> {
    let content = read_plan_preview(runtime_dir, 28);
    Paragraph::new(content).block(
        Block::default()
            .title("forge plan.md")
            .borders(Borders::ALL),
    )
}

fn read_plan_preview(runtime_dir: &Path, max_lines: usize) -> String {
    let path = runtime_dir.join("plan.md");
    let Ok(raw) = fs::read_to_string(&path) else {
        return "(plan.md not found in runtime directory)".to_string();
    };

    let mut lines = raw
        .lines()
        .map(|line| line.chars().take(220).collect::<String>())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return "(plan.md is empty)".to_string();
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        lines.push("...".to_string());
    }
    lines.join("\n")
}

fn render_activity_and_logs(runtime_dir: &Path) -> Paragraph<'static> {
    let feed = read_live_feed(runtime_dir);
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(vec![
            Span::styled("source: ", Style::default().fg(Color::DarkGray)),
            Span::raw(feed.source),
        ]),
        Line::from(vec![
            Span::styled("codex_now: ", Style::default().fg(Color::DarkGray)),
            Span::styled(feed.current, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "recent logs:",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    if feed.recent.is_empty() {
        lines.push(Line::from(Span::styled(
            "-",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for entry in feed.recent {
            lines.push(Line::from(vec![
                Span::styled("- ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("[{}] ", entry.kind),
                    style_for_event_kind(entry.kind).add_modifier(Modifier::BOLD),
                ),
                Span::styled(entry.text, style_for_event_kind(entry.kind)),
            ]));
        }
    }
    Paragraph::new(lines).block(
        Block::default()
            .title("forge live activity + logs")
            .borders(Borders::ALL),
    )
}

fn style_for_event_kind(kind: &'static str) -> Style {
    match kind {
        "FAILURE" => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        "LIMITER" => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        "SESSION" => Style::default().fg(Color::Cyan),
        "LOOP" => Style::default().fg(Color::Magenta),
        "PROGRESS" => Style::default().fg(Color::LightBlue),
        "QUOTA" => Style::default().fg(Color::Green),
        "SYSTEM" => Style::default().fg(Color::DarkGray),
        "ANALYSIS" => Style::default().fg(Color::LightMagenta),
        "SUCCESS" => Style::default().fg(Color::LightGreen),
        _ => Style::default().fg(Color::LightCyan),
    }
}

#[derive(Debug)]
struct LiveFeed {
    source: String,
    current: String,
    recent: Vec<LogLine>,
}

#[derive(Debug)]
struct LogLine {
    kind: &'static str,
    text: String,
}

#[derive(Debug)]
struct ParsedActivity {
    kind: Option<&'static str>,
    text: String,
}

fn read_live_feed(runtime_dir: &Path) -> LiveFeed {
    let Some(path) = resolve_log_source(runtime_dir) else {
        return LiveFeed {
            source: "-".to_string(),
            current: "-".to_string(),
            recent: Vec::new(),
        };
    };

    let raw = match fs::read_to_string(&path) {
        Ok(value) => value,
        Err(_) => {
            return LiveFeed {
                source: path.display().to_string(),
                current: "-".to_string(),
                recent: Vec::new(),
            }
        }
    };
    LiveFeed {
        source: path.display().to_string(),
        current: extract_latest_activity(&raw).unwrap_or_else(|| "-".to_string()),
        recent: extract_recent_activity_lines(&raw, 14),
    }
}

fn resolve_log_source(runtime_dir: &Path) -> Option<PathBuf> {
    let mut candidates = vec![runtime_dir.join("ralph.logs"), runtime_dir.join("live.log")];
    if let Some(project_dir) = runtime_dir.parent() {
        candidates.push(project_dir.join(".ralph").join("logs").join("ralph.log"));
        candidates.push(
            project_dir
                .join(".ralph")
                .join("logs")
                .join("ralph-gemini.log"),
        );
    }
    candidates.into_iter().find(|p| p.exists())
}

fn extract_recent_activity_lines(raw: &str, limit: usize) -> Vec<LogLine> {
    let mut out = Vec::new();
    let mut skipped_state_db_warns = 0_u64;
    for line in raw.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "[stdout]" || trimmed == "[stderr]" {
            continue;
        }
        if is_state_db_discrepancy_warn(trimmed) {
            skipped_state_db_warns += 1;
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            let Some(parsed) = parse_activity_event(&value) else {
                continue;
            };
            let label = parsed
                .kind
                .unwrap_or_else(|| classify_log_event(parsed.text.as_str()));
            out.push(LogLine {
                kind: label,
                text: parsed.text,
            });
        } else {
            let normalized: String = trimmed.chars().take(180).collect();
            let label = classify_log_event(&normalized);
            out.push(LogLine {
                kind: label,
                text: normalized,
            });
        }
        if out.len() >= limit {
            break;
        }
    }
    out.reverse();
    if skipped_state_db_warns > 0 {
        out.push(LogLine {
            kind: "SYSTEM",
            text: format!(
                "suppressed {} repeated state_db discrepancy warnings",
                skipped_state_db_warns
            ),
        });
    }
    out
}

fn extract_latest_activity(raw: &str) -> Option<String> {
    let mut fallback: Option<String> = None;
    for line in raw.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "[stdout]" || trimmed == "[stderr]" {
            continue;
        }
        if is_state_db_discrepancy_warn(trimmed) {
            continue;
        }

        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            if let Some(parsed) = parse_activity_event(&value) {
                return Some(parsed.text);
            }
            continue;
        }

        if trimmed.starts_with("202") {
            continue;
        }

        if fallback.is_none() {
            fallback = Some(trimmed.chars().take(180).collect());
        }
    }
    fallback
}

fn classify_log_event(line: &str) -> &'static str {
    if let Some(kind) = classify_prefix_tag(line) {
        return kind;
    }
    if is_state_db_discrepancy_warn(line) {
        return "SYSTEM";
    }
    let text = line.to_ascii_lowercase();
    if text.contains("failed to refresh available models")
        && text.contains("timeout waiting for child process to exit")
    {
        return "LIMITER";
    }
    if text.contains("permission denied")
        || text.contains("timed out")
        || text.contains("execution failed")
        || text.contains("non-ignorable diagnostics")
        || text.contains("failed")
        || text.contains("error")
    {
        return "FAILURE";
    }
    if text.contains("rate limit")
        || text.contains("api usage limit")
        || text.contains("circuit breaker")
        || text.contains("retrying in")
    {
        return "LIMITER";
    }
    if text.contains("session reset")
        || text.contains("starting new")
        || text.contains("resuming")
        || text.contains("resume strategy")
    {
        return "SESSION";
    }
    if text.contains("starting loop")
        || text.contains("completed loop")
        || text.contains("loop ")
        || text.contains("executing")
    {
        return "LOOP";
    }
    if text.contains("progress") || text.contains("working") {
        return "PROGRESS";
    }
    if text.contains("quota") {
        return "QUOTA";
    }
    if text.contains("analyzing") || text.contains("analysis") {
        return "ANALYSIS";
    }
    if text.contains("success") || text.contains("completed") {
        return "SUCCESS";
    }
    "INFO"
}

fn is_state_db_discrepancy_warn(line: &str) -> bool {
    let text = line.to_ascii_lowercase();
    text.contains("codex_core::state_db")
        && text.contains("record_discrepancy")
        && text.contains("find_thread_path_by_id_str_in_subdir")
}

fn classify_prefix_tag(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    let level = trimmed
        .strip_prefix('[')
        .and_then(|s| s.split_once(']'))
        .map(|(head, _)| head.to_ascii_uppercase())?;
    match level.as_str() {
        "FAILURE" | "ERROR" => Some("FAILURE"),
        "PROGRESS" | "IN_PROGRESS" => Some("PROGRESS"),
        "SUCCESS" | "COMPLETED" => Some("SUCCESS"),
        "ANALYSIS" | "REASONING" => Some("ANALYSIS"),
        "LOOP" => Some("LOOP"),
        "SESSION" => Some("SESSION"),
        "LIMITER" | "RATE_LIMIT" => Some("LIMITER"),
        "QUOTA" => Some("QUOTA"),
        "INFO" => Some("INFO"),
        _ => None,
    }
}

fn parse_activity_event(value: &Value) -> Option<ParsedActivity> {
    let item = value.get("item")?;
    let item_type = item.get("type")?.as_str()?;

    if item_type == "command_execution" {
        let command = item.get("command").and_then(Value::as_str).unwrap_or("-");
        let status = item.get("status").and_then(Value::as_str).unwrap_or("-");
        let kind = match status {
            "completed" => Some("SUCCESS"),
            "failed" => Some("FAILURE"),
            "in_progress" => Some("PROGRESS"),
            _ => Some("INFO"),
        };
        return Some(ParsedActivity {
            kind,
            text: format!("command ({status}): {command}"),
        });
    }

    if item_type == "agent_message" {
        let text = item.get("text").and_then(Value::as_str).unwrap_or("-");
        return Some(ParsedActivity {
            kind: None,
            text: format!("agent: {}", text.chars().take(180).collect::<String>()),
        });
    }

    if item_type == "reasoning" {
        let text = item.get("text").and_then(Value::as_str).unwrap_or("-");
        return Some(ParsedActivity {
            kind: Some("ANALYSIS"),
            text: format!("reasoning: {}", text.chars().take(180).collect::<String>()),
        });
    }

    None
}

fn epoch_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
}

fn format_elapsed(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

fn infer_session_id(runtime_dir: &Path, status: &RunStatus) -> Option<String> {
    if let Some(session_id) = status.session_id.clone() {
        if !session_id.trim().is_empty() {
            return Some(session_id);
        }
    }

    let path = resolve_log_source(runtime_dir)?;
    let raw = fs::read_to_string(path).ok()?;
    for line in raw.lines().rev() {
        let trimmed = line.trim();
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("thread.started") {
            if let Some(thread_id) = value.get("thread_id").and_then(Value::as_str) {
                if !thread_id.trim().is_empty() {
                    return Some(thread_id.to_string());
                }
            }
        }
    }
    None
}

fn format_context_line(usage: Option<&CodexUsageSnapshot>) -> String {
    let Some(usage) = usage else {
        return "-".to_string();
    };
    match (
        usage.context_left_percent,
        usage.context_used_tokens,
        usage.context_window_tokens,
    ) {
        (Some(left), Some(used), Some(window)) => {
            format!(
                "{}% left ({} used / {})",
                clamp_percent(left),
                format_compact_int(used),
                format_compact_int(window)
            )
        }
        _ => "-".to_string(),
    }
}

fn format_limit_line(left_percent: Option<i64>, resets_at: Option<&str>) -> String {
    let Some(left_percent) = left_percent else {
        return "-".to_string();
    };
    let clamped = clamp_percent(left_percent);
    let bar = render_limit_bar(clamped as usize, LIMIT_BAR_WIDTH);
    let mut line = format!("{bar} {clamped}% left");
    if let Some(reset) = resets_at {
        if !reset.trim().is_empty() {
            line.push_str(&format!(" (resets {reset})"));
        }
    }
    line
}

fn clamp_percent(percent: i64) -> i64 {
    percent.clamp(0, 100)
}

fn render_limit_bar(left_percent: usize, width: usize) -> String {
    let filled = ((left_percent.saturating_mul(width) + 50) / 100).min(width);
    let empty = width.saturating_sub(filled);
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

fn format_compact_int(value: i64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn read_codex_usage_for_session_id(session_id: &str) -> Option<CodexUsageSnapshot> {
    let session_file = resolve_codex_session_file(session_id)?;
    let key = session_file.display().to_string();
    let modified_key = file_modified_key(&session_file);

    let usage_cache = SESSION_USAGE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = usage_cache.lock() {
        if let Some(cached) = cache.get(&key) {
            if cached.modified_key == modified_key {
                return cached.snapshot.clone();
            }
        }
    }

    let snapshot = parse_latest_token_count_snapshot(&session_file);

    if let Ok(mut cache) = usage_cache.lock() {
        cache.insert(
            key,
            CachedSessionUsage {
                modified_key,
                snapshot: snapshot.clone(),
            },
        );
    }

    snapshot
}

fn resolve_codex_session_file(session_id: &str) -> Option<PathBuf> {
    let path_cache = SESSION_PATH_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = path_cache.lock() {
        if let Some(path) = cache.get(session_id) {
            if path.exists() {
                return Some(path.clone());
            }
        }
    }

    let base = codex_sessions_base_dir()?;
    let mut stack = vec![base];
    let mut resolved: Option<PathBuf> = None;

    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|v| v.to_str()) != Some("jsonl") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or_default();
            if name.contains(session_id) {
                resolved = Some(path);
                break;
            }
        }
        if resolved.is_some() {
            break;
        }
    }

    if let Some(path) = resolved.clone() {
        if let Ok(mut cache) = path_cache.lock() {
            cache.insert(session_id.to_string(), path);
        }
    }
    resolved
}

fn parse_latest_token_count_snapshot(path: &Path) -> Option<CodexUsageSnapshot> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut latest: Option<CodexUsageSnapshot> = None;

    for line in reader.lines().map_while(std::result::Result::ok) {
        if !line.contains("\"type\":\"token_count\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if payload.get("type").and_then(Value::as_str) != Some("token_count") {
            continue;
        }
        latest = Some(parse_usage_from_token_count_payload(payload));
    }

    latest
}

fn parse_usage_from_token_count_payload(payload: &Value) -> CodexUsageSnapshot {
    let total_tokens = payload
        .get("info")
        .and_then(|v| v.get("total_token_usage"))
        .and_then(|v| v.get("total_tokens"))
        .and_then(Value::as_i64);
    let window_tokens = payload
        .get("info")
        .and_then(|v| v.get("model_context_window"))
        .and_then(Value::as_i64);
    let context_left_percent = match (total_tokens, window_tokens) {
        (Some(total), Some(window)) if window > 0 => {
            Some(100 - ((total as f64 / window as f64) * 100.0).round() as i64)
        }
        _ => None,
    };

    let primary = payload.get("rate_limits").and_then(|v| v.get("primary"));
    let secondary = payload.get("rate_limits").and_then(|v| v.get("secondary"));
    let primary_used = primary
        .and_then(|v| v.get("used_percent"))
        .and_then(Value::as_f64);
    let secondary_used = secondary
        .and_then(|v| v.get("used_percent"))
        .and_then(Value::as_f64);
    let primary_resets = primary
        .and_then(|v| v.get("resets_at"))
        .and_then(Value::as_i64)
        .map(format_reset_timestamp);
    let secondary_resets = secondary
        .and_then(|v| v.get("resets_at"))
        .and_then(Value::as_i64)
        .map(format_reset_timestamp);

    CodexUsageSnapshot {
        context_left_percent,
        context_used_tokens: total_tokens,
        context_window_tokens: window_tokens,
        five_hour_left_percent: primary_used.map(|used| 100 - used.round() as i64),
        five_hour_resets_at: primary_resets,
        seven_day_left_percent: secondary_used.map(|used| 100 - used.round() as i64),
        seven_day_resets_at: secondary_resets,
    }
}

fn format_reset_timestamp(epoch_seconds: i64) -> String {
    let Some(utc) = Utc.timestamp_opt(epoch_seconds, 0).single() else {
        return epoch_seconds.to_string();
    };
    let local: DateTime<Local> = utc.with_timezone(&Local);
    let now = Local::now();
    if local.date_naive() == now.date_naive() {
        local.format("%-I:%M %p").to_string()
    } else if local.year() == now.year() {
        local.format("%b %-d").to_string()
    } else {
        local.format("%b %-d, %Y").to_string()
    }
}

fn codex_sessions_base_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".codex").join("sessions"))
}

fn file_modified_key(path: &Path) -> Option<u128> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let elapsed = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(elapsed.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn picks_latest_command_activity() {
        let raw = r#"
[stdout]
{"type":"item.started","item":{"type":"command_execution","command":"npm run test","status":"in_progress"}}
"#;
        let activity = extract_latest_activity(raw).expect("activity");
        assert!(activity.contains("npm run test"));
        assert!(activity.contains("in_progress"));
    }

    #[test]
    fn falls_back_to_text_lines() {
        let raw = r#"
[stderr]
plain text line
"#;
        let activity = extract_latest_activity(raw).expect("activity");
        assert_eq!(activity, "plain text line");
    }

    #[test]
    fn classifies_loop_line() {
        let label = classify_log_event("loop 2: codex exec started");
        assert_eq!(label, "LOOP");
    }

    #[test]
    fn extracts_recent_lines_with_labels() {
        let raw = r#"
[stdout]
{"type":"item.completed","item":{"type":"agent_message","text":"loop 1: codex exec started"}}
plain text line
"#;
        let recent = extract_recent_activity_lines(raw, 5);
        assert!(!recent.is_empty());
        assert!(recent.iter().any(|line| line.kind == "LOOP"));
    }

    #[test]
    fn command_execution_status_is_classified_by_status() {
        let raw = r#"
[stdout]
{"type":"item.started","item":{"type":"command_execution","command":"npm run lint","status":"in_progress"}}
{"type":"item.completed","item":{"type":"command_execution","command":"npm run lint","status":"completed"}}
"#;
        let recent = extract_recent_activity_lines(raw, 5);
        assert!(recent.iter().any(|line| line.kind == "PROGRESS"));
        assert!(recent.iter().any(|line| line.kind == "SUCCESS"));
    }

    #[test]
    fn prefix_tag_controls_event_classification() {
        assert_eq!(
            classify_log_event("[FAILURE] command (failed): npm run lint"),
            "FAILURE"
        );
        assert_eq!(
            classify_log_event("[PROGRESS] command (in_progress): npm run lint"),
            "PROGRESS"
        );
        assert_eq!(classify_log_event("[INFO] reasoning: checking"), "INFO");
    }

    #[test]
    fn models_refresh_timeout_is_not_classified_as_failure() {
        assert_eq!(
            classify_log_event(
                "[stderr] 2026-02-18T04:36:31Z ERROR codex_core::models_manager::manager: failed to refresh available models: timeout waiting for child process to exit"
            ),
            "LIMITER"
        );
    }

    #[test]
    fn state_db_discrepancy_warn_is_classified_as_system() {
        assert_eq!(
            classify_log_event(
                "[stderr] 2026-02-18T10:33:47Z WARN codex_core::state_db: state db record_discrepancy: find_thread_path_by_id_str_in_subdir, falling_back"
            ),
            "SYSTEM"
        );
    }

    #[test]
    fn stalled_for_secs_detects_stall_when_running() {
        let status = RunStatus {
            state: "running".to_string(),
            last_heartbeat_at_epoch: 100,
            ..RunStatus::default()
        };
        let stalled = stalled_for_secs(&status, 120, 15);
        assert_eq!(stalled, Some(20));
    }

    #[test]
    fn stalled_for_secs_ignores_recent_heartbeat() {
        let status = RunStatus {
            state: "running".to_string(),
            last_heartbeat_at_epoch: 100,
            ..RunStatus::default()
        };
        let stalled = stalled_for_secs(&status, 110, 15);
        assert_eq!(stalled, None);
    }

    #[test]
    fn read_plan_preview_returns_missing_message_when_no_plan() {
        let runtime_dir = temp_runtime_dir("missing");
        let preview = read_plan_preview(&runtime_dir, 10);
        assert!(preview.contains("plan.md not found"));
        let _ = fs::remove_dir_all(&runtime_dir);
    }

    #[test]
    fn read_plan_preview_reads_plan_content() {
        let runtime_dir = temp_runtime_dir("present");
        fs::create_dir_all(&runtime_dir).expect("create runtime dir");
        fs::write(
            runtime_dir.join("plan.md"),
            "Goal: improve architecture\nStep 1",
        )
        .expect("write plan");

        let preview = read_plan_preview(&runtime_dir, 10);
        assert!(preview.contains("Goal: improve architecture"));
        assert!(preview.contains("Step 1"));

        let _ = fs::remove_dir_all(&runtime_dir);
    }

    fn temp_runtime_dir(suffix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "forge-monitor-test-{suffix}-{}-{nanos}",
            std::process::id()
        ))
    }
}
