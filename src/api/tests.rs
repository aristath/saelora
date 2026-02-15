use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use http_body_util::BodyExt as _;
use tokio::sync::Mutex;

use crate::config;
use crate::db;

use super::auth;
use super::chat;
use super::cors;
use super::AppState;

fn test_state(data_dir: &Path) -> AppState {
    AppState {
        data_dir: data_dir.to_path_buf(),
        last_key_info_log: Arc::new(Mutex::new(Instant::now() - Duration::from_secs(3600))),
    }
}

async fn resp_json(resp: Response) -> serde_json::Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

fn bearer_headers(tok: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        header::AUTHORIZATION,
        header::HeaderValue::from_str(&format!("Bearer {}", tok)).unwrap(),
    );
    h
}

async fn register_whitelisted(st: AppState, email: &str, password: &str) -> (db::Manager, String) {
    let mgr = db::Manager::new(st.data_dir.clone());
    mgr.whitelist_add(email).unwrap();
    let body = Bytes::from(serde_json::json!({"email": email, "password": password}).to_string());
    let resp = auth::auth_register(State(st), body).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v = resp_json(resp).await;
    let tok = v.get("token").and_then(|t| t.as_str()).unwrap().to_string();
    assert!(!tok.trim().is_empty());
    (mgr, tok)
}

#[tokio::test]
async fn auth_me_and_logout_require_token() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);

    assert_eq!(
        auth::auth_me(State(st.clone()), HeaderMap::new())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        auth::auth_logout(State(st.clone()), HeaderMap::new())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[test]
fn bearer_token_parsing_is_case_insensitive_and_trims() {
    let mut h = HeaderMap::new();
    h.insert(
        header::AUTHORIZATION,
        header::HeaderValue::from_static("bEaReR   abc123  "),
    );
    assert_eq!(auth::bearer_token(&h).as_deref(), Some("abc123"));

    let mut h2 = HeaderMap::new();
    h2.insert(
        header::AUTHORIZATION,
        header::HeaderValue::from_static("Token abc123"),
    );
    assert!(auth::bearer_token(&h2).is_none());
}

#[tokio::test]
async fn request_password_link_ok_does_not_leak_url_or_create_user() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);

    let mgr = db::Manager::new(data_dir.clone());
    let us = mgr.users().unwrap();
    assert_eq!(us.count_users().unwrap(), 0);

    let body = Bytes::from(r#"{"email":"someone@example.com"}"#);
    let resp = auth::auth_request_password_link(State(st.clone()), body).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v = resp_json(resp).await;
    assert_eq!(v.get("ok").and_then(|b| b.as_bool()), Some(true));
    assert!(v.get("url").is_none());

    let us2 = mgr.users().unwrap();
    assert_eq!(us2.count_users().unwrap(), 0);
    assert_eq!(us2.count_magic_tokens().unwrap(), 0);
}

#[tokio::test]
async fn invite_accepts_json_and_form_and_dedupes() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);
    let mgr = db::Manager::new(data_dir.clone());

    let mut h1 = HeaderMap::new();
    h1.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    let resp1 = auth::invite(
        State(st.clone()),
        h1,
        Bytes::from(r#"{"email":"Test@Example.com"}"#),
    )
    .await;
    assert_eq!(resp1.status(), StatusCode::OK);
    assert_eq!(
        resp_json(resp1).await.get("ok").and_then(|b| b.as_bool()),
        Some(true)
    );

    let mut h2 = HeaderMap::new();
    h2.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    let resp2 = auth::invite(
        State(st.clone()),
        h2,
        Bytes::from("email=test%40example.com"),
    )
    .await;
    assert_eq!(resp2.status(), StatusCode::OK);

    assert_eq!(mgr.waitlist_count().unwrap(), 1);
    assert!(mgr.waitlist_has("test@example.com").unwrap());
}

#[tokio::test]
async fn invite_rejects_invalid_email() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);

    let mut h = HeaderMap::new();
    h.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    let resp = auth::invite(State(st.clone()), h, Bytes::from(r#"{"email":"nope"}"#)).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let v = resp_json(resp).await;
    assert_eq!(
        v.get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str()),
        Some("invalid email")
    );
}

