//! `cards` — the board (docs/doppel.md §6). Every transition is one guarded
//! UPDATE, so a card can only move along an allowed edge and two dispatchers
//! never claim the same card or exceed the running limit together.

use rusqlite::{OptionalExtension, Row, params};

use super::{Store, conflict, constraint_err, map_sqlite_err, next_id, not_found};
use crate::common::error::{AppError, ErrorKind};
use crate::common::store::{ChoiceOption, NewDecision};

/// The seed category assignments are filed under.
pub(crate) const TRIAGE_CATEGORY: &str = "triage";
/// How much of a card body a triage decision keeps as context.
const CONTEXT_CHARS: usize = 2_000;
/// How much of a session's final answer a card keeps.
const OUTCOME_CHARS: usize = 4_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewCard<'a> {
    pub(crate) source: &'a str,
    pub(crate) external_id: &'a str,
    pub(crate) title: &'a str,
    pub(crate) body: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CardRecord {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) external_id: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) state: String,
    pub(crate) project: Option<String>,
    pub(crate) priority: String,
    pub(crate) session_id: Option<String>,
    pub(crate) claimed_at: Option<String>,
    pub(crate) outcome: Option<String>,
    pub(crate) last_error: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

const CARD_COLUMNS: &str = "id, source, external_id, title, body, state, project, priority, \
     session_id, claimed_at, outcome, last_error, created_at, updated_at";

impl Store {
    /// A repeat of the same `(source, external_id)` is refused — the same
    /// message or issue never becomes two cards.
    pub(crate) fn add_card(&self, card: &NewCard<'_>) -> Result<CardRecord, AppError> {
        if card.title.trim().is_empty() {
            return Err(AppError::new(ErrorKind::Usage, "a card needs a title"));
        }
        let now = self.clock.now();
        let tx = self.conn.unchecked_transaction().map_err(map_sqlite_err)?;
        let id = next_id(&tx, "cards", "crd")?;
        tx.execute(
            "INSERT INTO cards (id, source, external_id, title, body, state, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'inbox', ?6, ?6)",
            params![id, card.source, card.external_id, card.title, card.body, now],
        )
        .map_err(|e| {
            constraint_err(
                e,
                format!("{}/{} is already on the board", card.source, card.external_id),
            )
        })?;
        tx.commit().map_err(map_sqlite_err)?;
        self.get_card(&id)
    }

    pub(crate) fn get_card(&self, id: &str) -> Result<CardRecord, AppError> {
        self.conn
            .query_row(
                &format!("SELECT {CARD_COLUMNS} FROM cards WHERE id = ?1"),
                params![id],
                row_to_card,
            )
            .optional()
            .map_err(map_sqlite_err)?
            .ok_or_else(|| not_found(format!("no card {id}")))
    }

