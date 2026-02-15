use ratatui::style::Style;
use ratatui::widgets::Block;

use super::{App, Screen, THEME};

mod agents_screen;
mod email_screen;
mod invites_screen;
mod menu_screen;
mod models_screen;
mod prompt_screen;
mod users_screen;
mod util;

pub(super) fn draw(f: &mut ratatui::Frame, app: &mut App) {
    let size = f.area();
    app.win_w = size.width;
    app.win_h = size.height;

    // Paint base background for the entire frame.
    let bg = Block::default().style(Style::default().bg(THEME.base));
    f.render_widget(bg, size);

    match app.screen {
        Screen::Menu => menu_screen::draw_menu(f, size, app),
        Screen::SystemPrompt => prompt_screen::draw_prompt(f, size, app),
        Screen::EmailConfig => email_screen::draw_email_config(f, size, app),
        Screen::Models => models_screen::draw_models(f, size, app),
        Screen::Invites => invites_screen::draw_invites(f, size, app),
        Screen::Users => users_screen::draw_users(f, size, app),
        Screen::Agents => agents_screen::draw_agents(f, size, app),
    }
}
