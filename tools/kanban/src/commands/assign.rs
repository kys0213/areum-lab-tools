use std::path::Path;

use crate::common::store::{Store, Transition};
use crate::output::{AppError, AssignData, Payload};

/// Assigns an item to a project by hand (`inbox`/`unmatched` → `backlog`),
/// optionally correcting the priority in the same move. An unknown project is
/// `not_found` rather than a nearest-name match; an item that already has a
/// project is a `conflict`, because re-targeting a claimed or finished item
/// would strand its agent (`move` is the escape hatch for that).
pub(crate) fn run_assign(
    db_path: &Path,
    id: &str,
    project: &str,
    priority: Option<&str>,
) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let Transition { before, after } = store.assign(id, project, priority)?;
    Ok(Payload::Assign(AssignData {
        id: after.id,
        state: after.state.as_str().to_owned(),
        previous_state: before.state.as_str().to_owned(),
        project: after
            .project
            .expect("assign writes the project it just verified is registered"),
        priority: after.priority.as_str().to_owned(),
        updated_at: after.updated_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::{TempBoard, seed_store};
    use crate::common::store::NewItem;
    use crate::output::{ErrorKind, exit_code};

    /// Registers `belt` and intakes one item, leaving it in `inbox`.
    fn seed_inbox(db_path: &Path) -> String {
        let mut store = seed_store(db_path);
        store.add_project("belt", "conveyor").expect("project add");
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
    fn assign_moves_an_inbox_item_to_backlog_and_reports_both_ends() {
        let board = TempBoard::new("assign-success");
        let id = seed_inbox(board.db_path());

        match run_assign(board.db_path(), &id, "belt", Some("P0")).unwrap() {
            Payload::Assign(data) => {
                assert_eq!(data.id, id);
                assert_eq!(data.previous_state, "inbox");
                assert_eq!(data.state, "backlog");
                assert_eq!(data.project, "belt");
                assert_eq!(data.priority, "P0");
            }
            other => panic!("expected Payload::Assign, got {other:?}"),
        }
    }

    #[test]
    fn assign_without_a_priority_keeps_the_one_the_item_had() {
        let board = TempBoard::new("assign-keeps-priority");
        let id = seed_inbox(board.db_path());
        match run_assign(board.db_path(), &id, "belt", None).unwrap() {
            Payload::Assign(data) => assert_eq!(data.priority, "P2"),
            other => panic!("expected Payload::Assign, got {other:?}"),
        }
    }

    #[test]
    fn assigning_an_already_assigned_item_is_a_conflict() {
        let board = TempBoard::new("assign-twice");
        let id = seed_inbox(board.db_path());
        run_assign(board.db_path(), &id, "belt", None).expect("first assign");

        let err = run_assign(board.db_path(), &id, "belt", None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(exit_code(&err), 8);
        assert!(err.message.contains("backlog"), "{}", err.message);
    }

    #[test]
    fn assign_to_an_unknown_project_is_not_found() {
        let board = TempBoard::new("assign-unknown-project");
        let id = seed_inbox(board.db_path());
        let err = run_assign(board.db_path(), &id, "ghost", None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert_eq!(exit_code(&err), 7);
        assert_eq!(err.message, "no project ghost");
    }

    #[test]
    fn assign_of_a_missing_item_is_not_found() {
        let board = TempBoard::new("assign-missing-item");
        seed_inbox(board.db_path());
        let err = run_assign(board.db_path(), "itm-999999", "belt", None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert_eq!(exit_code(&err), 7);
    }

    #[test]
    fn assign_rejects_an_unknown_priority_token_and_changes_nothing() {
        // clap keeps this out of the CLI path (Priority is a ValueEnum); the
        // store rejects it anyway so a non-CLI caller cannot smuggle a token
        // past the schema's CHECK, and the item stays exactly where it was.
        let board = TempBoard::new("assign-unknown-priority");
        let id = seed_inbox(board.db_path());
        let err = run_assign(board.db_path(), &id, "belt", Some("P9")).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage);
        assert_eq!(exit_code(&err), 2);

        let stored = Store::open(board.db_path()).unwrap().get_item(&id).unwrap();
        assert_eq!(stored.state.as_str(), "inbox");
    }
}
