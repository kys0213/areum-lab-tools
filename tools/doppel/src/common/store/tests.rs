use super::decisions::Insertion;
use super::*;
use crate::commands::testutil::{TempDir, memory_store, sample_decision};
use crate::common::error::ErrorKind;

/// Inserts one sample decision and files it under `design-scope`.
fn categorized(store: &mut Store, external_id: &str, topic: &str, decided_at: &str) -> String {
    store
        .insert_decisions(&[sample_decision(external_id, topic, decided_at)])
        .expect("insert");
    let id = store
        .list_decisions(&DecisionFilter {
            limit: 1000,
            ..DecisionFilter::default()
        })
        .unwrap()
        .into_iter()
        .find(|d| d.external_id == external_id)
        .expect("inserted decision is listed")
        .id;
    store.categorize(&id, "design-scope", "human").unwrap();
    id
}

fn seeded() -> Store {
    let store = memory_store();
    store.seed_categories().unwrap();
    store
}

#[test]
fn opening_a_file_creates_parents_and_reports_creation_once() {
    let dir = TempDir::new("open");
    let (_store, created) = Store::open_reporting_created(&dir.db_path()).unwrap();
    assert!(created);
    let (_store, created) = Store::open_reporting_created(&dir.db_path()).unwrap();
    assert!(!created);
}

#[test]
fn seeding_is_idempotent_and_keeps_a_retired_seed_retired() {
    let store = memory_store();
    let first = store.seed_categories().unwrap();
    assert_eq!(
        first,
        ["design-scope", "naming-api-test", "dependency-review"]
    );

    store.retire_category("design-scope").unwrap();
    assert!(store.seed_categories().unwrap().is_empty());
    assert_eq!(store.get_category("design-scope").unwrap().state, "retired");
}

#[test]
fn duplicate_category_is_a_conflict() {
    let store = seeded();
    let err = store.add_category("design-scope", "again").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
}

#[test]
fn retiring_twice_is_a_conflict_and_unknown_is_not_found() {
    let store = seeded();
    store.retire_category("naming-api-test").unwrap();
    assert_eq!(
        store.retire_category("naming-api-test").unwrap_err().kind,
        ErrorKind::Conflict
    );
    assert_eq!(
        store.retire_category("nope").unwrap_err().kind,
        ErrorKind::NotFound
    );
}

#[test]
fn reingesting_the_same_source_record_is_counted_not_duplicated() {
    let mut store = seeded();
    let batch = [
        sample_decision("s:1:0", "캐시", "2026-01-01T00:00:00Z"),
        sample_decision("s:2:0", "네이밍", "2026-01-02T00:00:00Z"),
    ];
    let first = store.insert_decisions(&batch).unwrap();
    assert_eq!(
        first,
        Insertion {
            added: 2,
            existing: 0
        }
    );

    let second = store.insert_decisions(&batch).unwrap();
    assert_eq!(
        second,
        Insertion {
            added: 0,
            existing: 2
        }
    );
}

#[test]
fn decisions_round_trip_their_json_columns() {
    let mut store = seeded();
    store
        .insert_decisions(&[sample_decision("s:1:0", "캐시", "2026-01-01T00:00:00Z")])
        .unwrap();
    let decision = store.get_decision("dec-000001").unwrap();
    assert_eq!(decision.options.len(), 2);
    assert_eq!(decision.options[1].label, "B");
    assert_eq!(decision.answers, ["A"]);
    assert_eq!(decision.category, None);
}

