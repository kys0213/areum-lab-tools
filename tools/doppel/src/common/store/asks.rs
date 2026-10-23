//! `asks` — questions and permission confirmations relayed from a session
//! (docs/doppel-protocol.md §6 "ask 상태"). The session's hook is the final
//! judge of which answer won, so doppel's own `answer` only reaches
//! `answered`; `close` from the hook is what settles an ask — and what writes
//! its answer into the judgment ledger.

use rusqlite::{OptionalExtension, Row, params};
use serde_json::Value;

use super::sessions::ensure_session;
use super::{
    Store, conflict, constraint_err, decode_json, encode_json, map_sqlite_err, next_id, not_found,
};
use crate::common::error::{AppError, ErrorKind};
use crate::common::store::{ChoiceOption, NewDecision};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AskRecord {
    pub(crate) id: String,
    pub(crate) session_id: String,
    pub(crate) tool_use_id: String,
    pub(crate) kind: String,
    /// The `AskUserQuestion` input, verbatim.
    pub(crate) questions: Value,
    pub(crate) state: String,
    /// `{ "<question>": "<label>" | ["<label>", ...] }`.
    pub(crate) answers: Option<Value>,
    pub(crate) answered_by: Option<String>,
    pub(crate) created_at: String,
    pub(crate) answered_at: Option<String>,
    pub(crate) deferred_at: Option<String>,
    pub(crate) closed_at: Option<String>,
}

const ASK_COLUMNS: &str = "id, session_id, tool_use_id, kind, questions, state, answers, \
     answered_by, created_at, answered_at, deferred_at, closed_at";

/// Who answered a permission at the terminal. Those decisions are already in
/// the transcript's `permissionDecision`, so the ledger does not take them
/// twice.
const ANSWERED_IN_SESSION: &str = "session";

impl Store {
    /// Opens an ask, or answers the existing one for the same tool call — the
    /// hook may retry a create whose response it lost.
    pub(crate) fn open_ask(
        &self,
        session_id: &str,
        tool_use_id: &str,
        kind: &str,
        questions: &Value,
    ) -> Result<AskRecord, AppError> {
        if !matches!(kind, "ask" | "permission") {
            return Err(usage(format!("unknown ask kind {kind}")));
        }
        question_texts(questions)?;
        let now = self.clock.now();
        let tx = self.conn.unchecked_transaction().map_err(map_sqlite_err)?;
        ensure_session(&tx, session_id, &now)?;
        let id = next_id(&tx, "asks", "ask")?;
        tx.execute(
            "INSERT INTO asks (id, session_id, tool_use_id, kind, questions, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6)
             ON CONFLICT(session_id, tool_use_id) DO NOTHING",
            params![
                id,
                session_id,
                tool_use_id,
                kind,
                encode_json(questions),
                now
            ],
        )
        .map_err(|e| constraint_err(e, format!("cannot open an ask for {tool_use_id}")))?;
        tx.commit().map_err(map_sqlite_err)?;
        self.ask_by_call(session_id, tool_use_id)
    }

    pub(crate) fn get_ask(&self, id: &str) -> Result<AskRecord, AppError> {
        self.query_one(
            &format!("SELECT {ASK_COLUMNS} FROM asks WHERE id = ?1"),
            params![id],
        )?
        .ok_or_else(|| not_found(format!("no ask {id}")))
    }

    /// Newest first.
    pub(crate) fn list_asks(&self, state: Option<&str>) -> Result<Vec<AskRecord>, AppError> {
        self.query_asks(
            &format!(
                "SELECT {ASK_COLUMNS} FROM asks WHERE (?1 IS NULL OR state = ?1)
                 ORDER BY created_at DESC, id DESC"
            ),
            params![state],
        )
    }

    /// A doppel surface's answer. Accepted while the ask waits; the hook
    /// confirms it with `close`.
    pub(crate) fn answer_ask(
        &self,
        id: &str,
        answers: &Value,
        answered_by: &str,
    ) -> Result<AskRecord, AppError> {
        let ask = self.get_ask(id)?;
        if !matches!(ask.state.as_str(), "open" | "deferred") {
            return Err(conflict(format!(
                "ask {id} is {}; it takes no answer",
                ask.state
            )));
        }
        validate_answers(&ask.questions, answers)?;
        let now = self.clock.now();
        self.conn
            .execute(
                "UPDATE asks SET state = 'answered', answers = ?2, answered_by = ?3, answered_at = ?4
                 WHERE id = ?1 AND state IN ('open','deferred')",
                params![id, encode_json(answers), answered_by, now],
            )
            .map_err(|e| constraint_err(e, format!("cannot answer {id}")))?;
        self.get_ask(id)
    }

