//! `briefs` — one per ask: the precedent-backed recommendation the brief
//! command wrote, and, once the ask is settled, whether the user took it.

use rusqlite::{OptionalExtension, Row, params};
use serde_json::Value;

use super::{Store, conflict, constraint_err, decode_json, encode_json, map_sqlite_err, not_found};
use crate::common::error::AppError;

/// A validated brief as the runner produced it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BriefDraft {
    pub(crate) background: Option<String>,
    pub(crate) recommendation: Option<Value>,
    pub(crate) new_opinion: Option<String>,
    pub(crate) confidence: String,
    pub(crate) reversible: Option<bool>,
    pub(crate) rationale: String,
    pub(crate) cited: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BriefRecord {
    pub(crate) ask_id: String,
    pub(crate) state: String,
    pub(crate) background: Option<String>,
    pub(crate) recommendation: Option<Value>,
    pub(crate) new_opinion: Option<String>,
    pub(crate) confidence: Option<String>,
    pub(crate) reversible: Option<bool>,
    pub(crate) rationale: Option<String>,
    pub(crate) cited: Vec<String>,
    pub(crate) precedents: Vec<String>,
    pub(crate) command: String,
    pub(crate) error: Option<String>,
    pub(crate) accepted: Option<bool>,
    pub(crate) created_at: String,
    pub(crate) finished_at: Option<String>,
}

/// Acceptance over settled briefs that made a recommendation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BriefStats {
    pub(crate) briefs: u32,
    pub(crate) failed: u32,
    pub(crate) new_opinions: u32,
    pub(crate) judged: u32,
    pub(crate) accepted: u32,
    pub(crate) by_category: Vec<CategoryStats>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CategoryStats {
    /// `None` for asks whose ledger entries are not categorized yet.
    pub(crate) category: Option<String>,
    pub(crate) judged: u32,
    pub(crate) accepted: u32,
}

const BRIEF_COLUMNS: &str = "ask_id, state, background, recommendation, new_opinion, confidence, \
     reversible, rationale, cited, precedents, command, error, accepted, created_at, finished_at";

impl Store {
    /// Marks the brief as being written. One brief per ask: a second start
    /// is a conflict, so a re-opened ask never runs the command twice.
    pub(crate) fn start_brief(
        &self,
        ask_id: &str,
        precedents: &[String],
        command: &str,
    ) -> Result<BriefRecord, AppError> {
        self.get_ask(ask_id)?;
        let now = self.clock.now();
        self.conn
            .execute(
                "INSERT INTO briefs (ask_id, state, precedents, command, created_at)
                 VALUES (?1, 'pending', ?2, ?3, ?4)",
                params![ask_id, encode_json(&precedents), command, now],
            )
            .map_err(|e| constraint_err(e, format!("ask {ask_id} already has a brief")))?;
        self.require_brief(ask_id)
    }

    pub(crate) fn finish_brief(
        &self,
        ask_id: &str,
        draft: &BriefDraft,
    ) -> Result<BriefRecord, AppError> {
        let now = self.clock.now();
        let changed = self
            .conn
            .execute(
                "UPDATE briefs SET state = 'ready', background = ?2, recommendation = ?3,
                   new_opinion = ?4, confidence = ?5, reversible = ?6, rationale = ?7, cited = ?8,
                   finished_at = ?9
                 WHERE ask_id = ?1 AND state = 'pending'",
                params![
                    ask_id,
                    draft.background,
                    draft.recommendation.as_ref().map(encode_json),
                    draft.new_opinion,
                    draft.confidence,
                    draft.reversible,
                    draft.rationale,
                    encode_json(&draft.cited),
                    now,
                ],
            )
            .map_err(|e| constraint_err(e, format!("brief for {ask_id} violates the schema")))?;
        if changed == 0 {
            return Err(conflict(format!("brief for {ask_id} is not pending")));
        }
        self.require_brief(ask_id)
    }

    pub(crate) fn fail_brief(&self, ask_id: &str, error: &str) -> Result<BriefRecord, AppError> {
        let now = self.clock.now();
        self.conn
            .execute(
                "UPDATE briefs SET state = 'failed', error = ?2, finished_at = ?3
                 WHERE ask_id = ?1 AND state = 'pending'",
                params![ask_id, error, now],
            )
            .map_err(map_sqlite_err)?;
        self.require_brief(ask_id)
    }

