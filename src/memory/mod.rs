use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context as _;
use tracing::{info, warn};

use crate::{agents, db, openrouter};

const CURATOR_SYSTEM_PROMPT: &str = r#"You maintain long-term memory for a single user conversation stream.
Return JSON only with this exact shape:
{
  "updates": [
    {
      "statement": "short canonical statement about the user",
      "delta": -1.0 to 1.0,
      "salience": 0.0 to 1.0,
      "confidence": 0.0 to 1.0,
      "evidence": "very short reason"
    }
  ],
  "links": [
    {
      "from": "statement text",
      "to": "statement text",
      "kind": "supports|conflicts|related",
      "strength": 0.0 to 1.0
    }
  ]
}
Rules:
- Use concise neutral wording.
- Create updates only when there is meaningful memory signal.
- Repetition should increase positive delta for the same meaning.
- Contradiction should emit negative delta.
- Do not include prose outside JSON."#;

#[derive(Debug, Clone)]
struct ScoredStatement {
    statement: db::MemoryStatementRecord,
    relevance: f64,
    score: f64,
}

#[derive(Debug, Default, serde::Deserialize)]
struct CuratorPayload {
    #[serde(default)]
    updates: Vec<CuratorUpdate>,
    #[serde(default)]
    links: Vec<CuratorLink>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct CuratorUpdate {
    #[serde(default, alias = "text", alias = "claim")]
    statement: String,
    #[serde(default, alias = "weight", alias = "score_delta")]
    delta: f64,
    #[serde(default)]
    salience: f64,
    #[serde(default)]
    confidence: f64,
    #[serde(default, alias = "reason")]
    evidence: String,
}

#[derive(Debug, Default, serde::Deserialize)]
struct CuratorLink {
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    strength: f64,
}

pub fn spawn_ingest_user_message(
    data_dir: PathBuf,
    user_id: String,
    conversation_id: String,
    message_id: i64,
) {
    tokio::spawn(async move {
        if let Err(e) =
            ingest_user_message(data_dir, user_id.clone(), conversation_id, message_id).await
        {
            warn!(err=%e, user_id=%user_id, message_id=message_id, "memory ingest failed");
        }
    });
}

pub async fn build_memory_context(
    data_dir: PathBuf,
    user_id: String,
    _conversation_id: String,
    latest_user_text: String,
) -> Option<String> {
    match build_memory_context_inner(data_dir, user_id, latest_user_text).await {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, "memory context build failed");
            None
        }
    }
}

async fn build_memory_context_inner(
    data_dir: PathBuf,
    user_id: String,
    latest_user_text: String,
) -> anyhow::Result<Option<String>> {
    let query_text = latest_user_text.trim();
    if query_text.is_empty() {
        return Ok(None);
    }

    let embed_backend = agents::client_for_task_from_disk(&data_dir, "memory_embed")?;
    let query_embedding =
        embed_single_text(&embed_backend.client, &embed_backend.model, query_text)
            .await
            .context("embed query")?;

    let mgr = db::Manager::new(data_dir);
    let uds = mgr.user_data(&user_id)?;
    let statements = uds.list_memory_statements_with_embeddings(600)?;
    if statements.is_empty() {
        return Ok(None);
    }

    let mut ranked: Vec<ScoredStatement> = statements
        .into_iter()
        .filter_map(|(statement, embedding)| {
            if embedding.len() != query_embedding.len() {
                return None;
            }
            let relevance = cosine_similarity(&query_embedding, &embedding);
            if relevance < 0.10 {
                return None;
            }
            let belief_strength = belief_strength(statement.belief_score);
            let salience = statement.salience.clamp(0.0, 1.0);
            let score = (relevance * 0.70) + (belief_strength * 0.20) + (salience * 0.10);
            Some(ScoredStatement {
                statement,
                relevance,
                score,
            })
        })
        .collect();

    if ranked.is_empty() {
        return Ok(None);
    }

    ranked.sort_by(|a, b| b.score.total_cmp(&a.score));
    ranked.truncate(8);

    let mut lines = Vec::with_capacity(ranked.len() + 2);
    lines.push("Memory hints (probabilistic; may be stale):".to_string());
    for s in ranked {
        lines.push(format!(
            "S{} belief={:.2} rel={:.2} sal={:.2} evidence={} first={} last={} text={}",
            s.statement.id,
            s.statement.belief_score,
            s.relevance,
            s.statement.salience,
            s.statement.evidence_count,
            s.statement.first_seen_message_id,
            s.statement.last_seen_message_id,
            s.statement.text
        ));
    }
    lines.push("Use memory to maintain continuity. Do not assume memory is always true; treat it as weighted signal.".to_string());

    Ok(Some(lines.join("\n")))
}

