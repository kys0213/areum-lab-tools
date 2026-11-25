//! SQLite persistence for the judgment ledger. Owns the schema of
//! docs/doppel-judgment.md §6 verbatim, so an inconsistent row — a reviewed
//! candidate, a superseded precedent that is not retired, a half-filed
//! category — is unwritable.
//!
//! Every failure leaves here as an [`AppError`] with the kind the CLI contract
//! promises (docs/doppel.md §9): a missing row is `not_found`, a refused
//! transition or a tripped constraint is `conflict`. A raw rusqlite error only
//! surfaces as `internal` when no specific kind applies.
//!
//! The daemon's tables (`sessions`, `asks`, docs/doppel-protocol.md §6) live
//! in the same file: an ask that closes writes its answer into the ledger in
//! the same database.
//!
//! Per-table behaviour lives in the child modules; this file holds what they
//! share — the connection, the clock, id minting and error mapping.

mod asks;
mod briefs;
mod cards;
mod categories;
mod decisions;
mod golden;
mod projects;
mod proposals;
mod search;
mod sessions;

use std::path::Path;

use rusqlite::{Connection, ErrorCode};

use crate::common::error::{AppError, ErrorKind};
use crate::common::time::now_rfc3339;

pub(crate) use asks::{AskRecord, validate_answers};
pub(crate) use briefs::{BriefDraft, BriefRecord};
pub(crate) use cards::{CardRecord, NewCard, TRIAGE_CATEGORY};
pub(crate) use categories::CategoryRecord;
pub(crate) use decisions::{ChoiceOption, DecisionFilter, DecisionRecord, NewDecision};
pub(crate) use golden::{GoldenFilter, GoldenRecord};
pub(crate) use projects::ProjectRecord;
pub(crate) use proposals::{ProposalDraft, ProposalRecord, ProposalStats};
pub(crate) use search::{SearchFilter, fts_query};
pub(crate) use sessions::{SessionEvent, SessionFacts};

/// Wall-clock source for every time column. Injected so tests pin exact
/// timestamps and, with them, every tie-break that orders by time. `Send`
/// because the daemon shares the store across its request handlers.
pub(crate) trait Clock: Send {
    fn now(&self) -> String;
}

pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> String {
        now_rfc3339()
    }
}

/// How long a statement waits on a lock held by another connection (a
/// concurrent `ingest`) before giving up.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(5_000);

pub(crate) struct Store {
    conn: Connection,
    clock: Box<dyn Clock>,
}

impl Store {
    /// Opens (creating if needed) the database with the wall clock.
    pub(crate) fn open(path: &Path) -> Result<Store, AppError> {
        Store::open_with_clock(path, Box::new(SystemClock))
    }

    /// Same as [`Store::open`], additionally reporting whether this call
    /// created the database file. `init` is the only caller that needs it.
    pub(crate) fn open_reporting_created(path: &Path) -> Result<(Store, bool), AppError> {
        Store::open_with_clock_reporting_created(path, Box::new(SystemClock))
    }

    pub(crate) fn open_with_clock(path: &Path, clock: Box<dyn Clock>) -> Result<Store, AppError> {
        Store::open_with_clock_reporting_created(path, clock).map(|(store, _created)| store)
    }

    /// Creates the parent directory, configures the connection (foreign keys,
    /// WAL, busy timeout) and ensures the schema — all idempotent, so opening
    /// an existing database is the same call as creating one. The existence
    /// check sits right before the call that would create the file.
    fn open_with_clock_reporting_created(
        path: &Path,
        clock: Box<dyn Clock>,
    ) -> Result<(Store, bool), AppError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppError::new(
                    ErrorKind::Internal,
                    format!("failed to create db directory {}: {e}", parent.display()),
                )
            })?;
        }

        let created = !path.exists();
        let conn = Connection::open(path).map_err(map_sqlite_err)?;
        conn.busy_timeout(BUSY_TIMEOUT).map_err(map_sqlite_err)?;
        enable_foreign_keys(&conn)?;
        enable_wal(&conn)?;
        create_schema(&conn)?;
        Ok((Store { conn, clock }, created))
    }

    /// In-memory database for tests. WAL is meaningless for `:memory:`.
    #[cfg(test)]
    pub(crate) fn open_in_memory(clock: Box<dyn Clock>) -> Result<Store, AppError> {
        let conn = Connection::open_in_memory().map_err(map_sqlite_err)?;
        enable_foreign_keys(&conn)?;
        create_schema(&conn)?;
        Ok(Store { conn, clock })
    }
}