    pub(crate) fn get_brief(&self, ask_id: &str) -> Result<Option<BriefRecord>, AppError> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {BRIEF_COLUMNS} FROM briefs WHERE ask_id = ?1"),
                params![ask_id],
                row_to_raw,
            )
            .optional()
            .map_err(map_sqlite_err)?;
        raw.map(RawBrief::decode).transpose()
    }

    /// Records whether the settled answer took the recommendation. Called
    /// when the ask closes; a brief without a recommendation stays unjudged.
    pub(super) fn judge_brief(&self, ask_id: &str, final_answers: &Value) -> Result<(), AppError> {
        let Some(brief) = self.get_brief(ask_id)? else {
            return Ok(());
        };
        let Some(recommended) = brief.recommendation.filter(|_| brief.state == "ready") else {
            return Ok(());
        };
        let accepted = same_answers(&recommended, final_answers);
        self.conn
            .execute(
                "UPDATE briefs SET accepted = ?2 WHERE ask_id = ?1",
                params![ask_id, accepted],
            )
            .map_err(map_sqlite_err)?;
        Ok(())
    }

    pub(crate) fn brief_stats(&self) -> Result<BriefStats, AppError> {
        let (briefs, failed, new_opinions, judged, accepted) = self
            .conn
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(state = 'failed'), 0),
                        COALESCE(SUM(state = 'ready' AND recommendation IS NULL), 0),
                        COALESCE(SUM(accepted IS NOT NULL), 0),
                        COALESCE(SUM(accepted = 1), 0)
                 FROM briefs",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .map_err(map_sqlite_err)?;
        // An ask's ledger entries share its `<session>:<tool_use_id>` key
        // prefix; the first question's filing stands for the ask.
        let mut stmt = self
            .conn
            .prepare(
                "SELECT d.category, COUNT(*), COALESCE(SUM(b.accepted = 1), 0)
                 FROM briefs b
                 JOIN asks a ON a.id = b.ask_id
                 LEFT JOIN decisions d ON d.source = 'session'
                   AND d.external_id IN (a.session_id || ':' || a.tool_use_id,
                                         a.session_id || ':' || a.tool_use_id || ':0')
                 WHERE b.accepted IS NOT NULL
                 GROUP BY d.category
                 ORDER BY d.category IS NULL, d.category",
            )
            .map_err(map_sqlite_err)?;
        let by_category = stmt
            .query_map([], |row| {
                Ok(CategoryStats {
                    category: row.get(0)?,
                    judged: row.get(1)?,
                    accepted: row.get(2)?,
                })
            })
            .map_err(map_sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite_err)?;
        Ok(BriefStats {
            briefs,
            failed,
            new_opinions,
            judged,
            accepted,
            by_category,
        })
    }

    fn require_brief(&self, ask_id: &str) -> Result<BriefRecord, AppError> {
        self.get_brief(ask_id)?
            .ok_or_else(|| not_found(format!("no brief for {ask_id}")))
    }
}

/// A single label and a one-element list are the same answer; list order is
/// not significant.
fn same_answers(a: &Value, b: &Value) -> bool {
    let labels = |v: &Value| -> Option<Vec<String>> {
        let mut list: Vec<String> = match v {
            Value::String(s) => vec![s.clone()],
            Value::Array(items) => items
                .iter()
                .map(|i| i.as_str().map(str::to_owned))
                .collect::<Option<_>>()?,
            _ => return None,
        };
        list.sort();
        Some(list)
    };
    match (a.as_object(), b.as_object()) {
        (Some(a), Some(b)) => {
            a.len() == b.len()
                && a.iter().all(|(question, answer)| {
                    b.get(question).is_some_and(|other| {
                        labels(answer).is_some() && labels(answer) == labels(other)
                    })
                })
        }
        _ => false,
    }
}

struct RawBrief {
    ask_id: String,
    state: String,
    background: Option<String>,
    recommendation: Option<String>,
    new_opinion: Option<String>,
    confidence: Option<String>,
    reversible: Option<bool>,
    rationale: Option<String>,
    cited: String,
    precedents: String,
    command: String,
    error: Option<String>,
    accepted: Option<bool>,
    created_at: String,
    finished_at: Option<String>,
}

impl RawBrief {
    fn decode(self) -> Result<BriefRecord, AppError> {
        Ok(BriefRecord {
            recommendation: self
                .recommendation
                .as_deref()
                .map(|r| decode_json(r, "briefs.recommendation"))
                .transpose()?,
            cited: decode_json(&self.cited, "briefs.cited")?,
            precedents: decode_json(&self.precedents, "briefs.precedents")?,
            ask_id: self.ask_id,
            state: self.state,
            background: self.background,
            new_opinion: self.new_opinion,
            confidence: self.confidence,
            reversible: self.reversible,
            rationale: self.rationale,
            command: self.command,
            error: self.error,
            accepted: self.accepted,
            created_at: self.created_at,
            finished_at: self.finished_at,
        })
    }
}

