use crate::openrouter;

pub(super) fn filter_client_messages(in_msgs: &[openrouter::Message]) -> Vec<openrouter::Message> {
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

pub(super) fn role_counts(msgs: &[openrouter::Message]) -> String {
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

pub(super) fn total_chars(msgs: &[openrouter::Message]) -> usize {
    msgs.iter().map(|m| m.content.len()).sum()
}
