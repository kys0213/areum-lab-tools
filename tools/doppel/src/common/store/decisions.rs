//! `decisions` — the judgment ledger. One row per moment the user decided
//! something. Rows are never deleted; re-ingesting the same source record is
//! a no-op thanks to `UNIQUE(source, external_id)`.

use rusqlite::{OptionalExtension, Row, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use super::{Store, constraint_err, decode_json, encode_json, map_sqlite_err, next_id, not_found};
use crate::common::error::AppError;

/// One choice the question offered. Stored as JSON in `options`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChoiceOption {
    pub(crate) label: String,
    pub(crate) description: String,
}

/// A ledger entry as an ingester produces it. The id and `created_at` are
/// minted here, so neither is part of the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewDecision {
    pub(crate) source: &'static str,
    pub(crate) external_id: String,
    pub(crate) kind: &'static str,
    pub(crate) project: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) question: String,
    pub(crate) options: Vec<ChoiceOption>,
    pub(crate) answers: Vec<String>,
    pub(crate) rationale: Option<String>,
    pub(crate) context: Option<String>,
    pub(crate) decided_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecisionRecord {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) external_id: String,
    pub(crate) kind: String,
    pub(crate) project: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) question: String,
    pub(crate) options: Vec<ChoiceOption>,
    pub(crate) answers: Vec<String>,
    pub(crate) rationale: Option<String>,
    pub(crate) context: Option<String>,
    pub(crate) category: Option<String>,
    pub(crate) category_by: Option<String>,
    pub(crate) decided_at: String,
    pub(crate) created_at: String,
}

/// How a batch insert went: rows added, and rows whose `(source,
/// external_id)` was already in the ledger.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Insertion {
    pub(crate) added: usize,
    pub(crate) existing: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DecisionFilter<'a> {
    pub(crate) kind: Option<&'a str>,
    pub(crate) category: Option<&'a str>,
    pub(crate) uncategorized: bool,
    pub(crate) limit: u32,
}

/// Both ends of a categorization, so the caller can say what it replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Recategorization {
    pub(crate) previous_category: Option<String>,
    pub(crate) after: DecisionRecord,
}

const DECISION_COLUMNS: &str = "id, source, external_id, kind, project, session_id, question, \
     options, answers, rationale, context, category, category_by, decided_at, created_at";

impl Store {
    /// Inserts the batch in one transaction. A record already in the ledger
    /// is counted, not rewritten — the first ingest's copy stands.
    pub(crate) fn insert_decisions(
        &mut self,
        batch: &[NewDecision],
    ) -> Result<Insertion, AppError> {
        let now = self.clock.now();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_err)?;
        let mut outcome = Insertion::default();
        for decision in batch {
            let id = next_id(&tx, "decisions", "dec")?;
            let changed = tx
                .execute(
                    "INSERT INTO decisions (id, source, external_id, kind, project, session_id,
                       question, options, answers, rationale, context, decided_at, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                     ON CONFLICT(source, external_id) DO NOTHING",
                    params![
                        id,
                        decision.source,
                        decision.external_id,
                        decision.kind,
                        decision.project,
                        decision.session_id,
                        decision.question,
                        encode_json(&decision.options),
                        encode_json(&decision.answers),
                        decision.rationale,
                        decision.context,
                        decision.decided_at,
                        now,
                    ],
                )
                .map_err(|e| {
                    constraint_err(
                        e,
                        format!(
                            "decision {}/{} violates the ledger schema",
                            decision.source, decision.external_id
                        ),
                    )
                })?;
            if changed == 1 {
                outcome.added += 1;
            } else {
                outcome.existing += 1;
            }
        }
        tx.commit().map_err(map_sqlite_err)?;
        Ok(outcome)
    }

    pub(crate) fn get_decision(&self, id: &str) -> Result<DecisionRecord, AppError> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {DECISION_COLUMNS} FROM decisions WHERE id = ?1"),
                params![id],
                row_to_raw,
            )
            .optional()
            .map_err(map_sqlite_err)?
            .ok_or_else(|| not_found(format!("no decision {id}")))?;
        raw.decode()
    }

    /// Newest decision first; `id` breaks ties so the order is total.
    pub(crate) fn list_decisions(
        &self,
        filter: &DecisionFilter<'_>,
    ) -> Result<Vec<DecisionRecord>, AppError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {DECISION_COLUMNS} FROM decisions
                 WHERE (?1 IS NULL OR kind = ?1)
                   AND (?2 IS NULL OR category = ?2)
                   AND (?3 = 0 OR category IS NULL)
                 ORDER BY decided_at DESC, id DESC
                 LIMIT ?4"
            ))
            .map_err(map_sqlite_err)?;
        let rows = stmt
            .query_map(
                params![
                    filter.kind,
                    filter.category,
                    filter.uncategorized,
                    filter.limit
                ],
                row_to_raw,
            )
            .map_err(map_sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite_err)?;
        rows.into_iter().map(RawDecision::decode).collect()
    }

    /// Files a decision under an active category. Refiling replaces the
    /// previous filing; the golden copy, if any, keeps what was reviewed.
    pub(crate) fn categorize(
        &self,
        id: &str,
        category: &str,
        by: &str,
    ) -> Result<Recategorization, AppError> {
        let before = self.get_decision(id)?;
        self.require_active_category(category)?;
        self.conn
            .execute(
                "UPDATE decisions SET category = ?2, category_by = ?3 WHERE id = ?1",
                params![id, category, by],
            )
            .map_err(|e| constraint_err(e, format!("cannot file {id} under {category}")))?;
        Ok(Recategorization {
            previous_category: before.category,
            after: self.get_decision(id)?,
        })
    }
}

/// A row as stored, JSON columns still encoded — decoding can fail, which the
/// rusqlite row mapper cannot express in our error type.
struct RawDecision {
    id: String,
    source: String,
    external_id: String,
    kind: String,
    project: Option<String>,
    session_id: Option<String>,
    question: String,
    options: String,
    answers: String,
    rationale: Option<String>,
    context: Option<String>,
    category: Option<String>,
    category_by: Option<String>,
    decided_at: String,
    created_at: String,
}

impl RawDecision {
    fn decode(self) -> Result<DecisionRecord, AppError> {
        Ok(DecisionRecord {
            options: decode_json(&self.options, "decisions.options")?,
            answers: decode_json(&self.answers, "decisions.answers")?,
            id: self.id,
            source: self.source,
            external_id: self.external_id,
            kind: self.kind,
            project: self.project,
            session_id: self.session_id,
            question: self.question,
            rationale: self.rationale,
            context: self.context,
            category: self.category,
            category_by: self.category_by,
            decided_at: self.decided_at,
            created_at: self.created_at,
        })
    }
}

fn row_to_raw(row: &Row<'_>) -> rusqlite::Result<RawDecision> {
    Ok(RawDecision {
        id: row.get(0)?,
        source: row.get(1)?,
        external_id: row.get(2)?,
        kind: row.get(3)?,
        project: row.get(4)?,
        session_id: row.get(5)?,
        question: row.get(6)?,
        options: row.get(7)?,
        answers: row.get(8)?,
        rationale: row.get(9)?,
        context: row.get(10)?,
        category: row.get(11)?,
        category_by: row.get(12)?,
        decided_at: row.get(13)?,
        created_at: row.get(14)?,
    })
}
