use crossterm::event::{KeyCode, KeyEvent};
use tokio::sync::mpsc;

use crate::db;

use super::super::{App, BgMsg, Screen};
use super::util::{move_table_selection, spawn_refresh_counts};

pub(super) async fn handle_invites_key(
    app: &mut App,
    k: KeyEvent,
    bg_tx: &mpsc::UnboundedSender<BgMsg>,
) -> anyhow::Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.screen = Screen::Menu;
            app.loading_waitlist = false;
            spawn_refresh_counts(app.data_dir.clone(), bg_tx.clone());
        }
        KeyCode::Up => {
            if !app.loading_waitlist {
                move_table_selection(&mut app.waitlist_state, app.waitlist.len(), -1);
            }
        }
        KeyCode::Down => {
            if !app.loading_waitlist {
                move_table_selection(&mut app.waitlist_state, app.waitlist.len(), 1);
            }
        }
        KeyCode::Char('r') => {
            app.loading_waitlist = true;
            let data_dir = app.data_dir.clone();
            let bg_tx = bg_tx.clone();
            tokio::spawn(async move {
                let mgr = db::Manager::new(data_dir);
                let recs = mgr.waitlist_list().map_err(|e| e.to_string());
                let _ = bg_tx.send(BgMsg::WaitlistLoaded(recs));
                let wrecs = mgr.whitelist_list().map_err(|e| e.to_string());
                let _ = bg_tx.send(BgMsg::WhitelistLoaded(wrecs));
                let w = mgr
                    .whitelist_count()
                    .map_err(|e| e.to_string())
                    .unwrap_or(0);
                let _ = bg_tx.send(BgMsg::WhitelistCount(w));
            });
        }
        KeyCode::Enter | KeyCode::Char('a') => {
            if app.loading_waitlist || app.waitlist.is_empty() {
                return Ok(());
            }
            let Some(idx) = app.waitlist_state.selected() else {
                return Ok(());
            };
            if idx >= app.waitlist.len() {
                return Ok(());
            }
            let email = app.waitlist[idx].email.clone();
            app.loading_waitlist = true;
            let data_dir = app.data_dir.clone();
            let bg_tx = bg_tx.clone();
            tokio::spawn(async move {
                let mgr = db::Manager::new(data_dir);
                let res = (|| -> Result<String, String> {
                    mgr.waitlist_remove(&email).map_err(|e| e.to_string())?;
                    mgr.whitelist_add(&email).map_err(|e| e.to_string())?;
                    Ok(format!("whitelisted {email}"))
                })();
                let _ = bg_tx.send(BgMsg::WaitlistActionDone(res));
                let recs = mgr.waitlist_list().map_err(|e| e.to_string());
                let _ = bg_tx.send(BgMsg::WaitlistLoaded(recs));
                let wrecs = mgr.whitelist_list().map_err(|e| e.to_string());
                let _ = bg_tx.send(BgMsg::WhitelistLoaded(wrecs));
            });
        }
        KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Backspace => {
            if app.loading_waitlist || app.waitlist.is_empty() {
                return Ok(());
            }
            let Some(idx) = app.waitlist_state.selected() else {
                return Ok(());
            };
            if idx >= app.waitlist.len() {
                return Ok(());
            }
            let email = app.waitlist[idx].email.clone();
            app.loading_waitlist = true;
            let data_dir = app.data_dir.clone();
            let bg_tx = bg_tx.clone();
            tokio::spawn(async move {
                let mgr = db::Manager::new(data_dir);
                let res = mgr
                    .waitlist_remove(&email)
                    .map_err(|e| e.to_string())
                    .map(|_| format!("removed {email}"));
                let _ = bg_tx.send(BgMsg::WaitlistActionDone(res));
                let recs = mgr.waitlist_list().map_err(|e| e.to_string());
                let _ = bg_tx.send(BgMsg::WaitlistLoaded(recs));
            });
        }
        _ => {}
    }
    Ok(())
}
