use std::path::Path;

use crate::common::store::{Store, Transition};
use crate::output::{AppError, Payload, ReleaseData};

/// Drops a claim (`running → backlog`), clearing `session_id`/`agent`/
/// `claimed_at`. Failure is a released claim, never a state — this is the only
/// recovery path for an item whose agent died.
///
/// The claim it reports is the one it just cleared, so it comes from the
/// transition's `before`, not from the row that is now unowned.
pub(crate) fn run_release(db_path: &Path, id: &str, reason: &str) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let Transition { before, after } = store.release(id)?;
    Ok(Payload::Release(ReleaseData {
        id: after.id,
        state: after.state.as_str().to_owned(),
        project: after
            .project
            .expect("the schema's composite CHECK forbids a backlog row without a project"),
        reason: reason.to_owned(),
        released_session_id: before.session_id,
        released_agent: before.agent,
        updated_at: after.updated_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::{TempBoard, seed_store};
    use crate::common::store::NewItem;
    use crate::output::{ErrorKind, exit_code};

    /// A `belt` item in `backlog`, optionally claimed by `sess-abc`/`claude`.
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

    #[test]
    fn release_returns_the_item_to_backlog_and_reports_the_claim_it_dropped() {
        let board = TempBoard::new("release-success");
        let id = seed_item(board.db_path(), true);

        match run_release(board.db_path(), &id, "build failed").unwrap() {
            Payload::Release(data) => {
                assert_eq!(data.id, id);
                assert_eq!(data.state, "backlog");
                assert_eq!(data.project, "belt");
                assert_eq!(data.reason, "build failed");
                assert_eq!(data.released_session_id.as_deref(), Some("sess-abc"));
                assert_eq!(data.released_agent.as_deref(), Some("claude"));
            }
            other => panic!("expected Payload::Release, got {other:?}"),
        }

        // The row itself is unowned again — the payload reported history, not
        // a claim the item still holds.
        let stored = Store::open(board.db_path()).unwrap().get_item(&id).unwrap();
        assert_eq!(stored.session_id, None);
        assert_eq!(stored.agent, None);
        assert_eq!(stored.claimed_at, None);
    }

    #[test]
    fn release_of_an_unclaimed_item_is_a_conflict() {
        let board = TempBoard::new("release-unclaimed");
        let id = seed_item(board.db_path(), false);
        let err = run_release(board.db_path(), &id, "build failed").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(exit_code(&err), 8);
        assert!(err.message.contains("backlog"), "{}", err.message);
    }

    #[test]
    fn release_of_a_missing_item_is_not_found() {
        let board = TempBoard::new("release-missing");
        let err = run_release(board.db_path(), "itm-999999", "build failed").unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert_eq!(exit_code(&err), 7);
    }
}
