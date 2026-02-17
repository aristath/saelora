use std::path::Path;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::Response;
use tracing::{info, warn};

use crate::{agents, config, openrouter};

use super::super::errors;
use super::super::AppState;

pub(super) fn client_from_disk(
    data_dir: &Path,
) -> anyhow::Result<(config::Settings, openrouter::Client, String)> {
    let resolved = agents::client_for_task_from_disk(data_dir, "chat")?;
    info!(model = %resolved.model, "chat backend selected");
    Ok((resolved.settings, resolved.client, resolved.model))
}

pub(super) fn backend_error(e: &openrouter::HttpError) -> Response {
    if let Some(st) = e.status {
        match st {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return errors::openai_error(
                    StatusCode::BAD_GATEWAY,
                    "backend authentication failed",
                    "server_error",
                    "backend_auth",
                );
            }
            StatusCode::NOT_FOUND => {
                return errors::openai_error(
                    StatusCode::BAD_GATEWAY,
                    "backend model not found",
                    "server_error",
                    "backend_model_not_found",
                );
            }
            StatusCode::TOO_MANY_REQUESTS => {
                return errors::openai_error(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate limited, please retry shortly",
                    "rate_limit_error",
                    "rate_limit_exceeded",
                );
            }
            StatusCode::BAD_REQUEST => {
                return errors::openai_error(
                    StatusCode::BAD_GATEWAY,
                    "backend rejected request",
                    "server_error",
                    "backend_bad_request",
                );
            }
            _ => {
                return errors::openai_error(
                    StatusCode::BAD_GATEWAY,
                    "backend error",
                    "server_error",
                    "backend_error",
                );
            }
        }
    }
    errors::openai_error(
        StatusCode::BAD_GATEWAY,
        "backend error",
        "server_error",
        "backend_error",
    )
}

pub(super) fn log_backend_err(op: &str, e: &openrouter::HttpError) {
    let mut body = e.body.trim().to_string();
    if body.len() > 800 {
        body.truncate(800);
        body.push_str("...");
    }
    warn!(
        op = op,
        status = %e.status.map(|s| s.as_u16()).unwrap_or(0),
        typ = %e.r#type,
        code = %e.code,
        msg = %e.message,
        body = %body,
        transport = ?e.transport,
        "backend error"
    );
}

pub(super) async fn maybe_log_key_info(
    st: &AppState,
    client: &openrouter::Client,
    e: &openrouter::HttpError,
) {
    let Some(status) = e.status else {
        return;
    };
    if status != StatusCode::TOO_MANY_REQUESTS
        && status != StatusCode::UNAUTHORIZED
        && status != StatusCode::FORBIDDEN
    {
        return;
    }
    let mut last = st.last_key_info_log.lock().await;
    if last.elapsed() < Duration::from_secs(30) {
        return;
    }
    *last = Instant::now();
    drop(last);

    let info = match client.get_key_info().await {
        Ok(i) => i,
        Err(err) => {
            warn!(err=%err, "key_info: backend error");
            return;
        }
    };

    let mut rl = String::new();
    if let Some(r) = info.rate_limit {
        if r.requests > 0 && r.interval > 0 {
            rl = format!(" rate_limit={}/{}s", r.requests, r.interval);
        }
    }
    info!(
        free_tier = info.is_free_tier,
        usage = info.usage,
        limit = info.limit,
        remaining = info.limit_remaining,
        label = info.label,
        extra = rl,
        "key_info"
    );
}
