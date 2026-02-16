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

#[test]
fn per_user_databases_are_isolated_and_persist_messages() {
    let (_td, mgr) = new_mgr();

    let user1 = "user_1";
    let user2 = "user_2";

    let uds1 = mgr.user_data(user1).unwrap();
    uds1.append_user_message("hello").unwrap();
    uds1.append_saelora_message("hi there").unwrap();
    // Blank content should be ignored.
    uds1.append_user_message("   ").unwrap();

    let uds2 = mgr.user_data(user2).unwrap();
    uds2.append_user_message("other").unwrap();

    // Re-open to verify data is actually persisted on disk.
    let uds1_reopen = mgr.user_data(user1).unwrap();
    let m1 = uds1_reopen.list_messages(100).unwrap();
    assert_eq!(m1.len(), 2);
    assert_eq!(m1[0].role, "user");
    assert_eq!(m1[0].content, "hello");
    assert_eq!(m1[1].role, "saelora");
    assert_eq!(m1[1].content, "hi there");

    let uds2_reopen = mgr.user_data(user2).unwrap();
    let m2 = uds2_reopen.list_messages(100).unwrap();
    assert_eq!(m2.len(), 1);
    assert_eq!(m2[0].role, "user");
    assert_eq!(m2[0].content, "other");

    let p1 = mgr.user_data_db_path(user1).unwrap();
    let p2 = mgr.user_data_db_path(user2).unwrap();
    assert!(p1.exists());
    assert!(p2.exists());
    assert_ne!(p1, p2);
}

#[test]
fn conversations_can_be_created_renamed_archived_and_have_scoped_history() {
    let (_td, mgr) = new_mgr();
    let uds = mgr.user_data("user_threads").unwrap();

    let c1 = uds
        .create_conversation_with_mode("First", "hourly")
        .unwrap();
    let c2 = uds.create_conversation("Second").unwrap();
    assert_ne!(c1.id, c2.id);
    assert_eq!(c1.mode, "hourly");
    assert_eq!(c2.mode, "instant");

    uds.append_user_message_in(&c1.id.to_string(), "hello one")
        .unwrap();
    uds.append_saelora_message_in(&c1.id.to_string(), "reply one")
        .unwrap();
    uds.append_user_message_in(&c2.id.to_string(), "hello two")
        .unwrap();

    let m1 = uds.list_messages_in(&c1.id.to_string(), 100).unwrap();
    assert_eq!(m1.len(), 2);
    assert_eq!(m1[0].content, "hello one");
    assert_eq!(m1[1].content, "reply one");

    let m2 = uds.list_messages_in(&c2.id.to_string(), 100).unwrap();
    assert_eq!(m2.len(), 1);
    assert_eq!(m2[0].content, "hello two");

    uds.rename_conversation(&c1.id.to_string(), "Renamed")
        .unwrap();
    uds.set_conversation_mode(&c1.id.to_string(), "daily")
        .unwrap();
    assert_eq!(uds.conversation_mode(&c1.id.to_string()).unwrap(), "daily");
    let listed = uds.list_conversations(false, 100).unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed
        .iter()
        .any(|c| c.id == c1.id && c.title == "Renamed" && c.mode == "daily"));

    uds.archive_conversation(&c1.id.to_string()).unwrap();
    let listed2 = uds.list_conversations(false, 100).unwrap();
    assert_eq!(listed2.len(), 1);
    assert_eq!(listed2[0].id, c2.id);
}
