use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use super::super::{App, THEME};

pub(super) fn draw_menu(f: &mut ratatui::Frame, area: Rect, app: &App) {
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

    let title = Paragraph::new(Line::from(vec![Span::styled(
        "Saelora Admin",
        Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Enter select | Ctrl+C quit",
        Style::default().fg(THEME.sub),
    )]));
    let stats = Paragraph::new(Line::from(vec![Span::styled(
        format!(
            "Messages sent: {}  ·  received: {}",
            app.msgs_sent, app.msgs_recv
        ),
        Style::default().fg(THEME.sub),
    )]));
    f.render_widget(title, chunks[0]);
    f.render_widget(stats, chunks[1]);
    f.render_widget(help, chunks[2]);

    let items = [
        "OpenRouter config".to_string(),
        "System Prompt".to_string(),
        "Mail (Mailjet)".to_string(),
        format!("Invites ({}/{})", app.invites_n, app.whitelist_n),
        format!("Users ({})", app.users_n),
        "Agents".to_string(),
    ];

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        if i == app.menu_idx {
            lines.push(Line::from(Span::styled(
                it.clone(),
                Style::default()
                    .fg(THEME.base)
                    .bg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                it.clone(),
                Style::default().fg(THEME.text),
            )));
        }
    }
    let list = Paragraph::new(Text::from(lines)).style(Style::default().bg(THEME.surface));
    f.render_widget(list, chunks[3]);
}
