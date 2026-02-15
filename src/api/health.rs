use axum::http::StatusCode;
use axum::response::IntoResponse;

pub(super) async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok\n")
}
