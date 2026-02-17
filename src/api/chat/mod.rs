mod backend;
mod sse;
mod util;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use tracing::{info, warn};

use crate::{config, db, memory, openrouter};

use super::{auth, errors, AppState};

const MODE_INSTANT: &str = "instant";
const MODE_HOURLY: &str = "hourly";
const MODE_DAILY: &str = "daily";
const MODE_WEEKLY: &str = "weekly";

pub(super) async fn chat_options() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ChatCompletionsRequest {
    #[serde(default)]
    model: String,
    #[serde(default)]
    conversation_id: String,
    messages: Vec<openrouter::Message>,
    #[serde(default)]
    stream: bool,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct ChatHistoryQuery {
    #[serde(default)]
    pub(super) limit: usize,
    #[serde(default)]
    pub(super) conversation_id: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct CreateConversationRequest {
    #[serde(default)]
    title: String,
    #[serde(default)]
    mode: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct RenameConversationRequest {
    #[serde(default)]
    title: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct SetConversationModeRequest {
    #[serde(default)]
    mode: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct ThreadMessageRequest {
    #[serde(default)]
    content: String,
}

fn normalize_mode(mode: &str) -> Option<&'static str> {
    match mode.trim().to_ascii_lowercase().as_str() {
        MODE_INSTANT => Some(MODE_INSTANT),
        MODE_HOURLY => Some(MODE_HOURLY),
        MODE_DAILY => Some(MODE_DAILY),
        MODE_WEEKLY => Some(MODE_WEEKLY),
        _ => None,
    }
}

fn mode_interval_ms(mode: &str) -> Option<i64> {
    match mode {
        MODE_HOURLY => Some(60 * 60 * 1000),
        MODE_DAILY => Some(24 * 60 * 60 * 1000),
        MODE_WEEKLY => Some(7 * 24 * 60 * 60 * 1000),
        _ => None,
    }
}

fn pending_after_last_saelora(rows: &[db::ChatMessageRecord]) -> (usize, Option<i64>) {
    let last_saelora_at = rows
        .iter()
        .rev()
        .find(|m| m.role == "saelora")
        .map(|m| m.created_at);
    let pending = rows
        .iter()
        .filter(|m| {
            m.role == "user"
                && last_saelora_at
                    .map(|last| m.created_at > last)
                    .unwrap_or(true)
        })
        .count();
    (pending, last_saelora_at)
}

pub(super) async fn chat_completions(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Require auth for chat.
    let u = match auth::authed_user(&st, &headers) {
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
    let latest_user_content = messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.trim().to_string())
        .filter(|s| !s.is_empty());
    if latest_user_content.is_none() {
        return errors::openai_error(
            StatusCode::BAD_REQUEST,
            "missing user message",
            "invalid_request_error",
            "",
        );
    }
    let conversation_id = req.conversation_id.trim().to_string();
    if conversation_id.is_empty() {
        return errors::openai_error(
            StatusCode::BAD_REQUEST,
            "missing conversation_id",
            "invalid_request_error",
            "",
        );
    }

    let mgr = db::Manager::new(st.data_dir.clone());
    let mut appended_user_message_id: Option<i64> = None;
    let history_messages: Vec<openrouter::Message> = {
        let uds = match mgr.user_data(&u.id) {
            Ok(v) => v,
            Err(e) => {
                warn!(err=%e, user_id=%u.id, "chat: user data open failed");
                return errors::openai_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to open user data",
                    "server_error",
                    "",
                );
            }
        };

        let mode = match uds.conversation_mode(&conversation_id) {
            Ok(m) => m,
            Err(db::DbError::InvalidConversation) => {
                return errors::openai_error(
                    StatusCode::BAD_REQUEST,
                    "invalid conversation id",
                    "invalid_request_error",
                    "",
                );
            }
            Err(db::DbError::NotFound) => {
                return errors::openai_error(
                    StatusCode::NOT_FOUND,
                    "conversation not found",
                    "invalid_request_error",
                    "",
                );
            }
            Err(e) => {
                warn!(err=%e, user_id=%u.id, conversation_id=%conversation_id, "chat: mode read failed");
                return errors::openai_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to read conversation mode",
                    "server_error",
                    "",
                );
            }
        };
        if mode != MODE_INSTANT {
            return errors::openai_error(
                StatusCode::BAD_REQUEST,
                "conversation mode is not instant",
                "invalid_request_error",
                "",
            );
        }

        // Persist the latest user turn first, then build model context from this conversation only.
        if let Some(text) = latest_user_content.as_ref() {
            match uds.append_user_message_in(&conversation_id, text) {
                Ok(id) => appended_user_message_id = Some(id),
                Err(db::DbError::InvalidConversation) => {
                    return errors::openai_error(
                        StatusCode::BAD_REQUEST,
                        "invalid conversation id",
                        "invalid_request_error",
                        "",
                    );
                }
                Err(db::DbError::NotFound) => {
                    return errors::openai_error(
                        StatusCode::NOT_FOUND,
                        "conversation not found",
                        "invalid_request_error",
                        "",
                    );
                }
                Err(e) => {
                    warn!(err=%e, user_id=%u.id, conversation_id=%conversation_id, "chat: append user message failed");
                    return errors::openai_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to persist message",
                        "server_error",
                        "",
                    );
                }
            }
        }

        let history_rows = match uds.list_messages_in(&conversation_id, 2000) {
            Ok(v) => v,
            Err(db::DbError::InvalidConversation) => {
                return errors::openai_error(
                    StatusCode::BAD_REQUEST,
                    "invalid conversation id",
                    "invalid_request_error",
                    "",
                );
            }
            Err(e) => {
                warn!(err=%e, user_id=%u.id, conversation_id=%conversation_id, "chat: history read failed");
                return errors::openai_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load conversation",
                    "server_error",
                    "",
                );
            }
        };
        history_rows
            .into_iter()
            .filter_map(|m| {
                let role = if m.role == "saelora" {
                    "assistant"
                } else if m.role == "user" {
                    "user"
                } else {
                    return None;
                };
                Some(openrouter::Message {
                    role: role.to_string(),
                    content: m.content,
                })
            })
            .collect()
    };

    if let Some(message_id) = appended_user_message_id {
        memory::spawn_ingest_user_message(
            st.data_dir.clone(),
            u.id.clone(),
            conversation_id.clone(),
            message_id,
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
    let memory_context = if let Some(text) = latest_user_content.clone() {
        memory::build_memory_context(
            st.data_dir.clone(),
            u.id.clone(),
            conversation_id.clone(),
            text,
        )
        .await
    } else {
        None
    };

    let mut or_req = openrouter::ChatCompletionRequest {
        model: backend_model.clone(),
        messages: Vec::with_capacity(history_messages.len() + 2),
        // Do not expose model controls over the public API (keep chat surface minimal and stable).
        temperature: None,
        max_tokens: None,
        stream: req.stream,
    };
    or_req.messages.push(openrouter::Message {
        role: "system".to_string(),
        content: system_prompt,
    });
    if let Some(ctx) = memory_context {
        or_req.messages.push(openrouter::Message {
            role: "system".to_string(),
            content: ctx,
        });
    }
    or_req.messages.extend(history_messages.clone());

    let public_id = format!("saelora-{}", Utc::now().format("%Y%m%dT%H%M%S%.3fZ"));
    let public_created = Utc::now().timestamp();

    info!(
        stream = req.stream,
        backend_model = backend_model,
        client_model = req.model.trim(),
        msgs_in = req.messages.len(),
        msgs_used = history_messages.len(),
        roles = %util::role_counts(&history_messages),
        chars = util::total_chars(&history_messages),
        "chat: request"
    );

    // Stats: treat each request as one user turn if it ends with a user message.
    let user_delta: i64 = if history_messages
        .last()
        .map(|m| m.role == "user")
        .unwrap_or(false)
    {
        1
    } else {
        0
    };
    if user_delta > 0 {
        if let Ok(us) = mgr.users() {
            let _ = us.incr_message_counts(user_delta, 0);
        }
    }

    if req.stream {
        return sse::stream_chat(
            st,
            client,
            or_req,
            public_id,
            public_created,
            u.id.clone(),
            conversation_id,
        )
        .await;
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
    let assistant_content = choice.message.content.clone();

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
    if let Ok(us) = mgr.users() {
        let _ = us.incr_message_counts(0, 1);
    }
    match mgr.user_data(&u.id) {
        Ok(uds) => {
            if let Err(e) = uds.append_saelora_message_in(&conversation_id, &assistant_content) {
                warn!(err=%e, user_id=%u.id, conversation_id=%conversation_id, "chat: append saelora message failed");
            }
        }
        Err(e) => {
            warn!(err=%e, user_id=%u.id, "chat: user data open failed for assistant persist");
        }
    }

    (StatusCode::OK, Json(out)).into_response()
}

pub(super) async fn chat_history(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ChatHistoryQuery>,
) -> Response {
    let u = match auth::authed_user(&st, &headers) {
        Ok(u) => u,
        Err(code) => return errors::auth_error(code, "unauthorized"),
    };

    let limit = if q.limit == 0 {
        2000usize
    } else {
        q.limit.min(10_000)
    };

    let mgr = db::Manager::new(st.data_dir.clone());
    let uds = match mgr.user_data(&u.id) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, user_id=%u.id, "chat_history: user data open failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to load history");
        }
    };
    let rows = match uds.list_messages_in(&q.conversation_id, limit) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, user_id=%u.id, "chat_history: list failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to load history");
        }
    };

    let messages: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|m| {
            let role = if m.role == "saelora" {
                "assistant"
            } else {
                "user"
            };
            serde_json::json!({
                "id": m.id,
                "role": role,
                "content": m.content,
                "created_at": m.created_at,
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({ "messages": messages })),
    )
        .into_response()
}

