mod invite;
mod register;
mod session;
mod setup;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::Response;

use crate::db;

use super::AppState;

pub(super) const SESSION_TTL_SECS: i64 = 14 * 24 * 3600;

pub(super) async fn invite(st: State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    invite::invite(st, headers, body).await
}

pub(super) async fn auth_setup(st: State<AppState>, body: Bytes) -> Response {
    setup::auth_setup(st, body).await
}

pub(super) async fn auth_request_password_link(st: State<AppState>, body: Bytes) -> Response {
    setup::auth_request_password_link(st, body).await
}

pub(super) async fn auth_register(st: State<AppState>, body: Bytes) -> Response {
    register::auth_register(st, body).await
}

pub(super) async fn auth_login(st: State<AppState>, body: Bytes) -> Response {
    register::auth_login(st, body).await
}

pub(super) async fn auth_logout(st: State<AppState>, headers: HeaderMap) -> Response {
    session::auth_logout(st, headers).await
}

pub(super) async fn auth_logout_all(st: State<AppState>, headers: HeaderMap) -> Response {
    session::auth_logout_all(st, headers).await
}

pub(super) async fn auth_me(st: State<AppState>, headers: HeaderMap) -> Response {
    session::auth_me(st, headers).await
}

#[cfg(test)]
pub(super) fn bearer_token(headers: &HeaderMap) -> Option<String> {
    session::bearer_token(headers)
}

pub(super) fn authed_user(
    st: &AppState,
    headers: &HeaderMap,
) -> Result<db::UserRecord, StatusCode> {
    session::authed_user(st, headers)
}
