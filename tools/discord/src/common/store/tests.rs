use std::path::PathBuf;

use super::*;

fn sample_ask(ask_id: &str, timeout_at: &str) -> NewAsk {
    NewAsk {
        ask_id: ask_id.to_owned(),
        channel_id: "chan-1".to_owned(),
        question: "proceed?".to_owned(),
        options: vec!["yes".to_owned(), "no".to_owned()],
        allow_text: true,
        created_at: "2024-01-01T00:00:00Z".to_owned(),
        timeout_at: timeout_at.to_owned(),
    }
}

/// Unique, not-yet-existing directory under the OS temp dir so file-backed
/// tests exercise the real "create parent dir" path without clobbering each
/// other or leftover runs.
fn unique_store_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "areum-discord-store-test-{}-{label}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn insert_then_get_roundtrips_all_fields() {
    let store = AskStore::open_in_memory().unwrap();
    let ask = sample_ask("ask-1", "2024-01-01T01:00:00Z");
    store.insert_ask(ask.clone()).unwrap();

    let record = store.get_ask("ask-1").unwrap().expect("ask should exist");
    assert_eq!(record.ask_id, "ask-1");
    assert_eq!(record.channel_id, "chan-1");
    assert_eq!(record.question, "proceed?");
    assert_eq!(record.options, vec!["yes".to_owned(), "no".to_owned()]);
    assert!(record.allow_text);
    assert_eq!(record.status, AskStatus::Pending);
    assert_eq!(record.kind, None);
    assert_eq!(record.value, None);
    assert_eq!(record.answered_by, None);
    assert_eq!(record.created_at, "2024-01-01T00:00:00Z");
    assert_eq!(record.timeout_at, "2024-01-01T01:00:00Z");
    assert_eq!(record.answered_at, None);
}

#[test]
fn get_ask_missing_returns_none() {
    let store = AskStore::open_in_memory().unwrap();
    assert!(store.get_ask("does-not-exist").unwrap().is_none());
}

#[test]
fn options_json_roundtrips_through_insert_and_get() {
    let store = AskStore::open_in_memory().unwrap();
    let mut ask = sample_ask("ask-opts", "2024-01-01T01:00:00Z");
    ask.options = vec!["a,b".to_owned(), "\"quoted\"".to_owned(), "한글".to_owned()];
    store.insert_ask(ask.clone()).unwrap();

    let record = store.get_ask("ask-opts").unwrap().unwrap();
    assert_eq!(record.options, ask.options);
}

#[test]
fn try_answer_transitions_pending_to_answered() {
    let store = AskStore::open_in_memory().unwrap();
    store
        .insert_ask(sample_ask("ask-2", "2024-01-01T01:00:00Z"))
        .unwrap();

    let accepted = store
        .try_answer("ask-2", "choice", "yes", "user-1", "2024-01-01T00:30:00Z")
        .unwrap();
    assert!(accepted);

    let record = store.get_ask("ask-2").unwrap().unwrap();
    assert_eq!(record.status, AskStatus::Answered);
    assert_eq!(record.kind.as_deref(), Some("choice"));
    assert_eq!(record.value.as_deref(), Some("yes"));
    assert_eq!(record.answered_by.as_deref(), Some("user-1"));
    assert_eq!(record.answered_at.as_deref(), Some("2024-01-01T00:30:00Z"));
}

#[test]
fn try_answer_rejects_second_response_after_first_wins() {
    let store = AskStore::open_in_memory().unwrap();
    store
        .insert_ask(sample_ask("ask-3", "2024-01-01T01:00:00Z"))
        .unwrap();

    let first = store
        .try_answer("ask-3", "choice", "yes", "user-1", "2024-01-01T00:10:00Z")
        .unwrap();
    let second = store
        .try_answer("ask-3", "choice", "no", "user-2", "2024-01-01T00:20:00Z")
        .unwrap();

    assert!(first);
    assert!(!second);

    // The first response wins — the second must not clobber its fields.
    let record = store.get_ask("ask-3").unwrap().unwrap();
    assert_eq!(record.value.as_deref(), Some("yes"));
    assert_eq!(record.answered_by.as_deref(), Some("user-1"));
}

