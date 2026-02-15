use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use tracing::{info, warn};

use crate::config;
use crate::db;
use crate::email;

use super::errors;
use super::AppState;

pub(super) async fn invite(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
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

#[derive(Debug, Clone, serde::Deserialize)]
struct SetupRequest {
    token: String,
    password: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RequestLinkRequest {
    email: String,
}

pub(super) async fn auth_setup(State(st): State<AppState>, body: Bytes) -> Response {
    let req: SetupRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return errors::auth_error(StatusCode::BAD_REQUEST, "invalid json"),
    };
    let token = req.token.trim().to_string();
    let password = req.password;
    if token.is_empty() {
        return errors::auth_error(StatusCode::BAD_REQUEST, "missing token");
    }
    if password.trim().len() < 8 {
        return errors::auth_error(StatusCode::BAD_REQUEST, "password too short");
    }

    let ph = match db::hash_password(&password) {
        Ok(h) => h,
        Err(_) => return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error"),
    };

    let mgr = db::Manager::new(st.data_dir.clone());
    let us = match mgr.users() {
        Ok(us) => us,
        Err(e) => {
            warn!(err=%e, "auth_setup: db open failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    // Resolve email for this token without consuming it.
    let email = match us.peek_magic_token_email(&token) {
        Ok(e) => e,
        Err(db::DbError::TokenInvalid) => {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid or expired token")
        }
        Err(e) => {
            warn!(err=%e, "auth_setup: token peek failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    // If this is a new account, require whitelist at setup time.
    let existed = match us.user_auth_row_by_email(&email) {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(e) => {
            warn!(err=%e, "auth_setup: user lookup failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };
    if !existed {
        let whitelisted = match mgr.whitelist_has(&email) {
            Ok(v) => v,
            Err(e) => {
                warn!(err=%e, "auth_setup: whitelist read failed");
                return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
            }
        };
        if !whitelisted {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid token");
        }
    }

    let uid = match us.consume_magic_token_set_password(&token, &ph, !existed) {
        Ok(uid) => uid,
        Err(db::DbError::TokenInvalid) => {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid or expired token")
        }
        Err(db::DbError::Unauthorized) => {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid token")
        }
        Err(e) => {
            warn!(err=%e, "auth_setup: db error");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
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
        Err(_) => return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error"),
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({ "ok": true, "token": tok })),
    )
        .into_response()
}

pub(super) async fn auth_request_password_link(
    State(st): State<AppState>,
    body: Bytes,
) -> Response {
    let req: RequestLinkRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return errors::auth_error(StatusCode::BAD_REQUEST, "invalid json"),
    };
    let email = req.email.trim().to_string();
    if email.is_empty() {
        return errors::auth_error(StatusCode::BAD_REQUEST, "missing email");
    }
    if !db::looks_like_email(&email) {
        return errors::auth_error(StatusCode::BAD_REQUEST, "invalid email");
    }

    let mgr = db::Manager::new(st.data_dir.clone());
    let us = match mgr.users() {
        Ok(us) => us,
        Err(e) => {
            warn!(err=%e, "auth_request_password_link: db open failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
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
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    }
    if !eligible {
        let whitelisted = match mgr.whitelist_has(&email) {
            Ok(v) => v,
            Err(e) => {
                warn!(err=%e, "auth_request_password_link: whitelist read failed");
                return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
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
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid email")
        }
        Err(db::DbError::Unauthorized) => {
            // Disabled accounts: behave like "not eligible".
            return (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response();
        }
        Err(e) => {
            warn!(err=%e, "auth_request_password_link: db error");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
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

pub(super) async fn auth_register(State(st): State<AppState>, body: Bytes) -> Response {
    let req: LoginRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return errors::auth_error(StatusCode::BAD_REQUEST, "invalid json"),
    };
    let email = req.email.trim().to_string();
    if email.is_empty() || req.password.trim().is_empty() {
        return errors::auth_error(StatusCode::BAD_REQUEST, "missing email or password");
    }
    if !db::looks_like_email(&email) {
        return errors::auth_error(StatusCode::BAD_REQUEST, "invalid email");
    }
    if req.password.trim().len() < 8 {
        return errors::auth_error(StatusCode::BAD_REQUEST, "password too short");
    }

    let mgr = db::Manager::new(st.data_dir.clone());
    let us = match mgr.users() {
        Ok(us) => us,
        Err(e) => {
            warn!(err=%e, "auth_register: db open failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    // Check if user exists.
    match us.has_user(&email) {
        Ok(true) => return errors::auth_error(StatusCode::CONFLICT, "account already exists"),
        Ok(false) => {}
        Err(e) => {
            warn!(err=%e, "auth_register: has_user failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    }

    let whitelisted = match mgr.whitelist_has(&email) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, "auth_register: whitelist read failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };
    let pending = match mgr.waitlist_has(&email) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, "auth_register: waitlist read failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };
    if !whitelisted {
        if pending {
            return errors::auth_error(StatusCode::FORBIDDEN, "you are on the waitlist");
        } else {
            return errors::auth_error(StatusCode::FORBIDDEN, "invite required");
        }
    }

    let ph = match db::hash_password(req.password.trim()) {
        Ok(h) => h,
        Err(_) => return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error"),
    };

    // Create user active.
    let user_id = match us.create_user(&email, &ph, "active") {
        Ok(id) => id,
        Err(db::DbError::UserExists) => {
            return errors::auth_error(StatusCode::CONFLICT, "account already exists")
        }
        Err(db::DbError::InvalidEmail) => {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid email")
        }
        Err(e) => {
            warn!(err=%e, "auth_register: create user failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };
    // Remove from whitelist.
    let _ = mgr.whitelist_remove(&email);
    let _ = mgr.waitlist_remove(&email);

    let tok = match us.create_session_token(&user_id, 30 * 24 * 3600) {
        Ok(t) => t,
        Err(_) => return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error"),
    };

    (StatusCode::OK, Json(serde_json::json!({ "token": tok }))).into_response()
}

pub(super) async fn auth_login(State(st): State<AppState>, body: Bytes) -> Response {
    let req: LoginRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return errors::auth_error(StatusCode::BAD_REQUEST, "invalid json"),
    };

    let mgr = db::Manager::new(st.data_dir.clone());
    let us = match mgr.users() {
        Ok(us) => us,
        Err(e) => {
            warn!(err=%e, "auth_login: db open failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    let u = match us.verify_login(&req.email, &req.password) {
        Ok(u) => u,
        Err(db::DbError::Unauthorized) => {
            return errors::auth_error(StatusCode::UNAUTHORIZED, "invalid credentials")
        }
        Err(e) => {
            warn!(err=%e, "auth_login: verify error");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    let tok = match us.create_session_token(&u.id, 30 * 24 * 3600) {
        Ok(t) => t,
        Err(e) => {
            warn!(err=%e, "auth_login: session create failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    (StatusCode::OK, Json(serde_json::json!({ "token": tok }))).into_response()
}

pub(super) async fn auth_logout(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(tok) = bearer_token(&headers) else {
        return errors::auth_error(StatusCode::UNAUTHORIZED, "missing token");
    };
    let mgr = db::Manager::new(st.data_dir.clone());
    let us = match mgr.users() {
        Ok(us) => us,
        Err(_) => return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error"),
    };
    let _ = us.revoke_session(&tok);
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

pub(super) async fn auth_me(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let u = match authed_user(&st, &headers) {
        Ok(u) => u,
        Err(code) => return errors::auth_error(code, "unauthorized"),
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({ "email": u.email, "status": u.status })),
    )
        .into_response()
}

pub(super) fn bearer_token(headers: &HeaderMap) -> Option<String> {
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

pub(super) fn authed_user(
    st: &AppState,
    headers: &HeaderMap,
) -> Result<db::UserRecord, StatusCode> {
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
