use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use tracing::warn;

use crate::{agents, config, db};

use super::{auth, errors, AppState};

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct SaveConfigRequest {
    #[serde(default)]
    settings: Option<config::Settings>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct InviteActionRequest {
    #[serde(default)]
    email: String,
    #[serde(default)]
    list: String, // "pending" | "whitelist"
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct UserStatusRequest {
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    status: String,
}

fn admin_guard(
    st: &AppState,
    headers: &HeaderMap,
) -> Result<(db::Manager, db::UserRecord), StatusCode> {
    let u = auth::authed_user(st, headers)?;
    let mgr = db::Manager::new(st.data_dir.clone());
    let us = match mgr.users() {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, "admin: users db open failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let is_admin = match us.is_admin_user(&u.id) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, user_id=%u.id, "admin: admin check failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    if !is_admin {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok((mgr, u))
}

fn admin_guard_response(code: StatusCode) -> Response {
    if code == StatusCode::FORBIDDEN {
        return errors::auth_error(code, "forbidden");
    }
    if code == StatusCode::UNAUTHORIZED {
        return errors::auth_error(code, "unauthorized");
    }
    errors::auth_error(code, "server error")
}

pub(super) async fn overview(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let (mgr, u) = match admin_guard(&st, &headers) {
        Ok(v) => v,
        Err(code) => return admin_guard_response(code),
    };
    let pending = mgr.waitlist_count().unwrap_or(0);
    let whitelist = mgr.whitelist_count().unwrap_or(0);
    let mut users_n = 0usize;
    let mut msgs_sent = 0u64;
    let mut msgs_recv = 0u64;
    if let Ok(us) = mgr.users() {
        users_n = us.count_users().unwrap_or(0);
        let (s, r) = us.get_message_counts().unwrap_or((0, 0));
        msgs_sent = s;
        msgs_recv = r;
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "admin_email": u.email,
            "pending_invites": pending,
            "whitelisted": whitelist,
            "users": users_n,
            "messages_sent": msgs_sent,
            "messages_received": msgs_recv,
        })),
    )
        .into_response()
}

pub(super) async fn get_config(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let _ = match admin_guard(&st, &headers) {
        Ok(v) => v,
        Err(code) => return admin_guard_response(code),
    };

    let path = config::settings_path(&st.data_dir);
    let mut settings = match config::load_settings(&path) {
        Ok(s) => s,
        Err(_) => config::default_settings(),
    };
    agents::normalize_settings(&mut settings);
    (
        StatusCode::OK,
        Json(serde_json::json!({ "settings": settings })),
    )
        .into_response()
}

pub(super) async fn save_config(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let _ = match admin_guard(&st, &headers) {
        Ok(v) => v,
        Err(code) => return admin_guard_response(code),
    };

    let req: SaveConfigRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return errors::auth_error(StatusCode::BAD_REQUEST, "invalid json"),
    };
    let mut settings = match req.settings {
        Some(s) => s,
        None => return errors::auth_error(StatusCode::BAD_REQUEST, "missing settings"),
    };
    agents::normalize_settings(&mut settings);
    let path = config::settings_path(&st.data_dir);
    if let Err(e) = config::save_settings(&path, &settings) {
        warn!(err=%e, "admin: save config failed");
        return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save settings");
    }
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

pub(super) async fn list_invites(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let (mgr, _) = match admin_guard(&st, &headers) {
        Ok(v) => v,
        Err(code) => return admin_guard_response(code),
    };
    let pending = match mgr.waitlist_list() {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, "admin: waitlist list failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to load invites");
        }
    };
    let whitelist = match mgr.whitelist_list() {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, "admin: whitelist list failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to load invites");
        }
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "pending": pending,
            "whitelist": whitelist
        })),
    )
        .into_response()
}

pub(super) async fn approve_invite(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (mgr, _) = match admin_guard(&st, &headers) {
        Ok(v) => v,
        Err(code) => return admin_guard_response(code),
    };
    let req: InviteActionRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return errors::auth_error(StatusCode::BAD_REQUEST, "invalid json"),
    };
    let email = req.email.trim();
    if !db::looks_like_email(email) {
        return errors::auth_error(StatusCode::BAD_REQUEST, "invalid email");
    }
    if let Err(e) = mgr.waitlist_remove(email) {
        warn!(err=%e, email=%email, "admin: waitlist remove failed");
    }
    if let Err(e) = mgr.whitelist_add(email) {
        warn!(err=%e, email=%email, "admin: whitelist add failed");
        return errors::auth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to approve invite",
        );
    }
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

pub(super) async fn remove_invite(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (mgr, _) = match admin_guard(&st, &headers) {
        Ok(v) => v,
        Err(code) => return admin_guard_response(code),
    };
    let req: InviteActionRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return errors::auth_error(StatusCode::BAD_REQUEST, "invalid json"),
    };
    let email = req.email.trim();
    if !db::looks_like_email(email) {
        return errors::auth_error(StatusCode::BAD_REQUEST, "invalid email");
    }
    let list = req.list.trim().to_ascii_lowercase();
    let res = if list == "whitelist" {
        mgr.whitelist_remove(email)
    } else {
        mgr.waitlist_remove(email)
    };
    if let Err(e) = res {
        warn!(err=%e, email=%email, list=%list, "admin: remove invite failed");
        return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to remove invite");
    }
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

pub(super) async fn list_users(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let (mgr, _) = match admin_guard(&st, &headers) {
        Ok(v) => v,
        Err(code) => return admin_guard_response(code),
    };
    let us = match mgr.users() {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, "admin: users open failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };
    match us.list_users() {
        Ok(users) => (StatusCode::OK, Json(serde_json::json!({ "users": users }))).into_response(),
        Err(e) => {
            warn!(err=%e, "admin: list users failed");
            errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to load users")
        }
    }
}

pub(super) async fn set_user_status(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (mgr, _) = match admin_guard(&st, &headers) {
        Ok(v) => v,
        Err(code) => return admin_guard_response(code),
    };
    let req: UserStatusRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return errors::auth_error(StatusCode::BAD_REQUEST, "invalid json"),
    };
    let id = req.user_id.trim();
    let status = req.status.trim();
    if id.is_empty() {
        return errors::auth_error(StatusCode::BAD_REQUEST, "missing user_id");
    }
    let us = match mgr.users() {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, "admin: users open failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error");
        }
    };
    match us.set_user_status(id, status) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
        Err(db::DbError::InvalidStatus) => {
            errors::auth_error(StatusCode::BAD_REQUEST, "invalid status")
        }
        Err(e) => {
            warn!(err=%e, user_id=%id, status=%status, "admin: set status failed");
            errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to update user")
        }
    }
}
