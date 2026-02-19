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
fn earliest_active_user_is_admin() {
    let (_td, mgr) = new_mgr();
    let us = mgr.users().unwrap();

    let ph = hash_password("password123").unwrap();
    let admin_id = us.create_user("admin@example.com", &ph, "active").unwrap();
    let other_id = us.create_user("other@example.com", &ph, "active").unwrap();

    assert!(us.is_admin_user(&admin_id).unwrap());
    assert!(!us.is_admin_user(&other_id).unwrap());

    us.set_user_status(&admin_id, "disabled").unwrap();
    assert!(!us.is_admin_user(&admin_id).unwrap());
    assert!(us.is_admin_user(&other_id).unwrap());
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

#[test]
fn message_embeddings_roundtrip_and_nearest_lookup() {
    let (_td, mgr) = new_mgr();
    let uds = mgr.user_data("embedding_user").unwrap();

    let m1 = uds
        .append_user_message("I slipped and hurt my foot")
        .unwrap();
    let m2 = uds.append_user_message("Foot still hurts today").unwrap();
    assert!(m1 > 0);
    assert!(m2 > m1);

    assert!(matches!(
        uds.upsert_message_embedding(m1, "embed-model", &[]),
        Err(DbError::InvalidData(_))
    ));

    uds.upsert_message_embedding(m1, "embed-model", &[0.1, 0.2, 0.3])
        .unwrap();
    uds.upsert_message_embedding(m2, "embed-model", &[0.12, 0.18, 0.29])
        .unwrap();
    let emb = uds
        .message_embedding(m1)
        .unwrap()
        .expect("embedding present");
    assert_eq!(emb.embedding.len(), 3);
    let nearest_msgs = uds
        .nearest_messages_by_embedding("embed-model", &[0.1, 0.2, 0.31], m1, Some(1), None, 4, 0.0)
        .unwrap();
    assert!(!nearest_msgs.is_empty());
    assert_eq!(nearest_msgs[0].id, m2);
}

#[test]
fn message_memory_ingest_backfill_is_oldest_first() {
    let (_td, mgr) = new_mgr();
    let uds = mgr.user_data("memory_ingest").unwrap();

    let m1 = uds.append_user_message("first").unwrap();
    let _m2 = uds.append_saelora_message("reply").unwrap();
    let m3 = uds.append_user_message("third").unwrap();
    let m4 = uds.append_user_message("fourth").unwrap();

    let next1 = uds.next_backfill_message().unwrap().unwrap();
    assert_eq!(next1.0, m1);

    uds.queue_message_ingest(m1).unwrap();
    assert!(uds.claim_message_ingest(m1).unwrap());
    assert!(!uds.claim_message_ingest(m1).unwrap());
    uds.complete_message_ingest(m1).unwrap();
    uds.upsert_message_embedding(m1, "embed-model", &[0.1, 0.2, 0.3])
        .unwrap();

    let next2 = uds.next_backfill_message().unwrap().unwrap();
    assert_eq!(next2.0, m3);

    // Failed rows remain selected up to attempt 9.
    for _ in 0..9 {
        assert!(uds.claim_message_ingest(m3).unwrap());
        uds.fail_message_ingest(m3, "transient").unwrap();
        let next = uds.next_backfill_message().unwrap().unwrap();
        assert_eq!(next.0, m3);
    }

    // 10th failure hits the cap, so backfill moves on.
    assert!(uds.claim_message_ingest(m3).unwrap());
    uds.fail_message_ingest(m3, "transient").unwrap();
    let next = uds.next_backfill_message().unwrap().unwrap();
    assert_eq!(next.0, m4);
}

#[test]
fn done_rows_without_embedding_are_reprocessed() {
    let (_td, mgr) = new_mgr();
    let uds = mgr.user_data("memory_done_reprocess").unwrap();

    let m1 = uds.append_user_message("first").unwrap();
    uds.queue_message_ingest(m1).unwrap();
    assert!(uds.claim_message_ingest(m1).unwrap());
    uds.complete_message_ingest(m1).unwrap();

    // A done row without an embedding should be picked again.
    let next = uds.next_backfill_message().unwrap().unwrap();
    assert_eq!(next.0, m1);
    assert!(uds.claim_message_ingest(m1).unwrap());
    uds.complete_message_ingest(m1).unwrap();

    // Once an embedding exists, row should no longer be picked.
    uds.upsert_message_embedding(m1, "embed-model", &[0.1, 0.2, 0.3])
        .unwrap();

    assert!(uds.next_backfill_message().unwrap().is_none());
}

#[test]
fn backfill_embedding_batch_candidates_skip_embedded_and_capped_failures() {
    let (_td, mgr) = new_mgr();
    let uds = mgr.user_data("memory_batch").unwrap();

    let m1 = uds.append_user_message("one").unwrap();
    let m2 = uds.append_user_message("two").unwrap();
    let m3 = uds.append_saelora_message("not user").unwrap();
    let m4 = uds.append_user_message("four").unwrap();
    assert!(m3 > 0);

    // Already embedded rows should not be returned for backfill embedding batches.
    uds.upsert_message_embedding(m2, "embed-model", &[0.2, 0.3, 0.4])
        .unwrap();

    // Exhaust retries for m4 so it is no longer eligible.
    for _ in 0..10 {
        assert!(uds.claim_message_ingest(m4).unwrap());
        uds.fail_message_ingest(m4, "transient").unwrap();
    }

    let batch = uds
        .list_backfill_messages_without_embedding_from(m1, 10)
        .unwrap();
    let ids = batch.into_iter().map(|m| m.id).collect::<Vec<_>>();
    assert_eq!(ids, vec![m1]);
}

#[test]
fn running_ingest_rows_are_recoverable_after_restart() {
    let (_td, mgr) = new_mgr();
    let uds = mgr.user_data("memory_recovery").unwrap();

    let m1 = uds.append_user_message("recover me").unwrap();
    uds.queue_message_ingest(m1).unwrap();
    assert!(uds.claim_message_ingest(m1).unwrap());
    assert!(!uds.claim_message_ingest(m1).unwrap());

    // Simulate restart recovery.
    let recovered = uds.recover_running_ingest_to_queued().unwrap();
    assert_eq!(recovered, 1);

    let next = uds.next_backfill_message().unwrap().unwrap();
    assert_eq!(next.0, m1);
    assert!(uds.claim_message_ingest(m1).unwrap());
}
