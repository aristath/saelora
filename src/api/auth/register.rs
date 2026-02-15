use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use tracing::warn;

use crate::db;

use super::super::errors;
use super::super::AppState;

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
