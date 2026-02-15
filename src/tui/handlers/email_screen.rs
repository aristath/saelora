use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config;

use super::super::{App, Screen};
use super::util::collect_settings;

pub(super) async fn handle_email_key(app: &mut App, k: KeyEvent) -> anyhow::Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.screen = Screen::Menu;
        }
        KeyCode::Tab | KeyCode::Down => {
            app.focus = (app.focus + 1) % 6;
        }
        KeyCode::BackTab | KeyCode::Up => {
            app.focus = (app.focus + 5) % 6;
        }
        KeyCode::Char('s') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            let s = collect_settings(app);
            config::save_settings(&config::settings_path(&app.data_dir), &s)?;
            app.info = "saved".to_string();
            app.err.clear();
        }
        _ => {
            edit_mail_field(app, k);
        }
    }
    Ok(())
}

fn edit_mail_field(app: &mut App, k: KeyEvent) {
    let target = match app.focus {
        0 => &mut app.mail_api_key,
        1 => &mut app.mail_api_secret,
        2 => &mut app.mail_from_email,
        3 => &mut app.mail_from_name,
        4 => &mut app.mail_base_url,
        5 => &mut app.public_base,
        _ => return,
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
}
