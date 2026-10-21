//! `golden` — precedents. A candidate copies its decision at proposal time,
//! so later refiling in the ledger never changes what the user reviewed.
//! Every candidate is also written to `golden_fts` in the same transaction;
//! the copied text never changes afterwards, so there is no index update path.

use rusqlite::{OptionalExtension, Row, params};

use super::{
    Store, conflict, constraint_err, decode_json, encode_json, map_sqlite_err, next_id, not_found,
};
use crate::common::error::AppError;
use crate::common::store::ChoiceOption;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GoldenRecord {
    pub(crate) id: String,
    pub(crate) decision_id: String,
    pub(crate) category: String,
    pub(crate) project: Option<String>,
    pub(crate) question: String,
    pub(crate) options: Vec<ChoiceOption>,
    pub(crate) answers: Vec<String>,
    pub(crate) rationale: Option<String>,
    pub(crate) context: Option<String>,
    pub(crate) decided_at: String,
    pub(crate) state: String,
    pub(crate) reason: String,
    pub(crate) superseded_by: Option<String>,
    pub(crate) note: Option<String>,
    pub(crate) created_at: String,
    pub(crate) reviewed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct GoldenFilter<'a> {
    pub(crate) state: Option<&'a str>,
    pub(crate) category: Option<&'a str>,
}

pub(super) const GOLDEN_COLUMNS: &str = "g.id, g.decision_id, g.category, g.project, g.question, \
     g.options, g.answers, g.rationale, g.context, g.decided_at, g.state, g.reason, \
     g.superseded_by, g.note, g.created_at, g.reviewed_at";

impl Store {
    /// Makes a categorized decision a candidate. A decision has at most one
    /// golden entry, and an uncategorized one cannot be a precedent — it
    /// would match no category filter.
    pub(crate) fn propose_golden(
        &self,
        decision_id: &str,
        reason: &str,
        note: Option<&str>,
    ) -> Result<GoldenRecord, AppError> {
        let decision = self.get_decision(decision_id)?;
        let Some(category) = decision.category.as_deref() else {
            return Err(conflict(format!(
                "decision {decision_id} has no category; categorize it before proposing"
            )));
        };
        let answers = encode_json(&decision.answers);
        let now = self.clock.now();

        let tx = self.conn.unchecked_transaction().map_err(map_sqlite_err)?;
        let id = next_id(&tx, "golden", "gld")?;
        tx.execute(
            "INSERT INTO golden (id, decision_id, category, project, question, options, answers,
               rationale, context, decided_at, state, reason, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'candidate', ?11, ?12, ?13)",
            params![
                id,
                decision.id,
                category,
                decision.project,
                decision.question,
                encode_json(&decision.options),
                answers,
                decision.rationale,
                decision.context,
                decision.decided_at,
                reason,
                note,
                now,
            ],
        )
        .map_err(|e| {
            constraint_err(
                e,
                format!("decision {decision_id} already has a golden entry"),
            )
        })?;
        tx.execute(
            "INSERT INTO golden_fts (golden_id, question, answers, rationale, context)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                decision.question,
                decision.answers.join("\n"),
                decision.rationale.as_deref().unwrap_or(""),
                decision.context.as_deref().unwrap_or(""),
            ],
        )
        .map_err(map_sqlite_err)?;
        tx.commit().map_err(map_sqlite_err)?;
        self.get_golden(&id)
    }

    pub(crate) fn confirm_golden(
        &self,
        id: &str,
        note: Option<&str>,
    ) -> Result<GoldenRecord, AppError> {
        let current = self.get_golden(id)?;
        if current.state != "candidate" {
            return Err(conflict(format!(
                "golden {id} is {}; only a candidate can be confirmed",
                current.state
            )));
        }
        let now = self.clock.now();
        self.conn
            .execute(
                "UPDATE golden SET state = 'confirmed', reviewed_at = ?2, note = COALESCE(?3, note)
                 WHERE id = ?1",
                params![id, now, note],
            )
            .map_err(|e| constraint_err(e, format!("cannot confirm {id}")))?;
        self.get_golden(id)
    }

    /// Retires a candidate (dropped in review) or a precedent (no longer
    /// holds). The replacement, when named, must be a different confirmed
    /// precedent — pointing at a candidate would let search lose the rule.
    pub(crate) fn retire_golden(
        &self,
        id: &str,
        superseded_by: Option<&str>,
        note: Option<&str>,
    ) -> Result<GoldenRecord, AppError> {
        let current = self.get_golden(id)?;
        if current.state == "retired" {
            return Err(conflict(format!("golden {id} is already retired")));
        }
        if let Some(replacement) = superseded_by {
            if replacement == id {
                return Err(conflict(format!("golden {id} cannot supersede itself")));
            }
            let target = self.get_golden(replacement)?;
            if target.state != "confirmed" {
                return Err(conflict(format!(
                    "golden {replacement} is {}; only a confirmed precedent can supersede",
                    target.state
                )));
            }
        }
        let now = self.clock.now();
        self.conn
            .execute(
                "UPDATE golden SET state = 'retired', superseded_by = ?2,
                   reviewed_at = COALESCE(reviewed_at, ?3), note = COALESCE(?4, note)
                 WHERE id = ?1",
                params![id, superseded_by, now, note],
            )
            .map_err(|e| constraint_err(e, format!("cannot retire {id}")))?;
        self.get_golden(id)
    }

    pub(crate) fn get_golden(&self, id: &str) -> Result<GoldenRecord, AppError> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {GOLDEN_COLUMNS} FROM golden g WHERE g.id = ?1"),
                params![id],
                row_to_raw_golden,
            )
            .optional()
            .map_err(map_sqlite_err)?
            .ok_or_else(|| not_found(format!("no golden {id}")))?;
        raw.decode()
    }

    pub(crate) fn list_golden(
        &self,
        filter: &GoldenFilter<'_>,
    ) -> Result<Vec<GoldenRecord>, AppError> {
        self.query_golden(
            &format!(
                "SELECT {GOLDEN_COLUMNS} FROM golden g
                 WHERE (?1 IS NULL OR g.state = ?1) AND (?2 IS NULL OR g.category = ?2)
                 ORDER BY g.id"
            ),
            params![filter.state, filter.category],
        )
    }

    /// Candidates in review order: overrides first (the recommendation was
    /// wrong — the most informative cases), then oldest proposal first.
    pub(crate) fn review_queue(&self) -> Result<Vec<GoldenRecord>, AppError> {
        self.query_golden(
            &format!(
                "SELECT {GOLDEN_COLUMNS} FROM golden g WHERE g.state = 'candidate'
                 ORDER BY CASE g.reason WHEN 'override' THEN 0 ELSE 1 END, g.created_at, g.id"
            ),
            [],
        )
    }

    fn query_golden(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<GoldenRecord>, AppError> {
        let mut stmt = self.conn.prepare(sql).map_err(map_sqlite_err)?;
        let rows = stmt
            .query_map(params, row_to_raw_golden)
            .map_err(map_sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite_err)?;
        rows.into_iter().map(RawGolden::decode).collect()
    }
}

