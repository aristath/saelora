use crossterm::event::{KeyCode, KeyEvent};
use tokio::sync::mpsc;

use crate::{config, db, openrouter};

use super::super::{App, BgMsg, Screen};

pub(super) async fn handle_menu_key(
    app: &mut App,
    k: KeyEvent,
    bg_tx: &mpsc::UnboundedSender<BgMsg>,
) -> anyhow::Result<()> {
    match k.code {
        KeyCode::Up => {
            if app.menu_idx > 0 {
                app.menu_idx -= 1;
            }
        }
        KeyCode::Down => {
            if app.menu_idx < 5 {
                app.menu_idx += 1;
            }
        }
        KeyCode::Enter => match app.menu_idx {
            0 => {
                open_openrouter_config(app);
                app.info.clear();
                app.err.clear();
            }
            1 => {
                app.screen = Screen::SystemPrompt;
                app.info.clear();
                app.err.clear();
            }
            2 => {
                app.screen = Screen::EmailConfig;
                app.info.clear();
                app.err.clear();
                app.focus = 0;
            }
            3 => {
                app.screen = Screen::Invites;
                app.info.clear();
                app.err.clear();
                app.loading_waitlist = true;
                let data_dir = app.data_dir.clone();
                let bg_tx = bg_tx.clone();
                tokio::spawn(async move {
                    let mgr = db::Manager::new(data_dir);
                    let recs = mgr.waitlist_list().map_err(|e| e.to_string());
                    let _ = bg_tx.send(BgMsg::WaitlistLoaded(recs));
                });
            }
            4 => {
                app.screen = Screen::Users;
                app.info.clear();
                app.err.clear();
                app.loading_users = true;
                let data_dir = app.data_dir.clone();
                let bg_tx = bg_tx.clone();
                tokio::spawn(async move {
                    let mgr = db::Manager::new(data_dir);
                    let res = (|| -> Result<Vec<db::UserRecord>, String> {
                        let us = mgr.users().map_err(|e| e.to_string())?;
                        us.list_users().map_err(|e| e.to_string())
                    })();
                    let _ = bg_tx.send(BgMsg::UsersLoaded(res));
                });
            }
            5 => {
                app.screen = Screen::Agents;
                app.info.clear();
                app.err.clear();
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

fn open_openrouter_config(app: &mut App) {
    if app.agents.is_empty() {
        app.agents = config::default_settings().agents;
        if app.task_chat.trim().is_empty() {
            app.task_chat = app
                .agents
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_default();
        }
        if app.task_summary.trim().is_empty() {
            app.task_summary = app
                .agents
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_default();
        }
        if app.task_memory.trim().is_empty() {
            app.task_memory = app.task_summary.clone();
        }
    }

    // Prefer the current chat agent if it's OpenRouter.
    let mut idx_opt = app.agents.iter().position(|a| {
        a.name == app.task_chat && matches!(a.provider, config::Provider::OpenRouter)
    });

    // Else: prefer "default" OpenRouter.
    if idx_opt.is_none() {
        idx_opt = app.agents.iter().position(|a| {
            a.name == "default" && matches!(a.provider, config::Provider::OpenRouter)
        });
    }

    // Else: first OpenRouter agent.
    if idx_opt.is_none() {
        idx_opt = app
            .agents
            .iter()
            .position(|a| matches!(a.provider, config::Provider::OpenRouter));
    }

    // Else: create a new OpenRouter agent.
    if idx_opt.is_none() {
        let mut a = config::default_settings().agents[0].clone();
        a.provider = config::Provider::OpenRouter;
        a.api_key.clear();
        a.base_url = openrouter::DEFAULT_BASE_URL.to_string();
        a.http_referer.clear();
        a.x_title = "Saelora".to_string();

        let mut name = "openrouter".to_string();
        let mut i = 1usize;
        while app.agents.iter().any(|x| x.name == name) {
            i += 1;
            name = format!("openrouter{}", i);
        }
        a.name = name;
        app.agents.push(a);
        idx_opt = Some(app.agents.len() - 1);
    }

    let idx = idx_opt.unwrap_or(0).min(app.agents.len().saturating_sub(1));

    app.screen = Screen::Agents;
    app.agents_state.select(Some(idx));
    app.agent_modal = true;
    app.agent_modal_idx = Some(idx);
    // Focus API key for quick setup.
    app.agent_focus = 4;
    app.model_picker = false;
    app.loading_models = false;
    app.loading_agent_models = false;
}