/// Mints the next id of a table whose ids are `<prefix>-<6 digits>`
/// (`dec-000017`, `gld-000003`). Called inside the inserting transaction, so
/// two writers serialize on SQLite's writer lock instead of minting the same
/// number.
fn next_id(conn: &Connection, table: &str, prefix: &str) -> Result<String, AppError> {
    let sql = format!(
        "SELECT MAX(CAST(substr(id, {}) AS INTEGER)) FROM {table}",
        prefix.len() + 2
    );
    let highest: Option<i64> = conn
        .query_row(&sql, [], |row| row.get(0))
        .map_err(map_sqlite_err)?;
    Ok(format!("{prefix}-{:06}", highest.unwrap_or(0) + 1))
}

/// SQLite leaves foreign keys off per connection by default, which would let
/// a decision point at a category that does not exist.
fn enable_foreign_keys(conn: &Connection) -> Result<(), AppError> {
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(map_sqlite_err)
}

/// Switches the connection to WAL, retrying on SQLITE_BUSY: the WAL
/// transition needs brief exclusive locks and SQLite skips the busy handler
/// for parts of it, so two processes racing the first open would otherwise
/// see an immediate "database is locked".
fn enable_wal(conn: &Connection) -> Result<(), AppError> {
    const RETRY_SLEEP: std::time::Duration = std::time::Duration::from_millis(10);
    let deadline = std::time::Instant::now() + BUSY_TIMEOUT;
    loop {
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Ok(()) => return Ok(()),
            Err(err) if is_busy(&err) && std::time::Instant::now() < deadline => {
                std::thread::sleep(RETRY_SLEEP);
            }
            Err(err) => return Err(map_sqlite_err(err)),
        }
    }
}

fn is_busy(err: &rusqlite::Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

/// Every statement is additive and `IF NOT EXISTS`, so opening an existing
/// database is a no-op and later stages add their tables by appending here.
/// A change that `IF NOT EXISTS` cannot express (a CHECK widening) is a
/// numbered migration in [`migrate`].
fn create_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS decisions ({DECISIONS_COLUMNS}"
    ))
    .map_err(map_sqlite_err)?;
    conn.execute_batch(CREATE_SCHEMA_SQL)
        .map_err(map_sqlite_err)?;
    migrate(conn)
}

/// The schema version this build writes (`PRAGMA user_version`).
const SCHEMA_VERSION: i64 = 1;

/// Brings an older database up to [`SCHEMA_VERSION`], one step at a time.
///
/// - 1: `decisions.kind` admits `triage`. SQLite cannot alter a CHECK, so the
///   table is rebuilt (create, copy, drop, rename) with foreign keys off —
///   the documented way to change a constraint without breaking `golden`'s
///   reference to it.
fn migrate(conn: &Connection) -> Result<(), AppError> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(map_sqlite_err)?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    let decisions_sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'decisions'",
            [],
            |row| row.get(0),
        )
        .map_err(map_sqlite_err)?;
    if !decisions_sql.contains("'triage'") {
        conn.pragma_update(None, "foreign_keys", "OFF")
            .map_err(map_sqlite_err)?;
        let rebuilt = conn.execute_batch(&format!(
            "BEGIN IMMEDIATE;
             CREATE TABLE decisions_v1 ({DECISIONS_COLUMNS}
             INSERT INTO decisions_v1 SELECT * FROM decisions;
             DROP TABLE decisions;
             ALTER TABLE decisions_v1 RENAME TO decisions;
             COMMIT;"
        ));
        if rebuilt.is_err() {
            let _ = conn.execute_batch("ROLLBACK");
        }
        enable_foreign_keys(conn)?;
        rebuilt.map_err(map_sqlite_err)?;
    }
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(map_sqlite_err)
}

/// The `decisions` columns and constraints, shared by the create and by the
/// rebuild in [`migrate`] (docs/doppel-judgment.md §6).
const DECISIONS_COLUMNS: &str = "id            TEXT PRIMARY KEY,
  source        TEXT NOT NULL,
  external_id   TEXT NOT NULL,
  kind          TEXT NOT NULL,
  project       TEXT,
  session_id    TEXT,
  question      TEXT NOT NULL,
  options       TEXT NOT NULL,
  answers       TEXT NOT NULL,
  rationale     TEXT,
  context       TEXT,
  category      TEXT REFERENCES categories(name),
  category_by   TEXT,
  decided_at    TEXT NOT NULL,
  created_at    TEXT NOT NULL,

  UNIQUE (source, external_id),
  CHECK (source IN ('session','plan','doppel')),
  CHECK (kind IN ('ask','permission','plan','triage')),
  CHECK ((category IS NULL) = (category_by IS NULL)),
  CHECK (category_by IS NULL OR category_by IN ('human','model'))
);
";

