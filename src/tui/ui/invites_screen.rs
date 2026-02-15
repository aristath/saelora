use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table};

use super::super::{App, THEME};
use super::util::{fmt_ts_ms, zebra_style};

pub(super) fn draw_invites(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ]
            .as_ref(),
        )
        .split(area);

    let title = if app.loading_waitlist {
        format!("Invites ({}/{}) ...", app.invites_n, app.whitelist_n)
    } else {
        format!("Invites ({}/{})", app.invites_n, app.whitelist_n)
    };
    let title = Paragraph::new(Line::from(vec![Span::styled(
        title,
        Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Up/Down: move | Enter: whitelist | X: remove | R: reload | Esc: back",
        Style::default().fg(THEME.sub),
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
        Style::default().fg(THEME.danger)
    } else {
        Style::default().fg(THEME.sub)
    };
    f.render_widget(Paragraph::new(status).style(status_style), chunks[2]);

    let body = chunks[3];
    let halves = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(body);

    // Left: waitlist.
    let header = Row::new(vec![Cell::from("Waitlist Email"), Cell::from("Requested")])
        .style(Style::default().fg(THEME.base).bg(THEME.accent_alt));
    let rows: Vec<Row> = app
        .waitlist
        .iter()
        .enumerate()
        .map(|(idx, r)| {
            let style = zebra_style(idx, app.waitlist_state.selected() == Some(idx));
            Row::new(vec![
                Cell::from(r.email.clone()),
                Cell::from(fmt_ts_ms(r.created_at)),
            ])
            .style(style)
        })
        .collect();
    let widths = [Constraint::Percentage(70), Constraint::Percentage(30)];
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().fg(THEME.base).bg(THEME.accent))
        .block(Block::default());
    let mut s1 = app.waitlist_state;
    f.render_stateful_widget(table, halves[0], &mut s1);

    // Right: whitelist.
    let header2 = Row::new(vec![Cell::from("Whitelisted Email"), Cell::from("Added")])
        .style(Style::default().fg(THEME.base).bg(THEME.accent_alt));
    let rows2: Vec<Row> = app
        .whitelist
        .iter()
        .enumerate()
        .map(|(idx, r)| {
            let style = zebra_style(idx, app.whitelist_state.selected() == Some(idx));
            Row::new(vec![
                Cell::from(r.email.clone()),
                Cell::from(fmt_ts_ms(r.created_at)),
            ])
            .style(style)
        })
        .collect();
    let table2 = Table::new(rows2, widths)
        .header(header2)
        .row_highlight_style(Style::default().fg(THEME.base).bg(THEME.accent))
        .block(Block::default());
    let mut s2 = app.whitelist_state;
    f.render_stateful_widget(table2, halves[1], &mut s2);
}
