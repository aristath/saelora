use axum::http::{header, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get};
use axum::Router;
use mime_guess::MimeGuess;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/site"]
struct SiteAssets;

#[derive(RustEmbed)]
#[folder = "web/app"]
struct AppAssets;

pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/app", get(|| async { Redirect::permanent("/app/") }))
        .route("/app/", get(app_index))
        .route("/app/*path", any(app_asset_any))
        .route("/", get(site_index))
        .route("/*path", any(site_asset_any))
}

async fn site_index() -> Response {
    serve_embedded::<SiteAssets>("index.html")
}

async fn site_asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    let p = clean_path(&path);
    if p.is_empty() {
        return serve_embedded::<SiteAssets>("index.html");
    }
    if is_unsafe_path(&p) {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve_embedded::<SiteAssets>(&p)
}

async fn site_asset_any(
    method: Method,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::NOT_FOUND.into_response();
    }
    site_asset(axum::extract::Path(path)).await
}

async fn app_index() -> Response {
    serve_embedded::<AppAssets>("index.html")
}

async fn app_asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    let p = clean_path(&path);
    if p.is_empty() {
        return serve_embedded::<AppAssets>("index.html");
    }
    if is_unsafe_path(&p) {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve_embedded::<AppAssets>(&p)
}

async fn app_asset_any(
    method: Method,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::NOT_FOUND.into_response();
    }
    app_asset(axum::extract::Path(path)).await
}

fn serve_embedded<A: RustEmbed>(path: &str) -> Response {
    if let Some(content) = A::get(path) {
        let body = content.data;
        let mime = MimeGuess::from_path(path).first_or_octet_stream();
        (
            [
                (header::CONTENT_TYPE, mime.as_ref()),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            body,
        )
            .into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

fn is_unsafe_path(p: &str) -> bool {
    if p.contains("..") {
        return true;
    }
    // Prevent dotfile peeking.
    p.split('/').any(|seg| seg.starts_with('.'))
}

fn clean_path(raw: &str) -> String {
    // Similar to Go's path.Clean("/"+raw) then stripping the leading slash.
    let mut p = raw.replace('\\', "/");
    while p.starts_with('/') {
        p.remove(0);
    }
    let uri: Uri = format!("/{}", p)
        .parse()
        .unwrap_or_else(|_| Uri::from_static("/"));
    let mut path = uri.path().to_string();
    if path.starts_with('/') {
        path.remove(0);
    }
    // Trim any trailing slash.
    while path.ends_with('/') {
        path.pop();
    }
    path
}