async fn ingest_user_message(
    data_dir: PathBuf,
    user_id: String,
    _conversation_id: String,
    message_id: i64,
) -> anyhow::Result<()> {
    let mgr = db::Manager::new(data_dir.clone());
    let uds = mgr.user_data(&user_id)?;
    let message = uds.message_by_id(message_id)?;
    if message.role != "user" {
        return Ok(());
    }
    if message.content.trim().is_empty() {
        return Ok(());
    }

    let embed_backend = agents::client_for_task_from_disk(&data_dir, "memory_embed")?;
    let msg_embedding = if let Some(existing) = uds.message_embedding(message_id)? {
        existing.embedding
    } else {
        let emb = embed_single_text(
            &embed_backend.client,
            &embed_backend.model,
            &message.content,
        )
        .await
        .context("embed message")?;
        uds.upsert_message_embedding(message_id, &embed_backend.model, &emb)?;
        emb
    };

    let recent_rows = uds
        .list_recent_messages_for_conversation(&message.conversation_id.to_string(), 24)
        .unwrap_or_default();

    let similar_rows = nearest_message_rows(&uds, message_id, &msg_embedding, 8)
        .unwrap_or_default()
        .into_iter()
        .map(|m| format!("{}:{}: {}", m.id, m.role, sanitize_line(&m.content)))
        .collect::<Vec<_>>();

    let known_statements = uds
        .list_memory_statements_with_embeddings(80)
        .unwrap_or_default()
        .into_iter()
        .take(12)
        .map(|(s, _)| {
            format!(
                "S{} belief={:.2} sal={:.2} evidence={} text={}",
                s.id,
                s.belief_score,
                s.salience,
                s.evidence_count,
                sanitize_line(&s.text)
            )
        })
        .collect::<Vec<_>>();

    let recent_text = recent_rows
        .iter()
        .map(|m| format!("{}:{}: {}", m.id, m.role, sanitize_line(&m.content)))
        .collect::<Vec<_>>()
        .join("\n");

    let curator_backend = agents::client_for_task_from_disk(&data_dir, "memory_curator")?;
    let curator_input = format!(
        "New message:\n{}:{}\n\nRecent conversation window:\n{}\n\nSemantically related older messages:\n{}\n\nKnown memory statements:\n{}\n",
        message.id,
        sanitize_line(&message.content),
        recent_text,
        if similar_rows.is_empty() {
            "(none)".to_string()
        } else {
            similar_rows.join("\n")
        },
        if known_statements.is_empty() {
            "(none)".to_string()
        } else {
            known_statements.join("\n")
        }
    );

    let req = openrouter::ChatCompletionRequest {
        model: curator_backend.model.clone(),
        messages: vec![
            openrouter::Message {
                role: "system".to_string(),
                content: CURATOR_SYSTEM_PROMPT.to_string(),
            },
            openrouter::Message {
                role: "user".to_string(),
                content: curator_input,
            },
        ],
        temperature: Some(0.0),
        max_tokens: Some(700),
        stream: false,
    };

    let raw = curator_backend
        .client
        .create_chat_completion(&req)
        .await
        .context("curator completion failed")?
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    let payload = parse_curator_payload(&raw).context("parse curator payload")?;
    if payload.updates.is_empty() {
        info!(user_id=%user_id, message_id=message_id, "memory ingest produced no updates");
        return Ok(());
    }

    let filtered_updates: Vec<&CuratorUpdate> = payload
        .updates
        .iter()
        .filter(|u| !u.statement.trim().is_empty())
        .collect();
    if filtered_updates.is_empty() {
        return Ok(());
    }

    let update_texts: Vec<String> = filtered_updates
        .iter()
        .map(|u| u.statement.trim().to_string())
        .collect();
    let statement_embeddings =
        embed_many_texts(&embed_backend.client, &embed_backend.model, &update_texts)
            .await
            .context("embed statements")?;

    let mut existing = uds.list_memory_statements_with_embeddings(500)?;
    let mut text_to_statement_id: HashMap<String, i64> = HashMap::new();

    for (upd, emb) in filtered_updates.iter().zip(statement_embeddings.iter()) {
        let statement_text = upd.statement.trim();
        if statement_text.is_empty() || emb.is_empty() {
            continue;
        }

        let mut match_id = 0_i64;
        let mut best_sim = 0.0_f64;
        for (rec, vec) in &existing {
            if vec.len() != emb.len() {
                continue;
            }
            let sim = cosine_similarity(emb, vec);
            if sim > best_sim {
                best_sim = sim;
                match_id = rec.id;
            }
        }
        if best_sim < 0.88 {
            match_id = 0;
        }

        let confidence = if upd.confidence <= 0.0 {
            0.6
        } else {
            upd.confidence
        }
        .clamp(0.0, 1.0);
        let salience = if upd.salience <= 0.0 {
            0.5
        } else {
            upd.salience
        }
        .clamp(0.0, 1.0);
        let delta = (upd.delta * confidence).clamp(-3.0, 3.0);

        let sid = uds.apply_memory_patch(db::MemoryPatch {
            statement_id: match_id,
            text: statement_text,
            delta,
            salience,
            confidence,
            message_id,
            note: upd.evidence.trim(),
            embedding: emb,
        })?;
        text_to_statement_id.insert(statement_key(statement_text), sid);

        if match_id == 0 {
            existing.push((
                db::MemoryStatementRecord {
                    id: sid,
                    text: statement_text.to_string(),
                    belief_score: delta,
                    salience,
                    first_seen_message_id: message_id,
                    last_seen_message_id: message_id,
                    evidence_count: 1,
                    created_at: 0,
                    updated_at: 0,
                },
                emb.clone(),
            ));
        } else {
            for (rec, vec) in &mut existing {
                if rec.id == sid {
                    rec.last_seen_message_id = message_id;
                    rec.evidence_count += 1;
                    rec.belief_score += delta;
                    rec.salience = salience;
                    *vec = emb.clone();
                    break;
                }
            }
        }
    }

    for link in payload.links {
        let Some(from_id) = text_to_statement_id
            .get(&statement_key(&link.from))
            .copied()
        else {
            continue;
        };
        let Some(to_id) = text_to_statement_id.get(&statement_key(&link.to)).copied() else {
            continue;
        };
        let kind = normalize_link_kind(&link.kind);
        uds.upsert_memory_link(from_id, to_id, kind, link.strength)?;
    }

    info!(
        user_id = %user_id,
        message_id = message_id,
        updates = text_to_statement_id.len(),
        "memory ingest applied"
    );

    Ok(())
}

