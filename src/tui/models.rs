use ratatui::text::Line;

use crate::openrouter;

use super::App;

pub(super) fn apply_sort(app: &mut App, col: usize) {
    let cols = 7usize;
    if col >= cols {
        return;
    }

    if app.sort_col == col {
        app.sort_asc = !app.sort_asc;
    } else {
        app.sort_col = col;
        // Default to descending for numeric columns.
        app.sort_asc = col == 0;
    }

    let asc = app.sort_asc;
    match col {
        0 => app.models.sort_by(|a, b| cmp_str(&a.id, &b.id, asc)),
        1 => app
            .models
            .sort_by(|a, b| cmp_i64(a.context_length, b.context_length, asc)),
        2 => app
            .models
            .sort_by(|a, b| cmp_cost(&a.pricing.prompt, &b.pricing.prompt, asc)),
        3 => app
            .models
            .sort_by(|a, b| cmp_cost(&a.pricing.completion, &b.pricing.completion, asc)),
        4 => app.models.sort_by(|a, b| {
            cmp_cost(
                &a.pricing.input_cache_read,
                &b.pricing.input_cache_read,
                asc,
            )
        }),
        5 => app.models.sort_by(|a, b| {
            cmp_cost(
                &a.pricing.input_cache_write,
                &b.pricing.input_cache_write,
                asc,
            )
        }),
        6 => app
            .models
            .sort_by(|a, b| cmp_cost(&a.pricing.request, &b.pricing.request, asc)),
        _ => {}
    }
}

fn cmp_str(a: &str, b: &str, asc: bool) -> std::cmp::Ordering {
    if asc {
        a.cmp(b)
    } else {
        b.cmp(a)
    }
}

fn cmp_i64(a: i64, b: i64, asc: bool) -> std::cmp::Ordering {
    if asc {
        a.cmp(&b)
    } else {
        b.cmp(&a)
    }
}

fn cmp_cost(a: &str, b: &str, asc: bool) -> std::cmp::Ordering {
    let pa = a.trim().parse::<f64>().ok();
    let pb = b.trim().parse::<f64>().ok();
    match (pa, pb) {
        (Some(aa), Some(bb)) => {
            if asc {
                aa.partial_cmp(&bb).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                bb.partial_cmp(&aa).unwrap_or(std::cmp::Ordering::Equal)
            }
        }
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => cmp_str(a, b, asc),
    }
}

pub(super) fn sort_label(col: usize, asc: bool) -> String {
    let name = match col {
        0 => "ID",
        1 => "Ctx",
        2 => "Prompt/$1M",
        3 => "Comp/$1M",
        4 => "CacheR/$1M",
        5 => "CacheW/$1M",
        6 => "Req",
        _ => "?",
    };
    format!("{} {}", name, if asc { "asc" } else { "desc" })
}

pub(super) fn pricing_lines(m: &openrouter::Model) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    if !m.pricing.prompt.trim().is_empty() {
        out.push(Line::from(format!(
            "  prompt: {} / 1M tokens",
            cost_per_1m(&m.pricing.prompt)
        )));
    }
    if !m.pricing.completion.trim().is_empty() {
        out.push(Line::from(format!(
            "  completion: {} / 1M tokens",
            cost_per_1m(&m.pricing.completion)
        )));
    }
    if !m.pricing.input_cache_read.trim().is_empty() {
        out.push(Line::from(format!(
            "  input_cache_read: {} / 1M tokens",
            cost_per_1m(&m.pricing.input_cache_read)
        )));
    }
    if !m.pricing.input_cache_write.trim().is_empty() {
        out.push(Line::from(format!(
            "  input_cache_write: {} / 1M tokens",
            cost_per_1m(&m.pricing.input_cache_write)
        )));
    }
    if !m.pricing.request.trim().is_empty() {
        out.push(Line::from(format!(
            "  request: {} / request",
            cost_per_req(&m.pricing.request)
        )));
    }
    out
}

