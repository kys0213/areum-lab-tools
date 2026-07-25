use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

/// Strictly increasing timestamps, one per call, so `created_at` ordering in
/// the claim is pinned by insertion order instead of by how fast the test ran.
#[derive(Default)]
struct SeqClock {
    ticks: AtomicU64,
}

impl Clock for SeqClock {
    fn now(&self) -> String {
        let n = self.ticks.fetch_add(1, Ordering::SeqCst);
        format!(
            "2024-01-01T{:02}:{:02}:{:02}Z",
            n / 3_600,
            (n / 60) % 60,
            n % 60
        )
    }
}

struct FixedClock(&'static str);

impl Clock for FixedClock {
    fn now(&self) -> String {
        self.0.to_owned()
    }
}

fn memory_store() -> Store {
    Store::open_in_memory(Box::new(SeqClock::default())).expect("in-memory board opens")
}

/// A unique, not-yet-existing directory under the OS temp dir. Mirrors the
/// `tools/discord` store fixtures: file-backed tests exercise the real
/// "create parent dir" path without clobbering each other.
fn unique_db_path(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "areum-kanban-store-test-{}-{label}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("kanban.db")
}

fn new_item(external_id: &str) -> NewItem<'_> {
    NewItem {
        source: "discord",
        external_id,
        title: "cache keeps drifting",
        body: "details",
    }
}

fn add_inbox(store: &mut Store, external_id: &str) -> ItemRecord {
    store.insert_item(&new_item(external_id)).expect("intake")
}

/// Intakes an item and assigns it to `project` at `priority`, i.e. the normal
/// route to a claimable `backlog` row.
fn add_backlog(store: &mut Store, external_id: &str, project: &str, priority: &str) -> String {
    let item = add_inbox(store, external_id);
    store
        .assign(&item.id, project, Some(priority))
        .expect("assign")
        .after
        .id
}

/// Bypasses the API to write a row directly, so the schema's own CHECK
/// constraints — not any Rust-side validation — decide whether it lands.
fn raw_insert(
    store: &Store,
    id: &str,
    state: &str,
    project: Option<&str>,
    session: Option<&str>,
) -> rusqlite::Result<usize> {
    store.conn.execute(
        "INSERT INTO items (id, source, external_id, title, body, state, project,
                            session_id, created_at, updated_at)
         VALUES (?1, 'cli', ?1, 't', 'b', ?2, ?3, ?4,
                 '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
        params![id, state, project, session],
    )
}

fn seeded_project(store: &Store) {
    store.add_project("belt", "conveyor").expect("project add");
}

// --- schema invariants -----------------------------------------------------

#[test]
fn opening_an_existing_board_reuses_its_schema_and_data() {
    let path = unique_db_path("idempotent-open");
    {
        let mut store =
            Store::open_with_clock(&path, Box::new(SeqClock::default())).expect("first open");
        seeded_project(&store);
        add_inbox(&mut store, "msg-1");
    }
    let store = Store::open_with_clock(&path, Box::new(SeqClock::default())).expect("second open");
    assert_eq!(store.list_projects().unwrap().len(), 1);
    assert_eq!(store.list_items(&ItemFilter::default()).unwrap().len(), 1);
}

#[test]
fn open_reporting_created_is_true_only_on_the_first_open() {
    let path = unique_db_path("reporting-created");
    let (_, created) =
        Store::open_with_clock_reporting_created(&path, Box::new(SeqClock::default()))
            .expect("first open");
    assert!(created, "a fresh path must report created = true");

    let (_, created) =
        Store::open_with_clock_reporting_created(&path, Box::new(SeqClock::default()))
            .expect("second open");
    assert!(!created, "an existing board must report created = false");
    let _ = std::fs::remove_dir_all(path.parent().expect("fixture path has a parent"));
}

#[test]
fn running_without_a_session_is_rejected_at_write_time() {
    let store = memory_store();
    seeded_project(&store);
    let err = raw_insert(&store, "itm-000001", "running", Some("belt"), None)
        .expect_err("running with no claim violates the composite CHECK");
    assert!(is_constraint_violation(&err), "{err}");
}

