use std::path::PathBuf;

use ratatui::widgets::TableState;
use tokio::sync::mpsc;

use crate::config;
use crate::db;

use super::super::{App, BgMsg};

pub(super) fn collect_settings(app: &App) -> config::Settings {
    let mut s = config::default_settings();
    s.chat.system_prompt = app.system_prompt.text().trim().to_string();
    s.mailjet.api_key = app.mail_api_key.trim().to_string();
    s.mailjet.api_secret = app.mail_api_secret.trim().to_string();
    s.mailjet.from_email = app.mail_from_email.trim().to_string();
    s.mailjet.from_name = app.mail_from_name.trim().to_string();
    s.mailjet.base_url = app.mail_base_url.trim().to_string();
    s.public_base = app.public_base.trim().to_string();
    s.agents = app.agents.clone();
    s.tasks.chat_agent = app.task_chat.clone();
    s.tasks.summary_agent = app.task_summary.clone();
    s.tasks.memory_curator_agent = app.task_memory_curator.clone();
    s.tasks.memory_embed_agent = app.task_memory_embed.clone();

    // Keep task bindings valid even if agents were renamed.
    if let Some(first) = s.agents.first() {
        if !s.agents.iter().any(|a| a.name == s.tasks.chat_agent) {
            s.tasks.chat_agent = first.name.clone();
        }
        if !s.agents.iter().any(|a| a.name == s.tasks.summary_agent) {
            s.tasks.summary_agent = first.name.clone();
        }
        if !s
            .agents
            .iter()
            .any(|a| a.name == s.tasks.memory_curator_agent)
        {
            s.tasks.memory_curator_agent = first.name.clone();
        }
        if !s
            .agents
            .iter()
            .any(|a| a.name == s.tasks.memory_embed_agent)
        {
            s.tasks.memory_embed_agent = first.name.clone();
        }
    }

    s
}

pub(super) fn move_table_selection(state: &mut TableState, len: usize, delta: i32) {
    if len == 0 {
        state.select(None);
        return;
    }
    let cur = state.selected().unwrap_or(0) as i32;
    let mut next = cur + delta;
    if next < 0 {
        next = 0;
    }
    if next as usize >= len {
        next = (len - 1) as i32;
    }
    state.select(Some(next as usize));
}

pub(super) fn spawn_refresh_counts(data_dir: PathBuf, bg_tx: mpsc::UnboundedSender<BgMsg>) {
    tokio::spawn(async move {
        let mgr = db::Manager::new(data_dir);

        let n = mgr.waitlist_count().map_err(|e| e.to_string()).unwrap_or(0);
        let _ = bg_tx.send(BgMsg::InviteCount(n));

        let w = mgr
            .whitelist_count()
            .map_err(|e| e.to_string())
            .unwrap_or(0);
        let _ = bg_tx.send(BgMsg::WhitelistCount(w));

        let users = (|| -> Result<usize, String> {
            let us = mgr.users().map_err(|e| e.to_string())?;
            us.count_users().map_err(|e| e.to_string())
        })()
        .unwrap_or(0);
        let _ = bg_tx.send(BgMsg::UsersCount(users));
    });
}