pub(super) async fn list_conversations(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let u = match auth::authed_user(&st, &headers) {
        Ok(u) => u,
        Err(code) => return errors::auth_error(code, "unauthorized"),
    };
    let mgr = db::Manager::new(st.data_dir.clone());
    let uds = match mgr.user_data(&u.id) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, user_id=%u.id, "list_conversations: user data open failed");
            return errors::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load conversations",
            );
        }
    };
    let convs = match uds.list_conversations(false, 200) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, user_id=%u.id, "list_conversations: list failed");
            return errors::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load conversations",
            );
        }
    };
    let out: Vec<serde_json::Value> = convs
        .into_iter()
        .map(|c| {
            serde_json::json!({
                "id": c.id,
                "title": c.title,
                "mode": c.mode,
                "created_at": c.created_at,
                "updated_at": c.updated_at,
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(serde_json::json!({ "conversations": out })),
    )
        .into_response()
}

pub(super) async fn create_conversation(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let u = match auth::authed_user(&st, &headers) {
        Ok(u) => u,
        Err(code) => return errors::auth_error(code, "unauthorized"),
    };
    let req: CreateConversationRequest = serde_json::from_slice(&body).unwrap_or_default();

    let mgr = db::Manager::new(st.data_dir.clone());
    let uds = match mgr.user_data(&u.id) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, user_id=%u.id, "create_conversation: user data open failed");
            return errors::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create conversation",
            );
        }
    };
    let mode = if req.mode.trim().is_empty() {
        MODE_INSTANT
    } else if let Some(m) = normalize_mode(&req.mode) {
        m
    } else {
        return errors::auth_error(StatusCode::BAD_REQUEST, "invalid mode");
    };
    let c = match if mode == MODE_INSTANT {
        uds.create_conversation(&req.title)
    } else {
        uds.create_conversation_with_mode(&req.title, mode)
    } {
        Ok(v) => v,
        Err(db::DbError::InvalidStatus) => {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid mode");
        }
        Err(e) => {
            warn!(err=%e, user_id=%u.id, "create_conversation: create failed");
            return errors::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create conversation",
            );
        }
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "conversation": {
                "id": c.id,
                "title": c.title,
                "mode": c.mode,
                "created_at": c.created_at,
                "updated_at": c.updated_at,
            }
        })),
    )
        .into_response()
}

