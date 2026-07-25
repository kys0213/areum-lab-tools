//! `move` is a Rust keyword, so the escape-hatch transition lives in
//! `move_item.rs` while the subcommand keeps its documented name.

use std::path::Path;

use crate::common::store::{Store, Transition};
use crate::output::{AppError, MoveData, Payload};

/// Forces an item into `state` — the escape hatch for transitions the regular
/// commands refuse. The schema's CHECK constraints still apply, so a move that
/// would leave an inconsistent row (a `running` item with no owner, say) is
/// rejected as a `conflict` before the write, with a message naming the
/// command that can get there instead.
pub(crate) fn run_move(db_path: &Path, id: &str, state: &str) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let Transition { before, after } = store.move_item(id, state)?;
    Ok(Payload::Move(MoveData {
        id: after.id,
        from_state: before.state.as_str().to_owned(),
        to_state: after.state.as_str().to_owned(),
        project: after.project,
        updated_at: after.updated_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::{TempBoard, seed_store};
    use crate::common::store::NewItem;
    use crate::output::{ErrorKind, exit_code};

    /// One `belt` item, in `backlog` and optionally claimed.
    fn seed_item(db_path: &Path, claimed: bool) -> String {
        let mut store = seed_store(db_path);
        store.add_project("belt", "conveyor").expect("project add");
        let item = store
            .insert_item(&NewItem {
                source: "discord",
                external_id: "msg-1",
                title: "cache drifts",
                body: "details",
            })
            .expect("intake");
        store.assign(&item.id, "belt", Some("P1")).expect("assign");
        if claimed {
            store
                .claim_next("belt", "sess-abc", "claude")
                .expect("claim");
        }
        item.id
    }

    fn move_data(db_path: &Path, id: &str, state: &str) -> MoveData {
        match run_move(db_path, id, state).unwrap() {
            Payload::Move(data) => data,
            other => panic!("expected Payload::Move, got {other:?}"),
        }
    }

    #[test]
    fn move_reports_both_ends_of_the_transition() {
        let board = TempBoard::new("move-success");
        let id = seed_item(board.db_path(), true);

        let data = move_data(board.db_path(), &id, "done");
        assert_eq!(data.id, id);
        assert_eq!(data.from_state, "running");
        assert_eq!(data.to_state, "done");
        assert_eq!(data.project.as_deref(), Some("belt"));
    }

    #[test]
    fn moving_back_to_inbox_clears_the_project_and_the_claim() {
        let board = TempBoard::new("move-to-inbox");
        let id = seed_item(board.db_path(), true);

        let data = move_data(board.db_path(), &id, "inbox");
        assert_eq!(data.from_state, "running");
        assert_eq!(data.to_state, "inbox");
        assert_eq!(data.project, None);

        let stored = Store::open(board.db_path()).unwrap().get_item(&id).unwrap();
        assert_eq!(stored.session_id, None);
        assert_eq!(stored.agent, None);
        assert_eq!(stored.claimed_at, None);
    }

    #[test]
    fn moving_an_unclaimed_item_to_running_is_a_conflict_that_points_at_next() {
        // `move` cannot invent a session, so `running` is unreachable this way
        // no matter what else is true of the row. The message has to say which
        // command can get there instead of leaking a CHECK failure.
        let board = TempBoard::new("move-to-running");
        let id = seed_item(board.db_path(), false);

        let err = run_move(board.db_path(), &id, "running").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(exit_code(&err), 8);
        assert!(err.message.contains("`next`"), "{}", err.message);

        let stored = Store::open(board.db_path()).unwrap().get_item(&id).unwrap();
        assert_eq!(stored.state.as_str(), "backlog");
    }

    #[test]
    fn moving_an_unassigned_item_into_a_project_state_is_a_conflict() {
        let board = TempBoard::new("move-unassigned");
        {
            let mut store = seed_store(board.db_path());
            store
                .insert_item(&NewItem {
                    source: "discord",
                    external_id: "msg-1",
                    title: "t",
                    body: "b",
                })
                .expect("intake");
        }
        for target in ["backlog", "done"] {
            let err = run_move(board.db_path(), "itm-000001", target).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Conflict, "target {target}");
            assert_eq!(exit_code(&err), 8, "target {target}");
            assert!(err.message.contains("project"), "{}", err.message);
        }
    }

    #[test]
    fn move_of_a_missing_item_is_not_found() {
        let board = TempBoard::new("move-missing");
        let err = run_move(board.db_path(), "itm-999999", "inbox").unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert_eq!(exit_code(&err), 7);
    }
}
