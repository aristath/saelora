use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_stream::try_stream;
use axum::body::Body;
use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::extract::State;
use axum::http::{header, HeaderMap, Method, Request, StatusCode, Uri};
use axum::middleware::{from_fn, from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use futures::{Stream, StreamExt};
use tokio::sync::{oneshot, Mutex, Notify};
use tracing::{info, warn};
use uuid::Uuid;

use crate::config;
use crate::db;
use crate::email;
use crate::httpui;
use crate::openrouter;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub addr: String,
    pub data_dir: PathBuf,
}

#[derive(Clone)]
struct AppState {
    data_dir: PathBuf,
    last_key_info_log: Arc<Mutex<Instant>>,
}

pub async fn run_server(
    cfg: ServerConfig,
    shutdown_rx: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let state = AppState {
        data_dir: cfg.data_dir.clone(),
        last_key_info_log: Arc::new(Mutex::new(Instant::now() - Duration::from_secs(3600))),
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/invite", post(invite))
        .route("/v1/auth/setup", post(auth_setup))
        .route(
            "/v1/auth/request-password-link",
            post(auth_request_password_link),
        )
        .route("/v1/auth/register", post(auth_register))
        .route("/v1/auth/login", post(auth_login))
        .route("/v1/auth/logout", post(auth_logout))
        .route("/v1/auth/me", get(auth_me))
        .route(
            "/v1/chat/completions",
            post(chat_completions).options(chat_options),
        )
        .merge(httpui::router::<AppState>())
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(1 << 20))
        .layer(from_fn_with_state(state.clone(), cors))
        .layer(from_fn(request_logger));

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
                    // The requested addr must bind, just like the Go implementation.
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

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok\n")
}

async fn invite(State(st): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    // Accept JSON {email} or form-encoded email=...
    #[derive(serde::Deserialize)]
    struct Payload {
        email: String,
    }

    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mut email = String::new();
    if ct.starts_with("application/json") {
        if let Ok(p) = serde_json::from_slice::<Payload>(&body) {
            email = p.email;
        }
    } else if let Ok(p) = serde_urlencoded::from_bytes::<Payload>(&body) {
        email = p.email;
    }

    let mgr = db::Manager::new(st.data_dir.clone());
    match mgr.waitlist_add(&email) {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(db::DbError::InvalidEmail) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": { "message": "invalid email" } })),
        )
            .into_response(),
        Err(e) => {
            warn!(err=%e, "invite: server error");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": { "message": "server error" } })),
            )
                .into_response()
        }
    }
}

