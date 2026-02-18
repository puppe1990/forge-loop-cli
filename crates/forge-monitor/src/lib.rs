use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use forge_core::{read_progress, read_status};
use forge_types::{ProgressSnapshot, RunStatus};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use serde_json::Value;
use std::fs;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const STALL_THRESHOLD_SECS: u64 = 15;

pub fn run_monitor(runtime_dir: &Path, refresh_ms: u64) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = monitor_loop(&mut terminal, runtime_dir, refresh_ms);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn monitor_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    runtime_dir: &Path,
    refresh_ms: u64,
) -> Result<()> {
    loop {
        let status = read_status(runtime_dir).unwrap_or_else(|_| RunStatus::default());
        let progress = read_progress(runtime_dir);

        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(32),
                    Constraint::Percentage(20),
                    Constraint::Percentage(48),
                ])
                .split(f.area());

            let top = render_status(&status);
            let bottom = render_progress(&progress);
            let activity = render_activity_and_logs(runtime_dir);

            f.render_widget(top, chunks[0]);
            f.render_widget(bottom, chunks[1]);
            f.render_widget(activity, chunks[2]);
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

fn render_status(status: &RunStatus) -> Paragraph<'static> {
    let now = epoch_now();
    let timer = if status.current_loop_started_at_epoch == 0 {
        "-".to_string()
    } else {
        format_elapsed(now.saturating_sub(status.current_loop_started_at_epoch))
    };
    let stalled_for = stalled_for_secs(status, now);
    let stalled = stalled_for.is_some();
    let stalled_text = stalled_for
        .map(format_elapsed)
        .unwrap_or_else(|| "-".to_string());

    let body = format!(
        "state: {}\ncurrent_loop: {}\nloop_timer: {}\nstalled: {}\nstalled_for: {}\ntotal_loops_executed: {}\ncompletion_indicators: {}\nexit_signal_seen: {}\ncircuit_state: {:?}\nsession_id: {}\nlast_error: {}\nupdated_at_epoch: {}\n\npress 'q' to quit",
        status.state,
        status.current_loop,
        timer,
        stalled,
        stalled_text,
        status.total_loops_executed,
        status.completion_indicators,
        status.exit_signal_seen,
        status.circuit_state,
        status.session_id.clone().unwrap_or_else(|| "-".to_string()),
        status.last_error.clone().unwrap_or_else(|| "-".to_string()),
        status.updated_at_epoch,
    );

    Paragraph::new(body).block(Block::default().title("forge status").borders(Borders::ALL))
}

fn stalled_for_secs(status: &RunStatus, now: u64) -> Option<u64> {
    if status.state != "running" || status.last_heartbeat_at_epoch == 0 {
        return None;
    }
    let elapsed = now.saturating_sub(status.last_heartbeat_at_epoch);
    if elapsed >= STALL_THRESHOLD_SECS {
        Some(elapsed)
    } else {
        None
    }
}

fn render_progress(progress: &ProgressSnapshot) -> Paragraph<'static> {
    let body = format!(
        "loops_with_progress: {}\nloops_without_progress: {}\nlast_summary: {}\nupdated_at_epoch: {}",
        progress.loops_with_progress,
        progress.loops_without_progress,
        progress.last_summary,
        progress.updated_at_epoch,
    );

    Paragraph::new(body).block(
        Block::default()
            .title("forge progress")
            .borders(Borders::ALL),
    )
}

fn render_activity_and_logs(runtime_dir: &Path) -> Paragraph<'static> {
    let feed = read_live_feed(runtime_dir);
    let mut body = format!(
        "source: {}\ncodex_now: {}\n\nrecent logs:\n",
        feed.source, feed.current
    );
    if feed.recent.is_empty() {
        body.push_str("-\n");
    } else {
        for line in feed.recent {
            body.push_str("- ");
            body.push_str(&line);
            body.push('\n');
        }
    }
    Paragraph::new(body).block(
        Block::default()
            .title("forge live activity + logs")
            .borders(Borders::ALL),
    )
}

#[derive(Debug)]
struct LiveFeed {
    source: String,
    current: String,
    recent: Vec<String>,
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

fn extract_recent_activity_lines(raw: &str, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in raw.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "[stdout]" || trimmed == "[stderr]" {
            continue;
        }
        let normalized = if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            if let Some(text) = parse_activity_event(&value) {
                text
            } else {
                continue;
            }
        } else {
            trimmed.chars().take(180).collect()
        };
        let label = classify_log_event(&normalized);
        out.push(format!("[{label}] {normalized}"));
        if out.len() >= limit {
            break;
        }
    }
    out.reverse();
    out
}

fn extract_latest_activity(raw: &str) -> Option<String> {
    for line in raw.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "[stdout]" || trimmed == "[stderr]" {
            continue;
        }

        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            if let Some(text) = parse_activity_event(&value) {
                return Some(text);
            }
            continue;
        }

        if trimmed.starts_with("202") {
            continue;
        }

        return Some(trimmed.chars().take(180).collect());
    }
    None
}

fn classify_log_event(line: &str) -> &'static str {
    let text = line.to_ascii_lowercase();
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

fn parse_activity_event(value: &Value) -> Option<String> {
    let item = value.get("item")?;
    let item_type = item.get("type")?.as_str()?;

    if item_type == "command_execution" {
        let command = item.get("command").and_then(Value::as_str).unwrap_or("-");
        let status = item.get("status").and_then(Value::as_str).unwrap_or("-");
        return Some(format!("command ({status}): {command}"));
    }

    if item_type == "agent_message" {
        let text = item.get("text").and_then(Value::as_str).unwrap_or("-");
        return Some(format!(
            "agent: {}",
            text.chars().take(180).collect::<String>()
        ));
    }

    if item_type == "reasoning" {
        let text = item.get("text").and_then(Value::as_str).unwrap_or("-");
        return Some(format!(
            "reasoning: {}",
            text.chars().take(180).collect::<String>()
        ));
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(recent.iter().any(|line| line.contains("[LOOP]")));
    }

    #[test]
    fn stalled_for_secs_detects_stall_when_running() {
        let status = RunStatus {
            state: "running".to_string(),
            last_heartbeat_at_epoch: 100,
            ..RunStatus::default()
        };
        let stalled = stalled_for_secs(&status, 120);
        assert_eq!(stalled, Some(20));
    }

    #[test]
    fn stalled_for_secs_ignores_recent_heartbeat() {
        let status = RunStatus {
            state: "running".to_string(),
            last_heartbeat_at_epoch: 100,
            ..RunStatus::default()
        };
        let stalled = stalled_for_secs(&status, 110);
        assert_eq!(stalled, None);
    }
}
