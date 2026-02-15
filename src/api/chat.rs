use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_stream::try_stream;
use axum::body::Body;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use futures::{Stream, StreamExt};
use tracing::{info, warn};

use crate::config;
use crate::db;
use crate::openrouter;

use super::auth;
use super::errors;
use super::AppState;

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

    let messages = filter_client_messages(&req.messages);
    if messages.is_empty() {
        return errors::openai_error(
            StatusCode::BAD_REQUEST,
            "no usable messages",
            "invalid_request_error",
            "",
        );
    }

    let (cfg, client, backend_model) = match client_from_disk(&st.data_dir) {
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
        roles = %role_counts(&messages),
        chars = total_chars(&messages),
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
        return stream_chat(st, client, or_req, public_id, public_created).await;
    }

    let resp = match client.create_chat_completion(&or_req).await {
        Ok(r) => r,
        Err(e) => {
            log_backend_err("chat", &e);
            maybe_log_key_info(&st, &client, &e).await;
            return backend_error(&e);
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

fn filter_client_messages(in_msgs: &[openrouter::Message]) -> Vec<openrouter::Message> {
    let mut out = Vec::with_capacity(in_msgs.len());
    for m in in_msgs {
        let role = m.role.trim();
        if role != "user" && role != "assistant" {
            continue;
        }
        if m.content.trim().is_empty() {
            continue;
        }
        out.push(openrouter::Message {
            role: role.to_string(),
            content: m.content.clone(),
        });
    }
    out
}

fn client_from_disk(
    data_dir: &Path,
) -> anyhow::Result<(config::Settings, openrouter::Client, String)> {
    let path = config::settings_path(data_dir);
    let mut cfg: config::Settings = match std::fs::read(&path) {
        Ok(b) => serde_json::from_slice(&b)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Create a default config on first run so the server can serve immediately.
            let s = config::default_settings();
            let _ = config::save_settings(&path, &s);
            s
        }
        Err(e) => return Err(anyhow::anyhow!(e)),
    };

    if cfg.agents.is_empty() {
        // Settings should always include at least one agent; if they don't, fall back to defaults.
        let s = config::default_settings();
        cfg.agents = s.agents;
        if cfg.tasks.chat_agent.is_empty() {
            cfg.tasks.chat_agent = cfg
                .agents
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_default();
        }
        if cfg.tasks.summary_agent.is_empty() {
            cfg.tasks.summary_agent = cfg
                .agents
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_default();
        }
    }
    if cfg.chat.system_prompt.trim().is_empty() {
        cfg.chat.system_prompt = config::DEFAULT_SYSTEM_PROMPT.trim().to_string();
    }
    let agent = cfg
        .resolve_agent("chat")
        .ok_or_else(|| anyhow::anyhow!("no agents configured"))?;
    if !cfg.agents.iter().any(|a| a.name == cfg.tasks.chat_agent) {
        tracing::warn!(
            configured = %cfg.tasks.chat_agent,
            fallback = %agent.name,
            "chat agent binding not found; using fallback"
        );
    }
    let base_url = agent.openai_base_url();
    tracing::info!(
        agent = %agent.name,
        provider = ?agent.provider,
        base_url = %base_url,
        model = %agent.model,
        "chat backend selected"
    );

    let client = openrouter::Client::new(openrouter::Config {
        api_key: agent.api_key.clone(),
        base_url,
        http_referer: if matches!(agent.provider, config::Provider::OpenRouter) {
            agent.http_referer.clone()
        } else {
            String::new()
        },
        x_title: if matches!(agent.provider, config::Provider::OpenRouter) {
            agent.x_title.clone()
        } else {
            String::new()
        },
    })?;

    let model = if agent.model.trim().is_empty() {
        "openai/gpt-4o-mini".to_string()
    } else {
        agent.model.trim().to_string()
    };
    Ok((cfg, client, model))
}

async fn stream_chat(
    st: AppState,
    client: openrouter::Client,
    req: openrouter::ChatCompletionRequest,
    public_id: String,
    public_created: i64,
) -> Response {
    let (status, headers, upstream) = match client.create_chat_completion_stream(&req).await {
        Ok(v) => v,
        Err(e) => {
            log_backend_err("chat_stream", &e);
            maybe_log_key_info(&st, &client, &e).await;
            return backend_error(&e);
        }
    };

    info!(
        backend_status = status.as_u16(),
        content_type = %headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or(""),
        "chat: backend stream opened"
    );

    let stream = rewrite_sse_stream(upstream, public_id, public_created, st.data_dir.clone());
    let body = Body::from_stream(stream);
    let mut resp = Response::new(body);
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/event-stream"),
    );
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    resp.headers_mut().insert(
        header::CONNECTION,
        header::HeaderValue::from_static("keep-alive"),
    );
    resp
}

