use std::path::Path;

use crate::commands::convert::golden_data;
use crate::common::store::{GoldenFilter, Store};
use crate::output::{AppError, GoldenListData, GoldenSummaryData, Payload};

pub(crate) fn run_golden_propose(
    db_path: &Path,
    decision_id: &str,
    reason: &str,
    note: Option<&str>,
) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    Ok(Payload::GoldenPropose(golden_data(store.propose_golden(
        decision_id,
        reason,
        note,
    )?)))
}

pub(crate) fn run_golden_confirm(
    db_path: &Path,
    id: &str,
    note: Option<&str>,
) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    Ok(Payload::GoldenConfirm(golden_data(
        store.confirm_golden(id, note)?,
    )))
}

pub(crate) fn run_golden_retire(
    db_path: &Path,
    id: &str,
    superseded_by: Option<&str>,
    note: Option<&str>,
) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    Ok(Payload::GoldenRetire(golden_data(store.retire_golden(
        id,
        superseded_by,
        note,
    )?)))
}

pub(crate) fn run_golden_list(
    db_path: &Path,
    filter: &GoldenFilter<'_>,
) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let golden: Vec<_> = store
        .list_golden(filter)?
        .into_iter()
        .map(|g| GoldenSummaryData {
            id: g.id,
            decision_id: g.decision_id,
            category: g.category,
            state: g.state,
            reason: g.reason,
            question: g.question,
            answers: g.answers,
            decided_at: g.decided_at,
        })
        .collect();
    Ok(Payload::GoldenList(GoldenListData {
        count: golden.len(),
        golden,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::{TempDir, sample_decision, seed_store};
    use crate::common::error::ErrorKind;

    fn ledger(label: &str) -> TempDir {
        let dir = TempDir::new(label);
        let mut store = seed_store(&dir.db_path());
        store.seed_categories().unwrap();
        store
            .insert_decisions(&[sample_decision("s:1:0", "캐시", "2026-01-01T00:00:00Z")])
            .unwrap();
        store
            .categorize("dec-000001", "design-scope", "human")
            .unwrap();
        dir
    }

    #[test]
    fn propose_confirm_retire_walk_the_lifecycle() {
        let dir = ledger("golden-lifecycle");
        let db = dir.db_path();
        match run_golden_propose(&db, "dec-000001", "override", Some("뒤집힘")).unwrap() {
            Payload::GoldenPropose(data) => {
                assert_eq!(data.id, "gld-000001");
                assert_eq!(data.state, "candidate");
                assert_eq!(data.reason, "override");
            }
            other => panic!("expected GoldenPropose, got {other:?}"),
        }
        match run_golden_confirm(&db, "gld-000001", None).unwrap() {
            Payload::GoldenConfirm(data) => assert_eq!(data.state, "confirmed"),
            other => panic!("expected GoldenConfirm, got {other:?}"),
        }
        match run_golden_retire(&db, "gld-000001", None, Some("더 안 맞음")).unwrap() {
            Payload::GoldenRetire(data) => {
                assert_eq!(data.state, "retired");
                assert_eq!(data.note.as_deref(), Some("더 안 맞음"));
            }
            other => panic!("expected GoldenRetire, got {other:?}"),
        }
        let err = run_golden_confirm(&db, "gld-000001", None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
    }

    #[test]
    fn list_filters_by_state() {
        let dir = ledger("golden-list");
        run_golden_propose(&dir.db_path(), "dec-000001", "manual", None).unwrap();
        let confirmed = GoldenFilter {
            state: Some("confirmed"),
            ..GoldenFilter::default()
        };
        match run_golden_list(&dir.db_path(), &confirmed).unwrap() {
            Payload::GoldenList(data) => assert_eq!(data.count, 0),
            other => panic!("expected GoldenList, got {other:?}"),
        }
        match run_golden_list(&dir.db_path(), &GoldenFilter::default()).unwrap() {
            Payload::GoldenList(data) => assert_eq!(data.golden[0].state, "candidate"),
            other => panic!("expected GoldenList, got {other:?}"),
        }
    }
}
