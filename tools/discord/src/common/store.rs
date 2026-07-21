//! Shared SQLite-backed storage for HITL `ask` state. Both the CLI's future
//! `ask` command and the daemon's interaction handler open the same
//! `~/.areum/discord/discord.db` file through this module — it owns the
//! schema and the conditional updates that resolve concurrent responses.
//! Connection is not `Sync`; async callers (the daemon) are responsible for
//! their own spawn_blocking/dedicated-thread pattern around an `AskStore`.
//!
//! Not yet wired into `commands/` — the `ask` command and daemon that consume
//! this module land in follow-up units, so `dead_code` is allowed here until
//! then rather than reporting the whole module unused.
#![allow(dead_code)]

use std::path::Path;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::common::error::{AppError, ErrorKind};

/// Lifecycle status of an ask, mirrored 1:1 with the `asks.status` TEXT column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AskStatus {
    Pending,
    Answered,
    TimedOut,
}

/// Fail-fast column decoding: an unknown status string in the DB is a schema
/// contract violation, surfaced as a conversion error rather than defaulted.
impl FromSql for AskStatus {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "pending" => Ok(AskStatus::Pending),
            "answered" => Ok(AskStatus::Answered),
            "timed_out" => Ok(AskStatus::TimedOut),
            other => Err(FromSqlError::Other(
                format!("unknown ask status in store: {other}").into(),
            )),
        }
    }
}

/// Decodes the `options` TEXT column (JSON array) straight from the row,
/// with the same fail-fast contract as [`AskStatus`]'s `FromSql`.
struct OptionsJson(Vec<String>);

impl FromSql for OptionsJson {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        serde_json::from_str(value.as_str()?)
            .map(OptionsJson)
            .map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

/// A new ask to persist. Status always starts at `pending`; the response
/// columns (`kind`/`value`/`answered_by`/`answered_at`) start NULL and are
/// filled in later by `try_answer`/`try_timeout`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NewAsk {
    pub(crate) ask_id: String,
    pub(crate) channel_id: String,
    pub(crate) question: String,
    pub(crate) options: Vec<String>,
    pub(crate) allow_text: bool,
    pub(crate) created_at: String,
    pub(crate) timeout_at: String,
}

/// A persisted `asks` row. Timestamps are RFC3339 strings, matching the rest
/// of the crate's timestamp convention (see `output::payload`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AskRecord {
    pub(crate) ask_id: String,
    pub(crate) channel_id: String,
    pub(crate) question: String,
    pub(crate) options: Vec<String>,
    pub(crate) allow_text: bool,
    pub(crate) status: AskStatus,
    pub(crate) kind: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) answered_by: Option<String>,
    pub(crate) created_at: String,
    pub(crate) timeout_at: String,
    pub(crate) answered_at: Option<String>,
}

/// Synchronous rusqlite wrapper. A single connection is not `Sync` — this
/// type is deliberately a thin owner of that connection and the SQL that
/// runs against it, nothing more (async usage patterns are a later unit's
/// concern).
pub(crate) struct AskStore {
    conn: Connection,
}

