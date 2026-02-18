mod admin;
mod auth;
mod chat;
mod cors;
mod errors;
mod health;
mod logging;
mod rate_limit;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::DefaultBodyLimit;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post};
use axum::Router;
use tokio::sync::{oneshot, Mutex, Notify};
use tracing::{info, warn};

use crate::httpui;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub addr: String,
    pub data_dir: PathBuf,
}

#[derive(Clone)]
struct AppState {
    data_dir: PathBuf,
    last_key_info_log: Arc<Mutex<Instant>>,
    auth_rate_limits: Arc<Mutex<HashMap<String, rate_limit::Bucket>>>,
}

pub async fn run_server(
    cfg: ServerConfig,
    shutdown_rx: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    crate::memory::ensure_backfill_worker(cfg.data_dir.clone());

    let state = AppState {
        data_dir: cfg.data_dir.clone(),
        last_key_info_log: Arc::new(Mutex::new(Instant::now() - Duration::from_secs(3600))),
        auth_rate_limits: Arc::new(Mutex::new(HashMap::new())),
    };

    let auth_routes = Router::new()
        .route("/healthz", get(health::healthz))
        .route("/invite", post(auth::invite))
        .route("/v1/auth/setup", post(auth::auth_setup))
        .route(
            "/v1/auth/request-password-link",
            post(auth::auth_request_password_link),
        )
        .route("/v1/auth/register", post(auth::auth_register))
        .route("/v1/auth/login", post(auth::auth_login))
        .route("/v1/auth/logout", post(auth::auth_logout))
        .route("/v1/auth/logout-all", post(auth::auth_logout_all))
        .route("/v1/auth/me", get(auth::auth_me))
        .route_layer(from_fn_with_state(
            state.clone(),
            rate_limit::auth_rate_limit,
        ));

    let app = Router::new()
        .merge(auth_routes)
        .route("/v1/admin/overview", get(admin::overview))
        .route(
            "/v1/admin/config",
            get(admin::get_config).post(admin::save_config),
        )
        .route("/v1/admin/invites", get(admin::list_invites))
        .route("/v1/admin/invites/approve", post(admin::approve_invite))
        .route("/v1/admin/invites/remove", post(admin::remove_invite))
        .route("/v1/admin/users", get(admin::list_users))
        .route("/v1/admin/users/status", post(admin::set_user_status))
        .route(
            "/v1/chat/completions",
            post(chat::chat_completions).options(chat::chat_options),
        )
        .route("/v1/chat/conversations", get(chat::list_conversations))
        .route("/v1/chat/conversations", post(chat::create_conversation))
        .route(
            "/v1/chat/conversations/:id/rename",
            post(chat::rename_conversation),
        )
        .route(
            "/v1/chat/conversations/:id/mode",
            post(chat::set_conversation_mode),
        )
        .route(
            "/v1/chat/conversations/:id/message",
            post(chat::thread_message),
        )
        .route(
            "/v1/chat/conversations/:id/tick",
            post(chat::tick_conversation),
        )
        .route(
            "/v1/chat/conversations/:id/archive",
            post(chat::archive_conversation),
        )
        .route("/v1/chat/history", get(chat::chat_history))
        .merge(httpui::router::<AppState>())
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(1 << 20))
        .layer(from_fn_with_state(state.clone(), cors::cors))
        .layer(from_fn(logging::request_logger));

    let shutdown_notify = Arc::new(Notify::new());
    let shutdown_notify2 = shutdown_notify.clone();
    tokio::spawn(async move {
        let _ = shutdown_rx.await;
        shutdown_notify2.notify_waiters();
    });

    let addrs = crate::listen_addrs(&cfg.addr);
    let mut tasks = Vec::new();
    let mut any = false;

    for (i, a) in addrs.into_iter().enumerate() {
        let listener = match tokio::net::TcpListener::bind(a).await {
            Ok(l) => {
                any = true;
                l
            }
            Err(e) => {
                if i == 0 {
                    // The requested addr must bind.
                    let hint = match e.kind() {
                        std::io::ErrorKind::AddrInUse => {
                            " (address in use: stop the other process or pick a different --addr)"
                        }
                        std::io::ErrorKind::PermissionDenied => {
                            " (permission denied: port may already be in use; stop the other process or pick a different --addr)"
                        }
                        _ => "",
                    };
                    return Err(anyhow::anyhow!("listen {} failed: {}{}", a, e, hint));
                }
                // Alternate loopback family is best-effort.
                warn!(addr=%a, err=%e, "http: listen failed");
                continue;
            }
        };

        let app = app.clone();
        let notify = shutdown_notify.clone();
        let task = tokio::spawn(async move {
            info!(addr=%a, "http: listening");
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    notify.notified().await;
                })
                .await?;
            Ok::<(), anyhow::Error>(())
        });
        tasks.push(task);
    }

    if !any {
        anyhow::bail!("listen: no listeners created");
    }

    for t in tasks {
        match t.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(anyhow::anyhow!("server task join error: {e}")),
        }
    }
    Ok(())
}
