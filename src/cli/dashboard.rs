//! sls dashboard - Interactive TUI dashboard for SLS monitoring
//!
//! Provides a keyboard-navigable, continuously updating terminal UI showing:
//! - System health status
//! - Indexed entries and sources count
//! - Recent errors/warnings with scrolling
//! - Activity timeline

use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::io;
use std::time::Duration;

use crate::db::{default_db_path, Database};
use crate::indexer::Indexer;

/// Dashboard application state
struct App {
    stats: DashboardStats,
    error_list_state: ListState,
    should_quit: bool,
    last_refresh: DateTime<Utc>,
}

impl App {
    fn new() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            stats: DashboardStats::default(),
            error_list_state: list_state,
            should_quit: false,
            last_refresh: Utc::now(),
        }
    }

    fn next_error(&mut self) {
        let len = self.stats.recent_errors.len();
        if len == 0 {
            return;
        }
        let i = match self.error_list_state.selected() {
            Some(i) => (i + 1) % len,
            None => 0,
        };
        self.error_list_state.select(Some(i));
    }

    fn prev_error(&mut self) {
        let len = self.stats.recent_errors.len();
        if len == 0 {
            return;
        }
        let i = match self.error_list_state.selected() {
            Some(i) => {
                if i == 0 {
                    len - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.error_list_state.select(Some(i));
    }
}

/// Dashboard statistics
#[derive(Debug, Clone, Default)]
struct DashboardStats {
    healthy: bool,
    indexed_entries: usize,
    sources_count: usize,
    last_indexed: Option<DateTime<Utc>>,
    recent_errors: Vec<RecentError>,
    error_count_1h: usize,
    warning_count_1h: usize,
}

#[derive(Debug, Clone)]
struct RecentError {
    timestamp: DateTime<Utc>,
    level: String,
    service: Option<String>,
    message: String,
}

/// Run the interactive TUI dashboard
pub async fn run(refresh_secs: u64) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut app = App::new();

    // Initial data load
    app.stats = collect_stats().await?;
    app.last_refresh = Utc::now();

    let refresh_duration = Duration::from_secs(refresh_secs);
    let tick_rate = Duration::from_millis(100);

    // Main loop
    let result = run_app(&mut terminal, &mut app, refresh_duration, tick_rate).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    refresh_duration: Duration,
    tick_rate: Duration,
) -> Result<()> {
    let mut last_tick = std::time::Instant::now();
    let mut last_refresh = std::time::Instant::now();

    loop {
        terminal.draw(|f| ui(f, app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                        KeyCode::Down | KeyCode::Char('j') => app.next_error(),
                        KeyCode::Up | KeyCode::Char('k') => app.prev_error(),
                        KeyCode::Char('r') => {
                            // Force refresh
                            app.stats = collect_stats().await?;
                            app.last_refresh = Utc::now();
                            last_refresh = std::time::Instant::now();
                        }
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = std::time::Instant::now();
        }

        // Auto-refresh stats
        if last_refresh.elapsed() >= refresh_duration {
            app.stats = collect_stats().await?;
            app.last_refresh = Utc::now();
            last_refresh = std::time::Instant::now();
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    // Main layout: header, body, footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Min(10),    // Body
            Constraint::Length(3),  // Footer
        ])
        .split(f.area());

    // Header
    let header = render_header(app);
    f.render_widget(header, chunks[0]);

    // Body - split into left (stats) and right (errors)
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[1]);

    // Left panel - Stats
    let stats_panel = render_stats_panel(&app.stats);
    f.render_widget(stats_panel, body_chunks[0]);

    // Right panel - Errors list
    let (errors_list, list_state) = render_errors_list(&app.stats, &mut app.error_list_state);
    f.render_stateful_widget(errors_list, body_chunks[1], list_state);

    // Footer
    let footer = render_footer(app);
    f.render_widget(footer, chunks[2]);
}

fn render_header(app: &App) -> Paragraph<'static> {
    let now = app.last_refresh.format("%Y-%m-%d %H:%M:%S UTC");
    let status = if app.stats.healthy {
        Span::styled(" HEALTHY ", Style::default().bg(Color::Green).fg(Color::Black))
    } else {
        Span::styled(" UNHEALTHY ", Style::default().bg(Color::Red).fg(Color::White))
    };

    let header_text = Line::from(vec![
        Span::styled("SLS Dashboard", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" | "),
        status,
        Span::raw(" | "),
        Span::styled(now.to_string(), Style::default().fg(Color::DarkGray)),
    ]);

    Paragraph::new(header_text)
        .block(Block::default().borders(Borders::ALL).title(" sls "))
        .style(Style::default())
}

