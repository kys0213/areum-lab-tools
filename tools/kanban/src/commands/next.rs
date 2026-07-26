use std::path::Path;

use crate::common::store::{ItemRecord, Store};
use crate::output::{AppError, ClaimedItemData, NextData, Payload};

/// Atomically claims the project's highest-priority backlog item
/// (`backlog → running`), recording `session`/`agent`/`claimed_at` in the same
/// statement that selects it — two agents calling at once must never receive
/// the same item.
///
/// An empty backlog is not an error: it answers `{"id": null}`. So does an
/// unregistered project name, which simply matches no backlog rows.
pub(crate) fn run_next(
    db_path: &Path,
    project: &str,
    session: &str,
    agent: &str,
) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let claimed = store.claim_next(project, session, agent)?;
    Ok(Payload::Next(match claimed {
        Some(item) => NextData::Claimed(claimed_item(item)),
        None => NextData::empty(),
    }))
}

/// Projects a freshly claimed row onto the payload, which types as non-null
/// what the row types as optional. Two different mechanisms guarantee those
/// four columns, so each `expect` names the one that covers it.
fn claimed_item(item: ItemRecord) -> ClaimedItemData {
    ClaimedItemData {
        id: item.id,
        title: item.title,
        body: item.body,
        project: item
            .project
            .expect("the schema's composite CHECK forbids a running row without a project"),
        priority: item.priority.as_str().to_owned(),
        state: item.state.as_str().to_owned(),
        session_id: item
            .session_id
            .expect("the schema's composite CHECK forbids a running row without a session"),
        agent: item
            .agent
            .expect("claim_next's UPDATE sets agent on every row it returns"),
        claimed_at: item
            .claimed_at
            .expect("claim_next's UPDATE sets claimed_at on every row it returns"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::{TempBoard, seed_store};
    use crate::common::store::NewItem;

    /// Seeds `belt` with one backlog item per `(external_id, priority)` pair,
    /// in the order given, on a clock that makes `created_at` strictly
    /// increasing — the claim's tie-break is otherwise untestable against a
    /// one-second wall clock.
    fn seed_backlog(db_path: &Path, seeds: &[(&str, &str)]) {
        let mut store = seed_store(db_path);
        store.add_project("belt", "conveyor").expect("project add");
        for (external_id, priority) in seeds {
            let item = store
                .insert_item(&NewItem {
                    source: "discord",
                    external_id,
                    title: external_id,
                    body: "details",
                })
                .expect("intake");
            store
                .assign(&item.id, "belt", Some(priority))
                .expect("assign");
        }
    }

    fn claim(db_path: &Path, session: &str, agent: &str) -> NextData {
        match run_next(db_path, "belt", session, agent).unwrap() {
            Payload::Next(data) => data,
            other => panic!("expected Payload::Next, got {other:?}"),
        }
    }

    #[test]
    fn claiming_an_empty_backlog_answers_id_null_rather_than_an_error() {
        // `{"id": null}` is the documented poll answer; an error would make a
        // quiet board look broken to every agent waiting on work.
        let board = TempBoard::new("next-empty");
        seed_backlog(board.db_path(), &[]);
        assert_eq!(
            claim(board.db_path(), "sess-abc", "claude"),
            NextData::empty()
        );
    }

    #[test]
    fn claiming_from_an_unregistered_project_answers_id_null_rather_than_not_found() {
        let board = TempBoard::new("next-unregistered-project");
        let data = match run_next(board.db_path(), "ghost", "sess-abc", "claude").unwrap() {
            Payload::Next(data) => data,
            other => panic!("expected Payload::Next, got {other:?}"),
        };
        assert_eq!(data, NextData::empty());
    }

    #[test]
    fn a_claim_reports_the_item_and_the_holder_it_just_recorded() {
        let board = TempBoard::new("next-success");
        seed_backlog(board.db_path(), &[("msg-1", "P1")]);

        match claim(board.db_path(), "sess-abc", "claude") {
            NextData::Claimed(data) => {
                assert_eq!(data.id, "itm-000001");
                assert_eq!(data.title, "msg-1");
                assert_eq!(data.body, "details");
                assert_eq!(data.project, "belt");
                assert_eq!(data.priority, "P1");
                assert_eq!(data.state, "running");
                assert_eq!(data.session_id, "sess-abc");
                assert_eq!(data.agent, "claude");
                assert!(!data.claimed_at.is_empty());
            }
            other => panic!("expected a claim, got {other:?}"),
        }

        // The board is drained, so the next poll is the empty answer again.
        assert_eq!(
            claim(board.db_path(), "sess-two", "codex"),
            NextData::empty()
        );
    }

    #[test]
    fn claims_come_out_highest_priority_first_then_oldest_first() {
        let board = TempBoard::new("next-order");
        seed_backlog(
            board.db_path(),
            &[("older-p2", "P2"), ("urgent-p0", "P0"), ("newer-p2", "P2")],
        );

        let mut titles = Vec::new();
        while let NextData::Claimed(data) = claim(board.db_path(), "sess-abc", "claude") {
            titles.push(data.title);
        }
        assert_eq!(titles, vec!["urgent-p0", "older-p2", "newer-p2"]);
    }
}
