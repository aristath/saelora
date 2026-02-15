use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Terminal;
use tokio::sync::mpsc;
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

use crate::config;
use crate::db;
use crate::openrouter;

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
    OpenRouterConfig,
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
    api_key: String,
    model: String,
    base_url: String,
    http_referer: String,
    x_title: String,
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
        let legacy = config::Agent {
            name: "default".to_string(),
            provider: config::Provider::OpenRouter,
            model: settings.openrouter.model.clone(),
            base_url: settings.openrouter.base_url.clone(),
            api_key: settings.openrouter.api_key.clone(),
            http_referer: settings.openrouter.http_referer.clone(),
            x_title: settings.openrouter.x_title.clone(),
        };
        settings.agents.push(legacy.clone());
        if settings.tasks.chat_agent.is_empty() {
            settings.tasks.chat_agent = legacy.name.clone();
        }
        if settings.tasks.summary_agent.is_empty() {
            settings.tasks.summary_agent = legacy.name.clone();
        }
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
        api_key: settings.openrouter.api_key.clone(),
        model: settings.openrouter.model.clone(),
        base_url: if settings.openrouter.base_url.trim().is_empty() {
            openrouter::DEFAULT_BASE_URL.to_string()
        } else {
            settings.openrouter.base_url.clone()
        },
        http_referer: settings.openrouter.http_referer.clone(),
        x_title: if settings.openrouter.x_title.trim().is_empty() {
            "Saelora".to_string()
        } else {
            settings.openrouter.x_title.clone()
        },
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
        term.draw(|f| draw(f, &mut app))?;

        tokio::select! {
            Some(msg) = ui_rx.recv() => {
                should_quit = handle_ui_msg(&mut app, msg, &bg_tx).await?;
            }
            Some(msg) = bg_rx.recv() => {
                handle_bg_msg(&mut app, msg);
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

fn handle_bg_msg(app: &mut App, msg: BgMsg) {
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
                    app.screen = Screen::OpenRouterConfig;
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

async fn handle_ui_msg(
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
                Screen::Menu => handle_menu_key(app, k, bg_tx).await?,
                Screen::OpenRouterConfig => handle_config_key(app, k, bg_tx).await?,
                Screen::SystemPrompt => handle_prompt_key(app, k, bg_tx).await?,
                Screen::EmailConfig => handle_email_key(app, k).await?,
                Screen::Models => handle_models_key(app, k, bg_tx).await?,
                Screen::Invites => handle_invites_key(app, k, bg_tx).await?,
                Screen::Users => handle_users_key(app, k, bg_tx).await?,
                Screen::Agents => handle_agents_key(app, k, bg_tx).await?,
            }
        }
    }
    Ok(false)
}

async fn handle_menu_key(
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
                app.screen = Screen::OpenRouterConfig;
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

async fn handle_config_key(
    app: &mut App,
    k: KeyEvent,
    bg_tx: &mpsc::UnboundedSender<BgMsg>,
) -> anyhow::Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.screen = Screen::Menu;
            // refresh counts
            let data_dir = app.data_dir.clone();
            let bg_tx = bg_tx.clone();
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
        KeyCode::Tab | KeyCode::Down => {
            app.focus = (app.focus + 1) % 5;
        }
        KeyCode::BackTab | KeyCode::Up => {
            app.focus = (app.focus + 4) % 5;
        }
        KeyCode::Enter => {
            if app.focus == 1 {
                // Model picker.
                if app.api_key.trim().is_empty() {
                    app.err = "API key is required to list models".to_string();
                    return Ok(());
                }
                app.screen = Screen::Models;
                app.loading_models = true;
                let api_key = app.api_key.clone();
                let base_url = if app.base_url.trim().is_empty() {
                    openrouter::DEFAULT_BASE_URL.to_string()
                } else {
                    app.base_url.clone()
                };
                let http_referer = app.http_referer.clone();
                let x_title = app.x_title.clone();
                let bg_tx = bg_tx.clone();
                tokio::spawn(async move {
                    let c = openrouter::Client::new(openrouter::Config {
                        api_key,
                        base_url,
                        http_referer,
                        x_title,
                    });
                    let res = match c {
                        Ok(c) => c.list_models().await.map_err(|e| e.to_string()),
                        Err(e) => Err(e.to_string()),
                    };
                    let _ = bg_tx.send(BgMsg::ModelsLoaded(res));
                });
            } else {
                app.focus = (app.focus + 1) % 5;
            }
        }
        KeyCode::Char('s') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            apply_openrouter_form_to_agent(app);
            let s = collect_settings(app);
            if s.openrouter.api_key.trim().is_empty() {
                app.err = "API key is required".to_string();
                return Ok(());
            }
            if s.openrouter.model.trim().is_empty() {
                app.err = "model is required".to_string();
                return Ok(());
            }
            config::save_settings(&config::settings_path(&app.data_dir), &s)?;
            app.info = "saved".to_string();
            app.err.clear();
        }
        _ => {
            edit_focused_field(app, k);
        }
    }
    Ok(())
}

async fn handle_prompt_key(
    app: &mut App,
    k: KeyEvent,
    bg_tx: &mpsc::UnboundedSender<BgMsg>,
) -> anyhow::Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.screen = Screen::Menu;
            let data_dir = app.data_dir.clone();
            let bg_tx = bg_tx.clone();
            tokio::spawn(async move {
                let mgr = db::Manager::new(data_dir);
                let n = mgr.waitlist_count().map_err(|e| e.to_string()).unwrap_or(0);
                let _ = bg_tx.send(BgMsg::InviteCount(n));
                let users = (|| -> Result<usize, String> {
                    let us = mgr.users().map_err(|e| e.to_string())?;
                    us.count_users().map_err(|e| e.to_string())
                })()
                .unwrap_or(0);
                let _ = bg_tx.send(BgMsg::UsersCount(users));
            });
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