#[test]
fn backlog_without_a_project_is_rejected_at_write_time() {
    let store = memory_store();
    let err = raw_insert(&store, "itm-000001", "backlog", None, None)
        .expect_err("backlog with no project violates the composite CHECK");
    assert!(is_constraint_violation(&err), "{err}");
}

#[test]
fn inbox_carrying_a_project_or_a_session_is_rejected_at_write_time() {
    let store = memory_store();
    seeded_project(&store);
    for (id, project, session) in [
        ("itm-000001", Some("belt"), None),
        ("itm-000002", None, Some("sess-abc")),
    ] {
        let err = raw_insert(&store, id, "inbox", project, session)
            .expect_err("inbox must carry neither a project nor a claim");
        assert!(is_constraint_violation(&err), "{err}");
    }
}

#[test]
fn an_unknown_state_or_priority_token_is_rejected_at_write_time() {
    let store = memory_store();
    let err = raw_insert(&store, "itm-000001", "failed", None, None)
        .expect_err("`failed` is not a board state");
    assert!(is_constraint_violation(&err), "{err}");

    let err = store
        .conn
        .execute(
            "INSERT INTO items (id, source, external_id, title, body, state, priority,
                                created_at, updated_at)
             VALUES ('itm-000002', 'cli', 'x', 't', 'b', 'inbox', 'P9', 'now', 'now')",
            [],
        )
        .expect_err("P9 is not a board priority");
    assert!(is_constraint_violation(&err), "{err}");
}

#[test]
fn done_accepts_both_an_agent_claim_and_a_human_completion() {
    // `done` is the deliberately loose branch: reachable from a claim (session
    // preserved) and from a human `move` (no session).
    let store = memory_store();
    seeded_project(&store);
    raw_insert(&store, "itm-000001", "done", Some("belt"), Some("sess-abc"))
        .expect("agent-completed item");
    raw_insert(&store, "itm-000002", "done", Some("belt"), None).expect("human-completed item");
    let err = raw_insert(&store, "itm-000003", "done", None, None)
        .expect_err("done still requires a project");
    assert!(is_constraint_violation(&err), "{err}");
}

#[test]
fn deleting_an_item_cascades_to_its_labels() {
    let mut store = memory_store();
    let item = add_inbox(&mut store, "msg-1");
    store
        .attach_label(&item.id, "kind", "bug", Some(0.9))
        .unwrap();
    store
        .conn
        .execute("DELETE FROM items WHERE id = ?1", params![item.id])
        .unwrap();
    assert!(store.labels_for(&item.id).unwrap().is_empty());
}

// --- projects --------------------------------------------------------------

#[test]
fn projects_round_trip_and_list_by_name() {
    let store = Store::open_in_memory(Box::new(FixedClock("2024-01-01T00:00:00Z"))).unwrap();
    let added = store.add_project("belt", "conveyor").unwrap();
    assert_eq!(
        added,
        ProjectRecord {
            name: "belt".into(),
            description: "conveyor".into(),
            created_at: "2024-01-01T00:00:00Z".into(),
        }
    );
    store.add_project("areum", "tools").unwrap();
    let names: Vec<String> = store
        .list_projects()
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, vec!["areum".to_owned(), "belt".to_owned()]);
}

#[test]
fn re_adding_a_project_is_a_conflict() {
    let store = memory_store();
    seeded_project(&store);
    let err = store.add_project("belt", "again").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
    assert!(err.message.contains("belt"), "{}", err.message);
}

#[test]
fn removing_an_unknown_project_is_not_found() {
    let store = memory_store();
    let err = store.remove_project("ghost").unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
}

#[test]
fn project_rm_is_refused_while_items_reference_it() {
    // This end-to-end behavior does NOT by itself prove `enable_foreign_keys`
    // is doing anything: the bundled libsqlite3-sys build compiles with
    // `-DSQLITE_DEFAULT_FOREIGN_KEYS=1`, so FK enforcement is already on
    // before any pragma call runs. `enable_foreign_keys_actually_turns_fk_on`
    // below isolates the function itself against that compile-time default.
    let mut store = memory_store();
    seeded_project(&store);
    let id = add_backlog(&mut store, "msg-1", "belt", "P1");

    let err = store.remove_project("belt").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
    assert!(err.message.contains("belt"), "{}", err.message);
    assert!(store.find_item(&id).unwrap().is_some(), "item was orphaned");

    // Once nothing references it, the same delete goes through.
    store.move_item(&id, "inbox").unwrap();
    store.remove_project("belt").unwrap();
    assert!(store.list_projects().unwrap().is_empty());
}

