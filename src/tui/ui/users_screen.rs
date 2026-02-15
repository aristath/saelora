use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table};

use super::super::App;
use super::util::{fmt_ts_ms, trunc};

pub(super) fn draw_users(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Percentage(45),
                Constraint::Percentage(45),
            ]
            .as_ref(),
        )
        .split(area);

    let title = if app.loading_users {
        "Users ...".to_string()
    } else {
        "Users".to_string()
    };
    let title = Paragraph::new(Line::from(vec![Span::styled(
        title,
        Style::default().add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Up/Down: move | A: active | D: disable | P: pending | R: reload | Esc: back",
        Style::default().fg(Color::DarkGray),
    )]));
    f.render_widget(title, chunks[0]);
    f.render_widget(help, chunks[1]);

    let mut status = String::new();
    if !app.err.trim().is_empty() {
        status = format!("Error: {}", app.err.trim());
    } else if !app.info.trim().is_empty() {
        status = app.info.trim().to_string();
    }
    let status_style = if !app.err.trim().is_empty() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(Paragraph::new(status).style(status_style), chunks[2]);

    if app.loading_users {
        f.render_widget(
            Paragraph::new("Loading users...").style(Style::default().fg(Color::DarkGray)),
            chunks[3],
        );
        return;
    }
    if app.users.is_empty() {
        f.render_widget(
            Paragraph::new("No users.").style(Style::default().fg(Color::DarkGray)),
            chunks[3],
        );
        return;
    }

    let cols = [("Email", 42), ("Status", 10), ("Created", 28), ("ID", 12)];
    let header = Row::new(cols.iter().map(|(t, _)| Cell::from(*t))).style(
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::White),
    );
    let rows: Vec<Row> = app
        .users
        .iter()
        .map(|r| {
            Row::new(vec![
                Cell::from(r.email.clone()),
                Cell::from(r.status.clone()),
                Cell::from(fmt_ts_ms(r.created_at)),
                Cell::from(trunc(&r.id, 12)),
            ])
        })
        .collect();
    let widths: Vec<Constraint> = cols.iter().map(|(_, w)| Constraint::Length(*w)).collect();
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .block(Block::default());
    let mut state = app.users_state;
    f.render_stateful_widget(table, chunks[3], &mut state);
}
