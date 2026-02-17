use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::{config, openrouter};

use super::super::{App, BgMsg, Screen};
use super::util::{collect_settings, move_table_selection};

pub(super) async fn handle_agents_key(
    app: &mut App,
    k: KeyEvent,
    bg_tx: &mpsc::UnboundedSender<BgMsg>,
) -> anyhow::Result<()> {
    if app.agent_modal {
        return handle_agent_modal_key(app, k, bg_tx);
    }

    if app.agents.is_empty() {
        app.agents.push(default_agent_from_app(app));
    }

    match k.code {
        KeyCode::Esc => {
            app.screen = Screen::Menu;
        }
        KeyCode::Up => {
            move_table_selection(&mut app.agents_state, app.agents.len() + 1, -1);
        }
        KeyCode::Down => {
            move_table_selection(&mut app.agents_state, app.agents.len() + 1, 1);
        }
        KeyCode::Enter => {
            if let Some(idx) = app.agents_state.selected() {
                if idx == app.agents.len() {
                    // Add agent row.
                    let new = default_agent_from_app(app);
                    app.agents.push(new);
                    app.agents_state.select(Some(app.agents.len() - 1));
                    app.agent_modal = true;
                    app.agent_modal_idx = Some(app.agents.len() - 1);
                    app.agent_focus = 0;
                } else {
                    app.agent_modal = true;
                    app.agent_modal_idx = Some(idx);
                    app.agent_focus = 0;
                }
            }
        }
        KeyCode::Char('s') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            let s = collect_settings(app);
            config::save_settings(&config::settings_path(&app.data_dir), &s)?;
            app.info = "saved".to_string();
            app.err.clear();
        }
        _ => {}
    }
    Ok(())
}

fn handle_agent_modal_key(
    app: &mut App,
    k: KeyEvent,
    bg_tx: &mpsc::UnboundedSender<BgMsg>,
) -> anyhow::Result<()> {
    let Some(idx) = app.agent_modal_idx else {
        app.agent_modal = false;
        return Ok(());
    };
    if idx >= app.agents.len() {
        app.agent_modal = false;
        return Ok(());
    }

    // Handle model picker navigation first.
    if app.model_picker {
        match k.code {
            KeyCode::Esc => {
                app.model_picker = false;
            }
            KeyCode::Up => {
                if app.model_picker_idx > 0 {
                    app.model_picker_idx -= 1;
                }
            }
            KeyCode::Down => {
                if app.model_picker_idx + 1 < app.agent_models.len() {
                    app.model_picker_idx += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(sel) = app.agent_models.get(app.model_picker_idx) {
                    app.agents[idx].model = sel.clone();
                }
                app.model_picker = false;
            }
            _ => {}
        }
        return Ok(());
    }

    match k.code {
        KeyCode::Esc => {
            app.agent_modal = false;
            app.agent_modal_idx = None;
        }
        KeyCode::Tab | KeyCode::Down => {
            app.agent_focus = (app.agent_focus + 1) % 11;
        }
        KeyCode::BackTab | KeyCode::Up => {
            app.agent_focus = (app.agent_focus + 10) % 11;
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if app.agent_focus == 1 {
                toggle_agent_provider(&mut app.agents[idx]);
            } else if app.agent_focus == 7 {
                app.task_chat = app.agents[idx].name.clone();
            } else if app.agent_focus == 8 {
                app.task_summary = app.agents[idx].name.clone();
            } else if app.agent_focus == 9 {
                app.task_memory_curator = app.agents[idx].name.clone();
            } else if app.agent_focus == 10 {
                app.task_memory_embed = app.agents[idx].name.clone();
            } else if app.agent_focus == 2 && !app.loading_agent_models {
                match app.agents[idx].provider {
                    config::Provider::OpenRouter => {
                        if app.agents[idx].api_key.trim().is_empty() {
                            app.err = "API key is required to list models".to_string();
                            return Ok(());
                        }
                        app.screen = Screen::Models;
                        app.loading_models = true;
                        app.models.clear();
                        app.models_state.select(Some(0));
                        app.sort_col = 0;
                        app.sort_asc = true;

                        let agent = app.agents[idx].clone();
                        let bg_tx = bg_tx.clone();
                        tokio::spawn(async move {
                            let base_url = agent.openai_base_url();
                            let c = openrouter::Client::new(openrouter::Config {
                                api_key: agent.api_key,
                                base_url,
                                http_referer: agent.http_referer,
                                x_title: agent.x_title,
                            });
                            let res = match c {
                                Ok(c) => c.list_models().await.map_err(|e| e.to_string()),
                                Err(e) => Err(e.to_string()),
                            };
                            let _ = bg_tx.send(BgMsg::ModelsLoaded(res));
                        });
                    }
                    config::Provider::Ollama => {
                        start_agent_model_load(app, idx, bg_tx.clone());
                    }
                }
            }
        }
        KeyCode::Char('s') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            // Validate before saving.
            if !validate_agents(app) {
                return Ok(());
            }
            let s = collect_settings(app);
            config::save_settings(&config::settings_path(&app.data_dir), &s)?;
            app.info = "saved".to_string();
            app.err.clear();
            app.agent_modal = false;
            app.agent_modal_idx = None;
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            app.task_chat = app.agents[idx].name.clone();
        }
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.task_summary = app.agents[idx].name.clone();
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            app.task_memory_curator = app.agents[idx].name.clone();
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            app.task_memory_embed = app.agents[idx].name.clone();
        }
        _ => {
            // Text edits.
            let target_opt = match app.agent_focus {
                0 => Some(&mut app.agents[idx].name),
                1 => None,
                2 => Some(&mut app.agents[idx].model),
                3 => Some(&mut app.agents[idx].base_url),
                4 => Some(&mut app.agents[idx].api_key),
                5 => Some(&mut app.agents[idx].http_referer),
                6 => Some(&mut app.agents[idx].x_title),
                7..=10 => None,
                _ => None,
            };
            if let Some(target) = target_opt {
                let old_name = if app.agent_focus == 0 {
                    Some(target.clone())
                } else {
                    None
                };
                match k.code {
                    KeyCode::Backspace => {
                        target.pop();
                    }
                    KeyCode::Char(c) => {
                        if !k.modifiers.contains(KeyModifiers::CONTROL)
                            && !k.modifiers.contains(KeyModifiers::ALT)
                        {
                            target.push(c);
                        }
                    }
                    _ => {}
                }
                if let Some(old) = old_name {
                    let new_name = app.agents[idx].name.clone();
                    if app.task_chat == old {
                        app.task_chat = new_name.clone();
                    }
                    if app.task_summary == old {
                        app.task_summary = new_name.clone();
                    }
                    if app.task_memory_curator == old {
                        app.task_memory_curator = new_name.clone();
                    }
                    if app.task_memory_embed == old {
                        app.task_memory_embed = new_name;
                    }
                }
            }
        }
    }
    Ok(())
}