#[test]
fn enable_foreign_keys_actually_turns_fk_on() {
    // `project_rm_is_refused_while_items_reference_it` cannot tell
    // `enable_foreign_keys` apart from a no-op, because this build's SQLite
    // already defaults foreign_keys to ON at compile time (verified via
    // libsqlite3-sys's build.rs: `-DSQLITE_DEFAULT_FOREIGN_KEYS=1`). Starting
    // from an explicit OFF and asserting the flip to ON tests the function's
    // own effect, independent of that default.
    let conn = Connection::open_in_memory().expect("in-memory connection");
    conn.pragma_update(None, "foreign_keys", "OFF")
        .expect("start from an explicit OFF, overriding the compile-time default");
    let before: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();
    assert_eq!(before, 0, "test setup must actually start from OFF");

    enable_foreign_keys(&conn).expect("enable_foreign_keys");

    let after: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();
    assert_eq!(after, 1, "enable_foreign_keys must turn FK enforcement on");
}

// --- intake ----------------------------------------------------------------

#[test]
fn item_ids_are_zero_padded_and_sequential() {
    assert_eq!(format_item_id(17), "itm-000017");
    assert_eq!(format_item_id(1), "itm-000001");

    let mut store = memory_store();
    assert_eq!(add_inbox(&mut store, "msg-1").id, "itm-000001");
    assert_eq!(add_inbox(&mut store, "msg-2").id, "itm-000002");
}

#[test]
fn intake_lands_in_inbox_with_the_default_priority() {
    let mut store = Store::open_in_memory(Box::new(FixedClock("2024-01-01T00:00:00Z"))).unwrap();
    let item = add_inbox(&mut store, "msg-1");
    assert_eq!(item.state, ItemState::Inbox);
    assert_eq!(item.priority, Priority::P2);
    assert_eq!(item.project, None);
    assert_eq!(item.session_id, None);
    assert_eq!(item.source, "discord");
    assert_eq!(item.external_id, "msg-1");
    assert_eq!(item.created_at, "2024-01-01T00:00:00Z");
    assert_eq!(item.updated_at, "2024-01-01T00:00:00Z");
}

#[test]
fn the_same_source_message_cannot_be_ingested_twice() {
    let mut store = memory_store();
    add_inbox(&mut store, "msg-1");
    let err = store.insert_item(&new_item("msg-1")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
    assert!(err.message.contains("msg-1"), "{}", err.message);
    assert_eq!(store.list_items(&ItemFilter::default()).unwrap().len(), 1);

    // The dedup key is the pair, so the same id from another source is fine.
    store
        .insert_item(&NewItem {
            source: "github",
            external_id: "msg-1",
            title: "t",
            body: "b",
        })
        .expect("a different source is a different item");
}

#[test]
fn a_missing_item_is_not_found() {
    let store = memory_store();
    let err = store.get_item("itm-000017").unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert_eq!(err.message, "no item itm-000017");
    assert!(store.find_item("itm-000017").unwrap().is_none());
}

// --- listing ---------------------------------------------------------------

#[test]
fn list_filters_by_project_state_and_label() {
    let mut store = memory_store();
    seeded_project(&store);
    store.add_project("areum", "tools").unwrap();
    let assigned = add_backlog(&mut store, "msg-1", "belt", "P1");
    let elsewhere = add_backlog(&mut store, "msg-2", "areum", "P1");
    let untouched = add_inbox(&mut store, "msg-3").id;
    store
        .attach_label(&untouched, "duplicate-of", &assigned, Some(0.8))
        .unwrap();

    let ids = |filter: &ItemFilter<'_>| -> Vec<String> {
        store
            .list_items(filter)
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect()
    };

    assert_eq!(
        ids(&ItemFilter::default()),
        vec![assigned.clone(), elsewhere.clone(), untouched.clone()]
    );
    assert_eq!(
        ids(&ItemFilter {
            project: Some("belt"),
            ..ItemFilter::default()
        }),
        vec![assigned.clone()]
    );
    assert_eq!(
        ids(&ItemFilter {
            state: Some("inbox"),
            ..ItemFilter::default()
        }),
        vec![untouched.clone()]
    );
    assert_eq!(
        ids(&ItemFilter {
            label: Some("duplicate-of"),
            ..ItemFilter::default()
        }),
        vec![untouched]
    );
    assert_eq!(
        ids(&ItemFilter {
            project: Some("belt"),
            state: Some("inbox"),
            label: None,
        }),
        Vec::<String>::new()
    );
}

