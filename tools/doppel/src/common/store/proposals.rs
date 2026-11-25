//! `proposals` — one per card: the precedent-backed assignment the triage
//! command proposed, whether doppel applied it, and, once a person assigns
//! the card, whether they took it.

use rusqlite::{OptionalExtension, Row, params};

use super::{Store, conflict, constraint_err, decode_json, encode_json, map_sqlite_err, not_found};
use crate::common::error::AppError;

/// A validated proposal as the runner produced it. `project` is `None` when
/// no registered project fits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProposalDraft {
    pub(crate) project: Option<String>,
    pub(crate) priority: String,
    pub(crate) confidence: String,
    pub(crate) rationale: String,
    pub(crate) cited: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProposalRecord {
    pub(crate) card_id: String,
    pub(crate) state: String,
    pub(crate) project: Option<String>,
    pub(crate) priority: Option<String>,
    pub(crate) confidence: Option<String>,
    pub(crate) rationale: Option<String>,
    pub(crate) cited: Vec<String>,
    pub(crate) precedents: Vec<String>,
    pub(crate) command: String,
    pub(crate) error: Option<String>,
    pub(crate) applied: bool,
    pub(crate) accepted: Option<bool>,
    pub(crate) created_at: String,
    pub(crate) finished_at: Option<String>,
}

/// Acceptance over proposals a person has judged by assigning the card.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ProposalStats {
    pub(crate) proposals: u32,
    pub(crate) failed: u32,
    pub(crate) applied: u32,
    pub(crate) judged: u32,
    pub(crate) accepted: u32,
}

const PROPOSAL_COLUMNS: &str = "card_id, state, project, priority, confidence, rationale, cited, \
     precedents, command, error, applied, accepted, created_at, finished_at";

impl Store {
    /// Marks a proposal as being written for a card still in `inbox`. A
    /// finished proposal is replaced — the new one is what the next
    /// assignment judges; one still pending is a conflict.
    pub(crate) fn start_proposal(
        &self,
        card_id: &str,
        precedents: &[String],
        command: &str,
    ) -> Result<ProposalRecord, AppError> {
        let card = self.get_card(card_id)?;
        if card.state != "inbox" {
            return Err(conflict(format!(
                "{card_id} is {}; only an inbox card gets a proposal",
                card.state
            )));
        }
        if self
            .get_proposal(card_id)?
            .is_some_and(|p| p.state == "pending")
        {
            return Err(conflict(format!(
                "a proposal for {card_id} is being written"
            )));
        }
        let now = self.clock.now();
        let tx = self.conn.unchecked_transaction().map_err(map_sqlite_err)?;
        tx.execute("DELETE FROM proposals WHERE card_id = ?1", params![card_id])
            .map_err(map_sqlite_err)?;
        tx.execute(
            "INSERT INTO proposals (card_id, state, precedents, command, created_at)
             VALUES (?1, 'pending', ?2, ?3, ?4)",
            params![card_id, encode_json(&precedents), command, now],
        )
        .map_err(map_sqlite_err)?;
        tx.commit().map_err(map_sqlite_err)?;
        self.require_proposal(card_id)
    }

    pub(crate) fn finish_proposal(
        &self,
        card_id: &str,
        draft: &ProposalDraft,
    ) -> Result<ProposalRecord, AppError> {
        let now = self.clock.now();
        let changed = self
            .conn
            .execute(
                "UPDATE proposals SET state = 'ready', project = ?2, priority = ?3,
                   confidence = ?4, rationale = ?5, cited = ?6, finished_at = ?7
                 WHERE card_id = ?1 AND state = 'pending'",
                params![
                    card_id,
                    draft.project,
                    draft.priority,
                    draft.confidence,
                    draft.rationale,
                    encode_json(&draft.cited),
                    now,
                ],
            )
            .map_err(|e| {
                constraint_err(e, format!("proposal for {card_id} violates the schema"))
            })?;
        if changed == 0 {
            return Err(conflict(format!("proposal for {card_id} is not pending")));
        }
        self.require_proposal(card_id)
    }

    pub(crate) fn fail_proposal(
        &self,
        card_id: &str,
        error: &str,
    ) -> Result<ProposalRecord, AppError> {
        let now = self.clock.now();
        self.conn
            .execute(
                "UPDATE proposals SET state = 'failed', error = ?2, finished_at = ?3
                 WHERE card_id = ?1 AND state = 'pending'",
                params![card_id, error, now],
            )
            .map_err(map_sqlite_err)?;
        self.require_proposal(card_id)
    }

