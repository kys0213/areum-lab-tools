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

    // Re-running migrate() against an already-migrated file must not error —
    // if it tried `CREATE TABLE asks` again (no `IF NOT EXISTS`), this
    // `.unwrap()` would panic on "table asks already exists".
    let store = AskStore::open(&db_path).unwrap();
    assert!(store.get_ask("persisted").unwrap().is_some());

    drop(store);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn open_sets_busy_timeout_for_file_backed_db() {
    let dir = unique_store_dir("busy-timeout");
    let db_path = dir.join("discord.db");

    let store = AskStore::open(&db_path).unwrap();
    let timeout_ms: i64 = store
        .conn
        .pragma_query_value(None, "busy_timeout", |row| row.get(0))
        .unwrap();
    assert_eq!(timeout_ms, 5_000);

    drop(store);
    std::fs::remove_dir_all(&dir).ok();
}

/// The real deployment shape: the CLI process and the daemon process each
/// hold their own `Connection` to the same on-disk file. This is the
/// scenario every other test (all in-memory or single-connection) misses —
/// a single `Connection` can't exercise cross-process visibility or the
/// `WHERE status = 'pending'` race guard for real.
#[test]
fn two_connections_see_each_others_inserts() {
    let dir = unique_store_dir("two-conn-insert");
    let db_path = dir.join("discord.db");

    let store_a = AskStore::open(&db_path).unwrap();
    store_a
        .insert_ask(sample_ask("cross-conn", "2024-01-01T01:00:00Z"))
        .unwrap();

    let store_b = AskStore::open(&db_path).unwrap();
    let record = store_b.get_ask("cross-conn").unwrap();
    assert!(
        record.is_some(),
        "insert via store_a's connection should be visible through store_b's separate connection"
    );

    drop(store_a);
    drop(store_b);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn two_connections_racing_try_answer_exactly_one_wins() {
    let dir = unique_store_dir("two-conn-race");
    let db_path = dir.join("discord.db");

    let setup = AskStore::open(&db_path).unwrap();
    setup
        .insert_ask(sample_ask("race-ask", "2024-01-01T01:00:00Z"))
        .unwrap();
    drop(setup);

    let path_a = db_path.clone();
    let path_b = db_path.clone();

    let handle_a = std::thread::spawn(move || {
        let store = AskStore::open(&path_a).unwrap();
        store
            .try_answer(
                "race-ask",
                "choice",
                "yes",
                "user-a",
                "2024-01-01T00:10:00Z",
            )
            .unwrap()
    });
    let handle_b = std::thread::spawn(move || {
        let store = AskStore::open(&path_b).unwrap();
        store
            .try_answer("race-ask", "choice", "no", "user-b", "2024-01-01T00:10:01Z")
            .unwrap()
    });

    let a_won = handle_a.join().unwrap();
    let b_won = handle_b.join().unwrap();

    assert_ne!(
        a_won, b_won,
        "exactly one of the two independent connections must win the race"
    );

    let verify = AskStore::open(&db_path).unwrap();
    let record = verify.get_ask("race-ask").unwrap().unwrap();
    assert_eq!(record.status, AskStatus::Answered);
    let expected_winner = if a_won { "user-a" } else { "user-b" };
    assert_eq!(record.answered_by.as_deref(), Some(expected_winner));

    drop(verify);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn try_answer_is_false_once_already_timed_out() {
    let store = AskStore::open_in_memory().unwrap();
    store
        .insert_ask(sample_ask("ask-6", "2024-01-01T01:00:00Z"))
        .unwrap();
    assert!(store.try_timeout("ask-6").unwrap());

    let accepted = store
        .try_answer("ask-6", "choice", "yes", "user-1", "2024-01-01T00:10:00Z")
        .unwrap();
    assert!(!accepted);

    let record = store.get_ask("ask-6").unwrap().unwrap();
    assert_eq!(record.status, AskStatus::TimedOut);
    assert_eq!(record.value, None);
    assert_eq!(record.answered_by, None);
}

#[test]
fn expire_due_includes_boundary_timeout_equal_to_now() {
    let store = AskStore::open_in_memory().unwrap();
    store
        .insert_ask(sample_ask("boundary-ask", "2024-03-01T00:00:00Z"))
        .unwrap();

    // expire_due uses `timeout_at <= now`; timeout_at == now must expire.
    let expired = store.expire_due("2024-03-01T00:00:00Z").unwrap();

    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].ask_id, "boundary-ask");
    assert_eq!(expired[0].status, AskStatus::TimedOut);
}

