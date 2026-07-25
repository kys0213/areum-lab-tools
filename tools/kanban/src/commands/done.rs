use std::path::Path;

use crate::common::store::Store;
use crate::output::{AppError, DoneData, Payload};

/// Completes an item (`running → done`), **keeping** `session_id`/`agent` so
/// the record of who did the work survives completion (docs §4). Calling it
/// on an item that is not claimed is a `conflict`, not a silent success.
pub(crate) fn run_done(db_path: &Path, id: &str) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let after = store.mark_done(id)?.after;
    Ok(Payload::Done(DoneData {
        id: after.id,
        state: after.state.as_str().to_owned(),
        project: after
            .project
            .expect("the schema's composite CHECK forbids a done row without a project"),
        session_id: after.session_id,
        agent: after.agent,
        updated_at: after.updated_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::{TempBoard, seed_store};
    use crate::common::store::NewItem;
    use crate::output::{ErrorKind, exit_code};

    /// A `belt` item claimed by `sess-abc`/`claude`, i.e. the only state from
    /// which `done` is legal.
    fn seed_running(db_path: &Path) -> String {
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
        store
            .claim_next("belt", "sess-abc", "claude")
            .expect("claim")
            .expect("backlog is not empty")
            .id
    }

    #[test]
    fn done_completes_a_claimed_item_and_preserves_the_session_that_did_it() {
        let board = TempBoard::new("done-success");
        let id = seed_running(board.db_path());

        match run_done(board.db_path(), &id).unwrap() {
            Payload::Done(data) => {
                assert_eq!(data.id, id);
                assert_eq!(data.state, "done");
                assert_eq!(data.project, "belt");
                assert_eq!(data.session_id.as_deref(), Some("sess-abc"));
                assert_eq!(data.agent.as_deref(), Some("claude"));
            }
            other => panic!("expected Payload::Done, got {other:?}"),
        }

        // The claim survives in the row itself, not only in the payload.
        let stored = Store::open(board.db_path()).unwrap().get_item(&id).unwrap();
        assert_eq!(stored.session_id.as_deref(), Some("sess-abc"));
        assert_eq!(stored.agent.as_deref(), Some("claude"));
    }

    #[test]
    fn done_on_an_unclaimed_item_is_a_conflict() {
        let board = TempBoard::new("done-unclaimed");
        let id = seed_running(board.db_path());
        run_done(board.db_path(), &id).expect("first completion");

        // Already `done`, so there is no claim left to complete.
        let err = run_done(board.db_path(), &id).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(exit_code(&err), 8);
        assert!(err.message.contains("done"), "{}", err.message);
    }

    #[test]
    fn done_on_a_backlog_item_is_a_conflict() {
        let board = TempBoard::new("done-backlog");
        {
            let mut store = seed_store(board.db_path());
            store.add_project("belt", "conveyor").unwrap();
            let item = store
                .insert_item(&NewItem {
                    source: "discord",
                    external_id: "msg-1",
                    title: "t",
                    body: "b",
                })
                .unwrap();
            store.assign(&item.id, "belt", None).unwrap();
        }
        let err = run_done(board.db_path(), "itm-000001").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(exit_code(&err), 8);
    }

    #[test]
    fn done_on_a_missing_item_is_not_found() {
        let board = TempBoard::new("done-missing");
        let err = run_done(board.db_path(), "itm-999999").unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert_eq!(exit_code(&err), 7);
    }
}