fn toggle_agent_provider(a: &mut config::Agent) {
    a.provider = match a.provider {
        config::Provider::OpenRouter => config::Provider::Ollama,
        config::Provider::Ollama => config::Provider::OpenRouter,
    };
    if matches!(a.provider, config::Provider::Ollama) && a.base_url.trim().is_empty() {
        a.base_url = "http://localhost:11434/v1".to_string();
    }
}

fn default_agent_from_app(app: &App) -> config::Agent {
    let mut a = app
        .agents
        .first()
        .cloned()
        .unwrap_or_else(|| config::default_settings().agents[0].clone());
    let mut name = format!("agent{}", app.agents.len() + 1);
    let mut i = app.agents.len() + 1;
    while app.agents.iter().any(|x| x.name == name) {
        i += 1;
        name = format!("agent{}", i);
    }
    a.name = name;
    a
}

fn validate_agents(app: &mut App) -> bool {
    // Names must be present and unique.
    for a in &app.agents {
        if a.name.trim().is_empty() {
            app.err = "agent name is required".to_string();
            return false;
        }
        if a.model.trim().is_empty() {
            app.err = format!("{}: model is required", a.name);
            return false;
        }
    }
    for i in 0..app.agents.len() {
        for j in (i + 1)..app.agents.len() {
            if app.agents[i].name == app.agents[j].name {
                app.err = format!("duplicate agent name: {}", app.agents[i].name);
                return false;
            }
        }
    }
    for a in &app.agents {
        if matches!(a.provider, config::Provider::OpenRouter) && a.api_key.trim().is_empty() {
            app.err = format!("{}: API key required for OpenRouter", a.name);
            return false;
        }
    }
    true
}

fn start_agent_model_load(app: &mut App, idx: usize, bg_tx: mpsc::UnboundedSender<BgMsg>) {
    if idx >= app.agents.len() || app.loading_agent_models {
        return;
    }
    app.loading_agent_models = true;
    app.agent_models.clear();
    app.model_picker = false;
    app.model_picker_idx = 0;
    let agent = app.agents[idx].clone();
    tokio::spawn(async move {
        let res = fetch_models_for_agent(agent).await;
        let _ = bg_tx.send(BgMsg::AgentModelsLoaded(res));
    });
}