#[test]
fn cleanup_keeps_row_exactly_at_cutoff_boundary() {
    let store = AskStore::open_in_memory().unwrap();
    let mut boundary = sample_ask("boundary-ask", "2024-01-02T01:00:00Z");
    boundary.created_at = "2024-01-02T00:00:00Z".to_owned();
    store.insert_ask(boundary).unwrap();

    // retention_days=30, now=2024-02-01 -> cutoff=2024-01-02T00:00:00Z exactly.
    // cleanup's condition is strict `<`, so a row created exactly at the
    // cutoff must be kept, not deleted.
    let deleted = store.cleanup(30, "2024-02-01T00:00:00Z").unwrap();

    assert_eq!(deleted, 0);
    assert!(store.get_ask("boundary-ask").unwrap().is_some());
}

/// Locks in the current behavior: `cleanup` has no `status` filter, so a
/// still-`pending` ask (e.g. the daemon never resolved it) is purged by
/// retention exactly like an answered/timed_out one. This may or may not be
/// intended — flagged for the QA verdict rather than changed here.
#[test]
fn cleanup_deletes_old_pending_rows_too() {
    let store = AskStore::open_in_memory().unwrap();
    let mut old_pending = sample_ask("old-pending", "2024-01-01T01:00:00Z");
    old_pending.created_at = "2024-01-01T00:00:00Z".to_owned();
    store.insert_ask(old_pending).unwrap();

    let deleted = store.cleanup(30, "2024-02-01T00:00:00Z").unwrap();

    assert_eq!(deleted, 1);
    assert!(store.get_ask("old-pending").unwrap().is_none());
}

#[test]
fn get_ask_errors_on_unknown_status_instead_of_defaulting() {
    let store = AskStore::open_in_memory().unwrap();
    store
        .insert_ask(sample_ask("bad-status", "2024-01-01T01:00:00Z"))
        .unwrap();
    // Bypass insert_ask/try_answer/try_timeout (which only ever write the
    // three known statuses) to simulate a schema contract violation.
    store
        .conn
        .execute(
            "UPDATE asks SET status = 'bogus' WHERE ask_id = ?1",
            params!["bad-status"],
        )
        .unwrap();

    let err = store
        .get_ask("bad-status")
        .expect_err("unknown status must surface as an error, not a silently-defaulted record");
    assert!(
        err.message.contains("unknown ask status"),
        "expected fail-fast status error, got: {}",
        err.message
    );
}

/// Reproduces a crash between the asks DDL and the schema_version stamp:
/// the table exists but the version row was never written. open() must
/// recover (treat the DDL as already applied and stamp the version), not
/// fail forever on "table asks already exists".
#[test]
fn open_recovers_from_migration_crash_between_ddl_and_version_stamp() {
    let dir = unique_store_dir("partial-migration");
    let db_path = dir.join("discord.db");
    std::fs::create_dir_all(&dir).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")
        .unwrap();
    conn.execute_batch(CREATE_ASKS_SQL).unwrap();
    drop(conn);

    let store = AskStore::open(&db_path).unwrap();
    store
        .insert_ask(sample_ask("recovered", "2024-01-01T01:00:00Z"))
        .unwrap();
    assert!(store.get_ask("recovered").unwrap().is_some());

    drop(store);
    std::fs::remove_dir_all(&dir).ok();
}

/// Two processes racing the very first open of the same file: BEGIN
/// IMMEDIATE serializes the migrations, so the loser waits (busy_timeout)
/// and then skips the already-applied migration instead of erroring.
#[test]
fn concurrent_first_open_of_same_file_both_succeed() {
    let dir = unique_store_dir("concurrent-open");
    let db_path = dir.join("discord.db");

    let path_a = db_path.clone();
    let path_b = db_path.clone();
    let handle_a = std::thread::spawn(move || AskStore::open(&path_a).map(|_| ()));
    let handle_b = std::thread::spawn(move || AskStore::open(&path_b).map(|_| ()));

    handle_a.join().unwrap().expect("first opener must succeed");
    handle_b
        .join()
        .unwrap()
        .expect("second opener must succeed");

    std::fs::remove_dir_all(&dir).ok();
}