async fn handle_email_key(app: &mut App, k: KeyEvent) -> anyhow::Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.screen = Screen::Menu;
        }
        KeyCode::Tab | KeyCode::Down => {
            app.focus = (app.focus + 1) % 5;
        }
        KeyCode::BackTab | KeyCode::Up => {
            app.focus = (app.focus + 4) % 5;
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

async fn handle_models_key(
    app: &mut App,
    k: KeyEvent,
    _bg_tx: &mpsc::UnboundedSender<BgMsg>,
) -> anyhow::Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.screen = Screen::OpenRouterConfig;
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
            app.model = id;
            apply_openrouter_form_to_agent(app);

            // Persist immediately (parity with Go).
            let s = collect_settings(app);
            if !s.openrouter.api_key.trim().is_empty() && !s.openrouter.model.trim().is_empty() {
                config::save_settings(&config::settings_path(&app.data_dir), &s)?;
            }

            app.screen = Screen::OpenRouterConfig;
            app.focus = 1;
        }
        KeyCode::Char(c) if ('1'..='9').contains(&c) => {
            if app.loading_models || app.models.is_empty() {
                return Ok(());
            }
            let col = (c as u8 - b'1') as usize;
            apply_sort(app, col);
        }
        _ => {}
    }
    Ok(())
}

async fn handle_invites_key(
    app: &mut App,
    k: KeyEvent,
    bg_tx: &mpsc::UnboundedSender<BgMsg>,
) -> anyhow::Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.screen = Screen::Menu;
            app.loading_waitlist = false;
            let data_dir = app.data_dir.clone();
            let bg_tx = bg_tx.clone();
            tokio::spawn(async move {
                let mgr = db::Manager::new(data_dir);
                let n = mgr.waitlist_count().map_err(|e| e.to_string()).unwrap_or(0);
                let _ = bg_tx.send(BgMsg::InviteCount(n));
                let users = (|| -> Result<usize, String> {
                    let us = mgr.users().map_err(|e| e.to_string())?;
                    us.count_users().map_err(|e| e.to_string())
                })()
                .unwrap_or(0);
                let _ = bg_tx.send(BgMsg::UsersCount(users));
            });
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

async fn handle_users_key(
    app: &mut App,
    k: KeyEvent,
    bg_tx: &mpsc::UnboundedSender<BgMsg>,
) -> anyhow::Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.screen = Screen::Menu;
            app.loading_users = false;
            let data_dir = app.data_dir.clone();
            let bg_tx = bg_tx.clone();
            tokio::spawn(async move {
                let mgr = db::Manager::new(data_dir);
                let n = mgr.waitlist_count().map_err(|e| e.to_string()).unwrap_or(0);
                let _ = bg_tx.send(BgMsg::InviteCount(n));
                let users = (|| -> Result<usize, String> {
                    let us = mgr.users().map_err(|e| e.to_string())?;
                    us.count_users().map_err(|e| e.to_string())
                })()
                .unwrap_or(0);
                let _ = bg_tx.send(BgMsg::UsersCount(users));
            });
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

