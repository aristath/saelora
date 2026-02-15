use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use tracing::warn;

use crate::db;

use super::super::AppState;

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
