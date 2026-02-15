use chrono::TimeZone;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::super::{App, THEME};

pub(super) fn render_field(label: &str, val: &str, focused: bool) -> Line<'static> {
    if focused {
        Line::from(vec![
            Span::styled(
                label.to_string(),
                Style::default().fg(THEME.base).bg(THEME.accent),
            ),
            Span::raw(" "),
            Span::styled(
                val.to_string(),
                Style::default().fg(THEME.base).bg(THEME.accent),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(label.to_string(), Style::default().fg(THEME.sub)),
            Span::raw(" "),
            Span::styled(val.to_string(), Style::default().fg(THEME.text)),
        ])
    }
}

pub(super) fn mask_key(k: &str) -> String {
    let t = k.trim();
    if t.is_empty() {
        return "".to_string();
    }
    let n = t.len().min(32);
    "*".repeat(n)
}

pub(super) fn stats_line(app: &App) -> Line<'static> {
    Line::from(Span::styled(
        format!(
            "Messages sent: {}  ·  received: {}",
            app.msgs_sent, app.msgs_recv
        ),
        Style::default().fg(Color::DarkGray),
    ))
}

pub(super) fn zebra_style(idx: usize, selected: bool) -> Style {
    if selected {
        Style::default()
            .fg(THEME.base)
            .bg(THEME.accent)
            .add_modifier(Modifier::BOLD)
    } else if idx.is_multiple_of(2) {
        Style::default().fg(THEME.text).bg(THEME.surface)
    } else {
        Style::default().fg(THEME.text).bg(THEME.surface_alt)
    }
}

pub(super) fn fmt_ts_ms(ms: i64) -> String {
    chrono::Utc
        .timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| ms.to_string())
}

pub(super) fn trunc(s: &str, max: usize) -> String {
    let t = s.trim();
    if max == 0 || t.chars().count() <= max {
        return t.to_string();
    }
    if max <= 3 {
        return t.chars().take(max).collect();
    }
    let mut out: String = t.chars().take(max - 3).collect();
    out.push_str("...");
    out
}

pub(super) fn wrap_lines(s: &str, width: usize) -> Vec<Line<'static>> {
    if width < 10 {
        return vec![Line::from(s.to_string())];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        if cur.is_empty() {
            cur.push_str(word);
            continue;
        }
        if cur.len() + 1 + word.len() > width {
            out.push(Line::from(cur.clone()));
            cur.clear();
            cur.push_str(word);
        } else {
            cur.push(' ');
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        out.push(Line::from(cur));
    }
    out
}