    /// The hook stopped waiting (a headless session moved on); a later answer
    /// is delivered through the session's attach connection instead.
    pub(crate) fn defer_ask(&self, id: &str) -> Result<AskRecord, AppError> {
        let ask = self.get_ask(id)?;
        if ask.state != "open" {
            return Err(conflict(format!(
                "ask {id} is {}; only an open ask defers",
                ask.state
            )));
        }
        let now = self.clock.now();
        self.conn
            .execute(
                "UPDATE asks SET state = 'deferred', deferred_at = ?2 WHERE id = ?1",
                params![id, now],
            )
            .map_err(|e| constraint_err(e, format!("cannot defer {id}")))?;
        self.get_ask(id)
    }

    /// Settles the ask with the answer that won in the session, overriding a
    /// doppel answer that lost the race. Without `answers` (the session ended,
    /// the winner's answer is unknown) a doppel answer, if any, is kept.
    pub(crate) fn close_ask(
        &mut self,
        id: &str,
        answers: Option<&Value>,
        answered_by: &str,
    ) -> Result<AskRecord, AppError> {
        let ask = self.get_ask(id)?;
        if ask.state == "closed" {
            return Err(conflict(format!("ask {id} is already closed")));
        }
        if let Some(answers) = answers {
            validate_answers(&ask.questions, answers)?;
        }
        let now = self.clock.now();
        self.conn
            .execute(
                "UPDATE asks SET state = 'closed', answers = COALESCE(?2, answers),
                   answered_by = ?3, closed_at = ?4 WHERE id = ?1",
                params![id, answers.map(encode_json), answered_by, now],
            )
            .map_err(|e| constraint_err(e, format!("cannot close {id}")))?;
        let closed = self.get_ask(id)?;
        if let Some(final_answers) = &closed.answers {
            self.judge_brief(id, final_answers)?;
        }
        let decisions = self.ledger_entries(&closed, &now)?;
        self.insert_decisions(&decisions)?;
        Ok(closed)
    }

    /// Answers waiting for a session that deferred them, to push when its
    /// attach connection (re)opens.
    pub(crate) fn deliverable_answers(&self, session_id: &str) -> Result<Vec<AskRecord>, AppError> {
        self.query_asks(
            &format!(
                "SELECT {ASK_COLUMNS} FROM asks
                 WHERE session_id = ?1 AND state = 'answered' AND deferred_at IS NOT NULL
                 ORDER BY created_at, id"
            ),
            params![session_id],
        )
    }