impl AskStore {
    /// Opens (creating if needed) a file-backed store: creates the parent
    /// directory, enables WAL journal mode + a busy timeout so the CLI and
    /// daemon can both hold connections open concurrently, then migrates.
    pub(crate) fn open(path: &Path) -> Result<AskStore, AppError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppError::new(
                    ErrorKind::Internal,
                    format!("failed to create db directory {}: {e}", parent.display()),
                )
            })?;
        }

        let mut conn = Connection::open(path).map_err(map_sqlite_err)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(map_sqlite_err)?;
        conn.busy_timeout(std::time::Duration::from_millis(5_000))
            .map_err(map_sqlite_err)?;
        migrate(&mut conn)?;
        Ok(AskStore { conn })
    }

    /// Opens an in-memory store for tests. WAL mode is meaningless for
    /// `:memory:` databases, so it's skipped here.
    pub(crate) fn open_in_memory() -> Result<AskStore, AppError> {
        let mut conn = Connection::open_in_memory().map_err(map_sqlite_err)?;
        migrate(&mut conn)?;
        Ok(AskStore { conn })
    }

    /// Inserts a new `pending` ask. `options` is stored as a JSON array.
    pub(crate) fn insert_ask(&self, ask: NewAsk) -> Result<(), AppError> {
        let options_json = serde_json::to_string(&ask.options).map_err(|e| {
            AppError::new(
                ErrorKind::Internal,
                format!("failed to serialize ask options: {e}"),
            )
        })?;
        self.conn
            .execute(
                "INSERT INTO asks (
                    ask_id, channel_id, question, options, allow_text,
                    status, created_at, timeout_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7)",
                params![
                    ask.ask_id,
                    ask.channel_id,
                    ask.question,
                    options_json,
                    ask.allow_text,
                    ask.created_at,
                    ask.timeout_at,
                ],
            )
            .map_err(map_sqlite_err)?;
        Ok(())
    }

    pub(crate) fn get_ask(&self, ask_id: &str) -> Result<Option<AskRecord>, AppError> {
        self.conn
            .query_row(
                &format!("SELECT {ASK_COLUMNS} FROM asks WHERE ask_id = ?1"),
                params![ask_id],
                row_to_record,
            )
            .optional()
            .map_err(map_sqlite_err)
    }

    /// Adopts `(kind, value, answered_by, answered_at)` as the ask's answer
    /// iff it is still `pending` — the `WHERE status = 'pending'` guard is
    /// the sole concurrency-resolution point: whichever caller's UPDATE
    /// commits first flips the row and wins the race, so a losing caller
    /// simply gets `false` back rather than corrupting the winner's answer.
    pub(crate) fn try_answer(
        &self,
        ask_id: &str,
        kind: &str,
        value: &str,
        answered_by: &str,
        answered_at: &str,
    ) -> Result<bool, AppError> {
        let changed = self
            .conn
            .execute(
                "UPDATE asks SET status = 'answered', kind = ?1, value = ?2,
                    answered_by = ?3, answered_at = ?4
                 WHERE ask_id = ?5 AND status = 'pending'",
                params![kind, value, answered_by, answered_at, ask_id],
            )
            .map_err(map_sqlite_err)?;
        Ok(changed == 1)
    }

    /// Same conditional-update pattern as `try_answer`, transitioning to
    /// `timed_out` instead. Returns `false` if the ask was no longer pending.
    pub(crate) fn try_timeout(&self, ask_id: &str) -> Result<bool, AppError> {
        let changed = self
            .conn
            .execute(
                "UPDATE asks SET status = 'timed_out' WHERE ask_id = ?1 AND status = 'pending'",
                params![ask_id],
            )
            .map_err(map_sqlite_err)?;
        Ok(changed == 1)
    }

    /// Transitions every `pending` ask whose `timeout_at` has passed `now`
    /// to `timed_out` and returns the transitioned records (for the daemon
    /// to disable their buttons). A single conditional UPDATE with
    /// RETURNING keeps the same `status = 'pending'` race guard as
    /// `try_answer`/`try_timeout` while avoiding a per-row query loop.
    pub(crate) fn expire_due(&self, now: &str) -> Result<Vec<AskRecord>, AppError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "UPDATE asks SET status = 'timed_out'
                 WHERE status = 'pending' AND timeout_at <= ?1
                 RETURNING {ASK_COLUMNS}"
            ))
            .map_err(map_sqlite_err)?;
        stmt.query_map(params![now], row_to_record)
            .map_err(map_sqlite_err)?
            .collect::<rusqlite::Result<Vec<AskRecord>>>()
            .map_err(map_sqlite_err)
    }

    /// Deletes resolved (answered/timed_out) asks whose `created_at` is
    /// older than `retention_days` before `now`, returning the count
    /// removed. Pending rows are never deleted regardless of age — an
    /// unresolved question silently vanishing would be data loss. The
    /// day-arithmetic cutoff is computed by SQLite's own `datetime()`
    /// modifiers rather than a date library, on the invariant that
    /// `created_at`/`now` are well-formed RFC3339 UTC strings (the
    /// convention this whole crate stores timestamps in).
    pub(crate) fn cleanup(&self, retention_days: u32, now: &str) -> Result<usize, AppError> {
        let cutoff_modifier = format!("-{retention_days} days");
        self.conn
            .execute(
                "DELETE FROM asks
                 WHERE status != 'pending' AND datetime(created_at) < datetime(?1, ?2)",
                params![now, cutoff_modifier],
            )
            .map_err(map_sqlite_err)
    }
}