fn row_to_raw(row: &Row<'_>) -> rusqlite::Result<RawBrief> {
    Ok(RawBrief {
        ask_id: row.get(0)?,
        state: row.get(1)?,
        background: row.get(2)?,
        recommendation: row.get(3)?,
        new_opinion: row.get(4)?,
        confidence: row.get(5)?,
        reversible: row.get(6)?,
        rationale: row.get(7)?,
        cited: row.get(8)?,
        precedents: row.get(9)?,
        command: row.get(10)?,
        error: row.get(11)?,
        accepted: row.get(12)?,
        created_at: row.get(13)?,
        finished_at: row.get(14)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::memory_store;
    use crate::common::error::ErrorKind;
    use serde_json::json;

    fn draft(recommendation: Option<Value>) -> BriefDraft {
        BriefDraft {
            background: Some("캐시 설계 중".into()),
            new_opinion: recommendation
                .is_none()
                .then(|| "판례가 없어 새로 판단".to_owned()),
            recommendation,
            confidence: "high".into(),
            reversible: Some(true),
            rationale: "지난번에도 디스크".into(),
            cited: vec!["gld-000001".into()],
        }
    }

    fn opened(store: &Store) -> String {
        let questions = json!([{"question": "위치는?", "options": []}]);
        store.open_ask("s1", "t1", "ask", &questions).unwrap().id
    }

    #[test]
    fn a_brief_goes_from_pending_to_ready_once() {
        let store = memory_store();
        let ask = opened(&store);
        let started = store
            .start_brief(&ask, &["gld-000001".into()], "claude -p /doppel:brief")
            .unwrap();
        assert_eq!(started.state, "pending");
        assert_eq!(
            store.start_brief(&ask, &[], "x").unwrap_err().kind,
            ErrorKind::Conflict
        );

        let ready = store
            .finish_brief(&ask, &draft(Some(json!({"위치는?": "디스크"}))))
            .unwrap();
        assert_eq!(ready.state, "ready");
        assert_eq!(ready.precedents, ["gld-000001"]);
        assert_eq!(ready.reversible, Some(true));
        assert_eq!(
            store.finish_brief(&ask, &draft(None)).unwrap_err().kind,
            ErrorKind::Conflict
        );
    }

    #[test]
    fn a_failed_brief_keeps_its_reason() {
        let store = memory_store();
        let ask = opened(&store);
        store.start_brief(&ask, &[], "x").unwrap();
        let failed = store.fail_brief(&ask, "not JSON").unwrap();
        assert_eq!(failed.state, "failed");
        assert_eq!(failed.error.as_deref(), Some("not JSON"));
    }

    #[test]
    fn acceptance_compares_answers_regardless_of_shape_and_order() {
        assert!(same_answers(&json!({"q": "A"}), &json!({"q": ["A"]})));
        assert!(same_answers(
            &json!({"q": ["A", "B"]}),
            &json!({"q": ["B", "A"]})
        ));
        assert!(!same_answers(&json!({"q": "A"}), &json!({"q": "B"})));
        assert!(!same_answers(
            &json!({"q": "A"}),
            &json!({"q": "A", "r": "B"})
        ));
    }

    #[test]
    fn closing_judges_the_recommendation_and_stats_count_it() {
        let mut store = memory_store();
        let ask = opened(&store);
        store.start_brief(&ask, &[], "x").unwrap();
        store
            .finish_brief(&ask, &draft(Some(json!({"위치는?": "디스크"}))))
            .unwrap();
        store
            .close_ask(&ask, Some(&json!({"위치는?": "디스크"})), "doppel-ui")
            .unwrap();
        assert_eq!(store.get_brief(&ask).unwrap().unwrap().accepted, Some(true));

        let questions = json!([{"question": "범위는?", "options": []}]);
        let other = store.open_ask("s1", "t2", "ask", &questions).unwrap().id;
        store.start_brief(&other, &[], "x").unwrap();
        store
            .finish_brief(&other, &draft(Some(json!({"범위는?": "API"}))))
            .unwrap();
        store
            .close_ask(&other, Some(&json!({"범위는?": "CLI"})), "session")
            .unwrap();

        store.seed_categories().unwrap();
        store
            .categorize("dec-000001", "design-scope", "human")
            .unwrap();

        let stats = store.brief_stats().unwrap();
        assert_eq!((stats.briefs, stats.judged, stats.accepted), (2, 2, 1));
        assert_eq!(
            stats.by_category,
            [
                CategoryStats {
                    category: Some("design-scope".into()),
                    judged: 1,
                    accepted: 1
                },
                CategoryStats {
                    category: None,
                    judged: 1,
                    accepted: 0
                },
            ]
        );
    }

    #[test]
    fn a_new_opinion_is_not_judged() {
        let mut store = memory_store();
        let ask = opened(&store);
        store.start_brief(&ask, &[], "x").unwrap();
        store.finish_brief(&ask, &draft(None)).unwrap();
        store
            .close_ask(&ask, Some(&json!({"위치는?": "디스크"})), "session")
            .unwrap();
        assert_eq!(store.get_brief(&ask).unwrap().unwrap().accepted, None);
        assert_eq!(store.brief_stats().unwrap().new_opinions, 1);
    }
}