    /// Board order: priority, then oldest first.
    pub(crate) fn list_cards(&self, state: Option<&str>) -> Result<Vec<CardRecord>, AppError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {CARD_COLUMNS} FROM cards WHERE (?1 IS NULL OR state = ?1)
                 ORDER BY priority, created_at, id"
            ))
            .map_err(map_sqlite_err)?;
        stmt.query_map(params![state], row_to_card)
            .map_err(map_sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite_err)
    }

    /// Assigns a project and priority to a card that is not being worked on.
    /// A person's assignment is a judgment and goes into the ledger.
    pub(crate) fn triage_card(
        &mut self,
        id: &str,
        project: &str,
        priority: &str,
        by: &str,
    ) -> Result<CardRecord, AppError> {
        self.get_project(project)?;
        let now = self.clock.now();
        let changed = self
            .conn
            .execute(
                "UPDATE cards SET state = 'triaged', project = ?2, priority = ?3, updated_at = ?4
                 WHERE id = ?1 AND state IN ('inbox','triaged')",
                params![id, project, priority, now],
            )
            .map_err(|e| {
                constraint_err(e, format!("cannot triage {id} as {project}/{priority}"))
            })?;
        if changed == 0 {
            return Err(refused(&self.get_card(id)?, "triage"));
        }
        let card = self.get_card(id)?;
        if by == "human" {
            self.record_triage(&card, project, priority, &now)?;
        }
        Ok(card)
    }

    /// A person's assignment into the ledger, filed under `triage` when that
    /// category is active — the kind decides the category, no model guesses
    /// it. When the assignment overrode doppel's proposal, the entry becomes
    /// a golden candidate so the next review sees it first.
    fn record_triage(
        &mut self,
        card: &CardRecord,
        project: &str,
        priority: &str,
        now: &str,
    ) -> Result<(), AppError> {
        let decision = self.triage_decision(card, now)?;
        let external_id = decision.external_id.clone();
        self.insert_decisions(&[decision])?;
        let decision_id: String = self
            .conn
            .query_row(
                "SELECT id FROM decisions WHERE source = 'doppel' AND external_id = ?1",
                params![external_id],
                |row| row.get(0),
            )
            .map_err(map_sqlite_err)?;
        let filed = self
            .get_category(TRIAGE_CATEGORY)
            .is_ok_and(|c| c.state == "active");
        if filed {
            self.categorize(&decision_id, TRIAGE_CATEGORY, "human")?;
        }
        let overrode = self.judge_proposal(&card.id, project, priority)? == Some(false);
        if overrode && filed {
            self.propose_golden(
                &decision_id,
                "override",
                Some("doppel 의 배정 제안을 뒤집음"),
            )?;
        }
        Ok(())
    }

    /// Atomically takes a triaged card for a new session, unless `max_running`
    /// cards already run. The check and the claim are one statement.
    pub(crate) fn claim_card(&self, id: &str, max_running: u32) -> Result<CardRecord, AppError> {
        let now = self.clock.now();
        let changed = self
            .conn
            .execute(
                "UPDATE cards SET state = 'running', claimed_at = ?2, session_id = NULL,
                   outcome = NULL, last_error = NULL, updated_at = ?2
                 WHERE id = ?1 AND state = 'triaged'
                   AND (SELECT COUNT(*) FROM cards WHERE state = 'running') < ?3",
                params![id, now, max_running],
            )
            .map_err(map_sqlite_err)?;
        if changed == 1 {
            return self.get_card(id);
        }
        let card = self.get_card(id)?;
        if card.state != "triaged" {
            return Err(refused(&card, "dispatch"));
        }
        Err(conflict(format!(
            "{max_running} card(s) already running; dispatch {id} when one finishes"
        )))
    }

    pub(crate) fn set_card_session(&self, id: &str, session_id: &str) -> Result<(), AppError> {
        let now = self.clock.now();
        self.conn
            .execute(
                "UPDATE cards SET session_id = ?2, updated_at = ?3 WHERE id = ?1 AND state = 'running'",
                params![id, session_id, now],
            )
            .map_err(map_sqlite_err)?;
        Ok(())
    }

    /// The session answered: the card waits for a person to look.
    pub(crate) fn finish_card(&self, id: &str, outcome: &str) -> Result<CardRecord, AppError> {
        let now = self.clock.now();
        let kept: String = outcome.chars().take(OUTCOME_CHARS).collect();
        let changed = self
            .conn
            .execute(
                "UPDATE cards SET state = 'review', outcome = ?2, updated_at = ?3
                 WHERE id = ?1 AND state = 'running'",
                params![id, kept, now],
            )
            .map_err(map_sqlite_err)?;
        if changed == 0 {
            return Err(refused(&self.get_card(id)?, "finish"));
        }
        self.get_card(id)
    }

    /// Back to `triaged`: the session failed, or a person wants it done
    /// again. Failure is not a state — the reason is kept in `last_error`.
    pub(crate) fn release_card(&self, id: &str, reason: &str) -> Result<CardRecord, AppError> {
        let now = self.clock.now();
        let changed = self
            .conn
            .execute(
                "UPDATE cards SET state = 'triaged', claimed_at = NULL, last_error = ?2, updated_at = ?3
                 WHERE id = ?1 AND state IN ('running','review')",
                params![id, reason, now],
            )
            .map_err(map_sqlite_err)?;
        if changed == 0 {
            return Err(refused(&self.get_card(id)?, "release"));
        }
        self.get_card(id)
    }

    pub(crate) fn done_card(&self, id: &str) -> Result<CardRecord, AppError> {
        let card = self.get_card(id)?;
        if card.state == "done" {
            return Err(refused(&card, "close"));
        }
        let now = self.clock.now();
        self.conn
            .execute(
                "UPDATE cards SET state = 'done', claimed_at = NULL, updated_at = ?2 WHERE id = ?1",
                params![id, now],
            )
            .map_err(map_sqlite_err)?;
        self.get_card(id)
    }

    /// Sessions the daemon launched end with it, so cards still `running`
    /// when a daemon starts belong to a previous run. Answers the ids moved.
    pub(crate) fn release_orphaned_cards(&self) -> Result<Vec<String>, AppError> {
        let orphaned: Vec<String> = self
            .list_cards(Some("running"))?
            .into_iter()
            .map(|c| c.id)
            .collect();
        for id in &orphaned {
            self.release_card(id, "the daemon stopped while the session ran")?;
        }
        Ok(orphaned)
    }

    /// One ledger entry per assignment, numbered per card so a re-triage is
    /// a new judgment rather than a duplicate of the first.
    fn triage_decision(&self, card: &CardRecord, now: &str) -> Result<NewDecision, AppError> {
        let earlier: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM decisions WHERE source = 'doppel' AND external_id LIKE ?1",
                params![format!("card:{}:%", card.id)],
                |row| row.get(0),
            )
            .map_err(map_sqlite_err)?;
        let options = self
            .list_projects()?
            .into_iter()
            .map(|p| ChoiceOption {
                label: p.name,
                description: p.description,
            })
            .collect();
        Ok(NewDecision {
            source: "doppel",
            external_id: format!("card:{}:{}", card.id, earlier + 1),
            kind: "triage",
            project: card.project.clone(),
            session_id: None,
            question: format!("작업 배정: {}", card.title),
            options,
            answers: vec![
                card.project.clone().unwrap_or_default(),
                card.priority.clone(),
            ],
            rationale: None,
            context: Some(card.body.chars().take(CONTEXT_CHARS).collect()),
            decided_at: now.to_owned(),
        })
    }
}