fn render_stats_panel(stats: &DashboardStats) -> Paragraph<'static> {
    let last_indexed_str = stats
        .last_indexed
        .map(|ts| {
            let age = Utc::now().signed_duration_since(ts);
            if age.num_seconds() < 60 {
                format!("{}s ago", age.num_seconds())
            } else if age.num_minutes() < 60 {
                format!("{}m ago", age.num_minutes())
            } else if age.num_hours() < 24 {
                format!("{}h ago", age.num_hours())
            } else {
                format!("{}d ago", age.num_days())
            }
        })
        .unwrap_or_else(|| "never".to_string());

    let error_style = if stats.error_count_1h > 0 {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Green)
    };

    let warning_style = if stats.warning_count_1h > 0 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Green)
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("Indexed Entries: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(stats.indexed_entries.to_string()),
        ]),
        Line::from(vec![
            Span::styled("Active Sources:  ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(stats.sources_count.to_string()),
        ]),
        Line::from(vec![
            Span::styled("Last Indexed:    ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(last_indexed_str),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Last Hour Activity",
            Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )),
        Line::from(vec![
            Span::styled("Errors:   ", Style::default()),
            Span::styled(stats.error_count_1h.to_string(), error_style),
        ]),
        Line::from(vec![
            Span::styled("Warnings: ", Style::default()),
            Span::styled(stats.warning_count_1h.to_string(), warning_style),
        ]),
    ];

    Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Status "))
        .wrap(Wrap { trim: false })
}

fn render_errors_list<'a>(
    stats: &'a DashboardStats,
    list_state: &'a mut ListState,
) -> (List<'a>, &'a mut ListState) {
    let items: Vec<ListItem> = if stats.recent_errors.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "No recent errors or warnings",
            Style::default().fg(Color::Green),
        )))]
    } else {
        stats
            .recent_errors
            .iter()
            .map(|err| {
                let time_str = err.timestamp.format("%H:%M:%S").to_string();
                let level_style = match err.level.as_str() {
                    "ERROR" => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    "WARN" | "WARNING" => Style::default().fg(Color::Yellow),
                    _ => Style::default(),
                };
                let service = err.service.as_deref().unwrap_or("unknown");
                let msg = if err.message.len() > 45 {
                    format!("{}...", &err.message[..42])
                } else {
                    err.message.clone()
                };

                let lines = vec![
                    Line::from(vec![
                        Span::styled(format!("[{}] ", time_str), Style::default().fg(Color::DarkGray)),
                        Span::styled(format!("{:<7}", err.level), level_style),
                        Span::raw(format!(" {}", service)),
                    ]),
                    Line::from(Span::styled(format!("  {}", msg), Style::default().fg(Color::Gray))),
                ];
                ListItem::new(lines)
            })
            .collect()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Recent Errors & Warnings "),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    (list, list_state)
}

