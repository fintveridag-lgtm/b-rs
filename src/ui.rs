use crate::state::{Flags, SharedState};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table};
use ratatui::{Frame, Terminal};
use std::io::stdout;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// Blokkerende TUI-løkke. Taster: q = avslutt, k = kill switch av/på,
/// p = pause strategi av/på.
pub fn run(state: SharedState, flags: Arc<Flags>) -> Result<()> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let result = event_loop(&mut terminal, &state, &flags);

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    state: &SharedState,
    flags: &Arc<Flags>,
) -> Result<()> {
    loop {
        terminal.draw(|f| draw(f, state, flags))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        flags.quit.store(true, Ordering::Relaxed);
                        return Ok(());
                    }
                    KeyCode::Char('k') => {
                        let was = flags.killed.load(Ordering::Relaxed);
                        flags.killed.store(!was, Ordering::Relaxed);
                    }
                    KeyCode::Char('p') => {
                        let was = flags.paused.load(Ordering::Relaxed);
                        flags.paused.store(!was, Ordering::Relaxed);
                        state.lock().unwrap().log(if was {
                            "Strategi gjenopptatt."
                        } else {
                            "Strategi satt på pause."
                        });
                    }
                    _ => {}
                }
            }
        }
        if flags.quit() {
            return Ok(());
        }
    }
}