#[tokio::test]
async fn register_requires_whitelist_and_reports_pending_vs_not_found() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);
    let mgr = db::Manager::new(data_dir.clone());

    let email = "r@example.com";
    let body =
        Bytes::from(serde_json::json!({"email": email, "password": "password123"}).to_string());
    let resp = auth::auth_register(State(st.clone()), body).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let v = resp_json(resp).await;
    assert_eq!(
        v.get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str()),
        Some("invite required")
    );

    mgr.waitlist_add(email).unwrap();
    let body2 =
        Bytes::from(serde_json::json!({"email": email, "password": "password123"}).to_string());
    let resp2 = auth::auth_register(State(st.clone()), body2).await;
    assert_eq!(resp2.status(), StatusCode::FORBIDDEN);
    let v2 = resp_json(resp2).await;
    assert_eq!(
        v2.get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str()),
        Some("you are on the waitlist")
    );
}

#[tokio::test]
async fn register_validates_email_and_password() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);

    // Invalid JSON.
    let resp1 = auth::auth_register(State(st.clone()), Bytes::from("{")).await;
    assert_eq!(resp1.status(), StatusCode::BAD_REQUEST);

    // Invalid email.
    let resp2 = auth::auth_register(
        State(st.clone()),
        Bytes::from(r#"{"email":"nope","password":"password123"}"#),
    )
    .await;
    assert_eq!(resp2.status(), StatusCode::BAD_REQUEST);

    // Short password.
    let resp3 = auth::auth_register(
        State(st.clone()),
        Bytes::from(r#"{"email":"a@example.com","password":"short"}"#),
    )
    .await;
    assert_eq!(resp3.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn register_creates_user_removes_lists_and_me_works() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);
    let mgr = db::Manager::new(data_dir.clone());

    let email = "Test@Example.com";
    mgr.waitlist_add(email).unwrap();
    mgr.whitelist_add(email).unwrap();

    let body = Bytes::from(
        serde_json::json!({"email": "test@example.com", "password": "password123"}).to_string(),
    );
    let resp = auth::auth_register(State(st.clone()), body).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v = resp_json(resp).await;
    let tok = v.get("token").and_then(|t| t.as_str()).unwrap().to_string();
    assert!(!tok.trim().is_empty());

    // Lists should be cleaned up.
    assert!(!mgr.waitlist_has("test@example.com").unwrap());
    assert!(!mgr.whitelist_has("test@example.com").unwrap());

    // /me should work with the returned token.
    let resp_me = auth::auth_me(State(st.clone()), bearer_headers(&tok)).await;
    assert_eq!(resp_me.status(), StatusCode::OK);
    let me = resp_json(resp_me).await;
    assert_eq!(
        me.get("email").and_then(|m| m.as_str()),
        Some("test@example.com")
    );
    assert_eq!(me.get("status").and_then(|m| m.as_str()), Some("active"));
}

#[tokio::test]
async fn register_conflicts_when_account_exists() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);
    let email = "exists@example.com";

    let (_mgr, _tok) = register_whitelisted(st.clone(), email, "password123").await;

    // Second register should conflict even if re-whitelisted.
    let mgr2 = db::Manager::new(data_dir.clone());
    mgr2.whitelist_add(email).unwrap();
    let body =
        Bytes::from(serde_json::json!({"email": email, "password": "password123"}).to_string());
    let resp2 = auth::auth_register(State(st.clone()), body).await;
    assert_eq!(resp2.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn login_and_logout_flow_is_consistent() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);
    let email = "login@example.com";

    let (_mgr, _tok0) = register_whitelisted(st.clone(), email, "password123").await;

    // Wrong password.
    let body_wrong =
        Bytes::from(serde_json::json!({"email": email, "password": "wrong"}).to_string());
    let resp_wrong = auth::auth_login(State(st.clone()), body_wrong).await;
    assert_eq!(resp_wrong.status(), StatusCode::UNAUTHORIZED);

    // Unknown email.
    let body_unknown = Bytes::from(
        serde_json::json!({"email": "unknown@example.com", "password": "password123"}).to_string(),
    );
    let resp_unknown = auth::auth_login(State(st.clone()), body_unknown).await;
    assert_eq!(resp_unknown.status(), StatusCode::UNAUTHORIZED);

    // Correct login.
    let body_ok =
        Bytes::from(serde_json::json!({"email": email, "password": "password123"}).to_string());
    let resp_ok = auth::auth_login(State(st.clone()), body_ok).await;
    assert_eq!(resp_ok.status(), StatusCode::OK);
    let v = resp_json(resp_ok).await;
    let tok = v.get("token").and_then(|t| t.as_str()).unwrap().to_string();

    // /me works.
    assert_eq!(
        auth::auth_me(State(st.clone()), bearer_headers(&tok))
            .await
            .status(),
        StatusCode::OK
    );

    // Logout revokes token.
    let resp_lo = auth::auth_logout(State(st.clone()), bearer_headers(&tok)).await;
    assert_eq!(resp_lo.status(), StatusCode::OK);
    assert_eq!(
        auth::auth_me(State(st.clone()), bearer_headers(&tok))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn request_password_link_validates_email_and_does_not_issue_for_disabled_or_no_password() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);

    // Invalid email is rejected (client error).
    let resp1 =
        auth::auth_request_password_link(State(st.clone()), Bytes::from(r#"{"email":""}"#)).await;
    assert_eq!(resp1.status(), StatusCode::BAD_REQUEST);

    let mgr = db::Manager::new(data_dir.clone());
    let us = mgr.users().unwrap();

    // Active user with an empty password hash is not eligible.
    let uid1 = us.create_user("nopw@example.com", "", "active").unwrap();
    assert!(!uid1.is_empty());

    let resp2 = auth::auth_request_password_link(
        State(st.clone()),
        Bytes::from(r#"{"email":"nopw@example.com"}"#),
    )
    .await;
    assert_eq!(resp2.status(), StatusCode::OK);
    assert_eq!(mgr.users().unwrap().count_magic_tokens().unwrap(), 0);

    // Disabled user is not eligible.
    let _uid2 = us
        .create_user("disabled@example.com", "x", "disabled")
        .unwrap();
    let resp3 = auth::auth_request_password_link(
        State(st.clone()),
        Bytes::from(r#"{"email":"disabled@example.com"}"#),
    )
    .await;
    assert_eq!(resp3.status(), StatusCode::OK);
    assert_eq!(mgr.users().unwrap().count_magic_tokens().unwrap(), 0);
}

#[tokio::test]
async fn request_password_link_for_existing_active_user_creates_token() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);

    let mgr = db::Manager::new(data_dir.clone());
    let us = mgr.users().unwrap();
    let ph = db::hash_password("password123").unwrap();
    let _uid = us.create_user("active@example.com", &ph, "active").unwrap();
    assert_eq!(us.count_users().unwrap(), 1);
    assert_eq!(us.count_magic_tokens().unwrap(), 0);

    let resp = auth::auth_request_password_link(
        State(st.clone()),
        Bytes::from(r#"{"email":"active@example.com"}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v = resp_json(resp).await;
    assert_eq!(v.get("ok").and_then(|b| b.as_bool()), Some(true));
    assert!(v.get("url").is_none());

    let us2 = mgr.users().unwrap();
    assert_eq!(us2.count_users().unwrap(), 1);
    assert_eq!(us2.count_magic_tokens().unwrap(), 1);
}

#[tokio::test]
async fn setup_for_existing_user_resets_password_and_consumes_token() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);
    let email = "reset@example.com";

    let (mgr, _tok0) = register_whitelisted(st.clone(), email, "password123").await;
    let us = mgr.users().unwrap();

    let tok = us.create_magic_token(email, 3600).unwrap();
    let resp = auth::auth_setup(
        State(st.clone()),
        Bytes::from(
            serde_json::json!({"token": tok.clone(), "password":"newpassword123"}).to_string(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v = resp_json(resp).await;
    assert_eq!(v.get("ok").and_then(|b| b.as_bool()), Some(true));
    assert!(v.get("token").and_then(|t| t.as_str()).unwrap().len() > 10);

    // Token is one-time.
    let resp2 = auth::auth_setup(
        State(st.clone()),
        Bytes::from(serde_json::json!({"token": tok, "password":"newpassword123"}).to_string()),
    )
    .await;
    assert_eq!(resp2.status(), StatusCode::BAD_REQUEST);

    // Old password no longer works; new password works.
    let resp_old = auth::auth_login(
        State(st.clone()),
        Bytes::from(serde_json::json!({"email": email, "password":"password123"}).to_string()),
    )
    .await;
    assert_eq!(resp_old.status(), StatusCode::UNAUTHORIZED);

    let resp_new = auth::auth_login(
        State(st.clone()),
        Bytes::from(serde_json::json!({"email": email, "password":"newpassword123"}).to_string()),
    )
    .await;
    assert_eq!(resp_new.status(), StatusCode::OK);
}

#[tokio::test]
async fn chat_requires_auth_and_errors_before_touching_backend() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);

    let resp = chat::chat_completions(
        State(st.clone()),
        HeaderMap::new(),
        Bytes::from(r#"{"messages":[{"role":"user","content":"hi"}]}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let v = resp_json(resp).await;
    assert_eq!(
        v.get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str()),
        Some("unauthorized")
    );

    // Authorized: invalid json should 400 without hitting the upstream.
    let (_mgr, tok) = register_whitelisted(st.clone(), "chat@example.com", "password123").await;
    let resp2 =
        chat::chat_completions(State(st.clone()), bearer_headers(&tok), Bytes::from("{")).await;
    assert_eq!(resp2.status(), StatusCode::BAD_REQUEST);
    let v2 = resp_json(resp2).await;
    assert_eq!(
        v2.get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str()),
        Some("invalid json")
    );

    // Authorized: messages filtered away should reject early.
    let body3 = Bytes::from(
        serde_json::json!({
            "model": "anything",
            "messages": [{"role":"system","content":"x"},{"role":"user","content":"   "}]
        })
        .to_string(),
    );
    let resp3 = chat::chat_completions(State(st.clone()), bearer_headers(&tok), body3).await;
    assert_eq!(resp3.status(), StatusCode::BAD_REQUEST);
    let v3 = resp_json(resp3).await;
    assert_eq!(
        v3.get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str()),
        Some("no usable messages")
    );
}

#[tokio::test]
async fn request_password_link_for_whitelisted_email_creates_token_but_no_user() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);

    let mgr = db::Manager::new(data_dir.clone());
    mgr.whitelist_add("whitelisted@example.com").unwrap();

    let body = Bytes::from(r#"{"email":"whitelisted@example.com"}"#);
    let resp = auth::auth_request_password_link(State(st.clone()), body).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v = resp_json(resp).await;
    assert_eq!(v.get("ok").and_then(|b| b.as_bool()), Some(true));
    assert!(v.get("url").is_none());

    let us = mgr.users().unwrap();
    assert_eq!(us.count_users().unwrap(), 0);
    assert_eq!(us.count_magic_tokens().unwrap(), 1);
}