/// A row as stored, JSON columns still encoded.
pub(super) struct RawGolden {
    id: String,
    decision_id: String,
    category: String,
    project: Option<String>,
    question: String,
    options: String,
    answers: String,
    rationale: Option<String>,
    context: Option<String>,
    decided_at: String,
    state: String,
    reason: String,
    superseded_by: Option<String>,
    note: Option<String>,
    created_at: String,
    reviewed_at: Option<String>,
}

impl RawGolden {
    pub(super) fn decode(self) -> Result<GoldenRecord, AppError> {
        Ok(GoldenRecord {
            options: decode_json(&self.options, "golden.options")?,
            answers: decode_json(&self.answers, "golden.answers")?,
            id: self.id,
            decision_id: self.decision_id,
            category: self.category,
            project: self.project,
            question: self.question,
            rationale: self.rationale,
            context: self.context,
            decided_at: self.decided_at,
            state: self.state,
            reason: self.reason,
            superseded_by: self.superseded_by,
            note: self.note,
            created_at: self.created_at,
            reviewed_at: self.reviewed_at,
        })
    }
}

/// Maps the [`GOLDEN_COLUMNS`] prefix of a row; columns after them are the
/// caller's (search appends its score).
pub(super) fn row_to_raw_golden(row: &Row<'_>) -> rusqlite::Result<RawGolden> {
    Ok(RawGolden {
        id: row.get(0)?,
        decision_id: row.get(1)?,
        category: row.get(2)?,
        project: row.get(3)?,
        question: row.get(4)?,
        options: row.get(5)?,
        answers: row.get(6)?,
        rationale: row.get(7)?,
        context: row.get(8)?,
        decided_at: row.get(9)?,
        state: row.get(10)?,
        reason: row.get(11)?,
        superseded_by: row.get(12)?,
        note: row.get(13)?,
        created_at: row.get(14)?,
        reviewed_at: row.get(15)?,
    })
}
