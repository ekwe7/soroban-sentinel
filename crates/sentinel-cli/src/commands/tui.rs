//! Interactive dashboard runner for `sentinel scan --tui`.

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::cursor::Hide;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use sentinel_core::{RiskLevel, TTLReport, ThresholdConfig};

use crate::commands::scan::{EXIT_CRITICAL_OR_ARCHIVED, EXIT_OK, EXIT_RPC_FAILURE, EXIT_WARN};
use crate::tui::{draw, App};

/// Restores the terminal even if the event loop exits early or errors.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
    }
}

/// Runs the interactive TUI and returns the CLI exit code for the scan result.
pub fn run_tui(report: TTLReport, thresholds: ThresholdConfig, network: &str) -> i32 {
    let status = report.overall_status;
    match run_tui_inner(report, thresholds, network) {
        Ok(()) => match status {
            RiskLevel::Ok => EXIT_OK,
            RiskLevel::Warn => EXIT_WARN,
            RiskLevel::Critical | RiskLevel::Archived => EXIT_CRITICAL_OR_ARCHIVED,
        },
        Err(error) => {
            eprintln!("Error: TUI failure: {}", error);
            EXIT_RPC_FAILURE
        }
    }
}

fn run_tui_inner(
    report: TTLReport,
    thresholds: ThresholdConfig,
    network: &str,
) -> Result<(), String> {
    enable_raw_mode().map_err(|error| format!("failed to enable raw mode: {error}"))?;
    let _guard = TerminalGuard;

    let mut stdout: Stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Hide)
        .map_err(|error| format!("failed to enter alternate screen: {error}"))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)
        .map_err(|error| format!("failed to initialize terminal: {error}"))?;

    let mut app = App::new(report, thresholds, network);

    loop {
        terminal
            .draw(|frame| draw(frame, &mut app))
            .map_err(|error| format!("failed to draw TUI: {error}"))?;

        if !event::poll(Duration::from_millis(250))
            .map_err(|error| format!("failed to poll terminal events: {error}"))?
        {
            continue;
        }

        match event::read().map_err(|error| format!("failed to read terminal event: {error}"))? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Up => app.move_up(),
                KeyCode::Down => app.move_down(),
                KeyCode::PageUp => app.page_up(),
                KeyCode::PageDown => app.page_down(),
                KeyCode::Tab => app.toggle_panel(),
                KeyCode::BackTab => app.toggle_panel(),
                KeyCode::Left => app.previous_sort_column(),
                KeyCode::Right => app.next_sort_column(),
                KeyCode::Char('s') | KeyCode::Char('S') => app.toggle_sort_direction(),
                _ => {}
            },
            // A redraw is enough to reflow widgets to the new terminal size.
            Event::Resize(_, _) => {}
            _ => {}
        }
    }

    Ok(())
}
