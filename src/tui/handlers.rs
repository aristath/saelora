use crossterm::event::{KeyCode, KeyModifiers};
use tokio::sync::mpsc;

use super::{App, BgMsg, Screen, UiMsg};

mod agents_screen;
mod email_screen;
mod invites_screen;
mod menu_screen;
mod models_screen;
mod prompt_screen;
mod users_screen;
mod util;

pub(super) fn handle_bg_msg(app: &mut App, msg: BgMsg) {
    match msg {
        BgMsg::InviteCount(n) => {
            app.invites_n = n;
        }
        BgMsg::WhitelistCount(n) => {
            app.whitelist_n = n;
        }
        BgMsg::UsersCount(n) => {
            app.users_n = n;
        }
        BgMsg::StatsLoaded((s, r)) => {
            app.msgs_sent = s;
            app.msgs_recv = r;
        }
        BgMsg::ModelsLoaded(res) => {
            app.loading_models = false;
            match res {
                Ok(mut ms) => {
                    ms.sort_by(|a, b| a.id.cmp(&b.id));
                    app.models = ms;
                    app.models_state.select(Some(0));
                    app.sort_col = 0;
                    app.sort_asc = true;
                }
                Err(e) => {
                    app.err = e;
                    app.screen = Screen::Agents;
                }
            }
        }
        BgMsg::WaitlistLoaded(res) => {
            app.loading_waitlist = false;
            match res {
                Ok(mut recs) => {
                    // Newest first for UX.
                    recs.reverse();
                    app.waitlist = recs;
                    app.waitlist_state.select(Some(0));
                    app.invites_n = app.waitlist.len();
                }
                Err(e) => {
                    app.err = e;
                    app.screen = Screen::Menu;
                }
            }
        }
        BgMsg::WhitelistLoaded(res) => {
            app.loading_waitlist = false;
            match res {
                Ok(mut recs) => {
                    recs.reverse();
                    app.whitelist = recs;
                    app.whitelist_state.select(Some(0));
                    app.whitelist_n = app.whitelist.len();
                }
                Err(e) => {
                    app.err = e;
                    app.screen = Screen::Menu;
                }
            }
        }
        BgMsg::WaitlistActionDone(res) => {
            app.loading_waitlist = false;
            match res {
                Ok(info) => {
                    app.info = info;
                    app.err.clear();
                }
                Err(e) => {
                    app.err = e;
                    app.info.clear();
                }
            }
        }
        BgMsg::UsersLoaded(res) => {
            app.loading_users = false;
            match res {
                Ok(recs) => {
                    app.users = recs;
                    app.users_state.select(Some(0));
                    app.users_n = app.users.len();
                }
                Err(e) => {
                    app.err = e;
                    app.screen = Screen::Menu;
                }
            }
        }
        BgMsg::UsersActionDone(res) => {
            app.loading_users = false;
            match res {
                Ok(info) => {
                    app.info = info;
                    app.err.clear();
                }
                Err(e) => {
                    app.err = e;
                    app.info.clear();
                }
            }
        }
        BgMsg::AgentModelsLoaded(res) => {
            app.loading_agent_models = false;
            match res {
                Ok(list) => {
                    app.agent_models = list;
                    app.model_picker = true;
                    app.model_picker_idx = 0;
                    app.err.clear();
                }
                Err(e) => {
                    app.err = e;
                    app.info.clear();
                    app.model_picker = false;
                }
            }
        }
    }
}

pub(super) async fn handle_ui_msg(
    app: &mut App,
    msg: UiMsg,
    bg_tx: &mpsc::UnboundedSender<BgMsg>,
) -> anyhow::Result<bool> {
    match msg {
        UiMsg::Resize(w, h) => {
            app.win_w = w;
            app.win_h = h;
        }
        UiMsg::Key(k) => {
            // Ctrl+C quits globally.
            if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c')) {
                return Ok(true);
            }

            match app.screen {
                Screen::Menu => menu_screen::handle_menu_key(app, k, bg_tx).await?,
                Screen::SystemPrompt => prompt_screen::handle_prompt_key(app, k, bg_tx).await?,
                Screen::EmailConfig => email_screen::handle_email_key(app, k).await?,
                Screen::Models => models_screen::handle_models_key(app, k).await?,
                Screen::Invites => invites_screen::handle_invites_key(app, k, bg_tx).await?,
                Screen::Users => users_screen::handle_users_key(app, k, bg_tx).await?,
                Screen::Agents => agents_screen::handle_agents_key(app, k, bg_tx).await?,
            }
        }
    }
    Ok(false)
}