    /// doppel assigned the card as proposed (auto triage).
    pub(crate) fn mark_proposal_applied(&self, card_id: &str) -> Result<(), AppError> {
        self.conn
            .execute(
                "UPDATE proposals SET applied = 1 WHERE card_id = ?1",
                params![card_id],
            )
            .map_err(|e| constraint_err(e, format!("proposal for {card_id} cannot be applied")))?;
        Ok(())
    }

    pub(crate) fn get_proposal(&self, card_id: &str) -> Result<Option<ProposalRecord>, AppError> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {PROPOSAL_COLUMNS} FROM proposals WHERE card_id = ?1"),
                params![card_id],
                row_to_raw,
            )
            .optional()
            .map_err(map_sqlite_err)?;
        raw.map(RawProposal::decode).transpose()
    }

    /// Records whether a person's assignment took the proposal, once: the
    /// first assignment after a ready proposal judges it. Answers `Some(false)`
    /// when this assignment overrode it.
    pub(super) fn judge_proposal(
        &self,
        card_id: &str,
        project: &str,
        priority: &str,
    ) -> Result<Option<bool>, AppError> {
        let Some(proposal) = self.get_proposal(card_id)? else {
            return Ok(None);
        };
        if proposal.state != "ready" || proposal.accepted.is_some() {
            return Ok(None);
        }
        let accepted = proposal.project.as_deref() == Some(project)
            && proposal.priority.as_deref() == Some(priority);
        self.conn
            .execute(
                "UPDATE proposals SET accepted = ?2 WHERE card_id = ?1",
                params![card_id, accepted],
            )
            .map_err(map_sqlite_err)?;
        Ok(Some(accepted))
    }

    pub(crate) fn proposal_stats(&self) -> Result<ProposalStats, AppError> {
        self.conn
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(state = 'failed'), 0),
                        COALESCE(SUM(applied = 1), 0),
                        COALESCE(SUM(accepted IS NOT NULL), 0),
                        COALESCE(SUM(accepted = 1), 0)
                 FROM proposals",
                [],
                |row| {
                    Ok(ProposalStats {
                        proposals: row.get(0)?,
                        failed: row.get(1)?,
                        applied: row.get(2)?,
                        judged: row.get(3)?,
                        accepted: row.get(4)?,
                    })
                },
            )
            .map_err(map_sqlite_err)
    }

    fn require_proposal(&self, card_id: &str) -> Result<ProposalRecord, AppError> {
        self.get_proposal(card_id)?
            .ok_or_else(|| not_found(format!("no proposal for {card_id}")))
    }
}

struct RawProposal {
    record: ProposalRecord,
    cited: String,
    precedents: String,
}

impl RawProposal {
    fn decode(self) -> Result<ProposalRecord, AppError> {
        Ok(ProposalRecord {
            cited: decode_json(&self.cited, "proposals.cited")?,
            precedents: decode_json(&self.precedents, "proposals.precedents")?,
            ..self.record
        })
    }
}

