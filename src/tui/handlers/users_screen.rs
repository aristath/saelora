use crossterm::event::{KeyCode, KeyEvent};
use tokio::sync::mpsc;

use crate::db;

use super::super::{App, BgMsg, Screen};
use super::util::{move_table_selection, spawn_refresh_counts};

pub(super) async fn handle_users_key(
    app: &mut App,
    k: KeyEvent,
    bg_tx: &mpsc::UnboundedSender<BgMsg>,
) -> anyhow::Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.screen = Screen::Menu;
            app.loading_users = false;
            spawn_refresh_counts(app.data_dir.clone(), bg_tx.clone());
        }
        KeyCode::Up => {
            if !app.loading_users {
                move_table_selection(&mut app.users_state, app.users.len(), -1);
            }
        }
        KeyCode::Down => {
            if !app.loading_users {
                move_table_selection(&mut app.users_state, app.users.len(), 1);
            }
        }
        KeyCode::Char('r') => {
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
        KeyCode::Char('a') | KeyCode::Char('d') | KeyCode::Char('p') => {
            if app.loading_users || app.users.is_empty() {
                return Ok(());
            }
            let Some(idx) = app.users_state.selected() else {
                return Ok(());
            };
            if idx >= app.users.len() {
                return Ok(());
            }
            let status = match k.code {
                KeyCode::Char('a') => "active",
                KeyCode::Char('d') => "disabled",
                KeyCode::Char('p') => "pending",
                _ => "active",
            }
            .to_string();
            let id = app.users[idx].id.clone();
            let email = app.users[idx].email.clone();

            app.loading_users = true;
            let data_dir = app.data_dir.clone();
            let bg_tx = bg_tx.clone();
            tokio::spawn(async move {
                let mgr = db::Manager::new(data_dir);
                let res = (|| -> Result<String, String> {
                    let us = mgr.users().map_err(|e| e.to_string())?;
                    us.set_user_status(&id, &status)
                        .map_err(|e| e.to_string())?;
                    Ok(format!("{email} -> {status}"))
                })();
                let _ = bg_tx.send(BgMsg::UsersActionDone(res));

                let res2 = (|| -> Result<Vec<db::UserRecord>, String> {
                    let us = mgr.users().map_err(|e| e.to_string())?;
                    us.list_users().map_err(|e| e.to_string())
                })();
                let _ = bg_tx.send(BgMsg::UsersLoaded(res2));
            });
        }
        _ => {}
    }
    Ok(())
}
