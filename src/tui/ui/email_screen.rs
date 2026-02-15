use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use super::super::{App, THEME};
use super::util::{mask_key, render_field};

pub(super) fn draw_email_config(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(area);

    let title = Paragraph::new(Line::from(vec![Span::styled(
        "Mailjet",
        Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Tab/Shift+Tab move | Ctrl+S save | Esc back",
        Style::default().fg(THEME.sub),
    )]));
    f.render_widget(title, chunks[0]);
    f.render_widget(help, chunks[1]);

    let rows = vec![
        render_field("API key:", &mask_key(&app.mail_api_key), app.focus == 0),
        render_field(
            "API secret:",
            &mask_key(&app.mail_api_secret),
            app.focus == 1,
        ),
        render_field("From email:", &app.mail_from_email, app.focus == 2),
        render_field("From name:", &app.mail_from_name, app.focus == 3),
        render_field(
            "Mailjet base URL (optional):",
            &app.mail_base_url,
            app.focus == 4,
        ),
        render_field("Public base (links):", &app.public_base, app.focus == 5),
    ];
    let content = Paragraph::new(Text::from(rows));
    f.render_widget(content, chunks[2]);

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
    f.render_widget(Paragraph::new(status).style(status_style), chunks[3]);
}
