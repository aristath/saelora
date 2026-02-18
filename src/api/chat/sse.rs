use std::path::{Path, PathBuf};

use async_stream::try_stream;
use axum::body::Body;
use axum::body::Bytes;
use axum::http::{header, StatusCode};
use axum::response::Response;
use futures::{Stream, StreamExt};
use tracing::{info, warn};

use crate::{db, openrouter};

use super::super::AppState;
use super::backend;

pub(super) async fn stream_chat(
    st: AppState,
    client: openrouter::Client,
    req: openrouter::ChatCompletionRequest,
    public_id: String,
    public_created: i64,
    user_id: String,
    conversation_id: String,
) -> Response {
    let (status, headers, upstream) = match client.create_chat_completion_stream(&req).await {
        Ok(v) => v,
        Err(e) => {
            backend::log_backend_err("chat_stream", &e);
            backend::maybe_log_key_info(&st, &client, &e).await;
            return backend::backend_error(&e);
        }
    };

    info!(
        backend_status = status.as_u16(),
        content_type = %headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or(""),
        "chat: backend stream opened"
    );

    let stream = rewrite_sse_stream(
        upstream,
        public_id,
        public_created,
        st.data_dir.clone(),
        user_id,
        conversation_id,
    );
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
    user_id: String,
    conversation_id: String,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    try_stream! {
        let mut buf: Vec<u8> = Vec::new();
        let mut assistant_content = String::new();
        let mut persisted = false;
        futures::pin_mut!(upstream);
        while let Some(chunk) = upstream.next().await {
            let chunk = match chunk {
                Ok(v) => v,
                Err(e) => {
                    persist_assistant_message(
                        &data_dir,
                        &user_id,
                        &conversation_id,
                        &assistant_content,
                        &mut persisted,
                    );
                    Err(std::io::Error::other(e))?;
                    unreachable!();
                }
            };
            buf.extend_from_slice(&chunk);

            while let Some((event, consumed)) = next_sse_event(&buf) {
                buf.drain(..consumed);

                let data = extract_sse_data(&event);
                if data.trim().is_empty() {
                    continue;
                }
                if data.trim() == "[DONE]" {
                    persist_assistant_message(
                        &data_dir,
                        &user_id,
                        &conversation_id,
                        &assistant_content,
                        &mut persisted,
                    );
                    yield Bytes::from_static(b"data: [DONE]\n\n");
                    return;
                }
                if let Some(tok) = extract_delta_content(&data) {
                    assistant_content.push_str(&tok);
                }

                let rewritten = rewrite_chunk_json(&data, &public_id, public_created);
                let mut out = Vec::with_capacity(rewritten.len() + 10);
                out.extend_from_slice(b"data: ");
                out.extend_from_slice(rewritten.as_bytes());
                out.extend_from_slice(b"\n\n");
                yield Bytes::from(out);
            }
        }
        persist_assistant_message(
            &data_dir,
            &user_id,
            &conversation_id,
            &assistant_content,
            &mut persisted,
        );
        yield Bytes::from_static(b"data: [DONE]\n\n");
    }
}

fn persist_assistant_message(
    data_dir: &Path,
    user_id: &str,
    conversation_id: &str,
    assistant_content: &str,
    persisted: &mut bool,
) {
    if *persisted {
        return;
    }
    *persisted = true;

    if assistant_content.trim().is_empty() {
        return;
    }

    let mgr = db::Manager::new(data_dir.to_path_buf());
    match mgr.user_data(user_id) {
        Ok(uds) => match uds.append_saelora_message_in(conversation_id, assistant_content) {
            Ok(_) => {
                if let Ok(us) = mgr.users() {
                    let _ = us.incr_message_counts(0, 1);
                }
            }
            Err(e) => {
                warn!(err=%e, user_id=%user_id, conversation_id=%conversation_id, "chat_stream: append saelora message failed");
            }
        },
        Err(e) => {
            warn!(err=%e, user_id=%user_id, "chat_stream: user data open failed");
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

fn extract_delta_content(raw: &str) -> Option<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return None;
    };
    let tok = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c0| c0.get("delta"))
        .and_then(|d| d.get("content"))
        .and_then(|c| c.as_str())?;
    if tok.is_empty() {
        return None;
    }
    Some(tok.to_string())
}