pub(super) async fn set_conversation_mode(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    body: Bytes,
) -> Response {
    let u = match auth::authed_user(&st, &headers) {
        Ok(u) => u,
        Err(code) => return errors::auth_error(code, "unauthorized"),
    };
    let req: SetConversationModeRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return errors::auth_error(StatusCode::BAD_REQUEST, "invalid json"),
    };
    let Some(mode) = normalize_mode(&req.mode) else {
        return errors::auth_error(StatusCode::BAD_REQUEST, "invalid mode");
    };

    let mgr = db::Manager::new(st.data_dir.clone());
    let uds = match mgr.user_data(&u.id) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, user_id=%u.id, "set_conversation_mode: user data open failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to update mode");
        }
    };
    let mode = match uds.set_conversation_mode(&conversation_id, mode) {
        Ok(v) => v,
        Err(db::DbError::InvalidConversation) => {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid conversation id");
        }
        Err(db::DbError::InvalidStatus) => {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid mode");
        }
        Err(db::DbError::NotFound) => {
            return errors::auth_error(StatusCode::NOT_FOUND, "conversation not found");
        }
        Err(e) => {
            warn!(err=%e, user_id=%u.id, conversation_id=%conversation_id, "set_conversation_mode: update failed");
            return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to update mode");
        }
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({ "ok": true, "mode": mode })),
    )
        .into_response()
}