#[test]
fn try_timeout_transitions_pending_to_timed_out() {
    let store = AskStore::open_in_memory().unwrap();
    store
        .insert_ask(sample_ask("ask-4", "2024-01-01T01:00:00Z"))
        .unwrap();

    assert!(store.try_timeout("ask-4").unwrap());
    assert_eq!(
        store.get_ask("ask-4").unwrap().unwrap().status,
        AskStatus::TimedOut
    );
}

#[test]
fn try_timeout_is_false_once_already_answered() {
    let store = AskStore::open_in_memory().unwrap();
    store
        .insert_ask(sample_ask("ask-5", "2024-01-01T01:00:00Z"))
        .unwrap();
    store
        .try_answer("ask-5", "choice", "yes", "user-1", "2024-01-01T00:10:00Z")
        .unwrap();

    assert!(!store.try_timeout("ask-5").unwrap());
    assert_eq!(
        store.get_ask("ask-5").unwrap().unwrap().status,
        AskStatus::Answered
    );
}

#[test]
fn expire_due_transitions_only_overdue_pending_asks() {
    let store = AskStore::open_in_memory().unwrap();
    store
        .insert_ask(sample_ask("overdue-pending", "2024-01-01T00:00:00Z"))
        .unwrap();
    store
        .insert_ask(sample_ask("future-pending", "2024-06-01T00:00:00Z"))
        .unwrap();
    store
        .insert_ask(sample_ask("overdue-answered", "2024-01-01T00:00:00Z"))
        .unwrap();
    store
        .try_answer(
            "overdue-answered",
            "choice",
            "yes",
            "user-1",
            "2023-12-31T23:59:00Z",
        )
        .unwrap();

    let expired = store.expire_due("2024-03-01T00:00:00Z").unwrap();

    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].ask_id, "overdue-pending");
    assert_eq!(expired[0].status, AskStatus::TimedOut);

    assert_eq!(
        store.get_ask("future-pending").unwrap().unwrap().status,
        AskStatus::Pending
    );
    assert_eq!(
        store.get_ask("overdue-answered").unwrap().unwrap().status,
        AskStatus::Answered
    );
}

#[test]
fn cleanup_deletes_rows_past_retention_and_keeps_recent_ones() {
    let store = AskStore::open_in_memory().unwrap();
    let mut old = sample_ask("old-ask", "2024-01-01T01:00:00Z");
    old.created_at = "2024-01-01T00:00:00Z".to_owned();
    store.insert_ask(old).unwrap();

    let mut recent = sample_ask("recent-ask", "2024-01-30T01:00:00Z");
    recent.created_at = "2024-01-30T00:00:00Z".to_owned();
    store.insert_ask(recent).unwrap();

    // retention_days=30, now=2024-02-01 -> cutoff=2024-01-02.
    // old-ask (2024-01-01) is before the cutoff and gets deleted;
    // recent-ask (2024-01-30) is after it and is kept.
    let deleted = store.cleanup(30, "2024-02-01T00:00:00Z").unwrap();

    assert_eq!(deleted, 1);
    assert!(store.get_ask("old-ask").unwrap().is_none());
    assert!(store.get_ask("recent-ask").unwrap().is_some());
}

#[test]
fn open_creates_wal_file_for_file_backed_db() {
    let dir = unique_store_dir("wal");
    let db_path = dir.join("discord.db");

    let store = AskStore::open(&db_path).unwrap();
    store
        .insert_ask(sample_ask("wal-ask", "2024-01-01T01:00:00Z"))
        .unwrap();

    let wal_path = dir.join("discord.db-wal");
    assert!(
        wal_path.exists(),
        "expected WAL file at {}",
        wal_path.display()
    );

    drop(store);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn reopening_existing_file_db_is_idempotent() {
    let dir = unique_store_dir("reopen");
    let db_path = dir.join("discord.db");

    {
        let store = AskStore::open(&db_path).unwrap();
        store
            .insert_ask(sample_ask("persisted", "2024-01-01T01:00:00Z"))
            .unwrap();
    }

    let store = AskStore::open(&db_path).unwrap();
    assert!(store.get_ask("persisted").unwrap().is_some());

    drop(store);
    std::fs::remove_dir_all(&dir).ok();
}
