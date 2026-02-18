use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;

use super::{errors, AppState};

#[derive(Debug, Clone, Copy)]
struct Rule {
    max_attempts: u32,
    window_ms: i64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Bucket {
    window_start_ms: i64,
    attempts: u32,
    last_seen_ms: i64,
}

pub(super) async fn auth_rate_limit(
    State(st): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let Some(rule) = rule_for_path(req.uri().path()) else {
        return next.run(req).await;
    };

    let now = now_ms();
    let key = format!("{}|{}", req.uri().path(), client_fingerprint(&req));
    let mut guard = st.auth_rate_limits.lock().await;
    if guard.len() > 4096 {
        prune_stale(&mut guard, now);
    }

    if !allow_attempt(&mut guard, &key, now, rule) {
        return errors::auth_error(
            StatusCode::TOO_MANY_REQUESTS,
            "too many attempts, try again later",
        );
    }
    drop(guard);
    next.run(req).await
}

fn allow_attempt(
    buckets: &mut HashMap<String, Bucket>,
    key: &str,
    now_ms: i64,
    rule: Rule,
) -> bool {
    let entry = buckets.entry(key.to_string()).or_insert(Bucket {
        window_start_ms: now_ms,
        attempts: 0,
        last_seen_ms: now_ms,
    });

    if now_ms.saturating_sub(entry.window_start_ms) >= rule.window_ms {
        entry.window_start_ms = now_ms;
        entry.attempts = 0;
    }

    entry.last_seen_ms = now_ms;
    entry.attempts = entry.attempts.saturating_add(1);
    entry.attempts <= rule.max_attempts
}

fn prune_stale(buckets: &mut HashMap<String, Bucket>, now_ms: i64) {
    // Keep at most recent six hours of keys.
    let max_age = 6 * 60 * 60 * 1000_i64;
    buckets.retain(|_, b| now_ms.saturating_sub(b.last_seen_ms) <= max_age);
}

fn rule_for_path(path: &str) -> Option<Rule> {
    match path {
        "/v1/auth/login" => Some(Rule {
            max_attempts: 24,
            window_ms: 5 * 60 * 1000,
        }),
        "/v1/auth/request-password-link" => Some(Rule {
            max_attempts: 12,
            window_ms: 15 * 60 * 1000,
        }),
        "/v1/auth/setup" => Some(Rule {
            max_attempts: 24,
            window_ms: 10 * 60 * 1000,
        }),
        "/v1/auth/register" => Some(Rule {
            max_attempts: 12,
            window_ms: 10 * 60 * 1000,
        }),
        "/invite" => Some(Rule {
            max_attempts: 20,
            window_ms: 60 * 60 * 1000,
        }),
        _ => None,
    }
}

fn client_fingerprint(req: &Request) -> String {
    if let Some(v) = req.headers().get("x-forwarded-for") {
        if let Ok(s) = v.to_str() {
            if let Some(first) = s.split(',').next() {
                let ip = first.trim();
                if !ip.is_empty() {
                    return ip.to_string();
                }
            }
        }
    }
    if let Some(v) = req.headers().get("x-real-ip") {
        if let Ok(s) = v.to_str() {
            let ip = s.trim();
            if !ip.is_empty() {
                return ip.to_string();
            }
        }
    }
    "unknown-client".to_string()
}

fn now_ms() -> i64 {
    let Ok(d) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return 0;
    };
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_attempt_blocks_after_limit_and_resets_after_window() {
        let mut buckets = HashMap::<String, Bucket>::new();
        let key = "k";
        let rule = Rule {
            max_attempts: 2,
            window_ms: 1000,
        };
        assert!(allow_attempt(&mut buckets, key, 100, rule));
        assert!(allow_attempt(&mut buckets, key, 200, rule));
        assert!(!allow_attempt(&mut buckets, key, 300, rule));
        assert!(allow_attempt(&mut buckets, key, 1200, rule));
    }

    #[test]
    fn rule_is_defined_for_auth_paths() {
        assert!(rule_for_path("/v1/auth/login").is_some());
        assert!(rule_for_path("/v1/auth/request-password-link").is_some());
        assert!(rule_for_path("/v1/auth/setup").is_some());
        assert!(rule_for_path("/invite").is_some());
        assert!(rule_for_path("/v1/chat/completions").is_none());
    }
}
