use std::path::Path;

use crate::common::store::{Store, Transition};
use crate::output::{AppError, Payload, PriorityData};

/// Corrects an item's priority without touching its state. `priority` is
/// already validated against `P0`~`P3` at the CLI boundary.
///
/// Both ends are reported so a caller can tell a real change from a no-op
/// re-assertion of the same value.
pub(crate) fn run_priority(db_path: &Path, id: &str, priority: &str) -> Result<Payload, AppError> {
    let mut store = Store::open(db_path)?;
    let Transition { before, after } = store.set_priority(id, priority)?;
    Ok(Payload::Priority(PriorityData {
        id: after.id,
        priority: after.priority.as_str().to_owned(),
        previous_priority: before.priority.as_str().to_owned(),
        updated_at: after.updated_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::{TempBoard, seed_store};
    use crate::common::store::NewItem;
    use crate::output::{ErrorKind, exit_code};

    /// One item in `inbox`, i.e. a state `priority` must not disturb.
    fn seed_inbox(db_path: &Path) -> String {
        let mut store = seed_store(db_path);
        store
            .insert_item(&NewItem {
                source: "discord",
                external_id: "msg-1",
                title: "cache drifts",
                body: "details",
            })
            .expect("intake")
            .id
    }

    #[test]
    fn priority_reports_both_ends_and_leaves_the_state_alone() {
        let board = TempBoard::new("priority-success");
        let id = seed_inbox(board.db_path());

        match run_priority(board.db_path(), &id, "P0").unwrap() {
            Payload::Priority(data) => {
                assert_eq!(data.id, id);
                assert_eq!(data.previous_priority, "P2");
                assert_eq!(data.priority, "P0");
            }
            other => panic!("expected Payload::Priority, got {other:?}"),
        }

        let stored = Store::open(board.db_path()).unwrap().get_item(&id).unwrap();
        assert_eq!(stored.priority.as_str(), "P0");
        assert_eq!(stored.state.as_str(), "inbox");
    }

    #[test]
    fn re_asserting_the_same_priority_is_reported_as_an_unchanged_pair() {
        let board = TempBoard::new("priority-noop");
        let id = seed_inbox(board.db_path());
        match run_priority(board.db_path(), &id, "P2").unwrap() {
            Payload::Priority(data) => {
                assert_eq!(data.previous_priority, "P2");
                assert_eq!(data.priority, "P2");
            }
            other => panic!("expected Payload::Priority, got {other:?}"),
        }
    }

    #[test]
    fn priority_of_a_missing_item_is_not_found() {
        let board = TempBoard::new("priority-missing");
        let err = run_priority(board.db_path(), "itm-999999", "P0").unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert_eq!(exit_code(&err), 7);
    }

    #[test]
    fn an_unknown_priority_token_is_a_usage_error_and_changes_nothing() {
        // clap keeps this out of the CLI path; the store rejects it anyway so
        // a non-CLI caller cannot smuggle a token past the schema's CHECK.
        let board = TempBoard::new("priority-unknown-token");
        let id = seed_inbox(board.db_path());
        let err = run_priority(board.db_path(), &id, "P9").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage);
        assert_eq!(exit_code(&err), 2);

        let stored = Store::open(board.db_path()).unwrap().get_item(&id).unwrap();
        assert_eq!(stored.priority.as_str(), "P2");
    }
}
