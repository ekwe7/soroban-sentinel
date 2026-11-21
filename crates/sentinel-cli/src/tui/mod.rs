//! Rendering and state helpers for the optional interactive TUI dashboard.
//!
//! This module is compiled only when the `tui` Cargo feature is enabled.

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table, Tabs, Wrap};
use ratatui::Frame;
use sentinel_core::{RiskLevel, StorageEntryReport, TTLReport, ThresholdConfig};

/// Column used to order the entry table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    /// Human-readable entry key.
    Key,
    /// Durability tier.
    Durability,
    /// Serialized entry size in bytes.
    ByteSize,
    /// Ledgers remaining before expiration.
    RemainingLedgers,
    /// Derived health status.
    HealthStatus,
}

impl SortColumn {
    /// Advances to the next sortable column.
    pub fn next(self) -> Self {
        match self {
            Self::Key => Self::Durability,
            Self::Durability => Self::ByteSize,
            Self::ByteSize => Self::RemainingLedgers,
            Self::RemainingLedgers => Self::HealthStatus,
            Self::HealthStatus => Self::Key,
        }
    }

    /// Moves to the previous sortable column.
    pub fn previous(self) -> Self {
        match self {
            Self::Key => Self::HealthStatus,
            Self::Durability => Self::Key,
            Self::ByteSize => Self::Durability,
            Self::RemainingLedgers => Self::ByteSize,
            Self::HealthStatus => Self::RemainingLedgers,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Key => "Entry Key",
            Self::Durability => "Durability",
            Self::ByteSize => "Byte Size",
            Self::RemainingLedgers => "Remaining Ledgers",
            Self::HealthStatus => "Health Status",
        }
    }
}

/// Which panel currently receives keyboard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    /// Scrollable entry table.
    Entries,
    /// Detail and TTL-health gauge for the selected entry.
    Detail,
}

impl Panel {
    /// Advances to the next panel.
    pub fn next(self) -> Self {
        match self {
            Self::Entries => Self::Detail,
            Self::Detail => Self::Entries,
        }
    }
}

/// Application state for the TUI event loop.
#[derive(Debug)]
pub struct App<'a> {
    /// A locally owned copy of the scan report, kept sorted for display.
    pub report: TTLReport,
    /// Thresholds used to color the health gauge.
    pub thresholds: ThresholdConfig,
    /// Network label for the title bar.
    pub network: &'a str,
    /// Index of the selected entry.
    pub selected: usize,
    /// First visible row in the table.
    pub table_offset: usize,
    /// Number of rows currently visible (updated by `draw`).
    pub viewport_height: usize,
    /// Panel receiving keyboard input.
    pub panel: Panel,
    /// Active sort column.
    pub sort_column: SortColumn,
    /// When true, the active column is sorted descending.
    pub sort_descending: bool,
}

impl<'a> App<'a> {
    /// Creates a new dashboard state with the first row selected.
    pub fn new(report: TTLReport, thresholds: ThresholdConfig, network: &'a str) -> Self {
        let mut app = Self {
            report,
            thresholds,
            network,
            selected: 0,
            table_offset: 0,
            viewport_height: 1,
            panel: Panel::Entries,
            sort_column: SortColumn::RemainingLedgers,
            sort_descending: false,
        };
        app.apply_sort();
        app
    }

