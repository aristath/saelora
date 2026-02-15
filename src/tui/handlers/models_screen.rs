use crossterm::event::{KeyCode, KeyEvent};

use crate::config;

use super::super::{models, App, Screen};
use super::util::{collect_settings, move_table_selection};

pub(super) async fn handle_models_key(app: &mut App, k: KeyEvent) -> anyhow::Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.screen = Screen::Agents;
            app.loading_models = false;
        }
        KeyCode::Up => {
            move_table_selection(&mut app.models_state, app.models.len(), -1);
        }
        KeyCode::Down => {
            move_table_selection(&mut app.models_state, app.models.len(), 1);
        }
        KeyCode::Enter => {
            if app.loading_models {
                return Ok(());
            }
            let Some(idx) = app.models_state.selected() else {
                return Ok(());
            };
            if idx >= app.models.len() {
                return Ok(());
            }
            let id = app.models[idx].id.clone();
            if let Some(aidx) = app.agent_modal_idx {
                if aidx < app.agents.len() {
                    app.agents[aidx].model = id;
                }
            }

            // Persist immediately.
            if let Some(aidx) = app.agent_modal_idx {
                if let Some(a) = app.agents.get(aidx) {
                    if matches!(a.provider, config::Provider::OpenRouter)
                        && !a.name.trim().is_empty()
                        && !a.api_key.trim().is_empty()
                        && !a.model.trim().is_empty()
                    {
                        let s = collect_settings(app);
                        config::save_settings(&config::settings_path(&app.data_dir), &s)?;
                    }
                }
            }

            app.screen = Screen::Agents;
            app.agent_modal = true;
            app.agent_focus = 2;
        }
        KeyCode::Char(c) if ('1'..='9').contains(&c) => {
            if app.loading_models || app.models.is_empty() {
                return Ok(());
            }
            let col = (c as u8 - b'1') as usize;
            models::apply_sort(app, col);
        }
        _ => {}
    }
    Ok(())
}