fn nearest_message_rows(
    uds: &db::UserDataStore,
    message_id: i64,
    query_embedding: &[f32],
    top_k: usize,
) -> Result<Vec<db::ChatMessageRecord>, db::DbError> {
    let embeddings = uds.list_message_embeddings(1_500)?;
    if embeddings.is_empty() {
        return Ok(vec![]);
    }

    let mut scored: Vec<(i64, f64)> = embeddings
        .into_iter()
        .filter(|e| e.message_id != message_id && e.embedding.len() == query_embedding.len())
        .map(|e| {
            (
                e.message_id,
                cosine_similarity(query_embedding, &e.embedding),
            )
        })
        .filter(|(_, s)| *s > 0.30)
        .collect();

    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(top_k);

    let mut out = Vec::new();
    for (mid, _) in scored {
        if let Ok(row) = uds.message_by_id(mid) {
            out.push(row);
        }
    }
    Ok(out)
}

async fn embed_single_text(
    client: &openrouter::Client,
    model: &str,
    text: &str,
) -> anyhow::Result<Vec<f32>> {
    let req = openrouter::EmbeddingRequest::single(model.to_string(), text.to_string());
    let resp = client.create_embedding(&req).await?;
    let first = resp
        .data
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing embedding"))?;
    if first.embedding.is_empty() {
        return Err(anyhow::anyhow!("empty embedding"));
    }
    Ok(first.embedding.clone())
}