/// docs/doppel-judgment.md §6 verbatim.
const CREATE_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS categories (
  name        TEXT PRIMARY KEY,
  description TEXT NOT NULL,
  state       TEXT NOT NULL DEFAULT 'active',
  created_at  TEXT NOT NULL,
  CHECK (state IN ('active','retired'))
);

CREATE TABLE IF NOT EXISTS golden (
  id            TEXT PRIMARY KEY,
  decision_id   TEXT NOT NULL UNIQUE REFERENCES decisions(id),
  category      TEXT NOT NULL REFERENCES categories(name),
  project       TEXT,
  question      TEXT NOT NULL,
  options       TEXT NOT NULL,
  answers       TEXT NOT NULL,
  rationale     TEXT,
  context       TEXT,
  decided_at    TEXT NOT NULL,
  state         TEXT NOT NULL,
  reason        TEXT NOT NULL,
  superseded_by TEXT REFERENCES golden(id),
  note          TEXT,
  created_at    TEXT NOT NULL,
  reviewed_at   TEXT,

  CHECK (state IN ('candidate','confirmed','retired')),
  CHECK (reason IN ('override','ingest','manual')),
  CHECK ((state = 'candidate') = (reviewed_at IS NULL)),
  CHECK (superseded_by IS NULL OR state = 'retired'),
  CHECK (superseded_by IS NULL OR superseded_by <> id)
);

CREATE VIRTUAL TABLE IF NOT EXISTS golden_fts USING fts5(
  golden_id UNINDEXED, question, answers, rationale, context,
  tokenize = 'unicode61'
);

CREATE TABLE IF NOT EXISTS sessions (
  session_id        TEXT PRIMARY KEY,
  cwd               TEXT,
  surface           TEXT NOT NULL DEFAULT 'unknown',
  launched_by       TEXT NOT NULL DEFAULT 'user',
  started_at        TEXT NOT NULL,
  last_event_at     TEXT NOT NULL,
  last_heartbeat_at TEXT NOT NULL,
  turn_active       INTEGER NOT NULL DEFAULT 0,
  fail_streak       INTEGER NOT NULL DEFAULT 0,
  last_summary      TEXT,
  ended_at          TEXT,
  end_reason        TEXT,

  CHECK (surface IN ('terminal','headless','unknown')),
  CHECK (launched_by IN ('doppel','user')),
  CHECK (turn_active IN (0,1)),
  CHECK (fail_streak >= 0),
  CHECK (ended_at IS NULL OR turn_active = 0)
);

CREATE TABLE IF NOT EXISTS asks (
  id          TEXT PRIMARY KEY,
  session_id  TEXT NOT NULL REFERENCES sessions(session_id),
  tool_use_id TEXT NOT NULL,
  kind        TEXT NOT NULL,
  questions   TEXT NOT NULL,
  state       TEXT NOT NULL,
  answers     TEXT,
  answered_by TEXT,
  created_at  TEXT NOT NULL,
  answered_at TEXT,
  deferred_at TEXT,
  closed_at   TEXT,

  UNIQUE (session_id, tool_use_id),
  CHECK (kind IN ('ask','permission')),
  CHECK (state IN ('open','deferred','answered','closed')),
  CHECK (state NOT IN ('open','deferred') OR (answers IS NULL AND answered_by IS NULL)),
  CHECK (state <> 'answered' OR (answers IS NOT NULL AND answered_by IS NOT NULL)),
  CHECK ((state = 'closed') = (closed_at IS NOT NULL)),
  CHECK (state <> 'deferred' OR deferred_at IS NOT NULL)
);