pub(super) async fn thread_message(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    body: Bytes,
) -> Response {
    let u = match auth::authed_user(&st, &headers) {
        Ok(u) => u,
        Err(code) => return errors::auth_error(code, "unauthorized"),
    };
    let req: ThreadMessageRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return errors::auth_error(StatusCode::BAD_REQUEST, "invalid json"),
    };
    let content = req.content.trim();
    if content.is_empty() {
        return errors::auth_error(StatusCode::BAD_REQUEST, "missing content");
    }

    let mgr = db::Manager::new(st.data_dir.clone());
    let uds = match mgr.user_data(&u.id) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, user_id=%u.id, "thread_message: user data open failed");
            return errors::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to queue message",
            );
        }
    };
    let mode = match uds.conversation_mode(&conversation_id) {
        Ok(v) => v,
        Err(db::DbError::InvalidConversation) => {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid conversation id");
        }
        Err(db::DbError::NotFound) => {
            return errors::auth_error(StatusCode::NOT_FOUND, "conversation not found");
        }
        Err(e) => {
            warn!(err=%e, user_id=%u.id, conversation_id=%conversation_id, "thread_message: mode lookup failed");
            return errors::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to queue message",
            );
        }
    };
    match uds.append_user_message_in(&conversation_id, content) {
        Ok(message_id) => {
            memory::spawn_ingest_user_message(
                st.data_dir.clone(),
                u.id.clone(),
                conversation_id.clone(),
                message_id,
            );
        }
        Err(db::DbError::InvalidConversation) => {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid conversation id");
        }
        Err(db::DbError::NotFound) => {
            return errors::auth_error(StatusCode::NOT_FOUND, "conversation not found");
        }
        Err(e) => {
            warn!(err=%e, user_id=%u.id, conversation_id=%conversation_id, "thread_message: append failed");
            return errors::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to queue message",
            );
        }
    }
    if let Ok(us) = mgr.users() {
        let _ = us.incr_message_counts(1, 0);
    }

    let rows: Vec<db::ChatMessageRecord> = uds
        .list_messages_in(&conversation_id, 2000)
        .unwrap_or_default();
    let (pending_count, _) = pending_after_last_saelora(&rows);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "mode": mode,
            "pending_count": pending_count,
        })),
    )
        .into_response()
}

