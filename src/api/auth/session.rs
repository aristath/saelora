use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::db;

use super::super::errors;
use super::super::AppState;

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

pub(super) async fn auth_logout_all(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let u = match authed_user(&st, &headers) {
        Ok(u) => u,
        Err(code) => return errors::auth_error(code, "unauthorized"),
    };
    let mgr = db::Manager::new(st.data_dir.clone());
    let us = match mgr.users() {
        Ok(us) => us,
        Err(_) => return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "server error"),
    };
    let _ = us.revoke_all_sessions_for_user(&u.id);
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
