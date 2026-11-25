//! `sessions` — the facts doppel keeps per connected Claude session. The
//! state shown to the user (docs/doppel.md §5) is not stored: it is derived
//! from these facts at read time, so a threshold change applies to every
//! session at once and "nothing happened for a while" states need no event.
//!
//! Events may arrive for a session doppel has not seen start (the daemon came
//! up mid-session): every write upserts the row first.

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{Store, map_sqlite_err, not_found};
use crate::common::error::AppError;

/// One session event as the plugin reports it (docs/doppel-protocol.md §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionEvent {
    Start {
        cwd: Option<String>,
        surface: String,
        launched_by: String,
    },
    End {
        reason: Option<String>,
    },
    Heartbeat,
    TurnStart,
    TurnComplete {
        summary: Option<String>,
    },
    ToolFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionFacts {
    pub(crate) session_id: String,
    pub(crate) cwd: Option<String>,
    pub(crate) surface: String,
    pub(crate) launched_by: String,
    pub(crate) started_at: String,
    pub(crate) last_event_at: String,
    pub(crate) last_heartbeat_at: String,
    pub(crate) turn_active: bool,
    pub(crate) fail_streak: u32,
    pub(crate) last_summary: Option<String>,
    pub(crate) ended_at: Option<String>,
    pub(crate) end_reason: Option<String>,
    /// Questions still waiting on the user (`open` or `deferred`).
    pub(crate) open_asks: u32,
    /// Permission confirmations still waiting on the user.
    pub(crate) open_permissions: u32,
}

const SESSION_SELECT: &str = "SELECT s.session_id, s.cwd, s.surface, s.launched_by, s.started_at,
       s.last_event_at, s.last_heartbeat_at, s.turn_active, s.fail_streak, s.last_summary,
       s.ended_at, s.end_reason,
       (SELECT COUNT(*) FROM asks a WHERE a.session_id = s.session_id
          AND a.kind = 'ask' AND a.state IN ('open','deferred')),
       (SELECT COUNT(*) FROM asks a WHERE a.session_id = s.session_id
          AND a.kind = 'permission' AND a.state IN ('open','deferred'))
     FROM sessions s";

impl Store {
    /// Applies one event. `session.end` also closes the session's waiting
    /// asks — nobody can answer them any more.
    pub(crate) fn record_session_event(
        &self,
        session_id: &str,
        event: &SessionEvent,
    ) -> Result<(), AppError> {
        let now = self.clock.now();
        let tx = self.conn.unchecked_transaction().map_err(map_sqlite_err)?;
        ensure_session(&tx, session_id, &now)?;
        match event {
            SessionEvent::Start {
                cwd,
                surface,
                launched_by,
            } => tx.execute(
                "UPDATE sessions SET cwd = ?2, surface = ?3, launched_by = ?4, started_at = ?5,
                   ended_at = NULL, end_reason = NULL WHERE session_id = ?1",
                params![session_id, cwd, surface, launched_by, now],
            ),
            SessionEvent::End { reason } => tx.execute(
                "UPDATE sessions SET ended_at = ?2, end_reason = ?3, turn_active = 0
                 WHERE session_id = ?1",
                params![session_id, now, reason],
            ),
            SessionEvent::Heartbeat => Ok(0),
            SessionEvent::TurnStart => tx.execute(
                "UPDATE sessions SET turn_active = 1, fail_streak = 0
                 WHERE session_id = ?1 AND ended_at IS NULL",
                params![session_id],
            ),
            SessionEvent::TurnComplete { summary } => tx.execute(
                "UPDATE sessions SET turn_active = 0, last_summary = ?2 WHERE session_id = ?1",
                params![session_id, summary],
            ),
            SessionEvent::ToolFailure => tx.execute(
                "UPDATE sessions SET fail_streak = fail_streak + 1 WHERE session_id = ?1",
                params![session_id],
            ),
        }
        .map_err(|e| {
            super::constraint_err(e, format!("event rejected for session {session_id}"))
        })?;
        // Every event, a heartbeat included, is a sign of life.
        tx.execute(
            "UPDATE sessions SET last_event_at = ?2, last_heartbeat_at = ?2 WHERE session_id = ?1",
            params![session_id, now],
        )
        .map_err(map_sqlite_err)?;
        if matches!(event, SessionEvent::End { .. }) {
            tx.execute(
                "UPDATE asks SET state = 'closed', answered_by = 'session-ended', closed_at = ?2
                 WHERE session_id = ?1 AND state IN ('open','deferred')",
                params![session_id, now],
            )
            .map_err(map_sqlite_err)?;
        }
        tx.commit().map_err(map_sqlite_err)
    }

    /// An attach connection is a sign of life too, without being an event
    /// that moves the session forward.
    pub(crate) fn touch_session(&self, session_id: &str) -> Result<(), AppError> {
        let now = self.clock.now();
        ensure_session(&self.conn, session_id, &now)?;
        self.conn
            .execute(
                "UPDATE sessions SET last_heartbeat_at = ?2 WHERE session_id = ?1",
                params![session_id, now],
            )
            .map_err(map_sqlite_err)?;
        Ok(())
    }