async fn handle_agents_key(
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

fn collect_settings(app: &App) -> config::Settings {
    let mut s = config::default_settings();
    s.openrouter.api_key = app.api_key.trim().to_string();
    s.openrouter.model = app.model.trim().to_string();
    s.openrouter.base_url = app.base_url.trim().to_string();
    s.openrouter.http_referer = app.http_referer.trim().to_string();
    s.openrouter.x_title = app.x_title.trim().to_string();
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
    // Keep task bindings valid even if agents were renamed.
    if let Some(first) = s.agents.first() {
        if !s.agents.iter().any(|a| a.name == s.tasks.chat_agent) {
            s.tasks.chat_agent = first.name.clone();
        }
        if !s.agents.iter().any(|a| a.name == s.tasks.summary_agent) {
            s.tasks.summary_agent = first.name.clone();
        }
    }
    s
}

fn edit_focused_field(app: &mut App, k: KeyEvent) {
    let target = match app.focus {
        0 => &mut app.api_key,
        1 => &mut app.model,
        2 => &mut app.base_url,
        3 => &mut app.http_referer,
        4 => &mut app.x_title,
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
        KeyCode::Tab => {
            app.agent_focus = (app.agent_focus + 1) % 9;
        }
        KeyCode::BackTab => {
            app.agent_focus = (app.agent_focus + 8) % 9;
        }
        KeyCode::Down => {
            app.agent_focus = (app.agent_focus + 1) % 9;
        }
        KeyCode::Up => {
            app.agent_focus = (app.agent_focus + 8) % 9;
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if app.agent_focus == 1 {
                toggle_agent_provider(&mut app.agents[idx]);
            } else if app.agent_focus == 7 {
                app.task_chat = app.agents[idx].name.clone();
            } else if app.agent_focus == 8 {
                app.task_summary = app.agents[idx].name.clone();
            } else if app.agent_focus == 2 && !app.loading_agent_models {
                start_agent_model_load(app, idx, bg_tx.clone());
            }
        }
        KeyCode::Char('s') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            // Validate before saving
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
        _ => {
            // Text edits
            let target_opt = match app.agent_focus {
                0 => Some(&mut app.agents[idx].name),
                1 => None,
                2 => Some(&mut app.agents[idx].model),
                3 => Some(&mut app.agents[idx].base_url),
                4 => Some(&mut app.agents[idx].api_key),
                5 => Some(&mut app.agents[idx].http_referer),
                6 => Some(&mut app.agents[idx].x_title),
                7 | 8 => None,
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
                        app.task_summary = new_name;
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
    config::Agent {
        name: format!("agent{}", app.agents.len() + 1),
        provider: config::Provider::OpenRouter,
        model: app.model.clone(),
        base_url: app.base_url.clone(),
        api_key: app.api_key.clone(),
        http_referer: app.http_referer.clone(),
        x_title: app.x_title.clone(),
    }
}

fn apply_openrouter_form_to_agent(app: &mut App) {
    // Prefer updating the current chat agent if it's OpenRouter; otherwise update the first
    // OpenRouter agent (or create one) so the legacy "OpenRouter config" screen stays meaningful.
    let mut idx_opt = None;
    if let Some(i) = app
        .agents
        .iter()
        .position(|a| a.name == app.task_chat && matches!(a.provider, config::Provider::OpenRouter))
    {
        idx_opt = Some(i);
    }
    if idx_opt.is_none() {
        if let Some(i) = app
            .agents
            .iter()
            .position(|a| a.name == "default" && matches!(a.provider, config::Provider::OpenRouter))
        {
            idx_opt = Some(i);
        }
    }
    if idx_opt.is_none() {
        if let Some(i) = app
            .agents
            .iter()
            .position(|a| matches!(a.provider, config::Provider::OpenRouter))
        {
            idx_opt = Some(i);
        }
    }
    if idx_opt.is_none() {
        // Create a dedicated OpenRouter agent.
        let mut name = "openrouter".to_string();
        if app.agents.iter().any(|a| a.name == name) {
            name = format!("openrouter{}", app.agents.len() + 1);
        }
        app.agents.push(config::Agent {
            name,
            provider: config::Provider::OpenRouter,
            model: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            http_referer: String::new(),
            x_title: String::new(),
        });
        idx_opt = Some(app.agents.len() - 1);
    }

    let idx = idx_opt.unwrap_or(0);
    if idx >= app.agents.len() {
        return;
    }
    let a = &mut app.agents[idx];
    a.provider = config::Provider::OpenRouter;
    a.api_key = app.api_key.trim().to_string();
    a.model = app.model.trim().to_string();
    a.base_url = app.base_url.trim().to_string();
    a.http_referer = app.http_referer.trim().to_string();
    a.x_title = app.x_title.trim().to_string();
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
        match a.provider {
            config::Provider::OpenRouter => {
                if a.api_key.trim().is_empty() {
                    app.err = format!("{}: API key required for OpenRouter", a.name);
                    return false;
                }
            }
            config::Provider::Ollama => {}
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
            let mut rb = client.get(url_v1.clone());
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

fn move_table_selection(state: &mut TableState, len: usize, delta: i32) {
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

fn apply_sort(app: &mut App, col: usize) {
    let cols = 7usize;
    if col >= cols {
        return;
    }

    if app.sort_col == col {
        app.sort_asc = !app.sort_asc;
    } else {
        app.sort_col = col;
        // Default to descending for numeric columns.
        app.sort_asc = col == 0;
    }

    let asc = app.sort_asc;
    match col {
        0 => app.models.sort_by(|a, b| cmp_str(&a.id, &b.id, asc)),
        1 => app
            .models
            .sort_by(|a, b| cmp_i64(a.context_length, b.context_length, asc)),
        2 => app
            .models
            .sort_by(|a, b| cmp_cost(&a.pricing.prompt, &b.pricing.prompt, asc)),
        3 => app
            .models
            .sort_by(|a, b| cmp_cost(&a.pricing.completion, &b.pricing.completion, asc)),
        4 => app.models.sort_by(|a, b| {
            cmp_cost(
                &a.pricing.input_cache_read,
                &b.pricing.input_cache_read,
                asc,
            )
        }),
        5 => app.models.sort_by(|a, b| {
            cmp_cost(
                &a.pricing.input_cache_write,
                &b.pricing.input_cache_write,
                asc,
            )
        }),
        6 => app
            .models
            .sort_by(|a, b| cmp_cost(&a.pricing.request, &b.pricing.request, asc)),
        _ => {}
    }
}

fn cmp_str(a: &str, b: &str, asc: bool) -> std::cmp::Ordering {
    if asc {
        a.cmp(b)
    } else {
        b.cmp(a)
    }
}

fn cmp_i64(a: i64, b: i64, asc: bool) -> std::cmp::Ordering {
    if asc {
        a.cmp(&b)
    } else {
        b.cmp(&a)
    }
}

fn cmp_cost(a: &str, b: &str, asc: bool) -> std::cmp::Ordering {
    let pa = a.trim().parse::<f64>().ok();
    let pb = b.trim().parse::<f64>().ok();
    match (pa, pb) {
        (Some(aa), Some(bb)) => {
            if asc {
                aa.partial_cmp(&bb).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                bb.partial_cmp(&aa).unwrap_or(std::cmp::Ordering::Equal)
            }
        }
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => cmp_str(a, b, asc),
    }
}

fn draw(f: &mut ratatui::Frame, app: &mut App) {
    let size = f.area();
    app.win_w = size.width;
    app.win_h = size.height;

    // Paint base background for the entire frame.
    let bg = Block::default().style(Style::default().bg(THEME.base));
    f.render_widget(bg, size);

    match app.screen {
        Screen::Menu => draw_menu(f, size, app),
        Screen::OpenRouterConfig => draw_config(f, size, app),
        Screen::SystemPrompt => draw_prompt(f, size, app),
        Screen::EmailConfig => draw_email_config(f, size, app),
        Screen::Models => draw_models(f, size, app),
        Screen::Invites => draw_invites(f, size, app),
        Screen::Users => draw_users(f, size, app),
        Screen::Agents => draw_agents(f, size, app),
    }
}

fn draw_menu(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ]
            .as_ref(),
        )
        .split(area);

    let title = Paragraph::new(Line::from(vec![Span::styled(
        "Saelora Admin",
        Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Enter select | Ctrl+C quit",
        Style::default().fg(THEME.sub),
    )]));
    let stats = Paragraph::new(Line::from(vec![Span::styled(
        format!(
            "Messages sent: {}  ·  received: {}",
            app.msgs_sent, app.msgs_recv
        ),
        Style::default().fg(THEME.sub),
    )]));
    f.render_widget(title, chunks[0]);
    f.render_widget(stats, chunks[1]);
    f.render_widget(help, chunks[2]);

    let items = [
        "OpenRouter config".to_string(),
        "System Prompt".to_string(),
        "Mail (Mailjet)".to_string(),
        format!("Invites ({}/{})", app.invites_n, app.whitelist_n),
        format!("Users ({})", app.users_n),
        "Agents".to_string(),
    ];

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        if i == app.menu_idx {
            lines.push(Line::from(Span::styled(
                it.clone(),
                Style::default()
                    .fg(THEME.base)
                    .bg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                it.clone(),
                Style::default().fg(THEME.text),
            )));
        }
    }
    let list = Paragraph::new(Text::from(lines)).style(Style::default().bg(THEME.surface));
    f.render_widget(list, chunks[3]);
}

fn draw_users(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Percentage(45),
                Constraint::Percentage(45),
            ]
            .as_ref(),
        )
        .split(area);

    let title = if app.loading_users {
        "Users ...".to_string()
    } else {
        "Users".to_string()
    };
    let title = Paragraph::new(Line::from(vec![Span::styled(
        title,
        Style::default().add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Up/Down: move | A: active | D: disable | P: pending | R: reload | Esc: back",
        Style::default().fg(Color::DarkGray),
    )]));
    f.render_widget(title, chunks[0]);
    f.render_widget(help, chunks[1]);

    let mut status = String::new();
    if !app.err.trim().is_empty() {
        status = format!("Error: {}", app.err.trim());
    } else if !app.info.trim().is_empty() {
        status = app.info.trim().to_string();
    }
    let status_style = if !app.err.trim().is_empty() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(Paragraph::new(status).style(status_style), chunks[2]);

    if app.loading_users {
        f.render_widget(
            Paragraph::new("Loading users...").style(Style::default().fg(Color::DarkGray)),
            chunks[3],
        );
        return;
    }
    if app.users.is_empty() {
        f.render_widget(
            Paragraph::new("No users.").style(Style::default().fg(Color::DarkGray)),
            chunks[3],
        );
        return;
    }

    let cols = [("Email", 42), ("Status", 10), ("Created", 28), ("ID", 12)];
    let header = Row::new(cols.iter().map(|(t, _)| Cell::from(*t))).style(
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::White),
    );
    let rows: Vec<Row> = app
        .users
        .iter()
        .map(|r| {
            Row::new(vec![
                Cell::from(r.email.clone()),
                Cell::from(r.status.clone()),
                Cell::from(fmt_ts_ms(r.created_at)),
                Cell::from(trunc(&r.id, 12)),
            ])
        })
        .collect();
    let widths: Vec<Constraint> = cols.iter().map(|(_, w)| Constraint::Length(*w)).collect();
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .block(Block::default());
    let mut state = app.users_state;
    f.render_stateful_widget(table, chunks[3], &mut state);
}

fn draw_config(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(area);

    let title = Paragraph::new(Line::from(vec![Span::styled(
        "OpenRouter Config",
        Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Tab/Shift+Tab move | Enter on Model lists models | Ctrl+S save | Esc back",
        Style::default().fg(THEME.sub),
    )]));
    let stats = Paragraph::new(stats_line(app));
    f.render_widget(title, chunks[0]);
    f.render_widget(help, chunks[1]);
    f.render_widget(stats, chunks[2]);

    let rows = vec![
        render_field("API key:", &mask_key(&app.api_key), app.focus == 0),
        render_field("Model:", &app.model, app.focus == 1),
        render_field("Base:", &app.base_url, app.focus == 2),
        render_field("Referer:", &app.http_referer, app.focus == 3),
        render_field("Title:", &app.x_title, app.focus == 4),
    ];
    let content = Paragraph::new(Text::from(rows));
    f.render_widget(content, chunks[2]);

    let mut status = String::new();
    if !app.err.trim().is_empty() {
        status = format!("Error: {}", app.err.trim());
    } else if !app.info.trim().is_empty() {
        status = app.info.trim().to_string();
    }
    let status_style = if !app.err.trim().is_empty() {
        Style::default().fg(THEME.danger)
    } else {
        Style::default().fg(THEME.sub)
    };
    f.render_widget(Paragraph::new(status).style(status_style), chunks[3]);

    let path = config::settings_path(&app.data_dir);
    f.render_widget(
        Paragraph::new(format!("Settings are stored locally in {}", path.display()))
            .style(Style::default().fg(Color::DarkGray)),
        chunks[4],
    );
}

fn draw_email_config(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(area);

    let title = Paragraph::new(Line::from(vec![Span::styled(
        "Mailjet",
        Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Tab/Shift+Tab move | Ctrl+S save | Esc back",
        Style::default().fg(THEME.sub),
    )]));
    f.render_widget(title, chunks[0]);
    f.render_widget(help, chunks[1]);

    let rows = vec![
        render_field("API key:", &mask_key(&app.mail_api_key), app.focus == 0),
        render_field(
            "API secret:",
            &mask_key(&app.mail_api_secret),
            app.focus == 1,
        ),
        render_field("From email:", &app.mail_from_email, app.focus == 2),
        render_field("From name:", &app.mail_from_name, app.focus == 3),
        render_field(
            "Mailjet base URL (optional):",
            &app.mail_base_url,
            app.focus == 4,
        ),
        render_field("Public base (links):", &app.public_base, app.focus == 5),
    ];
    let content = Paragraph::new(Text::from(rows));
    f.render_widget(content, chunks[2]);

    let mut status = String::new();
    if !app.err.trim().is_empty() {
        status = format!("Error: {}", app.err.trim());
    } else if !app.info.trim().is_empty() {
        status = app.info.trim().to_string();
    }
    let status_style = if !app.err.trim().is_empty() {
        Style::default().fg(THEME.danger)
    } else {
        Style::default().fg(THEME.sub)
    };
    f.render_widget(Paragraph::new(status).style(status_style), chunks[3]);
}

fn draw_prompt(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(area);

    let title = Paragraph::new(Line::from(vec![Span::styled(
        "System Prompt",
        Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Ctrl+S save | Esc back",
        Style::default().fg(THEME.sub),
    )]));
    let stats = Paragraph::new(stats_line(app));
    f.render_widget(title, chunks[0]);
    f.render_widget(help, chunks[1]);
    f.render_widget(stats, chunks[2]);

    app.system_prompt.render(f, chunks[3]);

    let mut status = String::new();
    if !app.err.trim().is_empty() {
        status = format!("Error: {}", app.err.trim());
    } else if !app.info.trim().is_empty() {
        status = app.info.trim().to_string();
    }
    let status_style = if !app.err.trim().is_empty() {
        Style::default().fg(THEME.danger)
    } else {
        Style::default().fg(THEME.sub)
    };
    f.render_widget(Paragraph::new(status).style(status_style), chunks[4]);
}

fn draw_models(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ]
            .as_ref(),
        )
        .split(area);

    let title_text = if app.loading_models {
        "OpenRouter Models ...".to_string()
    } else {
        "OpenRouter Models".to_string()
    };
    let title = Paragraph::new(Line::from(vec![Span::styled(
        title_text,
        Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Up/Down: move | Enter: select | 1-7: sort by column | Esc: back",
        Style::default().fg(THEME.sub),
    )]));
    let sort = Paragraph::new(Line::from(vec![Span::styled(
        format!("Sort: {}", sort_label(app.sort_col, app.sort_asc)),
        Style::default().fg(THEME.sub),
    )]));
    f.render_widget(title, chunks[0]);
    f.render_widget(help, chunks[1]);
    f.render_widget(sort, chunks[2]);

    if app.loading_models {
        f.render_widget(
            Paragraph::new("Loading models...").style(Style::default().fg(Color::DarkGray)),
            chunks[3],
        );
        return;
    }
    if app.models.is_empty() {
        f.render_widget(
            Paragraph::new("No models loaded.").style(Style::default().fg(Color::DarkGray)),
            chunks[3],
        );
        return;
    }

    let body = chunks[3];
    let halves = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(body);

    let cols = [
        ("ID", 34),
        ("Ctx", 7),
        ("Prompt/$1M", 10),
        ("Comp/$1M", 9),
        ("CacheR/$1M", 11),
        ("CacheW/$1M", 11),
        ("Req", 8),
    ];

    let header = Row::new(cols.iter().map(|(t, _)| Cell::from(*t))).style(
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(THEME.base)
            .bg(THEME.accent_alt),
    );
    let rows: Vec<Row> = app
        .models
        .iter()
        .enumerate()
        .map(|(idx, m)| {
            let style = zebra_style(idx, app.models_state.selected() == Some(idx));
            Row::new(vec![
                Cell::from(m.id.clone()),
                Cell::from(format_ctx(m.context_length)),
                Cell::from(cost_per_1m(&m.pricing.prompt)),
                Cell::from(cost_per_1m(&m.pricing.completion)),
                Cell::from(cost_per_1m(&m.pricing.input_cache_read)),
                Cell::from(cost_per_1m(&m.pricing.input_cache_write)),
                Cell::from(cost_per_req(&m.pricing.request)),
            ])
            .style(style)
        })
        .collect();

    let widths: Vec<Constraint> = cols.iter().map(|(_, w)| Constraint::Length(*w)).collect();
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().fg(THEME.base).bg(THEME.accent))
        .block(Block::default());

    let mut state = app.models_state;
    f.render_stateful_widget(table, halves[0], &mut state);

    // Details panel in the bottom half (fixed) to avoid layout jumping.
    let idx = state.selected().unwrap_or(0).min(app.models.len() - 1);
    let md = &app.models[idx];
    let desc = if !md.description.trim().is_empty() {
        md.description.trim()
    } else if !md.name.trim().is_empty() {
        md.name.trim()
    } else {
        "(no description)"
    };

    let mut lines = Vec::<Line<'static>>::new();
    lines.push(Line::from(vec![Span::styled(
        format!("Selected: {}", md.id),
        Style::default().add_modifier(Modifier::BOLD),
    )]));
    if !md.name.trim().is_empty() {
        lines.push(Line::from(format!("Name: {}", md.name.trim())));
    }
    if md.context_length > 0 {
        lines.push(Line::from(format!("Context: {} tokens", md.context_length)));
    }

    let pricing = pricing_lines(md);
    if !pricing.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Pricing:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.extend(pricing);
    }

    lines.push(Line::from(""));
    lines.extend(wrap_lines(desc, halves[1].width.saturating_sub(2) as usize));

    let details = Paragraph::new(Text::from(lines))
        .block(Block::default())
        .style(Style::default());
    f.render_widget(details, halves[1]);
}