    /// Moves the selection up by one row.
    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.ensure_visible();
    }

    /// Moves the selection down by one row.
    pub fn move_down(&mut self) {
        if self.report.entries.is_empty() {
            return;
        }
        self.selected = (self.selected + 1).min(self.report.entries.len() - 1);
        self.ensure_visible();
    }

    /// Moves the selection up by one viewport page.
    pub fn page_up(&mut self) {
        self.selected = self.selected.saturating_sub(self.viewport_height.max(1));
        self.ensure_visible();
    }

    /// Moves the selection down by one viewport page.
    pub fn page_down(&mut self) {
        if self.report.entries.is_empty() {
            return;
        }
        self.selected =
            (self.selected + self.viewport_height.max(1)).min(self.report.entries.len() - 1);
        self.ensure_visible();
    }

    /// Toggles focus between the table and detail panels.
    pub fn toggle_panel(&mut self) {
        self.panel = self.panel.next();
    }

    /// Sorts by the next column, keeping the table immediately up to date.
    pub fn next_sort_column(&mut self) {
        self.sort_column = self.sort_column.next();
        self.apply_sort();
    }

    /// Sorts by the previous column, keeping the table immediately up to date.
    pub fn previous_sort_column(&mut self) {
        self.sort_column = self.sort_column.previous();
        self.apply_sort();
    }

    /// Reverses the current sort direction.
    pub fn toggle_sort_direction(&mut self) {
        self.sort_descending = !self.sort_descending;
        self.apply_sort();
    }

    /// Sorts the displayed entries and resets the viewport to the first row.
    pub fn apply_sort(&mut self) {
        let column = self.sort_column;
        let descending = self.sort_descending;
        self.report.entries.sort_by(|left, right| {
            let ordering = match column {
                SortColumn::Key => left.key_identifier.cmp(&right.key_identifier),
                SortColumn::Durability => left
                    .durability
                    .to_string()
                    .cmp(&right.durability.to_string()),
                SortColumn::ByteSize => left.byte_size.cmp(&right.byte_size),
                SortColumn::RemainingLedgers => {
                    left.remaining_ledgers.cmp(&right.remaining_ledgers)
                }
                SortColumn::HealthStatus => risk_rank(left.status)
                    .cmp(&risk_rank(right.status))
                    .then_with(|| left.key_identifier.cmp(&right.key_identifier)),
            };
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
        self.selected = 0;
        self.table_offset = 0;
    }

    fn ensure_visible(&mut self) {
        let height = self.viewport_height.max(1);
        if self.selected >= self.table_offset + height {
            self.table_offset = self.selected - height + 1;
        } else if self.selected < self.table_offset {
            self.table_offset = self.selected;
        }
    }
}

fn risk_rank(status: RiskLevel) -> u8 {
    match status {
        RiskLevel::Ok => 0,
        RiskLevel::Warn => 1,
        RiskLevel::Critical => 2,
        RiskLevel::Archived => 3,
    }
}

/// Returns the selected entry, if any.
pub fn selected_entry<'app, 'net>(app: &'app App<'net>) -> Option<&'app StorageEntryReport> {
    app.report.entries.get(app.selected)
}

/// Computes a 0.0–1.0 health ratio relative to the warning threshold.
pub fn health_ratio(remaining_ledgers: i64, thresholds: ThresholdConfig) -> f64 {
    if thresholds.warning_ledgers == 0 || remaining_ledgers <= 0 {
        return 0.0;
    }
    (remaining_ledgers as f64 / f64::from(thresholds.warning_ledgers)).clamp(0.0, 1.0)
}

/// Maps a risk status to the gauge color.
pub fn health_color(status: RiskLevel) -> Color {
    match status {
        RiskLevel::Ok => Color::Green,
        RiskLevel::Warn => Color::Yellow,
        RiskLevel::Critical => Color::Red,
        RiskLevel::Archived => Color::DarkGray,
    }
}

/// Renders one frame of the dashboard.
pub fn draw(frame: &mut Frame<'_>, app: &mut App<'_>) {
    let area = frame.size();
    if area.width == 0 || area.height == 0 {
        return;
    }

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(area);

    render_tabs(frame, vertical[0], app);
    let middle = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
        .split(vertical[1]);
    render_table(frame, middle[0], app);
    render_detail(frame, middle[1], app);
    render_footer(frame, vertical[2], app);
}

fn render_tabs(frame: &mut Frame<'_>, area: ratatui::layout::Rect, app: &App<'_>) {
    let titles = vec![Line::from("Entries"), Line::from("Detail")];
    let tabs = Tabs::new(titles)
        .select(match app.panel {
            Panel::Entries => 0,
            Panel::Detail => 1,
        })
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Soroban Sentinel — {} ", app.network)),
        );
    frame.render_widget(tabs, area);
}