    pub(crate) fn get_session(&self, session_id: &str) -> Result<SessionFacts, AppError> {
        self.conn
            .query_row(
                &format!("{SESSION_SELECT} WHERE s.session_id = ?1"),
                params![session_id],
                row_to_facts,
            )
            .optional()
            .map_err(map_sqlite_err)?
            .ok_or_else(|| not_found(format!("no session {session_id}")))
    }

    /// Most recently active first.
    pub(crate) fn list_sessions(&self) -> Result<Vec<SessionFacts>, AppError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "{SESSION_SELECT} ORDER BY s.last_event_at DESC, s.session_id"
            ))
            .map_err(map_sqlite_err)?;
        stmt.query_map([], row_to_facts)
            .map_err(map_sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite_err)
    }
}

/// Inserts a bare row for a session doppel first hears of mid-flight.
pub(super) fn ensure_session(
    conn: &Connection,
    session_id: &str,
    now: &str,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO sessions (session_id, started_at, last_event_at, last_heartbeat_at)
         VALUES (?1, ?2, ?2, ?2) ON CONFLICT(session_id) DO NOTHING",
        params![session_id, now],
    )
    .map_err(map_sqlite_err)?;
    Ok(())
}

fn row_to_facts(row: &Row<'_>) -> rusqlite::Result<SessionFacts> {
    Ok(SessionFacts {
        session_id: row.get(0)?,
        cwd: row.get(1)?,
        surface: row.get(2)?,
        launched_by: row.get(3)?,
        started_at: row.get(4)?,
        last_event_at: row.get(5)?,
        last_heartbeat_at: row.get(6)?,
        turn_active: row.get::<_, i64>(7)? == 1,
        fail_streak: row.get(8)?,
        last_summary: row.get(9)?,
        ended_at: row.get(10)?,
        end_reason: row.get(11)?,
        open_asks: row.get(12)?,
        open_permissions: row.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::memory_store;
    use serde_json::json;

    fn start(store: &Store, id: &str) {
        store
            .record_session_event(
                id,
                &SessionEvent::Start {
                    cwd: Some("/work".into()),
                    surface: "terminal".into(),
                    launched_by: "user".into(),
                },
            )
            .unwrap();
    }

    #[test]
    fn events_move_the_turn_and_failure_facts() {
        let store = memory_store();
        start(&store, "s1");
        store
            .record_session_event("s1", &SessionEvent::TurnStart)
            .unwrap();
        store
            .record_session_event("s1", &SessionEvent::ToolFailure)
            .unwrap();
        store
            .record_session_event("s1", &SessionEvent::ToolFailure)
            .unwrap();
        let facts = store.get_session("s1").unwrap();
        assert!(facts.turn_active);
        assert_eq!(facts.fail_streak, 2);
        assert_eq!(facts.surface, "terminal");

        store
            .record_session_event(
                "s1",
                &SessionEvent::TurnComplete {
                    summary: Some("끝".into()),
                },
            )
            .unwrap();
        store
            .record_session_event("s1", &SessionEvent::TurnStart)
            .unwrap();
        let facts = store.get_session("s1").unwrap();
        assert_eq!(facts.fail_streak, 0, "a new turn starts a new streak");
        assert_eq!(facts.last_summary.as_deref(), Some("끝"));
    }

    #[test]
    fn an_event_for_an_unknown_session_creates_it() {
        let store = memory_store();
        store
            .record_session_event("late", &SessionEvent::Heartbeat)
            .unwrap();
        let facts = store.get_session("late").unwrap();
        assert_eq!(facts.surface, "unknown");
        assert_eq!(facts.launched_by, "user");
    }

    #[test]
    fn ending_a_session_closes_its_waiting_asks() {
        let store = memory_store();
        start(&store, "s1");
        let questions = json!([{"question": "q?", "options": []}]);
        store.open_ask("s1", "t1", "ask", &questions).unwrap();
        assert_eq!(store.get_session("s1").unwrap().open_asks, 1);

        store
            .record_session_event(
                "s1",
                &SessionEvent::End {
                    reason: Some("exit".into()),
                },
            )
            .unwrap();
        let facts = store.get_session("s1").unwrap();
        assert!(facts.ended_at.is_some());
        assert!(!facts.turn_active);
        assert_eq!(facts.open_asks, 0);
        let ask = store.get_ask("ask-000001").unwrap();
        assert_eq!(ask.state, "closed");
        assert_eq!(ask.answered_by.as_deref(), Some("session-ended"));
    }

    #[test]
    fn a_restart_reopens_an_ended_session() {
        let store = memory_store();
        start(&store, "s1");
        store
            .record_session_event("s1", &SessionEvent::End { reason: None })
            .unwrap();
        start(&store, "s1");
        assert_eq!(store.get_session("s1").unwrap().ended_at, None);
    }

    #[test]
    fn sessions_list_most_recent_first_and_unknown_is_not_found() {
        let store = memory_store();
        start(&store, "old");
        start(&store, "new");
        let ids: Vec<_> = store
            .list_sessions()
            .unwrap()
            .into_iter()
            .map(|s| s.session_id)
            .collect();
        assert_eq!(ids, ["new", "old"]);
        assert_eq!(
            store.get_session("nope").unwrap_err().kind,
            crate::common::error::ErrorKind::NotFound
        );
    }
}