#[test]
fn an_unknown_state_filter_is_a_usage_error() {
    let store = memory_store();
    let err = store
        .list_items(&ItemFilter {
            state: Some("failed"),
            ..ItemFilter::default()
        })
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

// --- atomic claim ----------------------------------------------------------

#[test]
fn claiming_an_empty_backlog_is_not_an_error() {
    let store = memory_store();
    seeded_project(&store);
    assert!(
        store
            .claim_next("belt", "sess-abc", "claude")
            .unwrap()
            .is_none()
    );
}

#[test]
fn claim_order_is_priority_then_oldest_first() {
    let mut store = memory_store();
    seeded_project(&store);
    let older_p2 = add_backlog(&mut store, "msg-1", "belt", "P2");
    let urgent = add_backlog(&mut store, "msg-2", "belt", "P0");
    let newer_p2 = add_backlog(&mut store, "msg-3", "belt", "P2");

    let claimed: Vec<String> =
        std::iter::from_fn(|| store.claim_next("belt", "s", "claude").unwrap())
            .map(|item| item.id)
            .collect();
    assert_eq!(claimed, vec![urgent, older_p2, newer_p2]);
}

#[test]
fn a_claim_records_the_holder_and_flips_the_state() {
    let mut store = memory_store();
    seeded_project(&store);
    let id = add_backlog(&mut store, "msg-1", "belt", "P1");
    let claimed = store
        .claim_next("belt", "sess-abc", "claude")
        .unwrap()
        .expect("backlog is not empty");
    assert_eq!(claimed.id, id);
    assert_eq!(claimed.state, ItemState::Running);
    assert_eq!(claimed.session_id.as_deref(), Some("sess-abc"));
    assert_eq!(claimed.agent.as_deref(), Some("claude"));
    assert_eq!(claimed.claimed_at.as_deref(), Some(&claimed.updated_at[..]));
    // A claimed item is no longer claimable.
    assert!(
        store
            .claim_next("belt", "sess-two", "codex")
            .unwrap()
            .is_none()
    );
}

#[test]
fn claims_are_scoped_to_the_requested_project() {
    let mut store = memory_store();
    seeded_project(&store);
    store.add_project("areum", "tools").unwrap();
    add_backlog(&mut store, "msg-1", "areum", "P0");
    assert!(
        store
            .claim_next("belt", "sess-abc", "claude")
            .unwrap()
            .is_none()
    );
}

#[test]
fn claiming_from_an_unregistered_project_is_not_an_error() {
    // `claim_next` has no `require_project` guard (unlike `assign`): a project
    // name that was never registered simply matches no backlog rows, so it
    // answers `None` the same as an empty backlog rather than `not_found`.
    let store = memory_store();
    assert!(
        store
            .claim_next("ghost-project", "sess-abc", "claude")
            .unwrap()
            .is_none()
    );
}

#[test]
fn two_connections_claiming_concurrently_never_receive_the_same_item() {
    // The correctness claim the whole design rests on. Two real connections
    // against one file-backed board drain the same backlog from two threads:
    // a read-then-write claim would hand the same row to both, showing up
    // here as a duplicate id (and as a total above the seeded count).
    const TOTAL: usize = 24;
    let path = unique_db_path("concurrent-claim");
    {
        let mut store = Store::open_with_clock(&path, Box::new(SeqClock::default())).unwrap();
        seeded_project(&store);
        for i in 0..TOTAL {
            add_backlog(&mut store, &format!("msg-{i}"), "belt", "P1");
        }
    }

    let start = std::sync::Barrier::new(2);
    let per_agent: Vec<Vec<String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = ["claude", "codex"]
            .into_iter()
            .map(|agent| {
                let start = &start;
                let path = path.as_path();
                scope.spawn(move || {
                    let store =
                        Store::open_with_clock(path, Box::new(SeqClock::default())).unwrap();
                    let session = format!("sess-{agent}");
                    start.wait();
                    let mut ids = Vec::new();
                    while let Some(item) = store.claim_next("belt", &session, agent).unwrap() {
                        ids.push(item.id);
                    }
                    ids
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("claim thread panicked"))
            .collect()
    });

    let mut all: Vec<String> = per_agent.concat();
    assert_eq!(
        all.len(),
        TOTAL,
        "every backlog item must be handed out exactly once: {per_agent:?}"
    );
    all.sort();
    all.dedup();
    assert_eq!(
        all.len(),
        TOTAL,
        "two connections received the same item id: {per_agent:?}"
    );

    // Every row is now claimed by exactly one of the two sessions.
    let store = Store::open_with_clock(&path, Box::new(SeqClock::default())).unwrap();
    for item in store.list_items(&ItemFilter::default()).unwrap() {
        assert_eq!(item.state, ItemState::Running, "{item:?}");
        assert!(item.session_id.is_some(), "{item:?}");
    }
    let _ = std::fs::remove_dir_all(path.parent().expect("fixture path has a parent"));
}

#[test]
fn a_single_backlog_item_is_claimed_by_exactly_one_of_two_racers() {
    let path = unique_db_path("single-item-race");
    {
        let mut store = Store::open_with_clock(&path, Box::new(SeqClock::default())).unwrap();
        seeded_project(&store);
        add_backlog(&mut store, "msg-1", "belt", "P0");
    }

    let start = std::sync::Barrier::new(2);
    let outcomes: Vec<Option<String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = ["claude", "codex"]
            .into_iter()
            .map(|agent| {
                let start = &start;
                let path = path.as_path();
                scope.spawn(move || {
                    let store =
                        Store::open_with_clock(path, Box::new(SeqClock::default())).unwrap();
                    start.wait();
                    store
                        .claim_next("belt", &format!("sess-{agent}"), agent)
                        .unwrap()
                        .map(|item| item.id)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("claim thread panicked"))
            .collect()
    });

    assert_eq!(
        outcomes.iter().filter(|o| o.is_some()).count(),
        1,
        "exactly one racer may win the only backlog item: {outcomes:?}"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("fixture path has a parent"));
}

// --- done / release --------------------------------------------------------

#[test]
fn done_preserves_the_claim_and_release_clears_it() {
    let mut store = memory_store();
    seeded_project(&store);
    let id = add_backlog(&mut store, "msg-1", "belt", "P1");

    store.claim_next("belt", "sess-abc", "claude").unwrap();
    let finished = store.mark_done(&id).unwrap();
    assert_eq!(finished.after.state, ItemState::Done);
    assert_eq!(finished.after.session_id.as_deref(), Some("sess-abc"));
    assert_eq!(finished.after.agent.as_deref(), Some("claude"));
    assert_eq!(finished.after.project.as_deref(), Some("belt"));
    assert!(finished.after.claimed_at.is_some());
    assert_ne!(finished.after.updated_at, finished.before.updated_at);

    let id = add_backlog(&mut store, "msg-2", "belt", "P1");
    store.claim_next("belt", "sess-two", "codex").unwrap();
    let released = store.release(&id).unwrap();
    assert_eq!(released.before.session_id.as_deref(), Some("sess-two"));
    assert_eq!(released.before.agent.as_deref(), Some("codex"));
    assert_eq!(released.after.state, ItemState::Backlog);
    assert_eq!(released.after.session_id, None);
    assert_eq!(released.after.agent, None);
    assert_eq!(released.after.claimed_at, None);
    assert_eq!(released.after.project.as_deref(), Some("belt"));

    // Releasing puts it back in the claimable pool.
    assert!(
        store
            .claim_next("belt", "sess-three", "claude")
            .unwrap()
            .is_some()
    );
}

#[test]
fn completing_or_releasing_an_unclaimed_item_is_a_conflict() {
    let mut store = memory_store();
    seeded_project(&store);
    let id = add_backlog(&mut store, "msg-1", "belt", "P1");
    for err in [
        store.mark_done(&id).unwrap_err(),
        store.release(&id).unwrap_err(),
    ] {
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert!(err.message.contains("backlog"), "{}", err.message);
    }
    assert_eq!(store.get_item(&id).unwrap().state, ItemState::Backlog);
}

#[test]
fn completing_a_missing_item_is_not_found() {
    let store = memory_store();
    assert_eq!(
        store.mark_done("itm-000017").unwrap_err().kind,
        ErrorKind::NotFound
    );
    assert_eq!(
        store.release("itm-000017").unwrap_err().kind,
        ErrorKind::NotFound
    );
}

// --- assign / priority -----------------------------------------------------

#[test]
fn assign_moves_an_inbox_item_to_backlog_and_may_correct_the_priority() {
    let mut store = memory_store();
    seeded_project(&store);
    let item = add_inbox(&mut store, "msg-1");
    let assigned = store.assign(&item.id, "belt", Some("P0")).unwrap();
    assert_eq!(assigned.before.state, ItemState::Inbox);
    assert_eq!(assigned.after.state, ItemState::Backlog);
    assert_eq!(assigned.after.project.as_deref(), Some("belt"));
    assert_eq!(assigned.after.priority, Priority::P0);

    let other = add_inbox(&mut store, "msg-2");
    let kept = store.assign(&other.id, "belt", None).unwrap();
    assert_eq!(
        kept.after.priority,
        Priority::P2,
        "priority stays untouched"
    );
}

#[test]
fn assign_to_an_unknown_project_is_not_found() {
    let mut store = memory_store();
    let item = add_inbox(&mut store, "msg-1");
    let err = store.assign(&item.id, "ghost", None).unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert_eq!(err.message, "no project ghost");
}

#[test]
fn assigning_an_already_assigned_item_is_a_conflict() {
    let mut store = memory_store();
    seeded_project(&store);
    let id = add_backlog(&mut store, "msg-1", "belt", "P1");
    let err = store.assign(&id, "belt", None).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
}

#[test]
fn assign_rejects_an_unknown_priority_before_touching_the_row() {
    let mut store = memory_store();
    seeded_project(&store);
    let item = add_inbox(&mut store, "msg-1");
    let err = store.assign(&item.id, "belt", Some("P9")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert_eq!(store.get_item(&item.id).unwrap().state, ItemState::Inbox);
}

#[test]
fn set_priority_reports_both_ends_and_leaves_the_state_alone() {
    let mut store = memory_store();
    seeded_project(&store);
    let id = add_backlog(&mut store, "msg-1", "belt", "P2");
    let changed = store.set_priority(&id, "P0").unwrap();
    assert_eq!(changed.before.priority, Priority::P2);
    assert_eq!(changed.after.priority, Priority::P0);
    assert_eq!(changed.after.state, ItemState::Backlog);

    assert_eq!(
        store.set_priority(&id, "P9").unwrap_err().kind,
        ErrorKind::Usage
    );
    assert_eq!(
        store.set_priority("itm-999999", "P0").unwrap_err().kind,
        ErrorKind::NotFound
    );
}

// --- move ------------------------------------------------------------------

#[test]
fn plan_move_clears_exactly_what_the_target_state_forbids() {
    let cleared = plan_move(ItemState::Unmatched, Some("belt"), Some("sess")).unwrap();
    assert_eq!(
        cleared,
        MovePlan {
            clear_project: true,
            clear_claim: true
        }
    );
    assert_eq!(
        plan_move(ItemState::Backlog, Some("belt"), Some("sess")).unwrap(),
        MovePlan {
            clear_project: false,
            clear_claim: true
        }
    );
    assert_eq!(
        plan_move(ItemState::Done, Some("belt"), Some("sess")).unwrap(),
        MovePlan {
            clear_project: false,
            clear_claim: false
        }
    );
    assert_eq!(
        plan_move(ItemState::Running, Some("belt"), Some("sess")).unwrap(),
        MovePlan {
            clear_project: false,
            clear_claim: false
        }
    );

    for (target, project, session) in [
        (ItemState::Backlog, None, None),
        (ItemState::Done, None, None),
        (ItemState::Running, None, None),
        (ItemState::Running, Some("belt"), None),
    ] {
        assert_eq!(
            plan_move(target, project, session).unwrap_err().kind,
            ErrorKind::Conflict,
            "{target:?} {project:?} {session:?}"
        );
    }
}

#[test]
fn moving_back_to_inbox_clears_the_project_and_the_claim() {
    let mut store = memory_store();
    seeded_project(&store);
    let id = add_backlog(&mut store, "msg-1", "belt", "P1");
    store.claim_next("belt", "sess-abc", "claude").unwrap();

    let moved = store.move_item(&id, "inbox").unwrap();
    assert_eq!(moved.before.state, ItemState::Running);
    assert_eq!(moved.after.state, ItemState::Inbox);
    assert_eq!(moved.after.project, None);
    assert_eq!(moved.after.session_id, None);
    assert_eq!(moved.after.agent, None);
    assert_eq!(moved.after.claimed_at, None);
}

#[test]
fn moving_a_claimed_item_to_done_keeps_the_claim() {
    let mut store = memory_store();
    seeded_project(&store);
    let id = add_backlog(&mut store, "msg-1", "belt", "P1");
    store.claim_next("belt", "sess-abc", "claude").unwrap();
    let moved = store.move_item(&id, "done").unwrap();
    assert_eq!(moved.after.state, ItemState::Done);
    assert_eq!(moved.after.session_id.as_deref(), Some("sess-abc"));
}

#[test]
fn moving_an_unassigned_item_into_an_assigned_state_is_a_conflict() {
    let mut store = memory_store();
    let item = add_inbox(&mut store, "msg-1");
    for target in ["backlog", "running", "done"] {
        let err = store.move_item(&item.id, target).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict, "target {target}");
    }
    assert_eq!(store.get_item(&item.id).unwrap().state, ItemState::Inbox);
}

#[test]
fn moving_to_an_unknown_state_is_a_usage_error() {
    let mut store = memory_store();
    let item = add_inbox(&mut store, "msg-1");
    assert_eq!(
        store.move_item(&item.id, "failed").unwrap_err().kind,
        ErrorKind::Usage
    );
}

// --- labels ----------------------------------------------------------------

#[test]
fn labels_round_trip_and_re_scoring_replaces_the_confidence() {
    let mut store = memory_store();
    let item = add_inbox(&mut store, "msg-1");
    store
        .attach_label(&item.id, "kind", "bug", Some(0.4))
        .unwrap();
    store.attach_label(&item.id, "area", "cache", None).unwrap();
    store
        .attach_label(&item.id, "kind", "bug", Some(0.9))
        .unwrap();

    let labels = store.labels_for(&item.id).unwrap();
    assert_eq!(
        labels,
        vec![
            LabelRecord {
                key: "area".into(),
                value: "cache".into(),
                confidence: None,
            },
            LabelRecord {
                key: "kind".into(),
                value: "bug".into(),
                confidence: Some(0.9),
            },
        ]
    );
}

#[test]
fn labelling_an_item_that_is_not_on_the_board_is_not_found() {
    let store = memory_store();
    let err = store
        .attach_label("itm-999999", "kind", "bug", None)
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert_eq!(err.message, "no item itm-999999");
}

// --- decoding --------------------------------------------------------------

#[test]
fn a_state_token_outside_the_schema_fails_the_row_decode() {
    // The CHECK keeps such a row out, so this only fires if the file was
    // written by something else — it must fail loudly, not decode to a
    // default state.
    let store = memory_store();
    store
        .conn
        .execute("CREATE TABLE probe (state TEXT NOT NULL)", [])
        .unwrap();
    store
        .conn
        .execute("INSERT INTO probe (state) VALUES ('failed')", [])
        .unwrap();
    let decoded: rusqlite::Result<ItemState> =
        store
            .conn
            .query_row("SELECT state FROM probe", [], |row| row.get("state"));
    assert!(decoded.is_err(), "unknown state token must not decode");
}