fn render_table(frame: &mut Frame<'_>, area: ratatui::layout::Rect, app: &mut App<'_>) {
    let focused = app.panel == Panel::Entries;
    let border_color = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let sort_marker = if app.sort_descending { "▼" } else { "▲" };
    let title = format!(
        " Storage Entries ({}) — sort: {} {} ",
        app.report.entries.len(),
        app.sort_column.label(),
        sort_marker
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(title);
    let inner = block.inner(area);
    app.viewport_height = usize::from(inner.height);

    let widths = vec![
        Constraint::Percentage(30),
        Constraint::Percentage(14),
        Constraint::Percentage(12),
        Constraint::Percentage(22),
        Constraint::Percentage(22),
    ];
    let column_titles = [
        "Entry Key",
        "Durability",
        "Byte Size",
        "Remaining Ledgers",
        "Health Status",
    ];
    let active = app.sort_column;
    let header = Row::new(
        column_titles
            .iter()
            .enumerate()
            .map(|(index, title)| {
                let sort = SortColumn::from_index(index).unwrap();
                if sort == active {
                    Cell::from(format!("{} {}", title, sort_marker))
                } else {
                    Cell::from(*title)
                }
            })
            .collect::<Vec<_>>(),
    )
    .style(Style::default().fg(Color::Yellow))
    .bottom_margin(1);

    let rows: Vec<Row<'_>> = app
        .report
        .entries
        .iter()
        .enumerate()
        .skip(app.table_offset)
        .take(app.viewport_height.max(1))
        .map(|(index, entry)| {
            let style = if index == app.selected && focused {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(display_key(&entry.key_identifier)),
                Cell::from(entry.durability.to_string()),
                Cell::from(entry.byte_size.to_string()),
                Cell::from(entry.remaining_ledgers.to_string()),
                Cell::from(entry.status.to_string()),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .highlight_style(Style::default().bg(Color::DarkGray));
    frame.render_widget(table, area);
}

impl SortColumn {
    fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Key),
            1 => Some(Self::Durability),
            2 => Some(Self::ByteSize),
            3 => Some(Self::RemainingLedgers),
            4 => Some(Self::HealthStatus),
            _ => None,
        }
    }
}

fn render_detail(frame: &mut Frame<'_>, area: ratatui::layout::Rect, app: &App<'_>) {
    let focused = app.panel == Panel::Detail;
    let border_color = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(" Entry Detail ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(entry) = selected_entry(app) else {
        frame.render_widget(
            Paragraph::new("No entries to display").wrap(Wrap { trim: true }),
            inner,
        );
        return;
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3)])
        .split(inner);

    let ratio = health_ratio(entry.remaining_ledgers, app.thresholds);
    let label = format!(
        "{:.0}%  {}/{} ledgers",
        ratio * 100.0,
        entry.remaining_ledgers.max(0),
        app.thresholds.warning_ledgers
    );
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(health_color(entry.status)))
        .ratio(ratio)
        .label(label);
    frame.render_widget(gauge, chunks[0]);

    let details = vec![
        Line::from(vec![
            Span::styled("Key: ", Style::default().fg(Color::Yellow)),
            Span::raw(display_key(&entry.key_identifier)),
        ]),
        Line::from(vec![
            Span::styled("Durability: ", Style::default().fg(Color::Yellow)),
            Span::raw(entry.durability.to_string()),
        ]),
        Line::from(vec![
            Span::styled("Live until: ", Style::default().fg(Color::Yellow)),
            Span::raw(entry.live_until_ledger.to_string()),
        ]),
        Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::Yellow)),
            Span::styled(
                entry.status.to_string(),
                Style::default().fg(health_color(entry.status)),
            ),
        ]),
        Line::from(vec![
            Span::styled("Thresholds: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!(
                "warn {} / critical {}",
                app.thresholds.warning_ledgers, app.thresholds.critical_ledgers
            )),
        ]),
    ];
    frame.render_widget(Paragraph::new(details).wrap(Wrap { trim: true }), chunks[1]);
}