#[tokio::test]
async fn setup_requires_whitelist_for_new_accounts_and_consumes_token_only_on_success() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();
    let st = test_state(&data_dir);

    let mgr = db::Manager::new(data_dir.clone());
    let us = mgr.users().unwrap();
    let tok = us.create_magic_token("newuser@example.com", 3600).unwrap();

    // Not whitelisted -> should fail and not consume the token.
    let body1 = Bytes::from(
        serde_json::json!({"token": tok.clone(), "password": "password123"}).to_string(),
    );
    let resp1 = auth::auth_setup(State(st.clone()), body1).await;
    assert_eq!(resp1.status(), StatusCode::BAD_REQUEST);
    assert_eq!(mgr.users().unwrap().count_users().unwrap(), 0);
    assert_eq!(mgr.users().unwrap().count_magic_tokens().unwrap(), 1);

    // Whitelist -> should succeed using the same token.
    mgr.whitelist_add("newuser@example.com").unwrap();
    let body2 =
        Bytes::from(serde_json::json!({"token": tok, "password": "password123"}).to_string());
    let resp2 = auth::auth_setup(State(st.clone()), body2).await;
    assert_eq!(resp2.status(), StatusCode::OK);
    assert_eq!(mgr.users().unwrap().count_users().unwrap(), 1);
    assert!(!mgr.whitelist_has("newuser@example.com").unwrap());
}

#[tokio::test]
async fn cors_allows_localhost_and_public_base_only() {
    let td = tempfile::tempdir().unwrap();
    let data_dir = td.path().to_path_buf();

    let mut s = config::default_settings();
    s.public_base = "saelora.ai".to_string();
    config::save_settings(&config::settings_path(&data_dir), &s).unwrap();

    assert!(cors::is_allowed_origin("http://localhost:5173", &data_dir));
    assert!(cors::is_allowed_origin("http://127.0.0.1:5173", &data_dir));
    assert!(cors::is_allowed_origin("https://saelora.ai", &data_dir));
    assert!(!cors::is_allowed_origin("https://evil.example", &data_dir));
}