fn row_to_raw(row: &Row<'_>) -> rusqlite::Result<RawProposal> {
    Ok(RawProposal {
        record: ProposalRecord {
            card_id: row.get(0)?,
            state: row.get(1)?,
            project: row.get(2)?,
            priority: row.get(3)?,
            confidence: row.get(4)?,
            rationale: row.get(5)?,
            cited: Vec::new(),
            precedents: Vec::new(),
            command: row.get(8)?,
            error: row.get(9)?,
            applied: row.get(10)?,
            accepted: row.get(11)?,
            created_at: row.get(12)?,
            finished_at: row.get(13)?,
        },
        cited: row.get(6)?,
        precedents: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::{TempDir, memory_store};
    use crate::common::error::ErrorKind;
    use crate::common::store::{DecisionFilter, NewCard};

    fn board(dir: &TempDir) -> Store {
        let store = memory_store();
        store.seed_categories().unwrap();
        for name in ["areum", "blog"] {
            let path = dir.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            store.add_project(name, path.to_str().unwrap(), "").unwrap();
        }
        store
    }

    fn inbox_card(store: &Store, external_id: &str) -> String {
        store
            .add_card(&NewCard {
                source: "cli",
                external_id,
                title: "캐시 고치기",
                body: "재시작하면 사라진다",
            })
            .unwrap()
            .id
    }

    fn draft(project: Option<&str>, priority: &str) -> ProposalDraft {
        ProposalDraft {
            project: project.map(str::to_owned),
            priority: priority.to_owned(),
            confidence: "high".into(),
            rationale: "지난 캐시 작업도 areum".into(),
            cited: vec![],
        }
    }

    #[test]
    fn a_proposal_is_written_once_at_a_time_and_only_for_the_inbox() {
        let dir = TempDir::new("proposal-life");
        let mut store = board(&dir);
        let id = inbox_card(&store, "1");
        store.start_proposal(&id, &[], "triage-cmd").unwrap();
        let again = store.start_proposal(&id, &[], "triage-cmd").unwrap_err();
        assert_eq!(again.kind, ErrorKind::Conflict);
        let failed = store.fail_proposal(&id, "timed out").unwrap();
        assert_eq!(failed.state, "failed");

        // A finished proposal is replaced by a new request.
        store.start_proposal(&id, &[], "triage-cmd").unwrap();
        let ready = store
            .finish_proposal(&id, &draft(Some("areum"), "P1"))
            .unwrap();
        assert_eq!(ready.state, "ready");
        assert_eq!(ready.error, None);

        store.triage_card(&id, "areum", "P1", "human").unwrap();
        let late = store.start_proposal(&id, &[], "triage-cmd").unwrap_err();
        assert_eq!(late.kind, ErrorKind::Conflict);
    }

    #[test]
    fn a_taken_proposal_is_accepted_and_the_assignment_is_filed_under_triage() {
        let dir = TempDir::new("proposal-taken");
        let mut store = board(&dir);
        let id = inbox_card(&store, "1");
        store.start_proposal(&id, &[], "c").unwrap();
        store
            .finish_proposal(&id, &draft(Some("areum"), "P1"))
            .unwrap();
        store.triage_card(&id, "areum", "P1", "human").unwrap();

        assert_eq!(
            store.get_proposal(&id).unwrap().unwrap().accepted,
            Some(true)
        );
        let decisions = store
            .list_decisions(&DecisionFilter {
                kind: Some("triage"),
                limit: 10,
                ..DecisionFilter::default()
            })
            .unwrap();
        assert_eq!(decisions[0].category.as_deref(), Some("triage"));
        assert_eq!(decisions[0].category_by.as_deref(), Some("human"));
        assert!(store.review_queue().unwrap().is_empty());
    }

    #[test]
    fn an_overridden_proposal_becomes_a_golden_candidate_judged_once() {
        let dir = TempDir::new("proposal-override");
        let mut store = board(&dir);
        let id = inbox_card(&store, "1");
        store.start_proposal(&id, &[], "c").unwrap();
        store
            .finish_proposal(&id, &draft(Some("areum"), "P1"))
            .unwrap();
        store.triage_card(&id, "blog", "P1", "human").unwrap();
        // A later re-assignment is not a second judgment.
        store.triage_card(&id, "areum", "P1", "human").unwrap();

        assert_eq!(
            store.get_proposal(&id).unwrap().unwrap().accepted,
            Some(false)
        );
        let queue = store.review_queue().unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].reason, "override");
        assert_eq!(queue[0].answers, ["blog", "P1"]);

        let stats = store.proposal_stats().unwrap();
        assert_eq!((stats.proposals, stats.judged, stats.accepted), (1, 1, 0));
    }

    #[test]
    fn only_a_proposal_with_a_project_can_be_applied() {
        let dir = TempDir::new("proposal-apply");
        let store = board(&dir);
        let none = inbox_card(&store, "1");
        store.start_proposal(&none, &[], "c").unwrap();
        store.finish_proposal(&none, &draft(None, "P2")).unwrap();
        assert_eq!(
            store.mark_proposal_applied(&none).unwrap_err().kind,
            ErrorKind::Conflict
        );
        let some = inbox_card(&store, "2");
        store.start_proposal(&some, &[], "c").unwrap();
        store
            .finish_proposal(&some, &draft(Some("areum"), "P2"))
            .unwrap();
        store.mark_proposal_applied(&some).unwrap();
        assert_eq!(store.proposal_stats().unwrap().applied, 1);
    }
}
