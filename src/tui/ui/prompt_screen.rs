use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::super::{App, THEME};
use super::util::stats_line;

pub(super) fn draw_prompt(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(area);

    let title = Paragraph::new(Line::from(vec![Span::styled(
        "System Prompt",
        Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Ctrl+S save | Esc back",
        Style::default().fg(THEME.sub),
    )]));
    let stats = Paragraph::new(stats_line(app));
    f.render_widget(title, chunks[0]);
    f.render_widget(help, chunks[1]);
    f.render_widget(stats, chunks[2]);

    app.system_prompt.render(f, chunks[3]);

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
    f.render_widget(Paragraph::new(status).style(status_style), chunks[4]);
}