fn draw_agents(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ]
            .as_ref(),
        )
        .split(area);

    let title = Paragraph::new(Line::from(vec![Span::styled(
        "Agents",
        Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Up/Down: select | Tab: next field | P: toggle provider | N: new | D: delete | C: set chat | Y: set summary | Ctrl+S save | Esc back",
        Style::default().fg(THEME.sub),
    )]));
    let tasks = Paragraph::new(Line::from(vec![Span::styled(
        format!(
            "Chat agent: {}   ·   Summary agent: {}",
            app.task_chat, app.task_summary
        ),
        Style::default().fg(THEME.sub),
    )]));

    f.render_widget(title, chunks[0]);
    f.render_widget(help, chunks[1]);
    f.render_widget(tasks, chunks[2]);

    let body = chunks[3];
    let halves = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)].as_ref())
        .split(body);

    // Left: agents table
    let cols = [("Name", 14), ("Provider", 10), ("Model", 26)];
    let header = Row::new(cols.iter().map(|(t, _)| Cell::from(*t))).style(
        Style::default()
            .fg(THEME.base)
            .bg(THEME.accent_alt)
            .add_modifier(Modifier::BOLD),
    );
    let mut rows: Vec<Row> = app
        .agents
        .iter()
        .enumerate()
        .map(|(idx, a)| {
            let style = zebra_style(idx, app.agents_state.selected() == Some(idx));
            Row::new(vec![
                Cell::from(a.name.clone()),
                Cell::from(match a.provider {
                    config::Provider::OpenRouter => "openrouter",
                    config::Provider::Ollama => "ollama",
                }),
                Cell::from(a.model.clone()),
            ])
            .style(style)
        })
        .collect();
    rows.push(
        Row::new(vec![
            Cell::from("Add agent"),
            Cell::from(""),
            Cell::from(""),
        ])
        .style(zebra_style(
            app.agents.len(),
            app.agents_state.selected() == Some(app.agents.len()),
        )),
    );
    let widths: Vec<Constraint> = cols.iter().map(|(_, w)| Constraint::Length(*w)).collect();
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().fg(THEME.base).bg(THEME.accent))
        .block(Block::default());
    let mut state = app.agents_state;
    f.render_stateful_widget(table, halves[0], &mut state);

    // Right: detail editor
    if app.agent_modal && app.agent_modal_idx.is_some() {
        draw_agent_modal(f, area, app);
    }
}

