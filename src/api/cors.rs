use std::path::Path;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Method, Request, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::config;

use super::AppState;

pub(super) async fn cors(State(st): State<AppState>, req: Request<Body>, next: Next) -> Response {
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_string();

    let allowed_origin = if origin.is_empty() {
        None
    } else if is_allowed_origin(&origin, &st.data_dir) {
        Some(origin)
    } else {
        None
    };

    // Preflight.
    if req.method() == Method::OPTIONS {
        let mut resp = StatusCode::NO_CONTENT.into_response();
        if let Some(o) = allowed_origin {
            let h = resp.headers_mut();
            if let Ok(v) = header::HeaderValue::from_str(&o) {
                h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
                h.insert(header::VARY, header::HeaderValue::from_static("Origin"));
                h.insert(
                    header::ACCESS_CONTROL_ALLOW_METHODS,
                    header::HeaderValue::from_static("GET,POST,OPTIONS"),
                );
                h.insert(
                    header::ACCESS_CONTROL_ALLOW_HEADERS,
                    header::HeaderValue::from_static("Content-Type, Authorization"),
                );
                h.insert(
                    header::ACCESS_CONTROL_MAX_AGE,
                    header::HeaderValue::from_static("600"),
                );
            }
        }
        return resp;
    }

    let mut resp = next.run(req).await;
    if let Some(o) = allowed_origin {
        if let Ok(v) = header::HeaderValue::from_str(&o) {
            let h = resp.headers_mut();
            h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
            h.insert(header::VARY, header::HeaderValue::from_static("Origin"));
            h.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                header::HeaderValue::from_static("GET,POST,OPTIONS"),
            );
            h.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                header::HeaderValue::from_static("Content-Type, Authorization"),
            );
        }
    }
    resp
}

pub(super) fn is_allowed_origin(origin: &str, data_dir: &Path) -> bool {
    let Some(o) = normalize_origin(origin) else {
        return false;
    };
    if is_local_dev_origin(&o) {
        return true;
    }

    let settings = config::load_settings(&config::settings_path(data_dir))
        .unwrap_or_else(|_| config::default_settings());
    let pub_base = settings.http_public_base();
    if pub_base.is_empty() {
        return false;
    }
    let Some(allowed) = normalize_origin(&pub_base) else {
        return false;
    };
    o == allowed
}

fn normalize_origin(s: &str) -> Option<String> {
    let uri: Uri = s.parse().ok()?;
    let scheme = uri.scheme_str()?;
    let auth = uri.authority()?;
    Some(format!("{}://{}", scheme, auth.as_str()))
}

fn is_local_dev_origin(origin: &str) -> bool {
    let uri: Uri = match origin.parse() {
        Ok(u) => u,
        Err(_) => return false,
    };
    if uri.scheme_str() != Some("http") {
        return false;
    }
    let Some(auth) = uri.authority() else {
        return false;
    };
    matches!(auth.host(), "localhost" | "127.0.0.1" | "::1")
}
