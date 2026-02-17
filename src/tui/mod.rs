use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{Event, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::style::Color;
use ratatui::widgets::TableState;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::config;
use crate::db;
use crate::openrouter;

mod handlers;
mod models;
mod prompt;
mod ui;

use prompt::PromptEditor;

// Catppuccin Mocha inspired palette for a calmer, more legible TUI.
struct Theme {
    base: Color,
    surface: Color,
    surface_alt: Color,
    text: Color,
    sub: Color,
    accent: Color,
    accent_alt: Color,
    danger: Color,
}

const THEME: Theme = Theme {
    base: Color::Rgb(30, 30, 46),          // base
    surface: Color::Rgb(49, 50, 68),       // surface0
    surface_alt: Color::Rgb(69, 71, 90),   // surface1
    text: Color::Rgb(205, 214, 244),       // text
    sub: Color::Rgb(186, 194, 222),        // subtext1
    accent: Color::Rgb(148, 226, 213),     // teal
    accent_alt: Color::Rgb(203, 166, 247), // mauve
    danger: Color::Rgb(243, 139, 168),     // red
};

#[derive(Debug)]
enum Screen {
    Menu,
    SystemPrompt,
    EmailConfig,
    Models,
    Invites,
    Users,
    Agents,
}

#[derive(Debug)]
enum BgMsg {
    InviteCount(usize),
    WhitelistCount(usize),
    UsersCount(usize),
    StatsLoaded((u64, u64)),
    ModelsLoaded(Result<Vec<openrouter::Model>, String>),
    WaitlistLoaded(Result<Vec<db::WaitlistRecord>, String>),
    WhitelistLoaded(Result<Vec<db::WaitlistRecord>, String>),
    WaitlistActionDone(Result<String, String>),
    UsersLoaded(Result<Vec<db::UserRecord>, String>),
    UsersActionDone(Result<String, String>),
    AgentModelsLoaded(Result<Vec<String>, String>),
}

#[derive(Debug)]
enum UiMsg {
    Key(KeyEvent),
    Resize(u16, u16),
}

struct App {
    data_dir: PathBuf,
    screen: Screen,

    win_w: u16,
    win_h: u16,

    menu_idx: usize,
    invites_n: usize,
    whitelist_n: usize,
    users_n: usize,
    msgs_sent: u64,
    msgs_recv: u64,

    focus: usize, // config focus field
    mail_api_key: String,
    mail_api_secret: String,
    mail_from_email: String,
    mail_from_name: String,
    mail_base_url: String,
    public_base: String,

    system_prompt: PromptEditor,

    info: String,
    err: String,

    // Models screen
    loading_models: bool,
    models: Vec<openrouter::Model>,
    models_state: TableState,
    sort_col: usize, // 0-based
    sort_asc: bool,

    // Waitlist screen
    loading_waitlist: bool,
    waitlist: Vec<db::WaitlistRecord>,
    waitlist_state: TableState,
    whitelist: Vec<db::WaitlistRecord>,
    whitelist_state: TableState,

    // Users screen
    loading_users: bool,
    users: Vec<db::UserRecord>,
    users_state: TableState,

    // Agents screen
    agents: Vec<config::Agent>,
    agents_state: TableState,
    agent_focus: usize,
    task_chat: String,
    task_summary: String,
    task_memory_curator: String,
    task_memory_embed: String,
    agent_modal: bool,
    agent_modal_idx: Option<usize>,
    agent_models: Vec<String>,
    model_picker: bool,
    model_picker_idx: usize,
    loading_agent_models: bool,
}

pub async fn run_tui(data_dir: PathBuf) -> anyhow::Result<()> {
    let mut settings = match config::load_settings(&config::settings_path(&data_dir)) {
        Ok(s) => s,
        Err(_) => config::default_settings(),
    };
    if settings.agents.is_empty() {
        settings.agents = config::default_settings().agents;
    }
    if settings.tasks.chat_agent.is_empty() {
        settings.tasks.chat_agent = settings
            .agents
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_default();
    }
    if settings.tasks.summary_agent.is_empty() {
        settings.tasks.summary_agent = settings
            .agents
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_default();
    }
    if settings.tasks.memory_curator_agent.is_empty() {
        settings.tasks.memory_curator_agent = settings.tasks.summary_agent.clone();
    }
    if settings.tasks.memory_embed_agent.is_empty() {
        settings.tasks.memory_embed_agent = settings.tasks.memory_curator_agent.clone();
    }
    if settings.chat.system_prompt.trim().is_empty() {
        settings.chat.system_prompt = config::DEFAULT_SYSTEM_PROMPT.trim().to_string();
    }
    // Ensure task bindings reference existing agents.
    if let Some(first) = settings.agents.first() {
        if !settings
            .agents
            .iter()
            .any(|a| a.name == settings.tasks.chat_agent)
        {
            settings.tasks.chat_agent = first.name.clone();
        }
        if !settings
            .agents
            .iter()
            .any(|a| a.name == settings.tasks.summary_agent)
        {
            settings.tasks.summary_agent = first.name.clone();
        }
        if !settings
            .agents
            .iter()
            .any(|a| a.name == settings.tasks.memory_curator_agent)
        {
            settings.tasks.memory_curator_agent = first.name.clone();
        }
        if !settings
            .agents
            .iter()
            .any(|a| a.name == settings.tasks.memory_embed_agent)
        {
            settings.tasks.memory_embed_agent = first.name.clone();
        }
    }

    let prompt = PromptEditor::from_text(
        &settings.chat.system_prompt,
        "System prompt (server-side). If empty, Saelora uses its default.",
    );

    let mut app = App {
        data_dir: data_dir.clone(),
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
        mail_api_key: settings.mailjet.api_key.clone(),
        mail_api_secret: settings.mailjet.api_secret.clone(),
        mail_from_email: settings.mailjet.from_email.clone(),
        mail_from_name: settings.mailjet.from_name.clone(),
        mail_base_url: settings.mailjet.base_url.clone(),
        public_base: settings.public_base.clone(),
        agents: settings.agents.clone(),
        agents_state: TableState::default(),
        agent_focus: 0,
        task_chat: settings.tasks.chat_agent.clone(),
        task_summary: settings.tasks.summary_agent.clone(),
        task_memory_curator: settings.tasks.memory_curator_agent.clone(),
        task_memory_embed: settings.tasks.memory_embed_agent.clone(),
        agent_modal: false,
        agent_modal_idx: None,
        agent_models: vec![],
        model_picker: false,
        model_picker_idx: 0,
        loading_agent_models: false,
        system_prompt: prompt,
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
    };
    if !app.agents.is_empty() {
        app.agents_state.select(Some(0));
    }

    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UiMsg>();
    let (bg_tx, mut bg_rx) = mpsc::unbounded_channel::<BgMsg>();

    // Terminal event thread.
    std::thread::spawn(move || loop {
        if let Ok(ev) = crossterm::event::read() {
            match ev {
                Event::Key(k) => {
                    let _ = ui_tx.send(UiMsg::Key(k));
                }
                Event::Resize(w, h) => {
                    let _ = ui_tx.send(UiMsg::Resize(w, h));
                }
                _ => {}
            }
        }
    });

    // Initial invite counts.
    {
        let data_dir = data_dir.clone();
        let bg_tx = bg_tx.clone();
        tokio::spawn(async move {
            let mgr = db::Manager::new(data_dir);
            let n = mgr.waitlist_count().map_err(|e| e.to_string());
            let _ = bg_tx.send(BgMsg::InviteCount(n.unwrap_or(0)));
            let w = mgr.whitelist_count().map_err(|e| e.to_string());
            let _ = bg_tx.send(BgMsg::WhitelistCount(w.unwrap_or(0)));
            if let Ok(us) = mgr.users() {
                if let Ok((s, r)) = us.get_message_counts() {
                    let _ = bg_tx.send(BgMsg::StatsLoaded((s, r)));
                }
            }
        });
    }

    // Periodic refresh for invite/users counts and waitlist list.
    {
        let data_dir = data_dir.clone();
        let bg_tx = bg_tx.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(3));
            loop {
                ticker.tick().await;
                let mgr = db::Manager::new(data_dir.clone());
                // Invite count
                if let Ok(n) = mgr.waitlist_count() {
                    let _ = bg_tx.send(BgMsg::InviteCount(n));
                }
                if let Ok(n) = mgr.whitelist_count() {
                    let _ = bg_tx.send(BgMsg::WhitelistCount(n));
                }
                // Waitlist + whitelist lists
                let recs = mgr.waitlist_list().map_err(|e| e.to_string());
                let _ = bg_tx.send(BgMsg::WaitlistLoaded(recs));
                let wrecs = mgr.whitelist_list().map_err(|e| e.to_string());
                let _ = bg_tx.send(BgMsg::WhitelistLoaded(wrecs));
                // Users count
                let users = (|| -> Result<usize, String> {
                    let us = mgr.users().map_err(|e| e.to_string())?;
                    us.count_users().map_err(|e| e.to_string())
                })();
                if let Ok(n) = users {
                    let _ = bg_tx.send(BgMsg::UsersCount(n));
                }
                if let Ok(us) = mgr.users() {
                    if let Ok((s, r)) = us.get_message_counts() {
                        let _ = bg_tx.send(BgMsg::StatsLoaded((s, r)));
                    }
                }
            }
        });
    }

    // Initial users count.
    {
        let data_dir = data_dir.clone();
        let bg_tx = bg_tx.clone();
        tokio::spawn(async move {
            let mgr = db::Manager::new(data_dir);
            let res = (|| -> Result<usize, String> {
                let us = mgr.users().map_err(|e| e.to_string())?;
                us.count_users().map_err(|e| e.to_string())
            })();
            let _ = bg_tx.send(BgMsg::UsersCount(res.unwrap_or(0)));
        });
    }

    // Terminal setup.
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;
    term.clear()?;

    let mut should_quit = false;
    while !should_quit {
        // Draw.
        term.draw(|f| ui::draw(f, &mut app))?;

        tokio::select! {
            Some(msg) = ui_rx.recv() => {
                should_quit = handlers::handle_ui_msg(&mut app, msg, &bg_tx).await?;
            }
            Some(msg) = bg_rx.recv() => {
                handlers::handle_bg_msg(&mut app, msg);
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    // Restore terminal.
    disable_raw_mode().ok();
    execute!(term.backend_mut(), LeaveAlternateScreen).ok();
    term.show_cursor().ok();

    Ok(())
}