fn draw_agent_modal(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let Some(idx) = app.agent_modal_idx else {
        return;
    };
    if idx >= app.agents.len() {
        return;
    }
    let agent = &app.agents[idx];
    let w = (area.width as f32 * 0.92) as u16;
    let h = 13u16;
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    let modal_area = Rect::new(x, y, w, h);
    let provider = match agent.provider {
        config::Provider::OpenRouter => "openrouter",
        config::Provider::Ollama => "ollama",
    };
    let api_mask = mask_key(&agent.api_key);

    let lines = vec![
        render_field("Name:", &agent.name, app.agent_focus == 0),
        render_field("Provider (Enter/Space):", provider, app.agent_focus == 1),
        render_field(
            "Model (Enter to list):",
            &agent.model,
            app.agent_focus == 2 && !app.model_picker && !app.loading_agent_models,
        ),
        render_field("Base URL:", &agent.base_url, app.agent_focus == 3),
        render_field("API key:", &api_mask, app.agent_focus == 4),
        render_field("Referer:", &agent.http_referer, app.agent_focus == 5),
        render_field("Title:", &agent.x_title, app.agent_focus == 6),
        render_field(
            "Use for chat:",
            if app.task_chat == agent.name {
                "yes"
            } else {
                "no"
            },
            app.agent_focus == 7,
        ),
        render_field(
            "Use for summary:",
            if app.task_summary == agent.name {
                "yes"
            } else {
                "no"
            },
            app.agent_focus == 8,
        ),
    ];

    let block = Block::default()
        .title(Span::styled(
            if app.loading_agent_models {
                "Loading models..."
            } else if app.model_picker {
                "Select model (Up/Down, Enter choose, Esc cancel)"
            } else {
                "Edit agent · Up/Down move · Enter toggles/opens · Ctrl+S save · Esc close"
            },
            Style::default().fg(THEME.sub),
        ))
        .borders(Borders::ALL)
        .style(Style::default().bg(THEME.surface).fg(THEME.text));

    let overlay_bg = Block::default().style(Style::default().bg(THEME.base));
    f.render_widget(overlay_bg, modal_area);
    f.render_widget(block, modal_area);
    if app.model_picker {
        let list_area = Rect::new(
            modal_area.x + 1,
            modal_area.y + 2,
            modal_area.width - 2,
            modal_area.height - 3,
        );
        let rows: Vec<Row> = app
            .agent_models
            .iter()
            .enumerate()
            .map(|(i, m)| {
                Row::new(vec![Cell::from(m.clone())])
                    .style(zebra_style(i, i == app.model_picker_idx))
            })
            .collect();
        let table = Table::new(rows, vec![Constraint::Percentage(100)])
            .row_highlight_style(Style::default().fg(THEME.base).bg(THEME.accent));
        let mut state = TableState::default();
        state.select(Some(
            app.model_picker_idx
                .min(app.agent_models.len().saturating_sub(1)),
        ));
        f.render_stateful_widget(table, list_area, &mut state);
    } else {
        f.render_widget(Paragraph::new(Text::from(lines)), modal_area);
    }
}