pub(super) async fn tick_conversation(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Response {
    let u = match auth::authed_user(&st, &headers) {
        Ok(u) => u,
        Err(code) => return errors::auth_error(code, "unauthorized"),
    };
    let mgr = db::Manager::new(st.data_dir.clone());
    let uds = match mgr.user_data(&u.id) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, user_id=%u.id, "tick_conversation: user data open failed");
            return errors::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to process thread",
            );
        }
    };
    let mode = match uds.conversation_mode(&conversation_id) {
        Ok(v) => v,
        Err(db::DbError::InvalidConversation) => {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid conversation id");
        }
        Err(db::DbError::NotFound) => {
            return errors::auth_error(StatusCode::NOT_FOUND, "conversation not found");
        }
        Err(e) => {
            warn!(err=%e, user_id=%u.id, conversation_id=%conversation_id, "tick_conversation: mode lookup failed");
            return errors::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to process thread",
            );
        }
    };
    if mode == MODE_INSTANT {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "mode": mode,
                "replied": false,
                "pending_count": 0,
            })),
        )
            .into_response();
    }

    let rows = match uds.list_messages_in(&conversation_id, 2000) {
        Ok(v) => v,
        Err(db::DbError::InvalidConversation) => {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid conversation id");
        }
        Err(db::DbError::NotFound) => {
            return errors::auth_error(StatusCode::NOT_FOUND, "conversation not found");
        }
        Err(e) => {
            warn!(err=%e, user_id=%u.id, conversation_id=%conversation_id, "tick_conversation: history read failed");
            return errors::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to process thread",
            );
        }
    };
    let (pending_count, last_saelora_at) = pending_after_last_saelora(&rows);
    if pending_count == 0 {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "mode": mode,
                "replied": false,
                "pending_count": 0,
            })),
        )
            .into_response();
    }

    let now_ms = Utc::now().timestamp_millis();
    if let (Some(interval_ms), Some(last_at)) = (mode_interval_ms(&mode), last_saelora_at) {
        let next_due_at = last_at + interval_ms;
        if now_ms < next_due_at {
            return (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "mode": mode,
                    "replied": false,
                    "pending_count": pending_count,
                    "next_due_at": next_due_at,
                })),
            )
                .into_response();
        }
    }

    let history_messages: Vec<openrouter::Message> = rows
        .iter()
        .filter_map(|m| {
            let role = if m.role == "saelora" {
                "assistant"
            } else if m.role == "user" {
                "user"
            } else {
                return None;
            };
            Some(openrouter::Message {
                role: role.to_string(),
                content: m.content.clone(),
            })
        })
        .collect();
    if history_messages.is_empty() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "mode": mode,
                "replied": false,
                "pending_count": pending_count,
            })),
        )
            .into_response();
    }

    let (cfg, client, backend_model) = match backend::client_from_disk(&st.data_dir) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, "tick_conversation: server not configured");
            return errors::auth_error(StatusCode::SERVICE_UNAVAILABLE, "server not configured");
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
    let latest_user_text = rows
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .unwrap_or_default();
    let memory_context = memory::build_memory_context(
        st.data_dir.clone(),
        u.id.clone(),
        conversation_id.clone(),
        latest_user_text,
    )
    .await;
    let mut or_req = openrouter::ChatCompletionRequest {
        model: backend_model,
        messages: Vec::with_capacity(history_messages.len() + 2),
        temperature: None,
        max_tokens: None,
        stream: false,
    };
    or_req.messages.push(openrouter::Message {
        role: "system".to_string(),
        content: system_prompt,
    });
    if let Some(ctx) = memory_context {
        or_req.messages.push(openrouter::Message {
            role: "system".to_string(),
            content: ctx,
        });
    }
    or_req.messages.extend(history_messages);

    let resp = match client.create_chat_completion(&or_req).await {
        Ok(r) => r,
        Err(e) => {
            backend::log_backend_err("tick_conversation", &e);
            backend::maybe_log_key_info(&st, &client, &e).await;
            return backend::backend_error(&e);
        }
    };
    let assistant_content = resp
        .choices
        .first()
        .map(|c| c.message.content.trim().to_string())
        .unwrap_or_default();
    if assistant_content.is_empty() {
        return errors::auth_error(StatusCode::BAD_GATEWAY, "empty backend response");
    }
    if let Err(e) = uds.append_saelora_message_in(&conversation_id, &assistant_content) {
        warn!(err=%e, user_id=%u.id, conversation_id=%conversation_id, "tick_conversation: append failed");
        return errors::auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save response");
    }
    if let Ok(us) = mgr.users() {
        let _ = us.incr_message_counts(0, 1);
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "mode": mode,
            "replied": true,
            "pending_count": 0,
            "message": {
                "role": "assistant",
                "content": assistant_content,
                "created_at": Utc::now().timestamp_millis(),
            }
        })),
    )
        .into_response()
}

pub(super) async fn rename_conversation(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    body: Bytes,
) -> Response {
    let u = match auth::authed_user(&st, &headers) {
        Ok(u) => u,
        Err(code) => return errors::auth_error(code, "unauthorized"),
    };
    let req: RenameConversationRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid json");
        }
    };
    let mgr = db::Manager::new(st.data_dir.clone());
    let uds = match mgr.user_data(&u.id) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, user_id=%u.id, "rename_conversation: user data open failed");
            return errors::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to rename conversation",
            );
        }
    };
    match uds.rename_conversation(&conversation_id, &req.title) {
        Ok(()) => {}
        Err(db::DbError::InvalidConversation) => {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid conversation id");
        }
        Err(db::DbError::NotFound) => {
            return errors::auth_error(StatusCode::NOT_FOUND, "conversation not found");
        }
        Err(e) => {
            warn!(err=%e, user_id=%u.id, conversation_id=%conversation_id, "rename_conversation: rename failed");
            return errors::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to rename conversation",
            );
        }
    }
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

pub(super) async fn archive_conversation(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Response {
    let u = match auth::authed_user(&st, &headers) {
        Ok(u) => u,
        Err(code) => return errors::auth_error(code, "unauthorized"),
    };
    let mgr = db::Manager::new(st.data_dir.clone());
    let uds = match mgr.user_data(&u.id) {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, user_id=%u.id, "archive_conversation: user data open failed");
            return errors::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to archive conversation",
            );
        }
    };
    match uds.archive_conversation(&conversation_id) {
        Ok(()) => {}
        Err(db::DbError::InvalidConversation) => {
            return errors::auth_error(StatusCode::BAD_REQUEST, "invalid conversation id");
        }
        Err(db::DbError::NotFound) => {
            return errors::auth_error(StatusCode::NOT_FOUND, "conversation not found");
        }
        Err(e) => {
            warn!(err=%e, user_id=%u.id, conversation_id=%conversation_id, "archive_conversation: archive failed");
            return errors::auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to archive conversation",
            );
        }
    }
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}