/// Single source of the `asks` column list shared by every statement that
/// yields full records (`get_ask` SELECT, `expire_due` RETURNING), so
/// `row_to_record`'s named lookups always have every column available.
const ASK_COLUMNS: &str = "ask_id, channel_id, question, options, allow_text, status, \
    kind, value, answered_by, created_at, timeout_at, answered_at";

fn row_to_record(row: &Row<'_>) -> rusqlite::Result<AskRecord> {
    Ok(AskRecord {
        ask_id: row.get("ask_id")?,
        channel_id: row.get("channel_id")?,
        question: row.get("question")?,
        options: row.get::<_, OptionsJson>("options")?.0,
        allow_text: row.get("allow_text")?,
        status: row.get("status")?,
        kind: row.get("kind")?,
        value: row.get("value")?,
        answered_by: row.get("answered_by")?,
        created_at: row.get("created_at")?,
        timeout_at: row.get("timeout_at")?,
        answered_at: row.get("answered_at")?,
    })
}

/// Applies pending schema migrations, tracked via a single-row
/// `schema_version` table. Safe to call on every `open` — a database already
/// at the latest version is a no-op. SP3/SP4 (message cache, event
/// subscription) grow the same DB file by appending `if current < N`
/// migration blocks here rather than introducing a new store.
///
/// The whole version-check + DDL + version-stamp runs inside one
/// `BEGIN IMMEDIATE` transaction: a crash mid-migration rolls back to a
/// clean slate, and two processes racing the first open serialize on the
/// write lock (the loser waits within busy_timeout, then re-reads the
/// version and skips). `IF NOT EXISTS` on the DDL is a second line of
/// defense for a database left half-migrated by a pre-transaction binary.
fn migrate(conn: &mut Connection) -> Result<(), AppError> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(map_sqlite_err)?;

    tx.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")
        .map_err(map_sqlite_err)?;

    let current: i64 = tx
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(map_sqlite_err)?
        .unwrap_or(0);

    if current < 1 {
        tx.execute_batch(CREATE_ASKS_SQL).map_err(map_sqlite_err)?;
        tx.execute_batch("INSERT INTO schema_version (version) VALUES (1)")
            .map_err(map_sqlite_err)?;
    }

    tx.commit().map_err(map_sqlite_err)
}

/// v1 DDL. Shared with the partial-migration recovery test, which uses it to
/// reproduce a crash between table creation and the version stamp.
const CREATE_ASKS_SQL: &str = "CREATE TABLE IF NOT EXISTS asks (
    ask_id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    question TEXT NOT NULL,
    options TEXT NOT NULL,
    allow_text INTEGER NOT NULL,
    status TEXT NOT NULL,
    kind TEXT,
    value TEXT,
    answered_by TEXT,
    created_at TEXT NOT NULL,
    timeout_at TEXT NOT NULL,
    answered_at TEXT
)";

fn map_sqlite_err(err: rusqlite::Error) -> AppError {
    AppError::new(ErrorKind::Internal, format!("sqlite error: {err}"))
}

#[cfg(test)]
mod tests;
