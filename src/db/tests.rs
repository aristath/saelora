use super::*;

fn new_mgr() -> (tempfile::TempDir, Manager) {
    let td = tempfile::tempdir().expect("tempdir");
    let mgr = Manager::new(td.path().to_path_buf());
    (td, mgr)
}

#[test]
fn looks_like_email_rejects_obvious_garbage() {
    assert!(looks_like_email("a@b.co"));
    assert!(looks_like_email("Test.User+tag@example.com"));

    assert!(!looks_like_email(""));
    assert!(!looks_like_email("   "));
    assert!(!looks_like_email("no-at-sign.example.com"));
    assert!(!looks_like_email("@example.com"));
    assert!(!looks_like_email("a@"));
    assert!(!looks_like_email("a@example")); // no dot
    assert!(!looks_like_email("a@exa mple.com"));
    // Leading/trailing whitespace should be tolerated (we trim user input).
    assert!(looks_like_email("a@example.com\n"));
    assert!(!looks_like_email("a@example.\ncom"));
}

#[test]
fn waitlist_add_is_case_insensitive_and_dedupes() {
    let (_td, mgr) = new_mgr();

    mgr.waitlist_add("Test@Example.com").unwrap();
    mgr.waitlist_add("test@example.com").unwrap();
    mgr.waitlist_add("TEST@EXAMPLE.COM").unwrap();

    assert_eq!(mgr.waitlist_count().unwrap(), 1);
    assert!(mgr.waitlist_has("test@example.com").unwrap());
    assert!(mgr.waitlist_has("TEST@example.com").unwrap());
}

#[test]
fn whitelist_add_remove_case_insensitive_and_dedupes() {
    let (_td, mgr) = new_mgr();

    mgr.whitelist_add("A@Example.com").unwrap();
    mgr.whitelist_add("a@example.com").unwrap();
    assert_eq!(mgr.whitelist_count().unwrap(), 1);
    assert!(mgr.whitelist_has("a@example.com").unwrap());

    mgr.whitelist_remove("A@EXAMPLE.COM").unwrap();
    assert_eq!(mgr.whitelist_count().unwrap(), 0);
    assert!(!mgr.whitelist_has("a@example.com").unwrap());
}

#[test]
fn create_user_and_verify_login_are_case_insensitive() {
    let (_td, mgr) = new_mgr();
    let us = mgr.users().unwrap();

    let ph = hash_password("password123").unwrap();
    let _uid = us.create_user("Test@Example.com", &ph, "active").unwrap();

    // Login should work regardless of email case.
    assert!(us.verify_login("test@example.com", "password123").is_ok());
    assert!(us.verify_login("TEST@EXAMPLE.COM", "password123").is_ok());

    // Wrong password should not authenticate.
    assert!(matches!(
        us.verify_login("test@example.com", "wrong"),
        Err(DbError::Unauthorized)
    ));
}

#[test]
fn disabled_user_cannot_login_or_receive_magic_tokens() {
    let (_td, mgr) = new_mgr();
    let us = mgr.users().unwrap();

    let ph = hash_password("password123").unwrap();
    let _uid = us
        .create_user("disabled@example.com", &ph, "disabled")
        .unwrap();

    assert!(matches!(
        us.verify_login("disabled@example.com", "password123"),
        Err(DbError::Unauthorized)
    ));

    // DB-level protection: refuse tokens for disabled accounts.
    assert!(matches!(
        us.create_magic_token("disabled@example.com", 3600),
        Err(DbError::Unauthorized)
    ));
}

#[test]
fn magic_token_requires_allow_create_user_for_new_accounts_and_is_one_time() {
    let (_td, mgr) = new_mgr();
    let us = mgr.users().unwrap();

    let tok = us.create_magic_token("new@example.com", 3600).unwrap();
    assert_eq!(us.peek_magic_token_email(&tok).unwrap(), "new@example.com");

    let ph = hash_password("password123").unwrap();

    // If creation isn't allowed, a new-account token should NOT create a user and should remain usable.
    assert!(matches!(
        us.consume_magic_token_set_password(&tok, &ph, false),
        Err(DbError::TokenInvalid)
    ));
    assert_eq!(us.peek_magic_token_email(&tok).unwrap(), "new@example.com");
    assert_eq!(us.count_users().unwrap(), 0);

    // Allow creation: should create user and consume the token.
    let uid = us
        .consume_magic_token_set_password(&tok, &ph, true)
        .unwrap();
    assert!(!uid.trim().is_empty());
    assert_eq!(us.count_users().unwrap(), 1);
    assert!(us.verify_login("new@example.com", "password123").is_ok());
    assert!(matches!(
        us.peek_magic_token_email(&tok),
        Err(DbError::TokenInvalid)
    ));
    assert!(matches!(
        us.consume_magic_token_set_password(&tok, &ph, true),
        Err(DbError::TokenInvalid)
    ));
}

#[test]
fn magic_token_expiry_is_enforced() {
    let (_td, mgr) = new_mgr();
    let us = mgr.users().unwrap();

    let tok = us.create_magic_token("exp@example.com", 3600).unwrap();
    us.test_force_expire_magic_token(&tok).unwrap();

    assert!(matches!(
        us.peek_magic_token_email(&tok),
        Err(DbError::TokenInvalid)
    ));
    let ph = hash_password("password123").unwrap();
    assert!(matches!(
        us.consume_magic_token_set_password(&tok, &ph, true),
        Err(DbError::TokenInvalid)
    ));
}

#[test]
fn session_tokens_revoke_and_expire_and_require_active_user() {
    let (_td, mgr) = new_mgr();
    let us = mgr.users().unwrap();

    let ph = hash_password("password123").unwrap();
    let uid = us.create_user("u@example.com", &ph, "active").unwrap();

    let tok = us.create_session_token(&uid, 3600).unwrap();
    assert!(us.auth_user_from_token(&tok).is_ok());

    us.revoke_session(&tok).unwrap();
    assert!(matches!(
        us.auth_user_from_token(&tok),
        Err(DbError::Unauthorized)
    ));

    // Expired tokens should not authenticate.
    let tok2 = us.create_session_token(&uid, 3600).unwrap();
    us.test_force_expire_session_token(&tok2).unwrap();
    assert!(matches!(
        us.auth_user_from_token(&tok2),
        Err(DbError::Unauthorized)
    ));

    // Disabled users should not authenticate even with an unexpired token.
    let tok3 = us.create_session_token(&uid, 3600).unwrap();
    us.set_user_status(&uid, "disabled").unwrap();
    assert!(matches!(
        us.auth_user_from_token(&tok3),
        Err(DbError::Unauthorized)
    ));
}
