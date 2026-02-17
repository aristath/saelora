use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use crate::config;

use super::super::{App, THEME};
use super::util::{mask_key, render_field, zebra_style};

pub(super) fn draw_agents(f: &mut ratatui::Frame, area: Rect, app: &App) {
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
        "Agents",
        Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Up/Down: select | Tab: next field | P: toggle provider | C/Y/M/E: bind task | Ctrl+S save | Esc back",
        Style::default().fg(THEME.sub),
    )]));
    let tasks = Paragraph::new(Line::from(vec![Span::styled(
        format!(
            "Chat: {}   Summary: {}   Curator: {}   Embed: {}",
            app.task_chat, app.task_summary, app.task_memory_curator, app.task_memory_embed
        ),
        Style::default().fg(THEME.sub),
    )]));

    f.render_widget(title, chunks[0]);
    f.render_widget(help, chunks[1]);
    f.render_widget(tasks, chunks[2]);

    let body = chunks[3];
    let halves = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)].as_ref())
        .split(body);

    // Left: agents table.
    let cols = [("Name", 14), ("Provider", 10), ("Model", 26)];
    let header = Row::new(cols.iter().map(|(t, _)| Cell::from(*t))).style(
        Style::default()
            .fg(THEME.base)
            .bg(THEME.accent_alt)
            .add_modifier(Modifier::BOLD),
    );
    let mut rows: Vec<Row> = app
        .agents
        .iter()
        .enumerate()
        .map(|(idx, a)| {
            let style = zebra_style(idx, app.agents_state.selected() == Some(idx));
            Row::new(vec![
                Cell::from(a.name.clone()),
                Cell::from(match a.provider {
                    config::Provider::OpenRouter => "openrouter",
                    config::Provider::Ollama => "ollama",
                }),
                Cell::from(a.model.clone()),
            ])
            .style(style)
        })
        .collect();
    rows.push(
        Row::new(vec![
            Cell::from("Add agent"),
            Cell::from(""),
            Cell::from(""),
        ])
        .style(zebra_style(
            app.agents.len(),
            app.agents_state.selected() == Some(app.agents.len()),
        )),
    );
    let widths: Vec<Constraint> = cols.iter().map(|(_, w)| Constraint::Length(*w)).collect();
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().fg(THEME.base).bg(THEME.accent))
        .block(Block::default());
    let mut state = app.agents_state;
    f.render_stateful_widget(table, halves[0], &mut state);

    // Right: modal editor.
    if app.agent_modal && app.agent_modal_idx.is_some() {
        draw_agent_modal(f, area, app);
    }
}

fn draw_agent_modal(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let Some(idx) = app.agent_modal_idx else {
        return;
    };
    if idx >= app.agents.len() {
        return;
    }
    let agent = &app.agents[idx];
    let w = (area.width as f32 * 0.92) as u16;
    let h = 13u16;
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    let modal_area = Rect::new(x, y, w, h);
    let provider = match agent.provider {
        config::Provider::OpenRouter => "openrouter",
        config::Provider::Ollama => "ollama",
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Agent")
        .border_style(Style::default().fg(THEME.accent_alt));
    f.render_widget(block.clone(), modal_area);
    let inner = block.inner(modal_area);

    let rows = vec![
        render_field("Name:", &agent.name, app.agent_focus == 0),
        render_field("Provider (P):", provider, app.agent_focus == 1),
        render_field(
            "Model (Enter):",
            if app.loading_agent_models {
                "loading..."
            } else {
                &agent.model
            },
            app.agent_focus == 2,
        ),
        render_field("Base URL:", &agent.base_url, app.agent_focus == 3),
        render_field("API key:", &mask_key(&agent.api_key), app.agent_focus == 4),
        render_field("HTTP Referer:", &agent.http_referer, app.agent_focus == 5),
        render_field("X-Title:", &agent.x_title, app.agent_focus == 6),
        render_field("Bind chat (C):", "", app.agent_focus == 7),
        render_field("Bind summary (Y):", "", app.agent_focus == 8),
        render_field("Bind memory curator (M):", "", app.agent_focus == 9),
        render_field("Bind memory embed (E):", "", app.agent_focus == 10),
    ];

    let hint = Line::from(Span::styled(
        "Up/Down: focus | Type: edit | Enter: toggle/list/select | C/Y/M/E bind selected task | Ctrl+S save | Esc close",
        Style::default().fg(THEME.sub),
    ));
    let mut content: Vec<Line<'static>> = rows;
    content.push(Line::from(""));
    content.push(hint);

    // Optional model picker overlay.
    if app.model_picker && !app.agent_models.is_empty() {
        content.push(Line::from(""));
        content.push(Line::from(Span::styled(
            "Models:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for (i, m) in app.agent_models.iter().enumerate().take(6) {
            let sel = i == app.model_picker_idx;
            let style = if sel {
                Style::default().fg(THEME.base).bg(THEME.accent)
            } else {
                Style::default().fg(THEME.text)
            };
            content.push(Line::from(Span::styled(m.clone(), style)));
        }
        if app.agent_models.len() > 6 {
            content.push(Line::from(Span::styled(
                format!("... ({} total)", app.agent_models.len()),
                Style::default().fg(THEME.sub),
            )));
        }
    }

    let para = Paragraph::new(Text::from(content)).style(Style::default().fg(THEME.text));
    f.render_widget(para, inner);
}