    /// Asks of a session that wait on a late answer: deferred, or answered
    /// after deferring but not yet delivered and closed. While any remain, a
    /// session doppel launched is kept able to take a new turn.
    pub(crate) fn pending_late_answers(&self, session_id: &str) -> Result<u32, AppError> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM asks WHERE session_id = ?1
                   AND (state = 'deferred' OR (state = 'answered' AND deferred_at IS NOT NULL))",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(map_sqlite_err)
    }

    fn ask_by_call(&self, session_id: &str, tool_use_id: &str) -> Result<AskRecord, AppError> {
        self.query_one(
            &format!("SELECT {ASK_COLUMNS} FROM asks WHERE session_id = ?1 AND tool_use_id = ?2"),
            params![session_id, tool_use_id],
        )?
        .ok_or_else(|| not_found(format!("no ask for {session_id}/{tool_use_id}")))
    }

    /// The ledger rows a closed ask contributes, keyed like the transcript
    /// ingest so a later ingest finds them already there.
    fn ledger_entries(&self, ask: &AskRecord, now: &str) -> Result<Vec<NewDecision>, AppError> {
        let Some(answers) = &ask.answers else {
            return Ok(Vec::new());
        };
        if ask.kind == "permission" && ask.answered_by.as_deref() == Some(ANSWERED_IN_SESSION) {
            return Ok(Vec::new());
        }
        let project = self.get_session(&ask.session_id)?.cwd;
        let questions = ask.questions.as_array().map(Vec::as_slice).unwrap_or(&[]);
        let mut entries = Vec::new();
        for (index, question) in questions.iter().enumerate() {
            let Some(text) = question["question"].as_str() else {
                continue;
            };
            let Some(chosen) = answer_labels(&answers[text]) else {
                continue;
            };
            let external_id = if ask.kind == "permission" {
                format!("{}:{}", ask.session_id, ask.tool_use_id)
            } else {
                format!("{}:{}:{index}", ask.session_id, ask.tool_use_id)
            };
            entries.push(NewDecision {
                source: "session",
                external_id,
                kind: if ask.kind == "permission" {
                    "permission"
                } else {
                    "ask"
                },
                project: project.clone(),
                session_id: Some(ask.session_id.clone()),
                question: text.to_owned(),
                options: options_of(question),
                answers: chosen,
                rationale: None,
                context: None,
                decided_at: now.to_owned(),
            });
        }
        Ok(entries)
    }

    fn query_one(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Option<AskRecord>, AppError> {
        let raw = self
            .conn
            .query_row(sql, params, row_to_raw)
            .optional()
            .map_err(map_sqlite_err)?;
        raw.map(RawAsk::decode).transpose()
    }

    fn query_asks(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<AskRecord>, AppError> {
        let mut stmt = self.conn.prepare(sql).map_err(map_sqlite_err)?;
        let rows = stmt
            .query_map(params, row_to_raw)
            .map_err(map_sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite_err)?;
        rows.into_iter().map(RawAsk::decode).collect()
    }
}

fn usage(message: impl Into<String>) -> AppError {
    AppError::new(ErrorKind::Usage, message)
}

/// The question texts of an `AskUserQuestion` input; refuses anything else.
fn question_texts(questions: &Value) -> Result<Vec<&str>, AppError> {
    let list = questions
        .as_array()
        .filter(|l| !l.is_empty())
        .ok_or_else(|| usage("questions must be a non-empty array"))?;
    list.iter()
        .map(|q| {
            q["question"]
                .as_str()
                .ok_or_else(|| usage("every question needs a \"question\" string"))
        })
        .collect()
}

/// Every question answered, nothing else: a label or a non-empty label list.
pub(crate) fn validate_answers(questions: &Value, answers: &Value) -> Result<(), AppError> {
    let map = answers
        .as_object()
        .ok_or_else(|| usage("answers must be an object keyed by question"))?;
    let texts = question_texts(questions)?;
    for text in &texts {
        let answer = map
            .get(*text)
            .ok_or_else(|| usage(format!("no answer for \"{text}\"")))?;
        if answer_labels(answer).is_none() {
            return Err(usage(format!(
                "answer for \"{text}\" must be a string or a non-empty list of strings"
            )));
        }
    }
    if let Some(extra) = map.keys().find(|k| !texts.contains(&k.as_str())) {
        return Err(usage(format!("answer for unknown question \"{extra}\"")));
    }
    Ok(())
}

fn answer_labels(answer: &Value) -> Option<Vec<String>> {
    match answer {
        Value::String(s) => Some(vec![s.clone()]),
        Value::Array(items) if !items.is_empty() => items
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => None,
    }
}

fn options_of(question: &Value) -> Vec<ChoiceOption> {
    question["options"]
        .as_array()
        .map(|options| {
            options
                .iter()
                .filter_map(|o| {
                    Some(ChoiceOption {
                        label: o["label"].as_str()?.to_owned(),
                        description: o["description"].as_str().unwrap_or("").to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

struct RawAsk {
    id: String,
    session_id: String,
    tool_use_id: String,
    kind: String,
    questions: String,
    state: String,
    answers: Option<String>,
    answered_by: Option<String>,
    created_at: String,
    answered_at: Option<String>,
    deferred_at: Option<String>,
    closed_at: Option<String>,
}

impl RawAsk {
    fn decode(self) -> Result<AskRecord, AppError> {
        Ok(AskRecord {
            questions: decode_json(&self.questions, "asks.questions")?,
            answers: self
                .answers
                .as_deref()
                .map(|a| decode_json(a, "asks.answers"))
                .transpose()?,
            id: self.id,
            session_id: self.session_id,
            tool_use_id: self.tool_use_id,
            kind: self.kind,
            state: self.state,
            answered_by: self.answered_by,
            created_at: self.created_at,
            answered_at: self.answered_at,
            deferred_at: self.deferred_at,
            closed_at: self.closed_at,
        })
    }
}

fn row_to_raw(row: &Row<'_>) -> rusqlite::Result<RawAsk> {
    Ok(RawAsk {
        id: row.get(0)?,
        session_id: row.get(1)?,
        tool_use_id: row.get(2)?,
        kind: row.get(3)?,
        questions: row.get(4)?,
        state: row.get(5)?,
        answers: row.get(6)?,
        answered_by: row.get(7)?,
        created_at: row.get(8)?,
        answered_at: row.get(9)?,
        deferred_at: row.get(10)?,
        closed_at: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::memory_store;
    use crate::common::store::{DecisionFilter, SessionEvent};
    use serde_json::json;

    fn questions() -> Value {
        json!([
            {"question": "위치는?", "multiSelect": false, "options": [
                {"label": "메모리", "description": "빠름"}, {"label": "디스크", "description": "영속"}
            ]},
            {"question": "범위는?", "multiSelect": true, "options": [
                {"label": "API", "description": ""}, {"label": "CLI", "description": ""}
            ]}
        ])
    }

    fn answers() -> Value {
        json!({"위치는?": "디스크", "범위는?": ["API", "CLI"]})
    }

    fn session(store: &Store) {
        store
            .record_session_event(
                "s1",
                &SessionEvent::Start {
                    cwd: Some("/work/repo".into()),
                    surface: "terminal".into(),
                    launched_by: "user".into(),
                },
            )
            .unwrap();
    }

    fn ledger(store: &Store) -> Vec<crate::common::store::DecisionRecord> {
        store
            .list_decisions(&DecisionFilter {
                limit: 100,
                ..DecisionFilter::default()
            })
            .unwrap()
    }

    #[test]
    fn opening_twice_for_the_same_call_answers_the_same_ask() {
        let store = memory_store();
        let first = store.open_ask("s1", "t1", "ask", &questions()).unwrap();
        let again = store.open_ask("s1", "t1", "ask", &questions()).unwrap();
        assert_eq!(first.id, again.id);
        assert_eq!(first.state, "open");
    }

    #[test]
    fn malformed_asks_are_usage_errors() {
        let store = memory_store();
        let kind = store
            .open_ask("s1", "t1", "vote", &questions())
            .unwrap_err();
        assert_eq!(kind.kind, ErrorKind::Usage);
        let empty = store.open_ask("s1", "t1", "ask", &json!([])).unwrap_err();
        assert_eq!(empty.kind, ErrorKind::Usage);
        let shape = store
            .open_ask("s1", "t1", "ask", &json!([{"header": "x"}]))
            .unwrap_err();
        assert_eq!(shape.kind, ErrorKind::Usage);
    }

    #[test]
    fn answers_must_cover_exactly_the_questions() {
        let store = memory_store();
        let ask = store.open_ask("s1", "t1", "ask", &questions()).unwrap();
        for bad in [
            json!({"위치는?": "디스크"}),
            json!({"위치는?": "디스크", "범위는?": []}),
            json!({"위치는?": "디스크", "범위는?": ["API"], "기타": "x"}),
            json!("디스크"),
        ] {
            let err = store.answer_ask(&ask.id, &bad, "doppel-ui").unwrap_err();
            assert_eq!(err.kind, ErrorKind::Usage, "{bad}");
        }
    }

    #[test]
    fn a_doppel_answer_waits_for_the_hook_to_close() {
        let mut store = memory_store();
        session(&store);
        let ask = store.open_ask("s1", "t1", "ask", &questions()).unwrap();
        let answered = store.answer_ask(&ask.id, &answers(), "doppel-ui").unwrap();
        assert_eq!(answered.state, "answered");
        assert!(
            ledger(&store).is_empty(),
            "nothing is recorded before close"
        );
        assert_eq!(
            store
                .answer_ask(&ask.id, &answers(), "slack")
                .unwrap_err()
                .kind,
            ErrorKind::Conflict
        );

        let closed = store
            .close_ask(&ask.id, Some(&answers()), "doppel-ui")
            .unwrap();
        assert_eq!(closed.state, "closed");
        let entries = ledger(&store);
        assert_eq!(entries.len(), 2);
        let second = entries.iter().find(|d| d.external_id == "s1:t1:1").unwrap();
        assert_eq!(second.answers, ["API", "CLI"]);
        assert_eq!(second.project.as_deref(), Some("/work/repo"));
        assert_eq!(second.options.len(), 2);
    }

    #[test]
    fn the_hooks_close_overrides_a_doppel_answer_that_lost() {
        let mut store = memory_store();
        session(&store);
        let ask = store.open_ask("s1", "t1", "ask", &questions()).unwrap();
        store.answer_ask(&ask.id, &answers(), "doppel-ui").unwrap();
        let terminal = json!({"위치는?": "메모리", "범위는?": ["CLI"]});
        let closed = store
            .close_ask(&ask.id, Some(&terminal), "session")
            .unwrap();
        assert_eq!(closed.answers, Some(terminal));
        assert_eq!(closed.answered_by.as_deref(), Some("session"));
        assert_eq!(
            store.close_ask(&ask.id, None, "session").unwrap_err().kind,
            ErrorKind::Conflict
        );
    }

    #[test]
    fn closing_without_answers_keeps_a_doppel_answer() {
        let mut store = memory_store();
        session(&store);
        let ask = store.open_ask("s1", "t1", "ask", &questions()).unwrap();
        store.answer_ask(&ask.id, &answers(), "doppel-ui").unwrap();
        let closed = store.close_ask(&ask.id, None, "doppel-ui").unwrap();
        assert_eq!(closed.answers, Some(answers()));
    }

    #[test]
    fn deferred_answers_are_deliverable_until_closed() {
        let mut store = memory_store();
        session(&store);
        let ask = store.open_ask("s1", "t1", "ask", &questions()).unwrap();
        assert!(store.defer_ask(&ask.id).unwrap().deferred_at.is_some());
        assert_eq!(
            store.defer_ask(&ask.id).unwrap_err().kind,
            ErrorKind::Conflict
        );
        assert!(store.deliverable_answers("s1").unwrap().is_empty());

        store.answer_ask(&ask.id, &answers(), "doppel-ui").unwrap();
        assert_eq!(store.deliverable_answers("s1").unwrap().len(), 1);
        store
            .close_ask(&ask.id, Some(&answers()), "doppel-ui")
            .unwrap();
        assert!(store.deliverable_answers("s1").unwrap().is_empty());
    }

    #[test]
    fn permissions_answered_at_the_terminal_stay_out_of_the_ledger() {
        let mut store = memory_store();
        session(&store);
        let permission = json!([{"question": "Bash 실행 허용: ls", "options": [
            {"label": "allow", "description": ""}, {"label": "deny", "description": ""}
        ]}]);
        let at_terminal = store
            .open_ask("s1", "t1", "permission", &permission)
            .unwrap();
        store
            .close_ask(
                &at_terminal.id,
                Some(&json!({"Bash 실행 허용: ls": "allow"})),
                "session",
            )
            .unwrap();
        assert!(ledger(&store).is_empty());

        let remote = store
            .open_ask("s1", "t2", "permission", &permission)
            .unwrap();
        store
            .close_ask(
                &remote.id,
                Some(&json!({"Bash 실행 허용: ls": "deny"})),
                "doppel-ui",
            )
            .unwrap();
        let entries = ledger(&store);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, "permission");
        assert_eq!(entries[0].external_id, "s1:t2");
    }

    #[test]
    fn listing_filters_by_state() {
        let store = memory_store();
        store.open_ask("s1", "t1", "ask", &questions()).unwrap();
        let second = store.open_ask("s1", "t2", "ask", &questions()).unwrap();
        store.defer_ask(&second.id).unwrap();
        assert_eq!(store.list_asks(None).unwrap().len(), 2);
        assert_eq!(store.list_asks(Some("deferred")).unwrap()[0].id, second.id);
    }
}
