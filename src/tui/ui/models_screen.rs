use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table};

use super::super::{models, App, THEME};
use super::util::{wrap_lines, zebra_style};

pub(super) fn draw_models(f: &mut ratatui::Frame, area: Rect, app: &App) {
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

    let title_text = if app.loading_models {
        "OpenRouter Models ...".to_string()
    } else {
        "OpenRouter Models".to_string()
    };
    let title = Paragraph::new(Line::from(vec![Span::styled(
        title_text,
        Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Up/Down: move | Enter: select | 1-7: sort by column | Esc: back",
        Style::default().fg(THEME.sub),
    )]));
    let sort = Paragraph::new(Line::from(vec![Span::styled(
        format!("Sort: {}", models::sort_label(app.sort_col, app.sort_asc)),
        Style::default().fg(THEME.sub),
    )]));
    f.render_widget(title, chunks[0]);
    f.render_widget(help, chunks[1]);
    f.render_widget(sort, chunks[2]);

    if app.loading_models {
        f.render_widget(
            Paragraph::new("Loading models...").style(Style::default().fg(Color::DarkGray)),
            chunks[3],
        );
        return;
    }
    if app.models.is_empty() {
        f.render_widget(
            Paragraph::new("No models loaded.").style(Style::default().fg(Color::DarkGray)),
            chunks[3],
        );
        return;
    }

    let body = chunks[3];
    let halves = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(body);

    let cols = [
        ("ID", 34),
        ("Ctx", 7),
        ("Prompt/$1M", 10),
        ("Comp/$1M", 9),
        ("CacheR/$1M", 11),
        ("CacheW/$1M", 11),
        ("Req", 8),
    ];

    let header = Row::new(cols.iter().map(|(t, _)| Cell::from(*t))).style(
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(THEME.base)
            .bg(THEME.accent_alt),
    );
    let rows: Vec<Row> = app
        .models
        .iter()
        .enumerate()
        .map(|(idx, m)| {
            let style = zebra_style(idx, app.models_state.selected() == Some(idx));
            Row::new(vec![
                Cell::from(m.id.clone()),
                Cell::from(models::format_ctx(m.context_length)),
                Cell::from(models::cost_per_1m(&m.pricing.prompt)),
                Cell::from(models::cost_per_1m(&m.pricing.completion)),
                Cell::from(models::cost_per_1m(&m.pricing.input_cache_read)),
                Cell::from(models::cost_per_1m(&m.pricing.input_cache_write)),
                Cell::from(models::cost_per_req(&m.pricing.request)),
            ])
            .style(style)
        })
        .collect();

    let widths: Vec<Constraint> = cols.iter().map(|(_, w)| Constraint::Length(*w)).collect();
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().fg(THEME.base).bg(THEME.accent))
        .block(Block::default());

    let mut state = app.models_state;
    f.render_stateful_widget(table, halves[0], &mut state);

    // Details panel in the bottom half (fixed) to avoid layout jumping.
    let idx = state.selected().unwrap_or(0).min(app.models.len() - 1);
    let md = &app.models[idx];
    let desc = if !md.description.trim().is_empty() {
        md.description.trim()
    } else if !md.name.trim().is_empty() {
        md.name.trim()
    } else {
        "(no description)"
    };

    let mut lines = Vec::<Line<'static>>::new();
    lines.push(Line::from(vec![Span::styled(
        format!("Selected: {}", md.id),
        Style::default().add_modifier(Modifier::BOLD),
    )]));
    if !md.name.trim().is_empty() {
        lines.push(Line::from(format!("Name: {}", md.name.trim())));
    }
    if md.context_length > 0 {
        lines.push(Line::from(format!("Context: {} tokens", md.context_length)));
    }

    let pricing = models::pricing_lines(md);
    if !pricing.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Pricing:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.extend(pricing);
    }

    lines.push(Line::from(""));
    lines.extend(wrap_lines(desc, halves[1].width.saturating_sub(2) as usize));

    let details = Paragraph::new(Text::from(lines))
        .block(Block::default())
        .style(Style::default());
    f.render_widget(details, halves[1]);
}
