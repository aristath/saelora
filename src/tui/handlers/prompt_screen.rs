use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::config;

use super::super::{App, BgMsg, Screen};
use super::util::{collect_settings, spawn_refresh_counts};

pub(super) async fn handle_prompt_key(
    app: &mut App,
    k: KeyEvent,
    bg_tx: &mpsc::UnboundedSender<BgMsg>,
) -> anyhow::Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.screen = Screen::Menu;
            spawn_refresh_counts(app.data_dir.clone(), bg_tx.clone());
        }
        KeyCode::Char('s') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            let mut s = collect_settings(app);
            s.chat.system_prompt = app.system_prompt.text().trim().to_string();
            if s.chat.system_prompt.trim().is_empty() {
                s.chat.system_prompt = config::DEFAULT_SYSTEM_PROMPT.trim().to_string();
                app.system_prompt.set_text(&s.chat.system_prompt);
            }
            config::save_settings(&config::settings_path(&app.data_dir), &s)?;
            app.info = "saved".to_string();
            app.err.clear();
        }
        _ => {
            app.system_prompt.handle_key(k);
        }
    }
    Ok(())
}