fn draw_invites(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ]
            .as_ref(),
        )
        .split(area);

    let title = if app.loading_waitlist {
        "Invites ...".to_string()
    } else {
        format!(
            "Invites pending:{} whitelist:{}",
            app.invites_n, app.whitelist_n
        )
    };
    let title = Paragraph::new(Line::from(vec![Span::styled(
        title,
        Style::default().add_modifier(Modifier::BOLD),
    )]));
    let help = Paragraph::new(Line::from(vec![Span::styled(
        "Up/Down: move | Enter/A: approve | D/X: remove | R: reload | Esc: back",
        Style::default().fg(Color::DarkGray),
    )]));
    f.render_widget(title, chunks[0]);
    f.render_widget(help, chunks[1]);

    let mut status = String::new();
    if !app.err.trim().is_empty() {
        status = format!("Error: {}", app.err.trim());
    } else if !app.info.trim().is_empty() {
        status = app.info.trim().to_string();
    }
    let status_style = if !app.err.trim().is_empty() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(Paragraph::new(status).style(status_style), chunks[2]);

    if app.loading_waitlist {
        f.render_widget(
            Paragraph::new("Loading waitlist...").style(Style::default().fg(Color::DarkGray)),
            chunks[3],
        );
        return;
    }
    if app.waitlist.is_empty() {
        f.render_widget(
            Paragraph::new("No pending requests.").style(Style::default().fg(Color::DarkGray)),
            chunks[3],
        );
    }

    // Pending table
    {
        let cols = [("Pending email", 46), ("Requested", 28)];
        let header = Row::new(cols.iter().map(|(t, _)| Cell::from(*t))).style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::White),
        );
        let rows: Vec<Row> = app
            .waitlist
            .iter()
            .map(|r| {
                Row::new(vec![
                    Cell::from(r.email.clone()),
                    Cell::from(fmt_ts_ms(r.created_at)),
                ])
            })
            .collect();
        let widths: Vec<Constraint> = cols.iter().map(|(_, w)| Constraint::Length(*w)).collect();
        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
            .block(Block::default());
        let mut state = app.waitlist_state;
        f.render_stateful_widget(table, chunks[3], &mut state);
    }

    // Whitelist table
    {
        if app.whitelist.is_empty() {
            f.render_widget(
                Paragraph::new("Whitelist is empty.").style(Style::default().fg(Color::DarkGray)),
                chunks[4],
            );
        } else {
            let cols = [("Whitelisted email", 46), ("Added", 28)];
            let header = Row::new(cols.iter().map(|(t, _)| Cell::from(*t))).style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::White),
            );
            let rows: Vec<Row> = app
                .whitelist
                .iter()
                .map(|r| {
                    Row::new(vec![
                        Cell::from(r.email.clone()),
                        Cell::from(fmt_ts_ms(r.created_at)),
                    ])
                })
                .collect();
            let widths: Vec<Constraint> =
                cols.iter().map(|(_, w)| Constraint::Length(*w)).collect();
            let table = Table::new(rows, widths)
                .header(header)
                .block(Block::default());
            f.render_widget(table, chunks[4]);
        }
    }
}

fn render_field(label: &str, val: &str, focused: bool) -> Line<'static> {
    let style = if focused {
        Style::default()
            .fg(THEME.base)
            .bg(THEME.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(THEME.text).bg(THEME.surface)
    };
    Line::from(vec![
        Span::styled(format!("{:<22} ", label), style),
        Span::styled(val.to_string(), style),
    ])
}

fn mask_key(k: &str) -> String {
    let t = k.trim();
    if t.is_empty() {
        return "".to_string();
    }
    let n = t.len().min(32);
    "*".repeat(n)
}

fn stats_line(app: &App) -> Line<'static> {
    Line::from(Span::styled(
        format!(
            "Messages sent: {}  ·  received: {}",
            app.msgs_sent, app.msgs_recv
        ),
        Style::default().fg(THEME.sub),
    ))
}