async fn chat_options() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ChatCompletionsRequest {
    #[serde(default)]
    model: String,
    messages: Vec<openrouter::Message>,
    #[serde(default)]
    stream: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
struct OpenAiErrorResponse {
    error: OpenAiError,
}

#[derive(Debug, Clone, serde::Serialize)]
struct OpenAiError {
    message: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    r#type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    code: String,
}

fn openai_error(code: StatusCode, msg: &str, typ: &str, api_code: &str) -> Response {
    let body = OpenAiErrorResponse {
        error: OpenAiError {
            message: msg.to_string(),
            r#type: typ.to_string(),
            code: api_code.to_string(),
        },
    };
    (code, Json(body)).into_response()
}

#[derive(Debug, Clone, serde::Serialize)]
struct AuthErrorResponse {
    error: AuthError,
}

#[derive(Debug, Clone, serde::Serialize)]
struct AuthError {
    message: String,
}

fn auth_error(code: StatusCode, msg: &str) -> Response {
    (
        code,
        Json(AuthErrorResponse {
            error: AuthError {
                message: msg.to_string(),
            },
        }),
    )
        .into_response()
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SetupRequest {
    token: String,
    password: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RequestLinkRequest {
    email: String,
}

async fn auth_setup(State(st): State<AppState>, body: Bytes) -> Response {
    let req: SetupRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return auth_error(StatusCode::BAD_REQUEST, "invalid json"),
    };
    let token = req.token.trim().to_string();
    let password = req.password;
    if token.is_empty() {
        return auth_error(StatusCode::BAD_REQUEST, "missing token");
    }
    if password.trim().len() < 8 {
        return auth_error(StatusCode::BAD_REQUEST, "password too short");
    }

    let ph = match db::hash_password(&password) {
        Ok(h) => h,
        Err(_) => return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error"),
    };

    let mgr = db::Manager::new(st.data_dir.clone());
    let us = match mgr.users() {
        Ok(us) => us,
        Err(e) => {
            warn!(err=%e, "auth_setup: db open failed");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    // Resolve email for this token without consuming it.
    let email = match us.peek_magic_token_email(&token) {
        Ok(e) => e,
        Err(db::DbError::TokenInvalid) => {
            return auth_error(StatusCode::BAD_REQUEST, "invalid or expired token")
        }
        Err(e) => {
            warn!(err=%e, "auth_setup: token peek failed");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    // If this is a new account, require whitelist at setup time.
    let existed = match us.user_auth_row_by_email(&email) {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(e) => {
            warn!(err=%e, "auth_setup: user lookup failed");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };
    if !existed {
        let whitelisted = match mgr.whitelist_has(&email) {
            Ok(v) => v,
            Err(e) => {
                warn!(err=%e, "auth_setup: whitelist read failed");
                return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
            }
        };
        if !whitelisted {
            return auth_error(StatusCode::BAD_REQUEST, "invalid token");
        }
    }

    let uid = match us.consume_magic_token_set_password(&token, &ph, !existed) {
        Ok(uid) => uid,
        Err(db::DbError::TokenInvalid) => {
            return auth_error(StatusCode::BAD_REQUEST, "invalid or expired token")
        }
        Err(db::DbError::Unauthorized) => {
            return auth_error(StatusCode::BAD_REQUEST, "invalid token")
        }
        Err(e) => {
            warn!(err=%e, "auth_setup: db error");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    // If this created an account from the whitelist, clean up the lists.
    if !existed {
        let _ = mgr.whitelist_remove(&email);
        let _ = mgr.waitlist_remove(&email);
    }

    // Auto-login after password set.
    let tok = match us.create_session_token(&uid, 30 * 24 * 3600) {
        Ok(t) => t,
        Err(_) => return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error"),
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({ "ok": true, "token": tok })),
    )
        .into_response()
}

async fn auth_request_password_link(State(st): State<AppState>, body: Bytes) -> Response {
    let req: RequestLinkRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return auth_error(StatusCode::BAD_REQUEST, "invalid json"),
    };
    let email = req.email.trim().to_string();
    if email.is_empty() {
        return auth_error(StatusCode::BAD_REQUEST, "missing email");
    }
    if !db::looks_like_email(&email) {
        return auth_error(StatusCode::BAD_REQUEST, "invalid email");
    }

    let mgr = db::Manager::new(st.data_dir.clone());
    let us = match mgr.users() {
        Ok(us) => us,
        Err(e) => {
            warn!(err=%e, "auth_request_password_link: db open failed");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    // Eligible if:
    // - existing active user with a password (reset), or
    // - whitelisted email (first-time setup).
    let mut eligible = false;
    match us.user_auth_row_by_email(&email) {
        Ok(Some((_id, status, ph))) => {
            if status == "active" && !ph.trim().is_empty() {
                eligible = true;
            }
        }
        Ok(None) => {}
        Err(e) => {
            warn!(err=%e, "auth_request_password_link: user lookup failed");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    }
    if !eligible {
        let whitelisted = match mgr.whitelist_has(&email) {
            Ok(v) => v,
            Err(e) => {
                warn!(err=%e, "auth_request_password_link: whitelist read failed");
                return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
            }
        };
        if whitelisted {
            eligible = true;
        }
    }

    // Always return OK to avoid leaking whether an email is eligible.
    if !eligible {
        return (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response();
    }

    // TTL: 1 hour.
    let tok = match us.create_magic_token(&email, 3600) {
        Ok(t) => t,
        Err(db::DbError::InvalidEmail) => {
            return auth_error(StatusCode::BAD_REQUEST, "invalid email")
        }
        Err(db::DbError::Unauthorized) => {
            // Disabled accounts: behave like "not eligible".
            return (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response();
        }
        Err(e) => {
            warn!(err=%e, "auth_request_password_link: db error");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    // Build an absolute URL so links in email work. If no public base is set,
    // fall back to localhost for dev.
    let settings = config::load_settings(&config::settings_path(&st.data_dir))
        .unwrap_or_else(|_| config::default_settings());
    let public_base = if settings.http_public_base().is_empty() {
        "http://localhost:8080".to_string()
    } else {
        settings.http_public_base()
    };
    let url = format!("{}/app/#setup={}", public_base.trim_end_matches('/'), tok);

    // Fire-and-forget email if Mailjet is configured.
    {
        let mail = settings.mailjet.clone();
        if !mail.api_key.trim().is_empty()
            && !mail.api_secret.trim().is_empty()
            && !mail.from_email.trim().is_empty()
        {
            let email_to = email.clone();
            let subj = "Your Saelora login link";
            let text = format!(
                "Use this link to set your password and sign in:\n{}\nIf you didn't request this, you can ignore it.",
                url
            );
            let html = format!(
                "<p>Use this link to set your password and sign in:</p><p><a href=\"{0}\">{0}</a></p><p>If you didn't request this, you can ignore it.</p>",
                url
            );
            tokio::spawn(async move {
                match email::send_mailjet(&mail, &email_to, subj, &text, &html).await {
                    Ok(_) => info!("mailjet: sent magic link to {}", email_to),
                    Err(e) => warn!(err=?e, "mailjet send failed"),
                }
            });
        } else if public_base.starts_with("http://localhost")
            || public_base.starts_with("http://127.0.0.1")
        {
            // Dev escape hatch when email isn't configured.
            info!(email=%email, url=%url, "magic link (mailjet not configured)");
        } else {
            info!(email=%email, "magic link generated");
        }
    }

    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

#[derive(Debug, Clone, serde::Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

async fn auth_register(State(st): State<AppState>, body: Bytes) -> Response {
    let req: LoginRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return auth_error(StatusCode::BAD_REQUEST, "invalid json"),
    };
    let email = req.email.trim().to_string();
    if email.is_empty() || req.password.trim().is_empty() {
        return auth_error(StatusCode::BAD_REQUEST, "missing email or password");
    }
    if !db::looks_like_email(&email) {
        return auth_error(StatusCode::BAD_REQUEST, "invalid email");
    }
    if req.password.trim().len() < 8 {
        return auth_error(StatusCode::BAD_REQUEST, "password too short");
    }

    let mgr = db::Manager::new(st.data_dir.clone());
    let us = match mgr.users() {
        Ok(us) => us,
        Err(e) => {
            warn!(err=%e, "auth_register: db open failed");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    // Check if user exists.
    match us.has_user(&email) {
        Ok(true) => return auth_error(StatusCode::CONFLICT, "account already exists"),
        Ok(false) => {}
        Err(e) => {
            warn!(err=%e, "auth_register: has_user failed");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    }

    let whitelisted = match mgr.whitelist_has(&email) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, "auth_register: whitelist read failed");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };
    let pending = match mgr.waitlist_has(&email) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, "auth_register: waitlist read failed");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };
    if !whitelisted {
        if pending {
            return auth_error(StatusCode::FORBIDDEN, "you are on the waitlist");
        } else {
            return auth_error(StatusCode::FORBIDDEN, "invite required");
        }
    }

    let ph = match db::hash_password(req.password.trim()) {
        Ok(h) => h,
        Err(_) => return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error"),
    };

    // Create user active.
    let user_id = match us.create_user(&email, &ph, "active") {
        Ok(id) => id,
        Err(db::DbError::UserExists) => {
            return auth_error(StatusCode::CONFLICT, "account already exists")
        }
        Err(db::DbError::InvalidEmail) => {
            return auth_error(StatusCode::BAD_REQUEST, "invalid email")
        }
        Err(e) => {
            warn!(err=%e, "auth_register: create user failed");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };
    // Remove from whitelist.
    let _ = mgr.whitelist_remove(&email);
    let _ = mgr.waitlist_remove(&email);

    let tok = match us.create_session_token(&user_id, 30 * 24 * 3600) {
        Ok(t) => t,
        Err(_) => return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error"),
    };

    (StatusCode::OK, Json(serde_json::json!({ "token": tok }))).into_response()
}

async fn auth_login(State(st): State<AppState>, body: Bytes) -> Response {
    let req: LoginRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return auth_error(StatusCode::BAD_REQUEST, "invalid json"),
    };

    let mgr = db::Manager::new(st.data_dir.clone());
    let us = match mgr.users() {
        Ok(us) => us,
        Err(e) => {
            warn!(err=%e, "auth_login: db open failed");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    let u = match us.verify_login(&req.email, &req.password) {
        Ok(u) => u,
        Err(db::DbError::Unauthorized) => {
            return auth_error(StatusCode::UNAUTHORIZED, "invalid credentials")
        }
        Err(e) => {
            warn!(err=%e, "auth_login: verify error");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    let tok = match us.create_session_token(&u.id, 30 * 24 * 3600) {
        Ok(t) => t,
        Err(e) => {
            warn!(err=%e, "auth_login: session create failed");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    (StatusCode::OK, Json(serde_json::json!({ "token": tok }))).into_response()
}

async fn auth_logout(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(tok) = bearer_token(&headers) else {
        return auth_error(StatusCode::UNAUTHORIZED, "missing token");
    };
    let mgr = db::Manager::new(st.data_dir.clone());
    let us = match mgr.users() {
        Ok(us) => us,
        Err(_) => return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error"),
    };
    let _ = us.revoke_session(&tok);
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

async fn auth_me(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let u = match authed_user(&st, &headers) {
        Ok(u) => u,
        Err(code) => return auth_error(code, "unauthorized"),
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({ "email": u.email, "status": u.status })),
    )
        .into_response()
}

async fn chat_completions(State(st): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    // Require auth for chat.
    let _u = match authed_user(&st, &headers) {
        Ok(u) => u,
        Err(code) => {
            return openai_error(
                code,
                "unauthorized",
                "invalid_request_error",
                "unauthorized",
            )
        }
    };

    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        != "application/json"
    {
        // Keep behavior predictable for the browser client.
    }

    let req: ChatCompletionsRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return openai_error(
                StatusCode::BAD_REQUEST,
                "invalid json",
                "invalid_request_error",
                "",
            )
        }
    };
    if req.messages.is_empty() {
        return openai_error(
            StatusCode::BAD_REQUEST,
            "missing messages",
            "invalid_request_error",
            "",
        );
    }

    let messages = filter_client_messages(&req.messages);
    if messages.is_empty() {
        return openai_error(
            StatusCode::BAD_REQUEST,
            "no usable messages",
            "invalid_request_error",
            "",
        );
    }

    let (cfg, client, backend_model) = match client_from_disk(&st.data_dir) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, "chat: server not configured");
            return openai_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "server not configured",
                "server_error",
                "",
            );
        }
    };

    let system_prompt = {
        let s = cfg.chat.system_prompt.trim();
        if s.is_empty() {
            config::DEFAULT_SYSTEM_PROMPT.trim().to_string()
        } else {
            s.to_string()
        }
    };

    let mut or_req = openrouter::ChatCompletionRequest {
        model: backend_model.clone(),
        messages: Vec::with_capacity(messages.len() + 1),
        // Do not expose model controls over the public API (keep chat surface minimal and stable).
        temperature: None,
        max_tokens: None,
        stream: req.stream,
    };
    or_req.messages.push(openrouter::Message {
        role: "system".to_string(),
        content: system_prompt,
    });
    or_req.messages.extend(messages.clone());

    let public_id = format!("saelora-{}", Utc::now().format("%Y%m%dT%H%M%S%.3fZ"));
    let public_created = Utc::now().timestamp();

    info!(
        stream = req.stream,
        backend_model = backend_model,
        client_model = req.model.trim(),
        msgs_in = req.messages.len(),
        msgs_used = messages.len(),
        roles = %role_counts(&messages),
        chars = total_chars(&messages),
        "chat: request"
    );

    // Stats: treat each request as one user turn if it ends with a user message.
    let user_delta: i64 = if messages.last().map(|m| m.role == "user").unwrap_or(false) {
        1
    } else {
        0
    };
    if user_delta > 0 {
        let mgr = db::Manager::new(st.data_dir.clone());
        if let Ok(us) = mgr.users() {
            let _ = us.incr_message_counts(user_delta, 0);
        }
    }

    if req.stream {
        return stream_chat(st, client, or_req, public_id, public_created).await;
    }

    let resp = match client.create_chat_completion(&or_req).await {
        Ok(r) => r,
        Err(e) => {
            log_backend_err("chat", &e);
            maybe_log_key_info(&st, &client, &e).await;
            return backend_error(&e);
        }
    };

    // Only return the first choice for now (parity with Go).
    let choice = resp
        .choices
        .first()
        .cloned()
        .unwrap_or(openrouter::ChatChoice {
            index: 0,
            message: openrouter::Message {
                role: "assistant".to_string(),
                content: String::new(),
            },
            finish_reason: None,
        });

    let out = serde_json::json!({
        "id": public_id,
        "object": "chat.completion",
        "created": public_created,
        "model": "saelora",
        "choices": [{
            "index": 0,
            "message": choice.message,
            "finish_reason": choice.finish_reason,
        }],
        "usage": resp.usage,
    });

    // Increment Saelora count on successful response.
    let mgr = db::Manager::new(st.data_dir.clone());
    if let Ok(us) = mgr.users() {
        let _ = us.incr_message_counts(0, 1);
    }

    (StatusCode::OK, Json(out)).into_response()
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let h = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let t = h.trim();
    let pfx = "bearer ";
    if t.len() < pfx.len() {
        return None;
    }
    if t[..pfx.len()].eq_ignore_ascii_case(pfx) {
        Some(t[pfx.len()..].trim().to_string())
    } else {
        None
    }
}

fn authed_user(st: &AppState, headers: &HeaderMap) -> Result<db::UserRecord, StatusCode> {
    let Some(tok) = bearer_token(headers) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let mgr = db::Manager::new(st.data_dir.clone());
    let us = mgr.users().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match us.auth_user_from_token(&tok) {
        Ok(u) => Ok(u),
        Err(db::DbError::Unauthorized) => Err(StatusCode::UNAUTHORIZED),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

fn filter_client_messages(in_msgs: &[openrouter::Message]) -> Vec<openrouter::Message> {
    let mut out = Vec::with_capacity(in_msgs.len());
    for m in in_msgs {
        let role = m.role.trim();
        if role != "user" && role != "assistant" {
            continue;
        }
        if m.content.trim().is_empty() {
            continue;
        }
        out.push(openrouter::Message {
            role: role.to_string(),
            content: m.content.clone(),
        });
    }
    out
}

fn client_from_disk(
    data_dir: &Path,
) -> anyhow::Result<(config::Settings, openrouter::Client, String)> {
    let path = config::settings_path(data_dir);
    let mut cfg: config::Settings = match std::fs::read(&path) {
        Ok(b) => serde_json::from_slice(&b)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Create a default config on first run so the server can serve immediately.
            let s = config::default_settings();
            let _ = config::save_settings(&path, &s);
            s
        }
        Err(e) => return Err(anyhow::anyhow!(e)),
    };
    if cfg.agents.is_empty() {
        let legacy = config::Agent {
            name: "default".to_string(),
            provider: config::Provider::OpenRouter,
            model: cfg.openrouter.model.clone(),
            base_url: cfg.openrouter.base_url.clone(),
            api_key: cfg.openrouter.api_key.clone(),
            http_referer: cfg.openrouter.http_referer.clone(),
            x_title: cfg.openrouter.x_title.clone(),
        };
        cfg.agents.push(legacy.clone());
        if cfg.tasks.chat_agent.is_empty() {
            cfg.tasks.chat_agent = legacy.name.clone();
        }
        if cfg.tasks.summary_agent.is_empty() {
            cfg.tasks.summary_agent = legacy.name.clone();
        }
    }
    if cfg.chat.system_prompt.trim().is_empty() {
        cfg.chat.system_prompt = config::DEFAULT_SYSTEM_PROMPT.trim().to_string();
    }
    let agent = cfg
        .resolve_agent("chat")
        .ok_or_else(|| anyhow::anyhow!("no agents configured"))?;
    if !cfg.agents.iter().any(|a| a.name == cfg.tasks.chat_agent) {
        tracing::warn!(
            configured = %cfg.tasks.chat_agent,
            fallback = %agent.name,
            "chat agent binding not found; using fallback"
        );
    }
    let base_url = agent.openai_base_url();
    tracing::info!(
        agent = %agent.name,
        provider = ?agent.provider,
        base_url = %base_url,
        model = %agent.model,
        "chat backend selected"
    );

    let client = openrouter::Client::new(openrouter::Config {
        api_key: agent.api_key.clone(),
        base_url,
        http_referer: if matches!(agent.provider, config::Provider::OpenRouter) {
            agent.http_referer.clone()
        } else {
            String::new()
        },
        x_title: if matches!(agent.provider, config::Provider::OpenRouter) {
            agent.x_title.clone()
        } else {
            String::new()
        },
    })?;

    let model = if agent.model.trim().is_empty() {
        "openai/gpt-4o-mini".to_string()
    } else {
        agent.model.trim().to_string()
    };
    Ok((cfg, client, model))
}

async fn stream_chat(
    st: AppState,
    client: openrouter::Client,
    req: openrouter::ChatCompletionRequest,
    public_id: String,
    public_created: i64,
) -> Response {
    let (status, headers, upstream) = match client.create_chat_completion_stream(&req).await {
        Ok(v) => v,
        Err(e) => {
            log_backend_err("chat_stream", &e);
            maybe_log_key_info(&st, &client, &e).await;
            return backend_error(&e);
        }
    };

    info!(
        backend_status = status.as_u16(),
        content_type = %headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or(""),
        "chat: backend stream opened"
    );

    let stream = rewrite_sse_stream(upstream, public_id, public_created, st.data_dir.clone());
    let body = Body::from_stream(stream);
    let mut resp = Response::new(body);
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/event-stream"),
    );
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    resp.headers_mut().insert(
        header::CONNECTION,
        header::HeaderValue::from_static("keep-alive"),
    );
    resp
}

fn rewrite_sse_stream(
    upstream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    public_id: String,
    public_created: i64,
    data_dir: PathBuf,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    try_stream! {
        let mut buf: Vec<u8> = Vec::new();
        futures::pin_mut!(upstream);
        while let Some(chunk) = upstream.next().await {
            let chunk = chunk.map_err(std::io::Error::other)?;
            buf.extend_from_slice(&chunk);

            while let Some((event, consumed)) = next_sse_event(&buf) {
                buf.drain(..consumed);

                let data = extract_sse_data(&event);
                if data.trim().is_empty() {
                    continue;
                }
                if data.trim() == "[DONE]" {
                    // Stats: only count Saelora's message once the stream finishes successfully.
                    let mgr = db::Manager::new(data_dir.clone());
                    if let Ok(us) = mgr.users() {
                        let _ = us.incr_message_counts(0, 1);
                    }
                    yield Bytes::from_static(b"data: [DONE]\n\n");
                    return;
                }

                let rewritten = rewrite_chunk_json(&data, &public_id, public_created);
                let mut out = Vec::with_capacity(rewritten.len() + 10);
                out.extend_from_slice(b"data: ");
                out.extend_from_slice(rewritten.as_bytes());
                out.extend_from_slice(b"\n\n");
                yield Bytes::from(out);
            }
        }
    }
}

fn next_sse_event(buf: &[u8]) -> Option<(Vec<u8>, usize)> {
    // Find \n\n or \r\n\r\n. We accept either.
    if let Some(pos) = find_subsequence(buf, b"\n\n") {
        let event = buf[..pos].to_vec();
        return Some((event, pos + 2));
    }
    if let Some(pos) = find_subsequence(buf, b"\r\n\r\n") {
        let event = buf[..pos].to_vec();
        return Some((event, pos + 4));
    }
    None
}

fn find_subsequence(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn extract_sse_data(event: &[u8]) -> String {
    let s = String::from_utf8_lossy(event);
    let mut lines = Vec::<String>::new();
    for line in s.lines() {
        let t = line.trim_end_matches('\r').trim();
        if let Some(rest) = t.strip_prefix("data:") {
            lines.push(rest.trim().to_string());
        }
    }
    lines.join("\n")
}

fn rewrite_chunk_json(raw: &str, public_id: &str, public_created: i64) -> String {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return raw.to_string();
    };

    if let serde_json::Value::Object(ref mut m) = v {
        m.insert(
            "id".to_string(),
            serde_json::Value::String(public_id.to_string()),
        );
        m.insert(
            "created".to_string(),
            serde_json::Value::Number(public_created.into()),
        );
        m.insert(
            "model".to_string(),
            serde_json::Value::String("saelora".to_string()),
        );
        if let Some(serde_json::Value::Object(ref mut em)) = m.get_mut("error") {
            em.insert(
                "message".to_string(),
                serde_json::Value::String("backend error".to_string()),
            );
        }
    }
    serde_json::to_string(&v).unwrap_or_else(|_| raw.to_string())
}

fn backend_error(e: &openrouter::HttpError) -> Response {
    if let Some(st) = e.status {
        match st {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return openai_error(
                    StatusCode::BAD_GATEWAY,
                    "backend authentication failed",
                    "server_error",
                    "backend_auth",
                );
            }
            StatusCode::NOT_FOUND => {
                return openai_error(
                    StatusCode::BAD_GATEWAY,
                    "backend model not found",
                    "server_error",
                    "backend_model_not_found",
                );
            }
            StatusCode::TOO_MANY_REQUESTS => {
                return openai_error(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate limited, please retry shortly",
                    "rate_limit_error",
                    "rate_limit_exceeded",
                );
            }
            StatusCode::BAD_REQUEST => {
                return openai_error(
                    StatusCode::BAD_GATEWAY,
                    "backend rejected request",
                    "server_error",
                    "backend_bad_request",
                );
            }
            _ => {
                return openai_error(
                    StatusCode::BAD_GATEWAY,
                    "backend error",
                    "server_error",
                    "backend_error",
                );
            }
        }
    }
    openai_error(
        StatusCode::BAD_GATEWAY,
        "backend error",
        "server_error",
        "backend_error",
    )
}

fn log_backend_err(op: &str, e: &openrouter::HttpError) {
    let mut body = e.body.trim().to_string();
    if body.len() > 800 {
        body.truncate(800);
        body.push_str("...");
    }
    warn!(
        op = op,
        status = %e.status.map(|s| s.as_u16()).unwrap_or(0),
        typ = %e.r#type,
        code = %e.code,
        msg = %e.message,
        body = %body,
        transport = ?e.transport,
        "backend error"
    );
}

async fn maybe_log_key_info(st: &AppState, client: &openrouter::Client, e: &openrouter::HttpError) {
    let Some(status) = e.status else { return };
    if status != StatusCode::TOO_MANY_REQUESTS
        && status != StatusCode::UNAUTHORIZED
        && status != StatusCode::FORBIDDEN
    {
        return;
    }
    let mut last = st.last_key_info_log.lock().await;
    if last.elapsed() < Duration::from_secs(30) {
        return;
    }
    *last = Instant::now();
    drop(last);

    let info = match client.get_key_info().await {
        Ok(i) => i,
        Err(err) => {
            warn!(err=%err, "key_info: backend error");
            return;
        }
    };

    let mut rl = String::new();
    if let Some(r) = info.rate_limit {
        if r.requests > 0 && r.interval > 0 {
            rl = format!(" rate_limit={}/{}s", r.requests, r.interval);
        }
    }
    info!(
        free_tier = info.is_free_tier,
        usage = info.usage,
        limit = info.limit,
        remaining = info.limit_remaining,
        label = info.label,
        extra = rl,
        "key_info"
    );
}

fn role_counts(msgs: &[openrouter::Message]) -> String {
    let mut u = 0;
    let mut a = 0;
    for m in msgs {
        match m.role.trim() {
            "user" => u += 1,
            "assistant" => a += 1,
            _ => {}
        }
    }
    format!("u={u} a={a}")
}

fn total_chars(msgs: &[openrouter::Message]) -> usize {
    msgs.iter().map(|m| m.content.len()).sum()
}

async fn cors(State(st): State<AppState>, req: Request<Body>, next: Next) -> Response {
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_string();

    let allowed_origin = if origin.is_empty() {
        None
    } else if is_allowed_origin(&origin, &st.data_dir) {
        Some(origin)
    } else {
        None
    };

    // Preflight.
    if req.method() == Method::OPTIONS {
        let mut resp = StatusCode::NO_CONTENT.into_response();
        if let Some(o) = allowed_origin {
            let h = resp.headers_mut();
            if let Ok(v) = header::HeaderValue::from_str(&o) {
                h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
                h.insert(header::VARY, header::HeaderValue::from_static("Origin"));
                h.insert(
                    header::ACCESS_CONTROL_ALLOW_METHODS,
                    header::HeaderValue::from_static("GET,POST,OPTIONS"),
                );
                h.insert(
                    header::ACCESS_CONTROL_ALLOW_HEADERS,
                    header::HeaderValue::from_static("Content-Type, Authorization"),
                );
                h.insert(
                    header::ACCESS_CONTROL_MAX_AGE,
                    header::HeaderValue::from_static("600"),
                );
            }
        }
        return resp;
    }

    let mut resp = next.run(req).await;
    if let Some(o) = allowed_origin {
        if let Ok(v) = header::HeaderValue::from_str(&o) {
            let h = resp.headers_mut();
            h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
            h.insert(header::VARY, header::HeaderValue::from_static("Origin"));
            h.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                header::HeaderValue::from_static("GET,POST,OPTIONS"),
            );
            h.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                header::HeaderValue::from_static("Content-Type, Authorization"),
            );
        }
    }
    resp
}

fn is_allowed_origin(origin: &str, data_dir: &Path) -> bool {
    let Some(o) = normalize_origin(origin) else {
        return false;
    };
    if is_local_dev_origin(&o) {
        return true;
    }

    let settings = config::load_settings(&config::settings_path(data_dir))
        .unwrap_or_else(|_| config::default_settings());
    let pub_base = settings.http_public_base();
    if pub_base.is_empty() {
        return false;
    }
    let Some(allowed) = normalize_origin(&pub_base) else {
        return false;
    };
    o == allowed
}

fn normalize_origin(s: &str) -> Option<String> {
    let uri: Uri = s.parse().ok()?;
    let scheme = uri.scheme_str()?;
    let auth = uri.authority()?;
    Some(format!("{}://{}", scheme, auth.as_str()))
}

fn is_local_dev_origin(origin: &str) -> bool {
    let uri: Uri = match origin.parse() {
        Ok(u) => u,
        Err(_) => return false,
    };
    if uri.scheme_str() != Some("http") {
        return false;
    }
    let Some(auth) = uri.authority() else {
        return false;
    };
    matches!(auth.host(), "localhost" | "127.0.0.1" | "::1")
}

async fn request_logger(req: Request<Body>, next: Next) -> Response {
    let rid = Uuid::new_v4().simple().to_string()[..16].to_string();
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    info!(rid=%rid, %method, %path, "http: ->");
    let start = Instant::now();
    let resp = next.run(req).await;
    info!(
        rid=%rid,
        %method,
        %path,
        status = resp.status().as_u16(),
        dur = ?start.elapsed(),
        "http: <-"
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt as _;

    fn test_state(data_dir: &Path) -> AppState {
        AppState {
            data_dir: data_dir.to_path_buf(),
            last_key_info_log: Arc::new(Mutex::new(Instant::now() - Duration::from_secs(3600))),
        }
    }

    async fn resp_json(resp: Response) -> serde_json::Value {
        let bytes = resp
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("json body")
    }

    fn bearer_headers(tok: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {}", tok)).unwrap(),
        );
        h
    }

    async fn register_whitelisted(
        st: AppState,
        email: &str,
        password: &str,
    ) -> (db::Manager, String) {
        let mgr = db::Manager::new(st.data_dir.clone());
        mgr.whitelist_add(email).unwrap();
        let body =
            Bytes::from(serde_json::json!({"email": email, "password": password}).to_string());
        let resp = auth_register(State(st), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = resp_json(resp).await;
        let tok = v.get("token").and_then(|t| t.as_str()).unwrap().to_string();
        assert!(!tok.trim().is_empty());
        (mgr, tok)
    }

    #[tokio::test]
    async fn request_password_link_ok_does_not_leak_url_or_create_user() {
        let td = tempfile::tempdir().unwrap();
        let data_dir = td.path().to_path_buf();
        let st = test_state(&data_dir);

        let mgr = db::Manager::new(data_dir.clone());
        let us = mgr.users().unwrap();
        assert_eq!(us.count_users().unwrap(), 0);

        let body = Bytes::from(r#"{"email":"someone@example.com"}"#);
        let resp = auth_request_password_link(State(st.clone()), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = resp_json(resp).await;
        assert_eq!(v.get("ok").and_then(|b| b.as_bool()), Some(true));
        assert!(v.get("url").is_none());

        let us2 = mgr.users().unwrap();
        assert_eq!(us2.count_users().unwrap(), 0);
        assert_eq!(us2.count_magic_tokens().unwrap(), 0);
    }

    #[tokio::test]
    async fn invite_accepts_json_and_form_and_dedupes() {
        let td = tempfile::tempdir().unwrap();
        let data_dir = td.path().to_path_buf();
        let st = test_state(&data_dir);
        let mgr = db::Manager::new(data_dir.clone());

        let mut h1 = HeaderMap::new();
        h1.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        let resp1 = invite(
            State(st.clone()),
            h1,
            Bytes::from(r#"{"email":"Test@Example.com"}"#),
        )
        .await;
        assert_eq!(resp1.status(), StatusCode::OK);
        assert_eq!(
            resp_json(resp1).await.get("ok").and_then(|b| b.as_bool()),
            Some(true)
        );

        let mut h2 = HeaderMap::new();
        h2.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        let resp2 = invite(
            State(st.clone()),
            h2,
            Bytes::from("email=test%40example.com"),
        )
        .await;
        assert_eq!(resp2.status(), StatusCode::OK);

        assert_eq!(mgr.waitlist_count().unwrap(), 1);
        assert!(mgr.waitlist_has("test@example.com").unwrap());
    }

    #[tokio::test]
    async fn invite_rejects_invalid_email() {
        let td = tempfile::tempdir().unwrap();
        let data_dir = td.path().to_path_buf();
        let st = test_state(&data_dir);

        let mut h = HeaderMap::new();
        h.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        let resp = invite(State(st.clone()), h, Bytes::from(r#"{"email":"nope"}"#)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let v = resp_json(resp).await;
        assert_eq!(
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str()),
            Some("invalid email")
        );
    }

    #[tokio::test]
    async fn register_requires_whitelist_and_reports_pending_vs_not_found() {
        let td = tempfile::tempdir().unwrap();
        let data_dir = td.path().to_path_buf();
        let st = test_state(&data_dir);
        let mgr = db::Manager::new(data_dir.clone());

        let email = "r@example.com";
        let body =
            Bytes::from(serde_json::json!({"email": email, "password": "password123"}).to_string());
        let resp = auth_register(State(st.clone()), body).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let v = resp_json(resp).await;
        assert_eq!(
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str()),
            Some("invite required")
        );

        mgr.waitlist_add(email).unwrap();
        let body2 =
            Bytes::from(serde_json::json!({"email": email, "password": "password123"}).to_string());
        let resp2 = auth_register(State(st.clone()), body2).await;
        assert_eq!(resp2.status(), StatusCode::FORBIDDEN);
        let v2 = resp_json(resp2).await;
        assert_eq!(
            v2.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str()),
            Some("you are on the waitlist")
        );
    }

    #[tokio::test]
    async fn register_creates_user_removes_lists_and_me_works() {
        let td = tempfile::tempdir().unwrap();
        let data_dir = td.path().to_path_buf();
        let st = test_state(&data_dir);
        let mgr = db::Manager::new(data_dir.clone());

        let email = "Test@Example.com";
        mgr.waitlist_add(email).unwrap();
        mgr.whitelist_add(email).unwrap();

        let body = Bytes::from(
            serde_json::json!({"email": "test@example.com", "password": "password123"}).to_string(),
        );
        let resp = auth_register(State(st.clone()), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = resp_json(resp).await;
        let tok = v.get("token").and_then(|t| t.as_str()).unwrap().to_string();
        assert!(!tok.trim().is_empty());

        // Lists should be cleaned up.
        assert!(!mgr.waitlist_has("test@example.com").unwrap());
        assert!(!mgr.whitelist_has("test@example.com").unwrap());

        // /me should work with the returned token.
        let resp_me = auth_me(State(st.clone()), bearer_headers(&tok)).await;
        assert_eq!(resp_me.status(), StatusCode::OK);
        let me = resp_json(resp_me).await;
        assert_eq!(
            me.get("email").and_then(|m| m.as_str()),
            Some("test@example.com")
        );
        assert_eq!(me.get("status").and_then(|m| m.as_str()), Some("active"));
    }

    #[tokio::test]
    async fn register_conflicts_when_account_exists() {
        let td = tempfile::tempdir().unwrap();
        let data_dir = td.path().to_path_buf();
        let st = test_state(&data_dir);
        let email = "exists@example.com";

        let (_mgr, _tok) = register_whitelisted(st.clone(), email, "password123").await;

        // Second register should conflict even if re-whitelisted.
        let mgr2 = db::Manager::new(data_dir.clone());
        mgr2.whitelist_add(email).unwrap();
        let body =
            Bytes::from(serde_json::json!({"email": email, "password": "password123"}).to_string());
        let resp2 = auth_register(State(st.clone()), body).await;
        assert_eq!(resp2.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn login_and_logout_flow_is_consistent() {
        let td = tempfile::tempdir().unwrap();
        let data_dir = td.path().to_path_buf();
        let st = test_state(&data_dir);
        let email = "login@example.com";

        let (_mgr, _tok0) = register_whitelisted(st.clone(), email, "password123").await;

        // Wrong password.
        let body_wrong =
            Bytes::from(serde_json::json!({"email": email, "password": "wrong"}).to_string());
        let resp_wrong = auth_login(State(st.clone()), body_wrong).await;
        assert_eq!(resp_wrong.status(), StatusCode::UNAUTHORIZED);

        // Unknown email.
        let body_unknown = Bytes::from(
            serde_json::json!({"email": "unknown@example.com", "password": "password123"})
                .to_string(),
        );
        let resp_unknown = auth_login(State(st.clone()), body_unknown).await;
        assert_eq!(resp_unknown.status(), StatusCode::UNAUTHORIZED);

        // Correct login.
        let body_ok =
            Bytes::from(serde_json::json!({"email": email, "password": "password123"}).to_string());
        let resp_ok = auth_login(State(st.clone()), body_ok).await;
        assert_eq!(resp_ok.status(), StatusCode::OK);
        let v = resp_json(resp_ok).await;
        let tok = v.get("token").and_then(|t| t.as_str()).unwrap().to_string();

        // /me works.
        assert_eq!(
            auth_me(State(st.clone()), bearer_headers(&tok))
                .await
                .status(),
            StatusCode::OK
        );

        // Logout revokes token.
        let resp_lo = auth_logout(State(st.clone()), bearer_headers(&tok)).await;
        assert_eq!(resp_lo.status(), StatusCode::OK);
        assert_eq!(
            auth_me(State(st.clone()), bearer_headers(&tok))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn request_password_link_validates_email_and_does_not_issue_for_disabled_or_no_password()
    {
        let td = tempfile::tempdir().unwrap();
        let data_dir = td.path().to_path_buf();
        let st = test_state(&data_dir);

        // Invalid email is rejected (client error).
        let resp1 =
            auth_request_password_link(State(st.clone()), Bytes::from(r#"{"email":""}"#)).await;
        assert_eq!(resp1.status(), StatusCode::BAD_REQUEST);

        let mgr = db::Manager::new(data_dir.clone());
        let us = mgr.users().unwrap();

        // Active user with an empty password hash is not eligible.
        let uid1 = us.create_user("nopw@example.com", "", "active").unwrap();
        assert!(!uid1.is_empty());

        let resp2 = auth_request_password_link(
            State(st.clone()),
            Bytes::from(r#"{"email":"nopw@example.com"}"#),
        )
        .await;
        assert_eq!(resp2.status(), StatusCode::OK);
        assert_eq!(mgr.users().unwrap().count_magic_tokens().unwrap(), 0);

        // Disabled user is not eligible.
        let _uid2 = us
            .create_user("disabled@example.com", "x", "disabled")
            .unwrap();
        let resp3 = auth_request_password_link(
            State(st.clone()),
            Bytes::from(r#"{"email":"disabled@example.com"}"#),
        )
        .await;
        assert_eq!(resp3.status(), StatusCode::OK);
        assert_eq!(mgr.users().unwrap().count_magic_tokens().unwrap(), 0);
    }

    #[tokio::test]
    async fn setup_for_existing_user_resets_password_and_consumes_token() {
        let td = tempfile::tempdir().unwrap();
        let data_dir = td.path().to_path_buf();
        let st = test_state(&data_dir);
        let email = "reset@example.com";

        let (mgr, _tok0) = register_whitelisted(st.clone(), email, "password123").await;
        let us = mgr.users().unwrap();

        let tok = us.create_magic_token(email, 3600).unwrap();
        let resp = auth_setup(
            State(st.clone()),
            Bytes::from(
                serde_json::json!({"token": tok.clone(), "password":"newpassword123"}).to_string(),
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = resp_json(resp).await;
        assert_eq!(v.get("ok").and_then(|b| b.as_bool()), Some(true));
        assert!(v.get("token").and_then(|t| t.as_str()).unwrap().len() > 10);

        // Token is one-time.
        let resp2 = auth_setup(
            State(st.clone()),
            Bytes::from(serde_json::json!({"token": tok, "password":"newpassword123"}).to_string()),
        )
        .await;
        assert_eq!(resp2.status(), StatusCode::BAD_REQUEST);

        // Old password no longer works; new password works.
        let resp_old = auth_login(
            State(st.clone()),
            Bytes::from(serde_json::json!({"email": email, "password":"password123"}).to_string()),
        )
        .await;
        assert_eq!(resp_old.status(), StatusCode::UNAUTHORIZED);

        let resp_new = auth_login(
            State(st.clone()),
            Bytes::from(
                serde_json::json!({"email": email, "password":"newpassword123"}).to_string(),
            ),
        )
        .await;
        assert_eq!(resp_new.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn chat_requires_auth_and_errors_before_touching_backend() {
        let td = tempfile::tempdir().unwrap();
        let data_dir = td.path().to_path_buf();
        let st = test_state(&data_dir);

        let resp = chat_completions(
            State(st.clone()),
            HeaderMap::new(),
            Bytes::from(r#"{"messages":[{"role":"user","content":"hi"}]}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let v = resp_json(resp).await;
        assert_eq!(
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str()),
            Some("unauthorized")
        );

        // Authorized: invalid json should 400 without hitting the upstream.
        let (_mgr, tok) = register_whitelisted(st.clone(), "chat@example.com", "password123").await;
        let resp2 =
            chat_completions(State(st.clone()), bearer_headers(&tok), Bytes::from("{")).await;
        assert_eq!(resp2.status(), StatusCode::BAD_REQUEST);
        let v2 = resp_json(resp2).await;
        assert_eq!(
            v2.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str()),
            Some("invalid json")
        );

        // Authorized: messages filtered away should reject early.
        let body3 = Bytes::from(
            serde_json::json!({
                "model": "anything",
                "messages": [{"role":"system","content":"x"},{"role":"user","content":"   "}]
            })
            .to_string(),
        );
        let resp3 = chat_completions(State(st.clone()), bearer_headers(&tok), body3).await;
        assert_eq!(resp3.status(), StatusCode::BAD_REQUEST);
        let v3 = resp_json(resp3).await;
        assert_eq!(
            v3.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str()),
            Some("no usable messages")
        );
    }

    #[tokio::test]
    async fn request_password_link_for_whitelisted_email_creates_token_but_no_user() {
        let td = tempfile::tempdir().unwrap();
        let data_dir = td.path().to_path_buf();
        let st = test_state(&data_dir);

        let mgr = db::Manager::new(data_dir.clone());
        mgr.whitelist_add("whitelisted@example.com").unwrap();

        let body = Bytes::from(r#"{"email":"whitelisted@example.com"}"#);
        let resp = auth_request_password_link(State(st.clone()), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = resp_json(resp).await;
        assert_eq!(v.get("ok").and_then(|b| b.as_bool()), Some(true));
        assert!(v.get("url").is_none());

        let us = mgr.users().unwrap();
        assert_eq!(us.count_users().unwrap(), 0);
        assert_eq!(us.count_magic_tokens().unwrap(), 1);
    }

    #[tokio::test]
    async fn setup_requires_whitelist_for_new_accounts_and_consumes_token_only_on_success() {
        let td = tempfile::tempdir().unwrap();
        let data_dir = td.path().to_path_buf();
        let st = test_state(&data_dir);

        let mgr = db::Manager::new(data_dir.clone());
        let us = mgr.users().unwrap();
        let tok = us.create_magic_token("newuser@example.com", 3600).unwrap();

        // Not whitelisted -> should fail and not consume the token.
        let body1 = Bytes::from(
            serde_json::json!({"token": tok.clone(), "password": "password123"}).to_string(),
        );
        let resp1 = auth_setup(State(st.clone()), body1).await;
        assert_eq!(resp1.status(), StatusCode::BAD_REQUEST);
        assert_eq!(mgr.users().unwrap().count_users().unwrap(), 0);
        assert_eq!(mgr.users().unwrap().count_magic_tokens().unwrap(), 1);

        // Whitelist -> should succeed using the same token.
        mgr.whitelist_add("newuser@example.com").unwrap();
        let body2 =
            Bytes::from(serde_json::json!({"token": tok, "password": "password123"}).to_string());
        let resp2 = auth_setup(State(st.clone()), body2).await;
        assert_eq!(resp2.status(), StatusCode::OK);
        assert_eq!(mgr.users().unwrap().count_users().unwrap(), 1);
        assert!(!mgr.whitelist_has("newuser@example.com").unwrap());
    }

    #[tokio::test]
    async fn cors_allows_localhost_and_public_base_only() {
        let td = tempfile::tempdir().unwrap();
        let data_dir = td.path().to_path_buf();

        let mut s = config::default_settings();
        s.public_base = "saelora.ai".to_string();
        config::save_settings(&config::settings_path(&data_dir), &s).unwrap();

        assert!(is_allowed_origin("http://localhost:5173", &data_dir));
        assert!(is_allowed_origin("http://127.0.0.1:5173", &data_dir));
        assert!(is_allowed_origin("https://saelora.ai", &data_dir));
        assert!(!is_allowed_origin("https://evil.example", &data_dir));
    }
}