async fn embed_many_texts(
    client: &openrouter::Client,
    model: &str,
    texts: &[String],
) -> anyhow::Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let req = openrouter::EmbeddingRequest {
        model: model.to_string(),
        input: openrouter::EmbeddingInput::Strings(texts.to_vec()),
    };
    let mut resp = client.create_embedding(&req).await?;
    if resp.data.is_empty() {
        return Err(anyhow::anyhow!("embedding response is empty"));
    }
    resp.data.sort_by(|a, b| a.index.cmp(&b.index));
    let out = resp
        .data
        .into_iter()
        .map(|d| d.embedding)
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    if out.len() != texts.len() {
        return Err(anyhow::anyhow!(
            "embedding count mismatch: got {}, expected {}",
            out.len(),
            texts.len()
        ));
    }
    Ok(out)
}

fn parse_curator_payload(raw: &str) -> anyhow::Result<CuratorPayload> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(CuratorPayload::default());
    }
    if let Ok(v) = serde_json::from_str::<CuratorPayload>(t) {
        return Ok(v);
    }

    let start = t
        .find('{')
        .ok_or_else(|| anyhow::anyhow!("json object not found"))?;
    let end = t
        .rfind('}')
        .ok_or_else(|| anyhow::anyhow!("json object not found"))?;
    if end <= start {
        return Err(anyhow::anyhow!("invalid json bounds"));
    }
    let candidate = &t[start..=end];
    Ok(serde_json::from_str::<CuratorPayload>(candidate)?)
}

fn statement_key(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

fn normalize_link_kind(kind: &str) -> &str {
    match kind.trim().to_ascii_lowercase().as_str() {
        "supports" => "supports",
        "conflicts" => "conflicts",
        _ => "related",
    }
}

fn sanitize_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ").trim().to_string()
}

fn belief_strength(score: f64) -> f64 {
    // Smoothly compress to [0, 1] so large repeated evidence does not dominate relevance.
    ((score.tanh()) + 1.0) * 0.5
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0_f64;
    let mut na = 0.0_f64;
    let mut nb = 0.0_f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let xf = f64::from(*x);
        let yf = f64::from(*y);
        dot += xf * yf;
        na += xf * xf;
        nb += yf * yf;
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_curator_payload_accepts_wrapped_json() {
        let raw = "Here is output:\n{\"updates\":[{\"statement\":\"likes beer\",\"delta\":0.6,\"salience\":0.8,\"confidence\":0.7}],\"links\":[]}";
        let p = parse_curator_payload(raw).unwrap();
        assert_eq!(p.updates.len(), 1);
        assert_eq!(p.updates[0].statement, "likes beer");
    }

    #[test]
    fn cosine_similarity_handles_basic_cases() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) > 0.999);
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]) < 0.001);
    }
}