#[test]
fn list_orders_newest_first_and_filters() {
    let mut store = seeded();
    store
        .insert_decisions(&[
            sample_decision("s:1:0", "old", "2026-01-01T00:00:00Z"),
            sample_decision("s:2:0", "new", "2026-03-01T00:00:00Z"),
            sample_decision("s:3:0", "mid", "2026-02-01T00:00:00Z"),
        ])
        .unwrap();
    store
        .categorize("dec-000003", "design-scope", "model")
        .unwrap();

    let all = store
        .list_decisions(&DecisionFilter {
            limit: 10,
            ..DecisionFilter::default()
        })
        .unwrap();
    let ids: Vec<_> = all.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, ["dec-000002", "dec-000003", "dec-000001"]);

    let uncategorized = store
        .list_decisions(&DecisionFilter {
            uncategorized: true,
            limit: 10,
            ..DecisionFilter::default()
        })
        .unwrap();
    assert_eq!(uncategorized.len(), 2);

    let filed = store
        .list_decisions(&DecisionFilter {
            category: Some("design-scope"),
            limit: 10,
            ..DecisionFilter::default()
        })
        .unwrap();
    assert_eq!(filed.len(), 1);
    assert_eq!(filed[0].category_by.as_deref(), Some("model"));

    let limited = store
        .list_decisions(&DecisionFilter {
            limit: 1,
            ..DecisionFilter::default()
        })
        .unwrap();
    assert_eq!(limited.len(), 1);
}

#[test]
fn categorizing_reports_the_previous_filing() {
    let mut store = seeded();
    let id = categorized(&mut store, "s:1:0", "캐시", "2026-01-01T00:00:00Z");
    let change = store.categorize(&id, "naming-api-test", "human").unwrap();
    assert_eq!(change.previous_category.as_deref(), Some("design-scope"));
    assert_eq!(change.after.category.as_deref(), Some("naming-api-test"));
}

#[test]
fn categorizing_into_a_retired_or_unknown_category_is_refused() {
    let mut store = seeded();
    let id = categorized(&mut store, "s:1:0", "캐시", "2026-01-01T00:00:00Z");
    store.retire_category("naming-api-test").unwrap();
    assert_eq!(
        store
            .categorize(&id, "naming-api-test", "human")
            .unwrap_err()
            .kind,
        ErrorKind::Conflict
    );
    assert_eq!(
        store.categorize(&id, "nope", "human").unwrap_err().kind,
        ErrorKind::NotFound
    );
}

#[test]
fn schema_refuses_a_half_filed_category() {
    let mut store = seeded();
    store
        .insert_decisions(&[sample_decision("s:1:0", "캐시", "2026-01-01T00:00:00Z")])
        .unwrap();
    let err = store
        .conn
        .execute(
            "UPDATE decisions SET category = 'design-scope' WHERE id = 'dec-000001'",
            [],
        )
        .unwrap_err();
    assert_eq!(
        err.sqlite_error_code(),
        Some(ErrorCode::ConstraintViolation)
    );
}

#[test]
fn proposing_requires_a_category_and_happens_once() {
    let mut store = seeded();
    store
        .insert_decisions(&[sample_decision("s:1:0", "캐시", "2026-01-01T00:00:00Z")])
        .unwrap();
    assert_eq!(
        store
            .propose_golden("dec-000001", "manual", None)
            .unwrap_err()
            .kind,
        ErrorKind::Conflict
    );

    store
        .categorize("dec-000001", "design-scope", "human")
        .unwrap();
    let golden = store
        .propose_golden("dec-000001", "manual", Some("첫 판례"))
        .unwrap();
    assert_eq!(golden.id, "gld-000001");
    assert_eq!(golden.state, "candidate");
    assert_eq!(golden.reviewed_at, None);
    assert_eq!(golden.note.as_deref(), Some("첫 판례"));

    assert_eq!(
        store
            .propose_golden("dec-000001", "manual", None)
            .unwrap_err()
            .kind,
        ErrorKind::Conflict
    );
}

#[test]
fn golden_keeps_its_copy_when_the_decision_is_refiled() {
    let mut store = seeded();
    let id = categorized(&mut store, "s:1:0", "캐시", "2026-01-01T00:00:00Z");
    store.propose_golden(&id, "manual", None).unwrap();
    store.categorize(&id, "naming-api-test", "human").unwrap();
    assert_eq!(
        store.get_golden("gld-000001").unwrap().category,
        "design-scope"
    );
}