fn zebra_style(idx: usize, selected: bool) -> Style {
    if selected {
        Style::default().fg(THEME.base).bg(THEME.accent)
    } else if idx.is_multiple_of(2) {
        Style::default().fg(THEME.text).bg(THEME.surface)
    } else {
        Style::default().fg(THEME.text).bg(THEME.surface_alt)
    }
}

fn format_ctx(n: i64) -> String {
    if n <= 0 {
        "".to_string()
    } else {
        n.to_string()
    }
}

fn cost_per_1m(v: &str) -> String {
    let t = v.trim();
    if t.is_empty() {
        return "".to_string();
    }
    if let Ok(f) = t.parse::<f64>() {
        return format!("${:.2}", f * 1_000_000.0);
    }
    t.to_string()
}

fn cost_per_req(v: &str) -> String {
    let t = v.trim();
    if t.is_empty() {
        return "".to_string();
    }
    if let Ok(f) = t.parse::<f64>() {
        return format!("${:.4}", f);
    }
    t.to_string()
}

fn fmt_ts_ms(ms: i64) -> String {
    if ms <= 0 {
        return "".to_string();
    }
    use chrono::TimeZone as _;
    chrono::Utc
        .timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| ms.to_string())
}

fn trunc(s: &str, max: usize) -> String {
    let t = s.trim();
    if max == 0 || t.chars().count() <= max {
        return t.to_string();
    }
    if max <= 3 {
        return t.chars().take(max).collect();
    }
    let mut out: String = t.chars().take(max - 3).collect();
    out.push_str("...");
    out
}

fn sort_label(col: usize, asc: bool) -> String {
    let name = match col {
        0 => "ID",
        1 => "Ctx",
        2 => "Prompt/$1M",
        3 => "Comp/$1M",
        4 => "CacheR/$1M",
        5 => "CacheW/$1M",
        6 => "Req",
        _ => "?",
    };
    format!("{} {}", name, if asc { "asc" } else { "desc" })
}

fn pricing_lines(m: &openrouter::Model) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    if !m.pricing.prompt.trim().is_empty() {
        out.push(Line::from(format!(
            "  prompt: {} / 1M tokens",
            cost_per_1m(&m.pricing.prompt)
        )));
    }
    if !m.pricing.completion.trim().is_empty() {
        out.push(Line::from(format!(
            "  completion: {} / 1M tokens",
            cost_per_1m(&m.pricing.completion)
        )));
    }
    if !m.pricing.input_cache_read.trim().is_empty() {
        out.push(Line::from(format!(
            "  input_cache_read: {} / 1M tokens",
            cost_per_1m(&m.pricing.input_cache_read)
        )));
    }
    if !m.pricing.input_cache_write.trim().is_empty() {
        out.push(Line::from(format!(
            "  input_cache_write: {} / 1M tokens",
            cost_per_1m(&m.pricing.input_cache_write)
        )));
    }
    if !m.pricing.request.trim().is_empty() {
        out.push(Line::from(format!(
            "  request: {} / request",
            cost_per_req(&m.pricing.request)
        )));
    }
    out
}

fn wrap_lines(s: &str, width: usize) -> Vec<Line<'static>> {
    if width < 10 {
        return vec![Line::from(s.to_string())];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        if cur.is_empty() {
            cur.push_str(word);
            continue;
        }
        if cur.len() + 1 + word.len() > width {
            out.push(Line::from(cur.clone()));
            cur.clear();
            cur.push_str(word);
        } else {
            cur.push(' ');
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        out.push(Line::from(cur));
    }
    out
}

#[derive(Debug, Clone)]
struct VisualLine {
    row: usize,
    start_col: usize, // char offset in the logical row
    end_col: usize,   // char offset (exclusive) in the logical row
    text: String,
}

#[derive(Debug, Clone)]
struct PromptEditor {
    placeholder: String,
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,        // char index within the row
    desired_x: Option<usize>, // visual x in cells for up/down
    scroll: usize,            // visual row scroll (soft-wrapped lines)
    last_wrap_w: usize,       // updated during render
}

impl PromptEditor {
    fn from_text(text: &str, placeholder: &str) -> Self {
        let mut lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self {
            placeholder: placeholder.to_string(),
            lines,
            cursor_row: 0,
            cursor_col: 0,
            desired_x: None,
            scroll: 0,
            last_wrap_w: 80,
        }
    }

    fn set_text(&mut self, text: &str) {
        self.lines = text.split('\n').map(|s| s.to_string()).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.desired_x = None;
        self.scroll = 0;
    }

    fn text(&self) -> String {
        self.lines.join("\n")
    }

    fn handle_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char(c) => {
                if k.modifiers.contains(KeyModifiers::CONTROL)
                    || k.modifiers.contains(KeyModifiers::ALT)
                {
                    return;
                }
                self.insert_char(c);
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Enter => self.insert_newline(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Up => self.move_visual(-1),
            KeyCode::Down => self.move_visual(1),
            KeyCode::Home => {
                self.cursor_col = 0;
                self.desired_x = None;
            }
            KeyCode::End => {
                self.cursor_col = self.line_len_chars(self.cursor_row);
                self.desired_x = None;
            }
            _ => {}
        }
    }

