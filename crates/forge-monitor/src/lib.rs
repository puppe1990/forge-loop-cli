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
use std::io::{self, Stdout};
use std::path::Path;
use std::time::Duration;

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
                .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(f.area());

            let top = render_status(&status);
            let bottom = render_progress(&progress);

            f.render_widget(top, chunks[0]);
            f.render_widget(bottom, chunks[1]);
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
    let body = format!(
        "state: {}\ncurrent_loop: {}\ntotal_loops_executed: {}\ncompletion_indicators: {}\nexit_signal_seen: {}\ncircuit_state: {:?}\nsession_id: {}\nlast_error: {}\nupdated_at_epoch: {}\n\npress 'q' to quit",
        status.state,
        status.current_loop,
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