fn render_footer(_app: &App) -> Paragraph<'static> {
    let help_text = Line::from(vec![
        Span::styled("q", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(": quit | "),
        Span::styled("j/k", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(": scroll | "),
        Span::styled("r", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(": refresh"),
    ]);

    Paragraph::new(help_text)
        .block(Block::default().borders(Borders::ALL))
        .style(Style::default().fg(Color::DarkGray))
}

async fn collect_stats() -> Result<DashboardStats> {
    let db_path = default_db_path();
    let db_exists = db_path.exists();

    if !db_exists {
        return Ok(DashboardStats {
            healthy: false,
            indexed_entries: 0,
            sources_count: 0,
            last_indexed: None,
            recent_errors: vec![],
            error_count_1h: 0,
            warning_count_1h: 0,
        });
    }

    // Get indexer stats
    let (indexed_entries, sources_count, last_indexed) = match Indexer::open_default() {
        Ok(indexer) => {
            let stats = indexer.stats().await?;
            let last_ts = stats
                .last_indexed_timestamp
                .and_then(|ts| Utc.timestamp_opt(ts, 0).single());
            (stats.total_entries, stats.total_sources, last_ts)
        }
        Err(_) => (0, 0, None),
    };

    // Get recent errors from database
    let (recent_errors, error_count_1h, warning_count_1h) = get_recent_errors()?;

    Ok(DashboardStats {
        healthy: sources_count > 0,
        indexed_entries,
        sources_count,
        last_indexed,
        recent_errors,
        error_count_1h,
        warning_count_1h,
    })
}

fn get_recent_errors() -> Result<(Vec<RecentError>, usize, usize)> {
    let db_path = default_db_path();
    if !db_path.exists() {
        return Ok((vec![], 0, 0));
    }

    let db = Database::open(&db_path)?;

    // Get recent errors (last 20 for scrolling)
    let mut stmt = db.conn().prepare(
        "SELECT timestamp_utc, level, service, message
         FROM log_entries
         WHERE level IN ('ERROR', 'WARN', 'WARNING')
         ORDER BY timestamp_utc DESC
         LIMIT 20",
    )?;

    let recent_errors = stmt
        .query_map([], |row| {
            let ts: i64 = row.get(0)?;
            let level: Option<String> = row.get(1)?;
            let service: Option<String> = row.get(2)?;
            let message: String = row.get(3)?;

            Ok(RecentError {
                timestamp: Utc.timestamp_opt(ts, 0).single().unwrap_or_else(Utc::now),
                level: level.unwrap_or_else(|| "UNKNOWN".to_string()),
                service,
                message,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Count errors/warnings in last hour
    let one_hour_ago = Utc::now().timestamp() - 3600;

    let error_count: usize = db.conn().query_row(
        "SELECT COUNT(*) FROM log_entries WHERE level = 'ERROR' AND timestamp_utc > ?",
        [one_hour_ago],
        |row| row.get(0),
    )?;

    let warning_count: usize = db.conn().query_row(
        "SELECT COUNT(*) FROM log_entries WHERE level IN ('WARN', 'WARNING') AND timestamp_utc > ?",
        [one_hour_ago],
        |row| row.get(0),
    )?;

    Ok((recent_errors, error_count, warning_count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dashboard_stats_creation() {
        let stats = DashboardStats {
            healthy: true,
            indexed_entries: 100,
            sources_count: 5,
            last_indexed: Some(Utc::now()),
            recent_errors: vec![],
            error_count_1h: 0,
            warning_count_1h: 0,
        };
        assert!(stats.healthy);
        assert_eq!(stats.indexed_entries, 100);
    }

    #[test]
    fn test_app_error_navigation() {
        let mut app = App::new();
        app.stats.recent_errors = vec![
            RecentError {
                timestamp: Utc::now(),
                level: "ERROR".to_string(),
                service: Some("test".to_string()),
                message: "Error 1".to_string(),
            },
            RecentError {
                timestamp: Utc::now(),
                level: "WARN".to_string(),
                service: Some("test".to_string()),
                message: "Warning 1".to_string(),
            },
        ];

        assert_eq!(app.error_list_state.selected(), Some(0));
        app.next_error();
        assert_eq!(app.error_list_state.selected(), Some(1));
        app.next_error();
        assert_eq!(app.error_list_state.selected(), Some(0)); // wraps
        app.prev_error();
        assert_eq!(app.error_list_state.selected(), Some(1)); // wraps back
    }
}