    fn render(&mut self, f: &mut ratatui::Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        f.render_widget(block.clone(), area);
        let inner = block.inner(area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let wrap_w = inner.width as usize;
        self.last_wrap_w = wrap_w.max(1);
        let visual = self.visual_lines(self.last_wrap_w);

        // Keep cursor visible in the scrolled visual viewport.
        let (cur_vy, cur_vx) = self.cursor_visual_pos(&visual);
        if self.desired_x.is_none() {
            self.desired_x = Some(cur_vx);
        }
        if cur_vy < self.scroll {
            self.scroll = cur_vy;
        } else if cur_vy >= self.scroll + inner.height as usize {
            self.scroll = cur_vy.saturating_sub(inner.height as usize - 1);
        }

        let start = self.scroll.min(visual.len());
        let end = (start + inner.height as usize).min(visual.len());

        let mut out_lines: Vec<Line<'static>> = Vec::new();
        if (self.lines.len() == 1 && self.lines[0].is_empty()) || visual.is_empty() {
            out_lines.push(Line::from(Span::styled(
                self.placeholder.clone(),
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for vl in &visual[start..end] {
                // Pad to width for stable cursor placement.
                let mut t = vl.text.clone();
                let w = t.width();
                if w < self.last_wrap_w {
                    t.push_str(&" ".repeat(self.last_wrap_w - w));
                }
                out_lines.push(Line::from(t));
            }
        }

        let para = Paragraph::new(Text::from(out_lines)).style(Style::default());
        f.render_widget(para, inner);

        let cur_screen_y = cur_vy.saturating_sub(self.scroll);
        if cur_screen_y < inner.height as usize {
            let x = inner.x.saturating_add(cur_vx as u16);
            let y = inner.y.saturating_add(cur_screen_y as u16);
            f.set_cursor_position((x, y));
        }
    }

    fn insert_char(&mut self, c: char) {
        let row = self.cursor_row.min(self.lines.len().saturating_sub(1));
        let col = self.cursor_col.min(self.line_len_chars(row));
        let s = &mut self.lines[row];
        let byte = char_to_byte_index(s, col);
        s.insert(byte, c);
        self.cursor_row = row;
        self.cursor_col = col + 1;
        self.desired_x = None;
    }

    fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let row = self.cursor_row;
            let col = self.cursor_col.min(self.line_len_chars(row));
            let s = &mut self.lines[row];
            let b0 = char_to_byte_index(s, col - 1);
            let b1 = char_to_byte_index(s, col);
            s.replace_range(b0..b1, "");
            self.cursor_col = col - 1;
            self.desired_x = None;
            return;
        }
        if self.cursor_row > 0 {
            let row = self.cursor_row;
            let cur = self.lines.remove(row);
            let prev = row - 1;
            let prev_len = self.line_len_chars(prev);
            self.lines[prev].push_str(&cur);
            self.cursor_row = prev;
            self.cursor_col = prev_len;
            self.desired_x = None;
        }
    }

    fn insert_newline(&mut self) {
        let row = self.cursor_row;
        let col = self.cursor_col.min(self.line_len_chars(row));
        let s = &mut self.lines[row];
        let byte = char_to_byte_index(s, col);
        let tail = s[byte..].to_string();
        s.truncate(byte);
        self.lines.insert(row + 1, tail);
        self.cursor_row = row + 1;
        self.cursor_col = 0;
        self.desired_x = None;
    }

    fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            self.desired_x = None;
            return;
        }
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.line_len_chars(self.cursor_row);
            self.desired_x = None;
        }
    }

    fn move_right(&mut self) {
        let len = self.line_len_chars(self.cursor_row);
        if self.cursor_col < len {
            self.cursor_col += 1;
            self.desired_x = None;
            return;
        }
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
            self.desired_x = None;
        }
    }

    fn move_visual(&mut self, delta: i32) {
        let wrap_w = self.last_wrap_w.max(1);
        let visual = self.visual_lines(wrap_w);
        if visual.is_empty() {
            return;
        }
        let (cur_vy, cur_vx) = self.cursor_visual_pos(&visual);
        let vx = self.desired_x.unwrap_or(cur_vx);

        let target_vy = if delta < 0 {
            cur_vy.saturating_sub(delta.unsigned_abs() as usize)
        } else {
            (cur_vy + delta as usize).min(visual.len().saturating_sub(1))
        };

        let vl = &visual[target_vy];
        let row = vl.row;
        let col = col_at_x(&self.lines[row], vl.start_col, vl.end_col, vx);
        self.cursor_row = row;
        self.cursor_col = col;
        self.desired_x = Some(vx);
    }

    fn cursor_visual_pos(&self, visual: &[VisualLine]) -> (usize, usize) {
        if visual.is_empty() {
            return (0, 0);
        }
        let row = self.cursor_row.min(self.lines.len().saturating_sub(1));
        let col = self.cursor_col.min(self.line_len_chars(row));
        for (i, vl) in visual.iter().enumerate() {
            if vl.row != row {
                continue;
            }
            if col < vl.start_col {
                continue;
            }
            if col > vl.end_col {
                continue;
            }
            let seg = slice_chars(&self.lines[row], vl.start_col, col);
            return (i, seg.width().min(self.last_wrap_w));
        }
        (0, 0)
    }

    fn visual_lines(&self, wrap_w: usize) -> Vec<VisualLine> {
        if wrap_w == 0 {
            return vec![];
        }
        let mut out = Vec::new();
        for (row, line) in self.lines.iter().enumerate() {
            if line.is_empty() {
                out.push(VisualLine {
                    row,
                    start_col: 0,
                    end_col: 0,
                    text: String::new(),
                });
                continue;
            }

            let chars: Vec<char> = line.chars().collect();
            let mut start = 0usize;
            while start < chars.len() {
                let (end, text) = wrap_segment(&chars, start, wrap_w);
                out.push(VisualLine {
                    row,
                    start_col: start,
                    end_col: end,
                    text,
                });
                start = end;
            }
        }
        out
    }

    fn line_len_chars(&self, row: usize) -> usize {
        self.lines.get(row).map(|s| s.chars().count()).unwrap_or(0)
    }
}

fn wrap_segment(chars: &[char], start: usize, max_w: usize) -> (usize, String) {
    // Prefer breaking on whitespace, but fall back to a hard break.
    let mut w = 0usize;
    let mut end = start;
    let mut last_space: Option<usize> = None;

    while end < chars.len() {
        let cw = chars[end].width().unwrap_or(0).max(1);
        if w + cw > max_w {
            break;
        }
        if chars[end].is_whitespace() {
            last_space = Some(end);
        }
        w += cw;
        end += 1;
    }

    if end == start {
        end = (start + 1).min(chars.len());
    } else if end < chars.len() {
        if let Some(sp) = last_space {
            if sp > start {
                end = sp;
            }
        }
    }

    let mut text: String = chars[start..end].iter().collect();
    while text.ends_with(' ') || text.ends_with('\t') {
        text.pop();
    }
    (end, text)
}

fn slice_chars(s: &str, start: usize, end: usize) -> String {
    s.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn char_to_byte_index(s: &str, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or_else(|| s.len())
}

fn col_at_x(s: &str, start_col: usize, end_col: usize, want_x: usize) -> usize {
    let mut x = 0usize;
    let mut col = start_col;
    for c in s
        .chars()
        .skip(start_col)
        .take(end_col.saturating_sub(start_col))
    {
        let cw = c.width().unwrap_or(0).max(1);
        if x + cw > want_x {
            break;
        }
        x += cw;
        col += 1;
    }
    col
}
