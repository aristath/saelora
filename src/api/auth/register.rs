use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use tracing::warn;

use crate::db;

use super::super::errors;
use super::super::AppState;
use super::SESSION_TTL_SECS;

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

    let _ = st;
    // Registration now requires email ownership proof via magic link.
    errors::auth_error(
        StatusCode::GONE,
        "direct registration is disabled; use \"Email me a login link\"",
    )
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

    let tok = match us.create_session_token(&u.id, SESSION_TTL_SECS) {
        Ok(t) => t,
        Err(e) => {
            warn!(err=%e, "auth_login: session create failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };

    (StatusCode::OK, Json(serde_json::json!({ "token": tok }))).into_response()
}
