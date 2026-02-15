use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use tracing::{info, warn};

use crate::{config, db, email};

use super::super::errors;
use super::super::AppState;

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