fn draw(f: &mut Frame, state: &SharedState, flags: &Arc<Flags>) {
    let st = state.lock().unwrap();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Percentage(40),
            Constraint::Percentage(35),
            Constraint::Min(5),
        ])
        .split(f.area());

    // --- Topplinje ---
    let mode_span = if st.mode == "live" {
        Span::styled(" LIVE ", Style::default().fg(Color::White).bg(Color::Red).add_modifier(Modifier::BOLD))
    } else {
        Span::styled(" PAPIR ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD))
    };
    let status_span = if flags.killed() {
        Span::styled(" KILL SWITCH ", Style::default().fg(Color::White).bg(Color::Red).add_modifier(Modifier::BOLD))
    } else if flags.paused() {
        Span::styled(" PAUSE ", Style::default().fg(Color::Black).bg(Color::Yellow))
    } else {
        Span::styled(" AKTIV ", Style::default().fg(Color::Black).bg(Color::Cyan))
    };
    let dd_color = if st.drawdown < 0.0 { Color::Red } else { Color::Green };
    let header = Line::from(vec![
        Span::styled(" b-rs ", Style::default().add_modifier(Modifier::BOLD)),
        mode_span,
        Span::raw(format!(" megler: {} ", st.broker_name)),
        status_span,
        Span::raw(format!("  kontanter: {:.0}  egenkapital: {:.0}  ", st.cash, st.equity)),
        Span::styled(format!("P&L: {:+.0}", st.drawdown), Style::default().fg(dd_color)),
        Span::raw("   [q] avslutt  [k] kill  [p] pause"),
    ]);
    f.render_widget(
        Paragraph::new(header).block(Block::bordered()),
        rows[0],
    );

    // --- Midtseksjon: watchlist + posisjoner ---
    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);

    draw_watchlist(f, mid[0], &st);
    draw_positions(f, mid[1], &st);

    // --- Ordrer ---
    draw_orders(f, rows[2], &st);

    // --- Logg ---
    let log_lines: Vec<Line> = st
        .logs
        .iter()
        .take(rows[3].height.saturating_sub(2) as usize)
        .map(|(ts, msg)| {
            Line::from(vec![
                Span::styled(format!("{} ", ts.with_timezone(&chrono::Local).format("%H:%M:%S")), Style::default().fg(Color::DarkGray)),
                Span::raw(msg.clone()),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(log_lines).block(Block::bordered().title(" Hendelser ")),
        rows[3],
    );
}

fn draw_watchlist(f: &mut Frame, area: Rect, st: &crate::state::UiState) {
    let header = Row::new(["Symbol", "Siste", "Endring"])
        .style(Style::default().add_modifier(Modifier::BOLD));
    let rows: Vec<Row> = st
        .quotes
        .values()
        .map(|q| {
            let pct = q.change_pct();
            let color = if pct >= 0.0 { Color::Green } else { Color::Red };
            Row::new(vec![
                Cell::from(q.symbol.clone()),
                Cell::from(format!("{:.2}", q.last)),
                Cell::from(Span::styled(format!("{pct:+.2}%"), Style::default().fg(color))),
            ])
        })
        .collect();
    let title = match st.last_tick {
        Some(ts) => format!(" Watchlist (oppdatert {}) ", ts.with_timezone(&chrono::Local).format("%H:%M:%S")),
        None => " Watchlist (venter på data …) ".to_string(),
    };
    let table = Table::new(
        rows,
        [Constraint::Length(12), Constraint::Length(10), Constraint::Length(10)],
    )
    .header(header)
    .block(Block::bordered().title(title));
    f.render_widget(table, area);
}

fn draw_positions(f: &mut Frame, area: Rect, st: &crate::state::UiState) {
    let header = Row::new(["Symbol", "Antall", "Snitt", "Verdi", "Urealisert"])
        .style(Style::default().add_modifier(Modifier::BOLD));
    let mut rows: Vec<Row> = st
        .positions
        .iter()
        .map(|p| {
            let unreal = p.unrealized();
            let color = if unreal >= 0.0 { Color::Green } else { Color::Red };
            Row::new(vec![
                Cell::from(p.symbol.clone()),
                Cell::from(format!("{:.0}", p.qty)),
                Cell::from(format!("{:.2}", p.avg_price)),
                Cell::from(format!("{:.0}", p.market_value())),
                Cell::from(Span::styled(format!("{unreal:+.0}"), Style::default().fg(color))),
            ])
        })
        .collect();

    // Nordnet-posisjoner (lesemodus) vises i samme panel, merket [NN].
    for p in &st.nordnet_positions {
        let label = if p.symbol == "?" { &p.name } else { &p.symbol };
        rows.push(Row::new(vec![
            Cell::from(Span::styled(
                format!("[NN] {label}"),
                Style::default().fg(Color::Blue),
            )),
            Cell::from(format!("{:.0}", p.qty)),
            Cell::from("-".to_string()),
            Cell::from(format!("{:.0}", p.market_value)),
            Cell::from("-".to_string()),
        ]));
    }

    let title = if st.nordnet_enabled {
        " Posisjoner (bot + [NN] Nordnet lesemodus) "
    } else {
        " Posisjoner "
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Length(8),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(11),
        ],
    )
    .header(header)
    .block(Block::bordered().title(title));
    f.render_widget(table, area);
}

fn draw_orders(f: &mut Frame, area: Rect, st: &crate::state::UiState) {
    let header = Row::new(["Tid", "Id", "Side", "Symbol", "Antall", "Kurs", "Status"])
        .style(Style::default().add_modifier(Modifier::BOLD));
    let rows: Vec<Row> = st
        .orders
        .iter()
        .take(area.height.saturating_sub(3) as usize)
        .map(|o| {
            let side_color = match o.side {
                crate::types::Side::Buy => Color::Green,
                crate::types::Side::Sell => Color::Red,
            };
            Row::new(vec![
                Cell::from(o.created.with_timezone(&chrono::Local).format("%H:%M:%S").to_string()),
                Cell::from(o.id.clone()),
                Cell::from(Span::styled(o.side.to_string(), Style::default().fg(side_color))),
                Cell::from(o.symbol.clone()),
                Cell::from(format!("{:.0}", o.qty)),
                Cell::from(format!("{:.2}", o.avg_price)),
                Cell::from(o.status.to_string()),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Length(9),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .block(Block::bordered().title(" Ordrer "));
    f.render_widget(table, area);
}