fn refused(card: &CardRecord, action: &str) -> AppError {
    conflict(format!(
        "cannot {action} {} while it is {}",
        card.id, card.state
    ))
}

fn row_to_card(row: &Row<'_>) -> rusqlite::Result<CardRecord> {
    Ok(CardRecord {
        id: row.get(0)?,
        source: row.get(1)?,
        external_id: row.get(2)?,
        title: row.get(3)?,
        body: row.get(4)?,
        state: row.get(5)?,
        project: row.get(6)?,
        priority: row.get(7)?,
        session_id: row.get(8)?,
        claimed_at: row.get(9)?,
        outcome: row.get(10)?,
        last_error: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::{TempDir, memory_store};
    use crate::common::store::DecisionFilter;

    fn card(external_id: &str) -> NewCard<'_> {
        NewCard {
            source: "discord",
            external_id,
            title: "캐시가 계속 어긋남",
            body: "재시작하면 사라진다",
        }
    }

    fn board(dir: &TempDir) -> Store {
        let store = memory_store();
        store.seed_categories().unwrap();
        let path = dir.path().join("repo");
        std::fs::create_dir_all(&path).unwrap();
        store
            .add_project("areum", path.to_str().unwrap(), "로컬 도구")
            .unwrap();
        store
    }

    #[test]
    fn the_same_source_record_is_one_card() {
        let dir = TempDir::new("cards-dup");
        let store = board(&dir);
        let first = store.add_card(&card("m1")).unwrap();
        assert_eq!(
            (first.id.as_str(), first.state.as_str()),
            ("crd-000001", "inbox")
        );
        assert_eq!(
            store.add_card(&card("m1")).unwrap_err().kind,
            ErrorKind::Conflict
        );
    }

    #[test]
    fn a_person_triaging_writes_a_numbered_ledger_entry() {
        let dir = TempDir::new("cards-triage");
        let mut store = board(&dir);
        let id = store.add_card(&card("m1")).unwrap().id;
        let triaged = store.triage_card(&id, "areum", "P1", "human").unwrap();
        assert_eq!(triaged.state, "triaged");
        store.triage_card(&id, "areum", "P0", "human").unwrap();
        store.triage_card(&id, "areum", "P3", "model").unwrap();

        let entries = store
            .list_decisions(&DecisionFilter {
                kind: Some("triage"),
                limit: 10,
                ..DecisionFilter::default()
            })
            .unwrap();
        let mut keys: Vec<_> = entries.iter().map(|d| d.external_id.as_str()).collect();
        keys.sort();
        assert_eq!(keys, ["card:crd-000001:1", "card:crd-000001:2"]);
        let latest = entries
            .iter()
            .find(|d| d.external_id.ends_with(":2"))
            .unwrap();
        assert_eq!(latest.answers, ["areum", "P0"]);
        assert_eq!(latest.options[0].label, "areum");
    }

    #[test]
    fn triage_needs_a_known_project_and_a_card_not_in_flight() {
        let dir = TempDir::new("cards-triage-guard");
        let mut store = board(&dir);
        let id = store.add_card(&card("m1")).unwrap().id;
        assert_eq!(
            store
                .triage_card(&id, "nope", "P1", "human")
                .unwrap_err()
                .kind,
            ErrorKind::NotFound
        );
        store.triage_card(&id, "areum", "P1", "human").unwrap();
        store.claim_card(&id, 2).unwrap();
        assert_eq!(
            store
                .triage_card(&id, "areum", "P1", "human")
                .unwrap_err()
                .kind,
            ErrorKind::Conflict
        );
    }

    #[test]
    fn claiming_respects_state_and_the_running_limit() {
        let dir = TempDir::new("cards-claim");
        let mut store = board(&dir);
        let a = store.add_card(&card("a")).unwrap().id;
        let b = store.add_card(&card("b")).unwrap().id;
        assert_eq!(
            store.claim_card(&a, 2).unwrap_err().kind,
            ErrorKind::Conflict
        );
        store.triage_card(&a, "areum", "P1", "human").unwrap();
        store.triage_card(&b, "areum", "P1", "human").unwrap();

        let running = store.claim_card(&a, 1).unwrap();
        assert_eq!(running.state, "running");
        assert!(running.claimed_at.is_some());
        let limited = store.claim_card(&b, 1).unwrap_err();
        assert_eq!(limited.kind, ErrorKind::Conflict);
        assert!(limited.message.contains("already running"));
    }

    #[test]
    fn a_run_ends_in_review_or_back_in_triaged() {
        let dir = TempDir::new("cards-run");
        let mut store = board(&dir);
        let id = store.add_card(&card("m1")).unwrap().id;
        store.triage_card(&id, "areum", "P1", "human").unwrap();
        store.claim_card(&id, 2).unwrap();
        store.set_card_session(&id, "sess-9").unwrap();
        let reviewed = store.finish_card(&id, "고쳤습니다").unwrap();
        assert_eq!(reviewed.state, "review");
        assert_eq!(reviewed.session_id.as_deref(), Some("sess-9"));

        let again = store.release_card(&id, "테스트가 빠졌다").unwrap();
        assert_eq!(again.state, "triaged");
        assert_eq!(again.last_error.as_deref(), Some("테스트가 빠졌다"));
        assert_eq!(again.claimed_at, None);

        store.claim_card(&id, 2).unwrap();
        let failed = store.release_card(&id, "no result").unwrap();
        assert_eq!(failed.state, "triaged");
        assert_eq!(
            store.finish_card(&id, "x").unwrap_err().kind,
            ErrorKind::Conflict
        );
    }

    #[test]
    fn done_is_final_and_orphans_are_released() {
        let dir = TempDir::new("cards-done");
        let mut store = board(&dir);
        let id = store.add_card(&card("m1")).unwrap().id;
        let other = store.add_card(&card("m2")).unwrap().id;
        store.triage_card(&other, "areum", "P1", "human").unwrap();
        store.claim_card(&other, 2).unwrap();

        assert_eq!(
            store.release_orphaned_cards().unwrap(),
            std::slice::from_ref(&other)
        );
        assert_eq!(store.get_card(&other).unwrap().state, "triaged");

        store.done_card(&id).unwrap();
        assert_eq!(store.done_card(&id).unwrap_err().kind, ErrorKind::Conflict);
    }

    #[test]
    fn the_board_lists_by_priority_then_age() {
        let dir = TempDir::new("cards-order");
        let mut store = board(&dir);
        let low = store.add_card(&card("low")).unwrap().id;
        let high = store.add_card(&card("high")).unwrap().id;
        store.triage_card(&low, "areum", "P3", "human").unwrap();
        store.triage_card(&high, "areum", "P0", "human").unwrap();
        let ids: Vec<_> = store
            .list_cards(Some("triaged"))
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, [high, low]);
    }
}