fn rewrite_sse_stream(
    upstream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    public_id: String,
    public_created: i64,
    data_dir: PathBuf,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    try_stream! {
        let mut buf: Vec<u8> = Vec::new();
        futures::pin_mut!(upstream);
        while let Some(chunk) = upstream.next().await {
            let chunk = chunk.map_err(std::io::Error::other)?;
            buf.extend_from_slice(&chunk);

            while let Some((event, consumed)) = next_sse_event(&buf) {
                buf.drain(..consumed);

                let data = extract_sse_data(&event);
                if data.trim().is_empty() {
                    continue;
                }
                if data.trim() == "[DONE]" {
                    // Stats: only count Saelora's message once the stream finishes successfully.
                    let mgr = db::Manager::new(data_dir.clone());
                    if let Ok(us) = mgr.users() {
                        let _ = us.incr_message_counts(0, 1);
                    }
                    yield Bytes::from_static(b"data: [DONE]\n\n");
                    return;
                }

                let rewritten = rewrite_chunk_json(&data, &public_id, public_created);
                let mut out = Vec::with_capacity(rewritten.len() + 10);
                out.extend_from_slice(b"data: ");
                out.extend_from_slice(rewritten.as_bytes());
                out.extend_from_slice(b"\n\n");
                yield Bytes::from(out);
            }
        }
    }
}

fn next_sse_event(buf: &[u8]) -> Option<(Vec<u8>, usize)> {
    // Find \n\n or \r\n\r\n. We accept either.
    if let Some(pos) = find_subsequence(buf, b"\n\n") {
        let event = buf[..pos].to_vec();
        return Some((event, pos + 2));
    }
    if let Some(pos) = find_subsequence(buf, b"\r\n\r\n") {
        let event = buf[..pos].to_vec();
        return Some((event, pos + 4));
    }
    None
}

fn find_subsequence(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn extract_sse_data(event: &[u8]) -> String {
    let s = String::from_utf8_lossy(event);
    let mut lines = Vec::<String>::new();
    for line in s.lines() {
        let t = line.trim_end_matches('\r').trim();
        if let Some(rest) = t.strip_prefix("data:") {
            lines.push(rest.trim().to_string());
        }
    }
    lines.join("\n")
}

fn rewrite_chunk_json(raw: &str, public_id: &str, public_created: i64) -> String {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return raw.to_string();
    };

    if let serde_json::Value::Object(ref mut m) = v {
        m.insert(
            "id".to_string(),
            serde_json::Value::String(public_id.to_string()),
        );
        m.insert(
            "created".to_string(),
            serde_json::Value::Number(public_created.into()),
        );
        m.insert(
            "model".to_string(),
            serde_json::Value::String("saelora".to_string()),
        );
        if let Some(serde_json::Value::Object(ref mut em)) = m.get_mut("error") {
            em.insert(
                "message".to_string(),
                serde_json::Value::String("backend error".to_string()),
            );
        }
    }
    serde_json::to_string(&v).unwrap_or_else(|_| raw.to_string())
}

fn backend_error(e: &openrouter::HttpError) -> Response {
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

fn log_backend_err(op: &str, e: &openrouter::HttpError) {
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

async fn maybe_log_key_info(st: &AppState, client: &openrouter::Client, e: &openrouter::HttpError) {
    let Some(status) = e.status else { return };
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

fn role_counts(msgs: &[openrouter::Message]) -> String {
    let mut u = 0;
    let mut a = 0;
    for m in msgs {
        match m.role.trim() {
            "user" => u += 1,
            "assistant" => a += 1,
            _ => {}
        }
    }
    format!("u={u} a={a}")
}

fn total_chars(msgs: &[openrouter::Message]) -> usize {
    msgs.iter().map(|m| m.content.len()).sum()
}