async fn fetch_models_for_agent(agent: config::Agent) -> Result<Vec<String>, String> {
    match agent.provider {
        config::Provider::OpenRouter => {
            let client = openrouter::Client::new(openrouter::Config {
                api_key: agent.api_key.clone(),
                base_url: agent.openai_base_url(),
                http_referer: agent.http_referer.clone(),
                x_title: agent.x_title.clone(),
            })
            .map_err(|e| e.to_string())?;
            let models = client
                .list_models()
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|m| m.id)
                .collect::<Vec<_>>();
            Ok(models)
        }
        config::Provider::Ollama => {
            let client = reqwest::Client::new();

            // Try OpenAI-compatible /v1/models first.
            let openai_base = agent.openai_base_url();
            let url_v1 = format!("{}/models", openai_base.trim_end_matches('/'));
            let mut rb = client.get(url_v1);
            if !agent.api_key.trim().is_empty() {
                rb = rb.bearer_auth(agent.api_key.trim());
            }
            let try_v1 = rb.send().await;
            let mut models: Vec<String> = Vec::new();
            if let Ok(resp) = try_v1 {
                if resp.status().is_success() {
                    if let Ok(v) = resp.json::<serde_json::Value>().await {
                        if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
                            for item in arr {
                                if let Some(id) = item.get("id").and_then(|s| s.as_str()) {
                                    models.push(id.to_string());
                                }
                            }
                        }
                    }
                }
            }

            // Fallback to Ollama native /api/tags if needed.
            if models.is_empty() {
                let url_tags = format!("{}/api/tags", agent.ollama_root_url());
                let mut rb2 = client.get(url_tags);
                if !agent.api_key.trim().is_empty() {
                    rb2 = rb2.bearer_auth(agent.api_key.trim());
                }
                let resp2 = rb2.send().await.map_err(|e| e.to_string())?;
                if !resp2.status().is_success() {
                    return Err(format!("model list failed: {}", resp2.status()));
                }
                let v: serde_json::Value = resp2.json().await.map_err(|e| e.to_string())?;
                if let Some(arr) = v.get("models").and_then(|m| m.as_array()) {
                    for item in arr {
                        if let Some(name) = item.get("name").and_then(|s| s.as_str()) {
                            models.push(name.to_string());
                        }
                    }
                }
            }

            if models.is_empty() {
                return Err("no models found".to_string());
            }
            models.sort();
            Ok(models)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::widgets::TableState;

    use super::*;

    fn empty_app() -> App {
        App {
            data_dir: PathBuf::new(),
            screen: Screen::Menu,
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
            system_prompt: crate::tui::PromptEditor::from_text("", ""),
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
            task_memory_curator: String::new(),
            task_memory_embed: String::new(),
            agent_modal: false,
            agent_modal_idx: None,
            agent_models: vec![],
            model_picker: false,
            model_picker_idx: 0,
            loading_agent_models: false,
        }
    }

    #[test]
    fn validate_agents_enforces_required_fields_and_uniqueness() {
        let mut app = empty_app();

        // Missing name.
        app.agents = vec![config::Agent {
            name: "".to_string(),
            provider: config::Provider::OpenRouter,
            model: "m".to_string(),
            base_url: String::new(),
            api_key: "k".to_string(),
            http_referer: String::new(),
            x_title: String::new(),
        }];
        assert!(!validate_agents(&mut app));
        assert_eq!(app.err, "agent name is required");

        // Missing model.
        app.agents[0].name = "a".to_string();
        app.agents[0].model.clear();
        assert!(!validate_agents(&mut app));
        assert_eq!(app.err, "a: model is required");

        // Duplicate names.
        app.agents = vec![
            config::Agent {
                name: "dup".to_string(),
                provider: config::Provider::OpenRouter,
                model: "m".to_string(),
                base_url: String::new(),
                api_key: "k".to_string(),
                http_referer: String::new(),
                x_title: String::new(),
            },
            config::Agent {
                name: "dup".to_string(),
                provider: config::Provider::Ollama,
                model: "m2".to_string(),
                base_url: String::new(),
                api_key: String::new(),
                http_referer: String::new(),
                x_title: String::new(),
            },
        ];
        assert!(!validate_agents(&mut app));
        assert_eq!(app.err, "duplicate agent name: dup");

        // OpenRouter agents must have API key; Ollama agents may omit it.
        app.agents = vec![config::Agent {
            name: "or".to_string(),
            provider: config::Provider::OpenRouter,
            model: "m".to_string(),
            base_url: String::new(),
            api_key: "".to_string(),
            http_referer: String::new(),
            x_title: String::new(),
        }];
        assert!(!validate_agents(&mut app));
        assert_eq!(app.err, "or: API key required for OpenRouter");

        app.agents = vec![config::Agent {
            name: "ol".to_string(),
            provider: config::Provider::Ollama,
            model: "m".to_string(),
            base_url: "http://127.0.0.1:11434".to_string(),
            api_key: "".to_string(),
            http_referer: String::new(),
            x_title: String::new(),
        }];
        assert!(validate_agents(&mut app));
    }
}
