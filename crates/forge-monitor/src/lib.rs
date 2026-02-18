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
use std::path::Path;
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
                    Constraint::Percentage(45),
                    Constraint::Percentage(30),
                    Constraint::Percentage(25),
                ])
                .split(f.area());

            let top = render_status(&status);
            let bottom = render_progress(&progress);
            let activity = render_activity(runtime_dir);

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

fn render_activity(runtime_dir: &Path) -> Paragraph<'static> {
    let activity = read_live_activity(runtime_dir);
    let body = format!("codex_now: {}", activity);
    Paragraph::new(body).block(
        Block::default()
            .title("forge live activity")
            .borders(Borders::ALL),
    )
}

fn read_live_activity(runtime_dir: &Path) -> String {
    let path = runtime_dir.join("live.log");
    let raw = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(_) => return "-".to_string(),
    };
    extract_latest_activity(&raw).unwrap_or_else(|| "-".to_string())
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
