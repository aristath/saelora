mod backend;
mod sse;
mod util;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use tracing::{info, warn};

use crate::{config, db, openrouter};

use super::{auth, errors, AppState};

pub(super) async fn chat_options() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ChatCompletionsRequest {
    #[serde(default)]
    model: String,
    messages: Vec<openrouter::Message>,
    #[serde(default)]
    stream: bool,
}

pub(super) async fn chat_completions(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Require auth for chat.
    let _u = match auth::authed_user(&st, &headers) {
        Ok(u) => u,
        Err(code) => {
            return errors::openai_error(
                code,
                "unauthorized",
                "invalid_request_error",
                "unauthorized",
            )
        }
    };

    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        != "application/json"
    {
        // Keep behavior predictable for the browser client.
    }

    let req: ChatCompletionsRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return errors::openai_error(
                StatusCode::BAD_REQUEST,
                "invalid json",
                "invalid_request_error",
                "",
            )
        }
    };
    if req.messages.is_empty() {
        return errors::openai_error(
            StatusCode::BAD_REQUEST,
            "missing messages",
            "invalid_request_error",
            "",
        );
    }

    let messages = util::filter_client_messages(&req.messages);
    if messages.is_empty() {
        return errors::openai_error(
            StatusCode::BAD_REQUEST,
            "no usable messages",
            "invalid_request_error",
            "",
        );
    }

    let (cfg, client, backend_model) = match backend::client_from_disk(&st.data_dir) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, "chat: server not configured");
            return errors::openai_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "server not configured",
                "server_error",
                "",
            );
        }
    };

    let system_prompt = {
        let s = cfg.chat.system_prompt.trim();
        if s.is_empty() {
            config::DEFAULT_SYSTEM_PROMPT.trim().to_string()
        } else {
            s.to_string()
        }
    };

    let mut or_req = openrouter::ChatCompletionRequest {
        model: backend_model.clone(),
        messages: Vec::with_capacity(messages.len() + 1),
        // Do not expose model controls over the public API (keep chat surface minimal and stable).
        temperature: None,
        max_tokens: None,
        stream: req.stream,
    };
    or_req.messages.push(openrouter::Message {
        role: "system".to_string(),
        content: system_prompt,
    });
    or_req.messages.extend(messages.clone());

    let public_id = format!("saelora-{}", Utc::now().format("%Y%m%dT%H%M%S%.3fZ"));
    let public_created = Utc::now().timestamp();

    info!(
        stream = req.stream,
        backend_model = backend_model,
        client_model = req.model.trim(),
        msgs_in = req.messages.len(),
        msgs_used = messages.len(),
        roles = %util::role_counts(&messages),
        chars = util::total_chars(&messages),
        "chat: request"
    );

    // Stats: treat each request as one user turn if it ends with a user message.
    let user_delta: i64 = if messages.last().map(|m| m.role == "user").unwrap_or(false) {
        1
    } else {
        0
    };
    if user_delta > 0 {
        let mgr = db::Manager::new(st.data_dir.clone());
        if let Ok(us) = mgr.users() {
            let _ = us.incr_message_counts(user_delta, 0);
        }
    }

    if req.stream {
        return sse::stream_chat(st, client, or_req, public_id, public_created).await;
    }

    let resp = match client.create_chat_completion(&or_req).await {
        Ok(r) => r,
        Err(e) => {
            backend::log_backend_err("chat", &e);
            backend::maybe_log_key_info(&st, &client, &e).await;
            return backend::backend_error(&e);
        }
    };

    // Only return the first choice for now.
    let choice = resp
        .choices
        .first()
        .cloned()
        .unwrap_or(openrouter::ChatChoice {
            index: 0,
            message: openrouter::Message {
                role: "assistant".to_string(),
                content: String::new(),
            },
            finish_reason: None,
        });

    let out = serde_json::json!({
        "id": public_id,
        "object": "chat.completion",
        "created": public_created,
        "model": "saelora",
        "choices": [{
            "index": 0,
            "message": choice.message,
            "finish_reason": choice.finish_reason,
        }],
        "usage": resp.usage,
    });

    // Increment Saelora count on successful response.
    let mgr = db::Manager::new(st.data_dir.clone());
    if let Ok(us) = mgr.users() {
        let _ = us.incr_message_counts(0, 1);
    }

    (StatusCode::OK, Json(out)).into_response()
}