#[test]
fn confirm_moves_only_candidates_and_stamps_the_review() {
    let mut store = seeded();
    let id = categorized(&mut store, "s:1:0", "캐시", "2026-01-01T00:00:00Z");
    store.propose_golden(&id, "manual", Some("before")).unwrap();

    let confirmed = store.confirm_golden("gld-000001", None).unwrap();
    assert_eq!(confirmed.state, "confirmed");
    assert!(confirmed.reviewed_at.is_some());
    assert_eq!(confirmed.note.as_deref(), Some("before"));

    assert_eq!(
        store.confirm_golden("gld-000001", None).unwrap_err().kind,
        ErrorKind::Conflict
    );
}

#[test]
fn retire_validates_the_replacement() {
    let mut store = seeded();
    let old = categorized(&mut store, "s:1:0", "캐시", "2026-01-01T00:00:00Z");
    let new = categorized(&mut store, "s:2:0", "캐시", "2026-02-01T00:00:00Z");
    store.propose_golden(&old, "ingest", None).unwrap();
    store.propose_golden(&new, "override", None).unwrap();
    store.confirm_golden("gld-000001", None).unwrap();

    // The replacement is still a candidate.
    assert_eq!(
        store
            .retire_golden("gld-000001", Some("gld-000002"), None)
            .unwrap_err()
            .kind,
        ErrorKind::Conflict
    );
    assert_eq!(
        store
            .retire_golden("gld-000001", Some("gld-000001"), None)
            .unwrap_err()
            .kind,
        ErrorKind::Conflict
    );
    assert_eq!(
        store
            .retire_golden("gld-000001", Some("gld-000404"), None)
            .unwrap_err()
            .kind,
        ErrorKind::NotFound
    );

    store.confirm_golden("gld-000002", None).unwrap();
    let retired = store
        .retire_golden("gld-000001", Some("gld-000002"), Some("뒤집힘"))
        .unwrap();
    assert_eq!(retired.state, "retired");
    assert_eq!(retired.superseded_by.as_deref(), Some("gld-000002"));
    assert_eq!(
        store
            .retire_golden("gld-000001", None, None)
            .unwrap_err()
            .kind,
        ErrorKind::Conflict
    );
}

#[test]
fn retiring_a_candidate_stamps_the_review_time() {
    // The schema ties `reviewed_at` to leaving `candidate`; a dropped
    // candidate must satisfy it too.
    let mut store = seeded();
    let id = categorized(&mut store, "s:1:0", "캐시", "2026-01-01T00:00:00Z");
    store.propose_golden(&id, "manual", None).unwrap();
    let retired = store.retire_golden("gld-000001", None, None).unwrap();
    assert!(retired.reviewed_at.is_some());
}

#[test]
fn review_queue_puts_overrides_first_then_oldest() {
    let mut store = seeded();
    for (n, reason) in ["manual", "override", "ingest", "override"]
        .iter()
        .enumerate()
    {
        let id = categorized(
            &mut store,
            &format!("s:{n}:0"),
            "캐시",
            "2026-01-01T00:00:00Z",
        );
        store.propose_golden(&id, reason, None).unwrap();
    }
    store.confirm_golden("gld-000003", None).unwrap();

    let queue: Vec<_> = store
        .review_queue()
        .unwrap()
        .into_iter()
        .map(|g| (g.id, g.reason))
        .collect();
    assert_eq!(
        queue,
        [
            ("gld-000002".to_owned(), "override".to_owned()),
            ("gld-000004".to_owned(), "override".to_owned()),
            ("gld-000001".to_owned(), "manual".to_owned()),
        ]
    );
}