CREATE TABLE IF NOT EXISTS projects (
  name        TEXT PRIMARY KEY,
  path        TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS cards (
  id          TEXT PRIMARY KEY,
  source      TEXT NOT NULL,
  external_id TEXT NOT NULL,
  title       TEXT NOT NULL,
  body        TEXT NOT NULL DEFAULT '',
  state       TEXT NOT NULL,
  project     TEXT REFERENCES projects(name),
  priority    TEXT NOT NULL DEFAULT 'P2',
  session_id  TEXT,
  claimed_at  TEXT,
  outcome     TEXT,
  last_error  TEXT,
  created_at  TEXT NOT NULL,
  updated_at  TEXT NOT NULL,

  UNIQUE (source, external_id),
  CHECK (state IN ('inbox','triaged','running','review','done')),
  CHECK (priority IN ('P0','P1','P2','P3')),
  CHECK (state NOT IN ('triaged','running','review') OR project IS NOT NULL),
  CHECK (state <> 'running' OR claimed_at IS NOT NULL),
  CHECK (state <> 'review' OR outcome IS NOT NULL)
);

CREATE TABLE IF NOT EXISTS proposals (
  card_id     TEXT PRIMARY KEY REFERENCES cards(id),
  state       TEXT NOT NULL,
  project     TEXT REFERENCES projects(name),
  priority    TEXT,
  confidence  TEXT,
  rationale   TEXT,
  cited       TEXT NOT NULL DEFAULT '[]',
  precedents  TEXT NOT NULL DEFAULT '[]',
  command     TEXT NOT NULL,
  error       TEXT,
  applied     INTEGER NOT NULL DEFAULT 0,
  accepted    INTEGER,
  created_at  TEXT NOT NULL,
  finished_at TEXT,

  CHECK (state IN ('pending','ready','failed')),
  CHECK (priority IS NULL OR priority IN ('P0','P1','P2','P3')),
  CHECK (confidence IS NULL OR confidence IN ('low','medium','high')),
  CHECK ((state = 'pending') = (finished_at IS NULL)),
  CHECK (state <> 'ready' OR (priority IS NOT NULL AND confidence IS NOT NULL
                              AND rationale IS NOT NULL)),
  CHECK (state <> 'failed' OR error IS NOT NULL),
  CHECK (applied IN (0,1)),
  CHECK (applied = 0 OR (state = 'ready' AND project IS NOT NULL)),
  CHECK (accepted IS NULL OR accepted IN (0,1)),
  CHECK (accepted IS NULL OR state = 'ready')
);

CREATE TABLE IF NOT EXISTS briefs (
  ask_id         TEXT PRIMARY KEY REFERENCES asks(id),
  state          TEXT NOT NULL,
  background     TEXT,
  recommendation TEXT,
  new_opinion    TEXT,
  confidence     TEXT,
  reversible     INTEGER,
  rationale      TEXT,
  cited          TEXT NOT NULL DEFAULT '[]',
  precedents     TEXT NOT NULL DEFAULT '[]',
  command        TEXT NOT NULL,
  error          TEXT,
  accepted       INTEGER,
  created_at     TEXT NOT NULL,
  finished_at    TEXT,

  CHECK (state IN ('pending','ready','failed')),
  CHECK (confidence IS NULL OR confidence IN ('low','medium','high')),
  CHECK (reversible IS NULL OR reversible IN (0,1)),
  CHECK (accepted IS NULL OR accepted IN (0,1)),
  CHECK ((state = 'pending') = (finished_at IS NULL)),
  CHECK (state <> 'ready' OR (confidence IS NOT NULL AND rationale IS NOT NULL
                              AND (recommendation IS NOT NULL OR new_opinion IS NOT NULL))),
  CHECK (state <> 'failed' OR error IS NOT NULL),
  CHECK (accepted IS NULL OR (state = 'ready' AND recommendation IS NOT NULL))
);
";

fn not_found(message: impl Into<String>) -> AppError {
    AppError::new(ErrorKind::NotFound, message)
}

fn conflict(message: impl Into<String>) -> AppError {
    AppError::new(ErrorKind::Conflict, message)
}

/// Every constraint this schema declares encodes a ledger rule, so tripping
/// one is a `conflict` with the caller-meaningful message.
fn constraint_err(err: rusqlite::Error, message: impl Into<String>) -> AppError {
    if err.sqlite_error_code() == Some(ErrorCode::ConstraintViolation) {
        conflict(message)
    } else {
        map_sqlite_err(err)
    }
}

fn map_sqlite_err(err: rusqlite::Error) -> AppError {
    AppError::new(ErrorKind::Internal, format!("sqlite error: {err}"))
}

/// JSON columns (`options`, `answers`) are written only by this module, so a
/// value that fails to decode means the file was edited by hand — fail
/// rather than show a partial row.
fn decode_json<T: serde::de::DeserializeOwned>(raw: &str, column: &str) -> Result<T, AppError> {
    serde_json::from_str(raw).map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("corrupt {column} column ({e}): {raw}"),
        )
    })
}

fn encode_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("plain data serializes infallibly")
}

#[cfg(test)]
mod tests;
