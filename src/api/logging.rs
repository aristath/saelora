use axum::body::Body;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use tracing::info;
use uuid::Uuid;

pub(super) async fn request_logger(req: Request<Body>, next: Next) -> Response {
    let rid = Uuid::new_v4().simple().to_string()[..16].to_string();
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    info!(rid=%rid, %method, %path, "http: ->");
    let start = std::time::Instant::now();
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