#[test]
fn list_golden_filters_by_state_and_category() {
    let mut store = seeded();
    let a = categorized(&mut store, "s:1:0", "캐시", "2026-01-01T00:00:00Z");
    let b = categorized(&mut store, "s:2:0", "네이밍", "2026-01-01T00:00:00Z");
    store.propose_golden(&a, "manual", None).unwrap();
    store.categorize(&b, "naming-api-test", "human").unwrap();
    store.propose_golden(&b, "manual", None).unwrap();
    store.confirm_golden("gld-000002", None).unwrap();

    let confirmed = store
        .list_golden(&GoldenFilter {
            state: Some("confirmed"),
            ..GoldenFilter::default()
        })
        .unwrap();
    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0].category, "naming-api-test");

    let design = store
        .list_golden(&GoldenFilter {
            category: Some("design-scope"),
            ..GoldenFilter::default()
        })
        .unwrap();
    assert_eq!(design.len(), 1);
    assert_eq!(design[0].id, "gld-000001");
}

fn confirmed_precedents(store: &mut Store) {
    let topics = [
        ("s:1:0", "저장소 테스트 전략", "2026-01-01T00:00:00Z"),
        ("s:2:0", "캐시 무효화", "2026-02-01T00:00:00Z"),
        ("s:3:0", "저장소 이름", "2026-03-01T00:00:00Z"),
    ];
    for (external_id, topic, at) in topics {
        let id = categorized(store, external_id, topic, at);
        store.propose_golden(&id, "manual", None).unwrap();
    }
    store.confirm_golden("gld-000001", None).unwrap();
    store.confirm_golden("gld-000002", None).unwrap();
}

#[test]
fn search_matches_korean_stems_by_prefix() {
    let mut store = seeded();
    confirmed_precedents(&mut store);
    // "저장소에" never appears; the stem "저장소" does.
    let expression = fts_query_for("저장소").unwrap();
    let hits = store
        .search_golden(
            &expression,
            &SearchFilter {
                limit: 10,
                ..SearchFilter::default()
            },
        )
        .unwrap();
    let ids: Vec<_> = hits.iter().map(|h| h.golden.id.as_str()).collect();
    // gld-000003 matches too but is still a candidate.
    assert_eq!(ids, ["gld-000001"]);
}

#[test]
fn search_includes_candidates_only_when_asked() {
    let mut store = seeded();
    confirmed_precedents(&mut store);
    let expression = fts_query_for("저장소").unwrap();
    let hits = store
        .search_golden(
            &expression,
            &SearchFilter {
                include_candidates: true,
                limit: 10,
                ..SearchFilter::default()
            },
        )
        .unwrap();
    let mut ids: Vec<_> = hits.iter().map(|h| h.golden.id.clone()).collect();
    ids.sort();
    assert_eq!(ids, ["gld-000001", "gld-000003"]);
    assert!(hits.iter().all(|h| h.score.is_finite()));
}

#[test]
fn search_never_returns_retired_and_honours_filters() {
    let mut store = seeded();
    confirmed_precedents(&mut store);
    store.retire_golden("gld-000002", None, None).unwrap();
    let expression = fts_query_for("캐시").unwrap();
    let all = SearchFilter {
        include_candidates: true,
        limit: 10,
        ..SearchFilter::default()
    };
    assert!(store.search_golden(&expression, &all).unwrap().is_empty());

    let expression = fts_query_for("저장소").unwrap();
    let other_project = SearchFilter {
        project: Some("/elsewhere"),
        ..all
    };
    assert!(
        store
            .search_golden(&expression, &other_project)
            .unwrap()
            .is_empty()
    );
    let other_category = SearchFilter {
        category: Some("naming-api-test"),
        ..all
    };
    assert!(
        store
            .search_golden(&expression, &other_category)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn search_is_deterministic() {
    let mut store = seeded();
    confirmed_precedents(&mut store);
    let expression = fts_query_for("저장소 캐시 전략").unwrap();
    let filter = SearchFilter {
        include_candidates: true,
        limit: 10,
        ..SearchFilter::default()
    };
    let first = store.search_golden(&expression, &filter).unwrap();
    let second = store.search_golden(&expression, &filter).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.len(), 3);
}

fn fts_query_for(text: &str) -> Option<String> {
    fts_query(text)
}