fn render_footer(frame: &mut Frame<'_>, area: ratatui::layout::Rect, app: &App<'_>) {
    let panel_hint = match app.panel {
        Panel::Entries => "Up/Down: select  PgUp/PgDn: page",
        Panel::Detail => "Tab: switch panel",
    };
    let footer = Paragraph::new(format!(
        " Tab: panel | ←/→: sort | s: reverse | {} | q/Esc: quit | {} entries ",
        panel_hint,
        app.report.entries.len()
    ))
    .wrap(Wrap { trim: true })
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, area);
}

fn display_key(identifier: &str) -> String {
    identifier
        .strip_prefix("persistent:")
        .unwrap_or(identifier)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::{Durability, EntryType, StorageEntryReport};

    fn thresholds() -> ThresholdConfig {
        ThresholdConfig::new(100, 25)
    }

    fn report(count: usize) -> TTLReport {
        let entries = (0..count)
            .map(|index| {
                StorageEntryReport::new(
                    format!("persistent:KEY{}", index),
                    EntryType::PersistentData,
                    Durability::Persistent,
                    64,
                    1_000,
                    index as i64 * 10,
                    0.0,
                    0.0,
                    RiskLevel::Ok,
                )
            })
            .collect();
        TTLReport {
            contract_id: "CTEST".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 900,
            ledger_velocity_seconds: 5.0,
            entries,
            overall_status: RiskLevel::Ok,
            archival_recommendations: Vec::new(),
            sac_asset_info: None,
        }
    }

    #[test]
    fn health_ratio_is_clamped() {
        assert_eq!(health_ratio(-1, thresholds()), 0.0);
        assert_eq!(health_ratio(0, thresholds()), 0.0);
        assert_eq!(health_ratio(50, thresholds()), 0.5);
        assert_eq!(health_ratio(500, thresholds()), 1.0);
    }

    #[test]
    fn navigation_clamps_and_scrolls() {
        let mut report = report(10);
        report.entries.reverse();
        let mut app = App::new(report, thresholds(), "testnet");
        app.viewport_height = 3;

        assert_eq!(
            selected_entry(&app).unwrap().key_identifier,
            "persistent:KEY0"
        );

        app.move_up();
        assert_eq!(app.selected, 0);
        assert_eq!(app.table_offset, 0);

        for _ in 0..9 {
            app.move_down();
        }
        assert_eq!(app.selected, 9);
        assert_eq!(app.table_offset, 7);

        app.page_down();
        assert_eq!(app.selected, 9);
        app.page_up();
        assert_eq!(app.selected, 6);
        assert_eq!(app.table_offset, 6);
    }

    #[test]
    fn toggle_panel_cycles() {
        let app = App::new(report(1), thresholds(), "testnet");
        assert_eq!(app.panel, Panel::Entries);
        let mut app = app;
        app.toggle_panel();
        assert_eq!(app.panel, Panel::Detail);
        app.toggle_panel();
        assert_eq!(app.panel, Panel::Entries);
    }

    #[test]
    fn display_key_strips_persistent_prefix() {
        assert_eq!(display_key("persistent:ADMIN"), "ADMIN");
        assert_eq!(display_key("instance"), "instance");
    }

    #[test]
    fn sorting_is_applied_immediately_in_both_directions() {
        let mut report = report(3);
        report.entries[0].remaining_ledgers = 30;
        report.entries[1].remaining_ledgers = 10;
        report.entries[2].remaining_ledgers = 20;

        let mut app = App::new(report, thresholds(), "testnet");
        assert_eq!(
            selected_entry(&app).unwrap().key_identifier,
            "persistent:KEY1"
        );

        app.toggle_sort_direction();
        assert_eq!(
            selected_entry(&app).unwrap().key_identifier,
            "persistent:KEY0"
        );

        app.sort_column = SortColumn::Key;
        app.apply_sort();
        assert_eq!(
            selected_entry(&app).unwrap().key_identifier,
            "persistent:KEY2"
        );
    }
}