pub(super) fn format_ctx(n: i64) -> String {
    if n <= 0 {
        "".to_string()
    } else {
        n.to_string()
    }
}

pub(super) fn cost_per_1m(v: &str) -> String {
    let t = v.trim();
    if t.is_empty() {
        return "".to_string();
    }
    if let Ok(f) = t.parse::<f64>() {
        return format!("${:.2}", f * 1_000_000.0);
    }
    t.to_string()
}

pub(super) fn cost_per_req(v: &str) -> String {
    let t = v.trim();
    if t.is_empty() {
        return "".to_string();
    }
    if let Ok(f) = t.parse::<f64>() {
        return format!("${:.4}", f);
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::widgets::TableState;

    use super::*;
    use crate::openrouter;

    fn empty_app() -> App {
        App {
            data_dir: PathBuf::new(),
            screen: super::super::Screen::Menu,
            win_w: 0,
            win_h: 0,
            menu_idx: 0,
            invites_n: 0,
            whitelist_n: 0,
            users_n: 0,
            msgs_sent: 0,
            msgs_recv: 0,
            focus: 0,
            mail_api_key: String::new(),
            mail_api_secret: String::new(),
            mail_from_email: String::new(),
            mail_from_name: String::new(),
            mail_base_url: String::new(),
            public_base: String::new(),
            system_prompt: super::super::PromptEditor::from_text("", ""),
            info: String::new(),
            err: String::new(),
            loading_models: false,
            models: vec![],
            models_state: TableState::default(),
            sort_col: 0,
            sort_asc: true,
            loading_waitlist: false,
            waitlist: vec![],
            waitlist_state: TableState::default(),
            whitelist: vec![],
            whitelist_state: TableState::default(),
            loading_users: false,
            users: vec![],
            users_state: TableState::default(),
            agents: vec![],
            agents_state: TableState::default(),
            agent_focus: 0,
            task_chat: String::new(),
            task_summary: String::new(),
            agent_modal: false,
            agent_modal_idx: None,
            agent_models: vec![],
            model_picker: false,
            model_picker_idx: 0,
            loading_agent_models: false,
        }
    }

    fn model(id: &str, ctx: i64, prompt: &str, req: &str) -> openrouter::Model {
        openrouter::Model {
            id: id.to_string(),
            name: String::new(),
            description: String::new(),
            context_length: ctx,
            pricing: openrouter::types::Pricing {
                prompt: prompt.to_string(),
                request: req.to_string(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn apply_sort_sorts_numeric_columns_desc_by_default_and_toggles() {
        let mut app = empty_app();
        app.models = vec![
            model("a", 8, "0.01", "0.0"),
            model("b", 32, "0.02", "0.0"),
            model("c", 16, "0.03", "0.0"),
        ];

        // Column 1 = context length, defaults to descending.
        apply_sort(&mut app, 1);
        assert_eq!(app.sort_col, 1);
        assert!(!app.sort_asc);
        assert_eq!(app.models[0].context_length, 32);
        assert_eq!(app.models[1].context_length, 16);
        assert_eq!(app.models[2].context_length, 8);

        // Toggle same column -> ascending.
        apply_sort(&mut app, 1);
        assert!(app.sort_asc);
        assert_eq!(app.models[0].context_length, 8);
        assert_eq!(app.models[2].context_length, 32);
    }

    #[test]
    fn apply_sort_sorts_costs_and_keeps_numeric_before_empty() {
        let mut app = empty_app();
        app.models = vec![
            model("x", 0, "0.002", ""),
            model("y", 0, "0.0001", ""),
            model("z", 0, "", ""),
        ];

        // Column 2 = prompt cost, defaults to descending.
        apply_sort(&mut app, 2);
        assert_eq!(app.models[0].id, "x");
        assert_eq!(app.models[1].id, "y");
        assert_eq!(app.models[2].id, "z"); // empty/non-numeric goes last
    }
}
