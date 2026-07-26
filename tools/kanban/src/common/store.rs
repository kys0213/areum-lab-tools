//! SQLite persistence for the board. Owns the schema of docs/kanban-board.md
//! §6 verbatim — including the composite state/project/session invariant that
//! makes an inconsistent row unwritable — plus the single-statement atomic
//! claim that `next` relies on. Every mutation that reports both ends of a
//! transition reads and writes inside one `BEGIN IMMEDIATE` transaction (see
//! [`Store::apply`]), so the `before` it answers with is an image of the row
//! it actually wrote.
//!
//! Every failure leaves here as an [`AppError`] with the kind the CLI contract
//! promises (§8): a missing item or project is `not_found`, and every
//! constraint the schema declares (UNIQUE, CHECK, FOREIGN KEY) as well as
//! every refused transition is `conflict`. A raw rusqlite error only surfaces
//! as `internal` when no specific kind applies.
//!
//! Only the classifier's `classification`/`golden` tables are missing: they
//! are additive DDL owned by later stages and land the same way this schema
//! does.

use std::path::Path;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};
use rusqlite::{Connection, ErrorCode, OptionalExtension, Row, params};

use crate::cli::{ItemState, Priority};
use crate::common::error::{AppError, ErrorKind};
use crate::common::time::now_rfc3339;

/// Wall-clock source for the `created_at`/`updated_at`/`claimed_at` columns.
/// Injected rather than called inside the query code so tests pin exact
/// timestamps and, with them, the claim's `ORDER BY created_at` tie-break.
pub(crate) trait Clock {
    fn now(&self) -> String;
}

pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> String {
        now_rfc3339()
    }
}

/// A registered project row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectRecord {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) created_at: String,
}

/// An inbound issue as the caller pushes it. State is always `inbox` and the
/// id is minted here, so neither is part of the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NewItem<'a> {
    pub(crate) source: &'a str,
    pub(crate) external_id: &'a str,
    pub(crate) title: &'a str,
    pub(crate) body: &'a str,
}

/// A persisted `items` row. `state`/`priority` decode into the CLI's token
/// enums (the same values the schema's CHECK constraints allow), so an
/// unexpected token in the file fails at the decode boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ItemRecord {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) external_id: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) state: ItemState,
    pub(crate) project: Option<String>,
    pub(crate) priority: Priority,
    pub(crate) session_id: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) claimed_at: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

/// Both ends of a state change. Every mutating operation reports it, because
/// the command payloads answer with what changed (`previous_state`,
/// `previous_priority`, the claim `release` dropped), not only the new row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Transition {
    pub(crate) before: ItemRecord,
    pub(crate) after: ItemRecord,
}

/// A label row. `confidence` is NULL when a human attached the label — only
/// the classifier produces a score.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LabelRecord {
    pub(crate) key: String,
    pub(crate) value: String,
    pub(crate) confidence: Option<f64>,
}

/// `list` filters. `label` matches a label *key* (`--label duplicate-of`),
/// which is what the command surface documents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ItemFilter<'a> {
    pub(crate) project: Option<&'a str>,
    pub(crate) state: Option<&'a str>,
    pub(crate) label: Option<&'a str>,
}

/// How long a statement waits on a lock held by another connection before
/// giving up. Concurrent `next` callers are serialized by SQLite's writer
/// lock, so they must wait rather than fail.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(5_000);

pub(crate) struct Store {
    conn: Connection,
    clock: Box<dyn Clock>,
}

impl Store {
    /// Opens (creating if needed) the board database with the wall clock.
    pub(crate) fn open(path: &Path) -> Result<Store, AppError> {
        Store::open_with_clock(path, Box::new(SystemClock))
    }

    /// Same as [`Store::open`], additionally reporting whether this call
    /// created the board file rather than opening one that already existed.
    /// `init` is the only caller that needs the flag; every other caller
    /// keeps using [`Store::open`].
    pub(crate) fn open_reporting_created(path: &Path) -> Result<(Store, bool), AppError> {
        Store::open_with_clock_reporting_created(path, Box::new(SystemClock))
    }

    /// Same as [`Store::open`] with an injected clock. Creates the parent
    /// directory, configures the connection (foreign keys, WAL, busy timeout)
    /// and ensures the schema — all idempotent, so opening an existing board
    /// is the same call as creating one.
    pub(crate) fn open_with_clock(path: &Path, clock: Box<dyn Clock>) -> Result<Store, AppError> {
        Store::open_with_clock_reporting_created(path, clock).map(|(store, _created)| store)
    }

    /// Same as [`Store::open_with_clock`], additionally reporting whether
    /// this call created the board file. The existence check happens here,
    /// immediately before the call that would create the file, rather than
    /// in the caller — checking from outside this function would race this
    /// function's own directory/file creation below.
    pub(crate) fn open_with_clock_reporting_created(
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
        // busy_timeout is set before anything that can take a lock so every
        // later statement queues instead of failing immediately.
        conn.busy_timeout(BUSY_TIMEOUT).map_err(map_sqlite_err)?;
        enable_foreign_keys(&conn)?;
        enable_wal(&conn)?;
        create_schema(&conn)?;
        Ok((Store { conn, clock }, created))
    }

    /// In-memory board for tests that do not need two connections. WAL is
    /// meaningless for `:memory:`, so it is skipped.
    #[cfg(test)]
    pub(crate) fn open_in_memory(clock: Box<dyn Clock>) -> Result<Store, AppError> {
        let conn = Connection::open_in_memory().map_err(map_sqlite_err)?;
        enable_foreign_keys(&conn)?;
        create_schema(&conn)?;
        Ok(Store { conn, clock })
    }

    pub(crate) fn add_project(
        &self,
        name: &str,
        description: &str,
    ) -> Result<ProjectRecord, AppError> {
        let created_at = self.clock.now();
        self.conn
            .execute(
                "INSERT INTO projects (name, description, created_at) VALUES (?1, ?2, ?3)",
                params![name, description, created_at],
            )
            .map_err(|e| constraint_err(e, format!("project {name} already exists")))?;
        Ok(ProjectRecord {
            name: name.to_owned(),
            description: description.to_owned(),
            created_at,
        })
    }

    pub(crate) fn list_projects(&self) -> Result<Vec<ProjectRecord>, AppError> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, description, created_at FROM projects ORDER BY name ASC")
            .map_err(map_sqlite_err)?;
        stmt.query_map([], |row| {
            Ok(ProjectRecord {
                name: row.get("name")?,
                description: row.get("description")?,
                created_at: row.get("created_at")?,
            })
        })
        .map_err(map_sqlite_err)?
        .collect::<rusqlite::Result<Vec<ProjectRecord>>>()
        .map_err(map_sqlite_err)
    }

    /// Removes a project. Items referencing it hold a foreign key, so the
    /// delete is refused as a `conflict` rather than orphaning them — that
    /// enforcement is the reason every connection sets `foreign_keys = ON`.
    pub(crate) fn remove_project(&self, name: &str) -> Result<(), AppError> {
        let removed = self
            .conn
            .execute("DELETE FROM projects WHERE name = ?1", params![name])
            .map_err(|e| {
                constraint_err(
                    e,
                    format!("project {name} still has items; move or finish them first"),
                )
            })?;
        if removed == 0 {
            return Err(not_found(format!("no project {name}")));
        }
        Ok(())
    }

    /// Mints the next `itm-NNNNNN` id and inserts the item in `inbox`. Both
    /// steps run inside one immediate transaction so two concurrent intakes
    /// cannot mint the same id. A repeat of `(source, external_id)` is a
    /// `conflict`, never a second item.
    pub(crate) fn insert_item(&mut self, item: &NewItem<'_>) -> Result<ItemRecord, AppError> {
        let now = self.clock.now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(map_sqlite_err)?;
        let id = next_item_id(&tx)?;
        let record = tx
            .query_row(
                &format!(
                    "INSERT INTO items (id, source, external_id, title, body, state, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'inbox', ?6, ?6)
                     RETURNING {ITEM_COLUMNS}"
                ),
                params![id, item.source, item.external_id, item.title, item.body, now],
                row_to_item,
            )
            .map_err(|e| {
                constraint_err(
                    e,
                    format!(
                        "item from {}/{} is already on the board",
                        item.source, item.external_id
                    ),
                )
            })?;
        tx.commit().map_err(map_sqlite_err)?;
        Ok(record)
    }

    pub(crate) fn get_item(&self, id: &str) -> Result<ItemRecord, AppError> {
        require_item(&self.conn, id)
    }

    pub(crate) fn list_items(&self, filter: &ItemFilter<'_>) -> Result<Vec<ItemRecord>, AppError> {
        let state = filter.state.map(parse_state).transpose()?;
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {ITEM_COLUMNS} FROM items {ITEM_FILTER_SQL}"
            ))
            .map_err(map_sqlite_err)?;
        stmt.query_map(
            params![filter.project, state.map(ItemState::as_str), filter.label],
            row_to_item,
        )
        .map_err(map_sqlite_err)?
        .collect::<rusqlite::Result<Vec<ItemRecord>>>()
        .map_err(map_sqlite_err)
    }

    /// Atomically claims the project's next backlog item: the select and the
    /// update are one statement, so SQLite's writer serialization alone
    /// guarantees two agents never receive the same row. An empty backlog is
    /// not an error — it answers `None`.
    pub(crate) fn claim_next(
        &self,
        project: &str,
        session: &str,
        agent: &str,
    ) -> Result<Option<ItemRecord>, AppError> {
        let now = self.clock.now();
        self.conn
            .query_row(
                &format!(
                    "UPDATE items
                        SET state = 'running', session_id = ?1, agent = ?2,
                            claimed_at = ?3, updated_at = ?3
                      WHERE id = (SELECT id FROM items
                                   WHERE project = ?4 AND state = 'backlog'
                                   ORDER BY priority ASC, created_at ASC
                                   LIMIT 1)
                     RETURNING {ITEM_COLUMNS}"
                ),
                params![session, agent, now, project],
                row_to_item,
            )
            .optional()
            .map_err(map_sqlite_err)
    }

    /// Completes a claimed item. `session_id`/`agent` are deliberately left
    /// in place: who did the work has to survive completion (§4).
    pub(crate) fn mark_done(&mut self, id: &str) -> Result<Transition, AppError> {
        self.apply(id, |conn, before, now| {
            let hint = format!(
                "item {id} is {}, not running; only a claimed item can be completed",
                before.state.as_str()
            );
            guarded_update(
                conn,
                &format!(
                    "UPDATE items SET state = 'done', updated_at = ?2
                      WHERE id = ?1 AND state = 'running'
                     RETURNING {ITEM_COLUMNS}"
                ),
                params![id, now],
                &hint,
            )
        })
    }

    /// Drops a claim and returns the item to `backlog`, clearing
    /// `session_id`/`agent`/`claimed_at`. The dropped claim is readable from
    /// the returned transition's `before`, so that snapshot has to be the
    /// image of the very row this write revokes — see [`Store::apply`].
    pub(crate) fn release(&mut self, id: &str) -> Result<Transition, AppError> {
        self.apply(id, |conn, before, now| {
            let hint = format!(
                "item {id} is {}, not running; there is no claim to release",
                before.state.as_str()
            );
            guarded_update(
                conn,
                &format!(
                    "UPDATE items
                        SET state = 'backlog', session_id = NULL, agent = NULL,
                            claimed_at = NULL, updated_at = ?2
                      WHERE id = ?1 AND state = 'running'
                     RETURNING {ITEM_COLUMNS}"
                ),
                params![id, now],
                &hint,
            )
        })
    }

    /// Assigns an unclassified item to a project (`inbox`/`unmatched` →
    /// `backlog`), optionally correcting the priority in the same write. An
    /// already-assigned item is refused: re-targeting a claimed or finished
    /// item would strand its agent, so `move` is the escape hatch for that.
    pub(crate) fn assign(
        &mut self,
        id: &str,
        project: &str,
        priority: Option<&str>,
    ) -> Result<Transition, AppError> {
        let priority = priority.map(parse_priority).transpose()?;
        self.apply(id, move |conn, before, now| {
            // Inside the transaction, so a project that survives this check
            // cannot be removed before the write lands on it.
            require_project(conn, project)?;
            let hint = format!(
                "item {id} is {}; only an inbox or unmatched item can be assigned",
                before.state.as_str()
            );
            guarded_update(
                conn,
                &format!(
                    "UPDATE items
                        SET state = 'backlog', project = ?2,
                            priority = COALESCE(?3, priority), updated_at = ?4
                      WHERE id = ?1 AND state IN ('inbox', 'unmatched')
                     RETURNING {ITEM_COLUMNS}"
                ),
                params![id, project, priority.map(Priority::as_str), now],
                &hint,
            )
        })
    }

    /// Corrects the priority without touching the state.
    ///
    /// The statement carries no precondition — a priority correction is legal
    /// from every state — but the read still shares [`Store::apply`]'s
    /// transaction, because the payload reports `previous_priority` from the
    /// snapshot and that value has to be the one this write replaced.
    pub(crate) fn set_priority(
        &mut self,
        id: &str,
        priority: &str,
    ) -> Result<Transition, AppError> {
        let priority = parse_priority(priority)?;
        self.apply(id, move |conn, _before, now| {
            // The row was read in this transaction and nothing but `id` guards
            // the statement, so zero rows here is unreachable short of the
            // transaction failing to isolate.
            let hint = format!(
                "item {id} is no longer available to take priority {}",
                priority.as_str()
            );
            guarded_update(
                conn,
                &format!(
                    "UPDATE items SET priority = ?2, updated_at = ?3
                      WHERE id = ?1
                     RETURNING {ITEM_COLUMNS}"
                ),
                params![id, priority.as_str(), now],
                &hint,
            )
        })
    }

    /// The `move` escape hatch. The target state dictates which columns must
    /// be cleared for the row to stay inside the schema's invariant (see
    /// [`plan_move`]); a target the current row cannot legally reach is a
    /// `conflict` rather than a silent no-op.
    pub(crate) fn move_item(&mut self, id: &str, state: &str) -> Result<Transition, AppError> {
        let target = parse_state(state)?;
        self.apply(id, move |conn, before, now| {
            apply_move(conn, before, target, now)
        })
    }

    /// Attaches (or re-scores) a label. The classifier owns the write path;
    /// `list`/`show` read it back through [`Store::labels_for`].
    ///
    /// No command writes labels yet, so outside the tests the first caller
    /// arrives with the classifier. The expectation is scoped to this one
    /// method rather than the module, so anything else going unused still
    /// fails the build, and it lifts on its own once that caller lands.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "label writes belong to the classifier; the read path already ships"
        )
    )]
    pub(crate) fn attach_label(
        &self,
        item_id: &str,
        key: &str,
        value: &str,
        confidence: Option<f64>,
    ) -> Result<(), AppError> {
        self.conn
            .execute(
                "INSERT INTO labels (item_id, key, value, confidence) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (item_id, key, value) DO UPDATE SET confidence = excluded.confidence",
                params![item_id, key, value, confidence],
            )
            // The only constraint the upsert can still trip is the item_id
            // foreign key, i.e. a label for an item that is not on the board.
            .map_err(|e| {
                if is_constraint_violation(&e) {
                    not_found(format!("no item {item_id}"))
                } else {
                    map_sqlite_err(e)
                }
            })?;
        Ok(())
    }

    pub(crate) fn labels_for(&self, item_id: &str) -> Result<Vec<LabelRecord>, AppError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT key, value, confidence FROM labels
                  WHERE item_id = ?1 ORDER BY key ASC, value ASC",
            )
            .map_err(map_sqlite_err)?;
        stmt.query_map(params![item_id], |row| {
            Ok(LabelRecord {
                key: row.get("key")?,
                value: row.get("value")?,
                confidence: row.get("confidence")?,
            })
        })
        .map_err(map_sqlite_err)?
        .collect::<rusqlite::Result<Vec<LabelRecord>>>()
        .map_err(map_sqlite_err)
    }

    /// Reads the row, lets `write` plan and run its update from that exact
    /// snapshot, and pairs both ends into the [`Transition`] the payloads are
    /// built from.
    ///
    /// The read and the write share one `BEGIN IMMEDIATE` transaction, and
    /// that — not the statement's `WHERE` — is what makes `before` an honest
    /// image of the row this call wrote. The preconditions in the `WHERE`
    /// (`state = 'running'`, `state IN ('inbox','unmatched')`) are literals:
    /// they keep an illegal transition from landing, but on their own they
    /// match any row that happens to satisfy them, including one another
    /// process moved into that state after the snapshot was taken. Holding the
    /// write lock across both statements removes that window, so zero rows
    /// updated means the snapshot itself failed the precondition — an illegal
    /// transition, reported as a `conflict` with the caller's hint, never a
    /// silent success and no longer a lost race.
    ///
    /// `write` is handed the timestamp rather than reading the clock itself so
    /// `updated_at` is stamped inside that same window, and a row that is not
    /// on the board costs no tick at all.
    fn apply(
        &mut self,
        id: &str,
        write: impl FnOnce(&Connection, &ItemRecord, &str) -> Result<ItemRecord, AppError>,
    ) -> Result<Transition, AppError> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(map_sqlite_err)?;
        let before = require_item(&tx, id)?;
        let now = self.clock.now();
        let after = write(&tx, &before, &now)?;
        tx.commit().map_err(map_sqlite_err)?;
        Ok(Transition { before, after })
    }
}

/// Writes a `move` planned from `before`, guarding the update on that exact
/// snapshot.
///
/// The guard is redundant with [`Store::apply`]'s transaction and kept
/// deliberately: [`plan_move`] decides in Rust which columns to clear by
/// reading `before.project`/`before.session_id`, and a `running → done` move
/// is the one transition the composite CHECK would happily let land on a row
/// that had changed underneath (`done` is its loose branch). Restating the
/// snapshot in the `WHERE` states that dependency in the statement itself, so
/// the plan can never be applied to a row it was not planned from — whatever
/// isolation the surrounding call happens to provide.
fn apply_move(
    conn: &Connection,
    before: &ItemRecord,
    target: ItemState,
    now: &str,
) -> Result<ItemRecord, AppError> {
    let plan = plan_move(
        target,
        before.project.as_deref(),
        before.session_id.as_deref(),
    )?;
    // plan_move already rejected every target this row cannot legally reach,
    // and the snapshot was read inside this transaction, so zero rows updated
    // means the guard and the row disagree — the isolation the caller relies
    // on failed, not a refused transition.
    let hint = format!(
        "item {} could not be moved to {}: the row no longer matches the {} snapshot \
         the move was planned from",
        before.id,
        target.as_str(),
        before.state.as_str()
    );
    // `IS` rather than `=` so the NULL columns the plan read compare equal to
    // themselves: an `inbox`/`unmatched` snapshot carries a NULL project and a
    // NULL session, and `=` would make the guard match nothing at all.
    let sql = format!(
        "UPDATE items
            SET state = ?2,
                project    = CASE WHEN ?3 THEN NULL ELSE project END,
                session_id = CASE WHEN ?4 THEN NULL ELSE session_id END,
                agent      = CASE WHEN ?4 THEN NULL ELSE agent END,
                claimed_at = CASE WHEN ?4 THEN NULL ELSE claimed_at END,
                updated_at = ?5
          WHERE id = ?1
            AND state = ?6
            AND project IS ?7
            AND session_id IS ?8
         RETURNING {ITEM_COLUMNS}"
    );
    guarded_update(
        conn,
        &sql,
        params![
            before.id,
            target.as_str(),
            plan.clear_project,
            plan.clear_claim,
            now,
            before.state.as_str(),
            before.project,
            before.session_id,
        ],
        &hint,
    )
}

fn fetch_item(conn: &Connection, id: &str) -> Result<Option<ItemRecord>, AppError> {
    conn.query_row(
        &format!("SELECT {ITEM_COLUMNS} FROM items WHERE id = ?1"),
        params![id],
        row_to_item,
    )
    .optional()
    .map_err(map_sqlite_err)
}

fn require_item(conn: &Connection, id: &str) -> Result<ItemRecord, AppError> {
    fetch_item(conn, id)?.ok_or_else(|| not_found(format!("no item {id}")))
}

fn require_project(conn: &Connection, name: &str) -> Result<(), AppError> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM projects WHERE name = ?1)",
            params![name],
            |row| row.get(0),
        )
        .map_err(map_sqlite_err)?;
    if exists {
        Ok(())
    } else {
        Err(not_found(format!("no project {name}")))
    }
}

/// Runs an `UPDATE ... RETURNING` and answers the row it wrote. Matching
/// nothing is a `conflict` carrying `conflict_hint`, as is tripping any
/// constraint the schema declares.
fn guarded_update(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
    conflict_hint: &str,
) -> Result<ItemRecord, AppError> {
    match conn.query_row(sql, params, row_to_item).optional() {
        Ok(Some(after)) => Ok(after),
        Ok(None) => Err(AppError::new(ErrorKind::Conflict, conflict_hint)),
        Err(err) if is_constraint_violation(&err) => Err(AppError::new(
            ErrorKind::Conflict,
            format!("{conflict_hint} ({err})"),
        )),
        Err(err) => Err(map_sqlite_err(err)),
    }
}

/// Which columns a `move` has to clear for the target state to satisfy the
/// schema's composite invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MovePlan {
    clear_project: bool,
    clear_claim: bool,
}

/// Pure transition planner for `move`. `inbox`/`unmatched` must end up with
/// neither a project nor a claim; `backlog` keeps its project but drops the
/// claim; `running` and `done` keep both.
///
/// A target the row cannot reach is rejected here, before the statement runs,
/// so the caller gets an actionable `conflict` instead of an opaque CHECK
/// failure. `move` cannot invent a session, so an unclaimed item can never
/// become `running` — that is `next`'s job, and the message says so.
fn plan_move(
    target: ItemState,
    project: Option<&str>,
    session_id: Option<&str>,
) -> Result<MovePlan, AppError> {
    match target {
        ItemState::Inbox | ItemState::Unmatched => Ok(MovePlan {
            clear_project: true,
            clear_claim: true,
        }),
        ItemState::Backlog if project.is_some() => Ok(MovePlan {
            clear_project: false,
            clear_claim: true,
        }),
        ItemState::Done if project.is_some() => Ok(MovePlan {
            clear_project: false,
            clear_claim: false,
        }),
        ItemState::Running if project.is_some() && session_id.is_some() => Ok(MovePlan {
            clear_project: false,
            clear_claim: false,
        }),
        ItemState::Running => Err(AppError::new(
            ErrorKind::Conflict,
            "running needs a project and a claim, and `move` can invent neither; \
             use `next` to claim a backlog item",
        )),
        ItemState::Backlog | ItemState::Done => Err(AppError::new(
            ErrorKind::Conflict,
            format!("{} needs a project; assign the item first", target.as_str()),
        )),
    }
}

/// Single source of the `items` column list, shared by every SELECT and
/// RETURNING so [`row_to_item`]'s named lookups always resolve.
const ITEM_COLUMNS: &str = "id, source, external_id, title, body, state, project, priority, \
    session_id, agent, claimed_at, created_at, updated_at";

/// `list` filter clause. Each filter is bound as NULL when absent, so one
/// prepared statement serves every combination.
const ITEM_FILTER_SQL: &str = "WHERE (?1 IS NULL OR project = ?1)
       AND (?2 IS NULL OR state = ?2)
       AND (?3 IS NULL OR EXISTS (SELECT 1 FROM labels
                                   WHERE labels.item_id = items.id AND labels.key = ?3))
     ORDER BY created_at ASC, id ASC";

fn row_to_item(row: &Row<'_>) -> rusqlite::Result<ItemRecord> {
    Ok(ItemRecord {
        id: row.get("id")?,
        source: row.get("source")?,
        external_id: row.get("external_id")?,
        title: row.get("title")?,
        body: row.get("body")?,
        state: row.get("state")?,
        project: row.get("project")?,
        priority: row.get("priority")?,
        session_id: row.get("session_id")?,
        agent: row.get("agent")?,
        claimed_at: row.get("claimed_at")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// Fail-fast column decoding: a state token outside the CHECK constraint is a
/// schema contract violation, surfaced as a conversion error rather than
/// defaulted to some "safe" state.
impl FromSql for ItemState {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let raw = value.as_str()?;
        state_from_token(raw).ok_or_else(|| {
            FromSqlError::Other(format!("unknown item state in store: {raw}").into())
        })
    }
}

impl FromSql for Priority {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let raw = value.as_str()?;
        priority_from_token(raw).ok_or_else(|| {
            FromSqlError::Other(format!("unknown item priority in store: {raw}").into())
        })
    }
}

fn state_from_token(raw: &str) -> Option<ItemState> {
    match raw {
        "inbox" => Some(ItemState::Inbox),
        "unmatched" => Some(ItemState::Unmatched),
        "backlog" => Some(ItemState::Backlog),
        "running" => Some(ItemState::Running),
        "done" => Some(ItemState::Done),
        _ => None,
    }
}

fn priority_from_token(raw: &str) -> Option<Priority> {
    match raw {
        "P0" => Some(Priority::P0),
        "P1" => Some(Priority::P1),
        "P2" => Some(Priority::P2),
        "P3" => Some(Priority::P3),
        _ => None,
    }
}

/// Rejects an unknown token before it reaches the schema, so a bad argument
/// reads as a usage error instead of an opaque CHECK failure. Reachable only
/// from non-CLI callers — clap validates the CLI's own arguments.
fn parse_state(raw: &str) -> Result<ItemState, AppError> {
    state_from_token(raw).ok_or_else(|| {
        AppError::new(
            ErrorKind::Usage,
            format!("unknown state {raw}; expected inbox|unmatched|backlog|running|done"),
        )
    })
}

fn parse_priority(raw: &str) -> Result<Priority, AppError> {
    priority_from_token(raw).ok_or_else(|| {
        AppError::new(
            ErrorKind::Usage,
            format!("unknown priority {raw}; expected P0|P1|P2|P3"),
        )
    })
}

/// Next `itm-NNNNNN` id. The maximum is taken numerically rather than
/// lexicographically so the sequence stays correct past six digits.
fn next_item_id(conn: &Connection) -> Result<String, AppError> {
    let highest: Option<i64> = conn
        .query_row(
            "SELECT MAX(CAST(substr(id, 5) AS INTEGER)) FROM items",
            [],
            |row| row.get(0),
        )
        .map_err(map_sqlite_err)?;
    Ok(format_item_id(highest.unwrap_or(0) + 1))
}

/// The documented id shape (§6): `itm-` plus a zero-padded sequence number.
fn format_item_id(sequence: i64) -> String {
    format!("itm-{sequence:06}")
}

/// Enforces the `items.project` and `labels.item_id` foreign keys. SQLite
/// leaves them off per connection by default, which would let `project rm`
/// orphan items instead of being refused.
fn enable_foreign_keys(conn: &Connection) -> Result<(), AppError> {
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(map_sqlite_err)
}

/// Switches the connection to WAL, retrying on SQLITE_BUSY. The retry loop
/// (rather than trusting busy_timeout) is deliberate: the WAL transition needs
/// brief exclusive locks and SQLite skips the busy handler for parts of that
/// dance, so two agents racing the first open of a fresh board would otherwise
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

/// Creates the board schema. Every statement is additive and
/// `IF NOT EXISTS`, so opening an existing board is a no-op, two processes
/// racing the first open simply serialize on SQLite's writer lock, and a
/// crash mid-batch heals on the next open. Later stages add their tables by
/// appending here — which is also why the batch is not skipped behind a
/// version check: a board created before those tables existed still has to
/// pick them up.
fn create_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(CREATE_SCHEMA_SQL)
        .map_err(map_sqlite_err)
}

/// docs/kanban-board.md §6 verbatim. The composite CHECK is the invariant the
/// whole state model rests on; `done` is the loose branch on purpose, because
/// it is reachable both from an agent claim (session preserved) and from a
/// human `move` (no session).
const CREATE_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS projects (
  name        TEXT PRIMARY KEY,
  description TEXT NOT NULL,
  created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS items (
  id          TEXT PRIMARY KEY,
  source      TEXT NOT NULL,
  external_id TEXT NOT NULL,
  title       TEXT NOT NULL,
  body        TEXT NOT NULL,
  state       TEXT NOT NULL,
  project     TEXT REFERENCES projects(name),
  priority    TEXT NOT NULL DEFAULT 'P2',
  session_id  TEXT,
  agent       TEXT,
  claimed_at  TEXT,
  created_at  TEXT NOT NULL,
  updated_at  TEXT NOT NULL,

  UNIQUE (source, external_id),
  CHECK (priority IN ('P0','P1','P2','P3')),
  CHECK (state IN ('inbox','unmatched','backlog','running','done')),
  CHECK (
    (state IN ('inbox','unmatched') AND project IS NULL     AND session_id IS NULL)
 OR (state = 'backlog'              AND project IS NOT NULL AND session_id IS NULL)
 OR (state = 'running'              AND project IS NOT NULL AND session_id IS NOT NULL)
 OR (state = 'done'                 AND project IS NOT NULL)
  )
);

CREATE TABLE IF NOT EXISTS labels (
  item_id    TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  key        TEXT NOT NULL,
  value      TEXT NOT NULL,
  confidence REAL,
  PRIMARY KEY (item_id, key, value)
);
";

fn not_found(message: impl Into<String>) -> AppError {
    AppError::new(ErrorKind::NotFound, message)
}

/// Every constraint this schema declares — UNIQUE, CHECK, FOREIGN KEY —
/// encodes a board rule, so tripping one is a `conflict` (exit 8) with the
/// caller-meaningful message, not an opaque internal failure.
fn constraint_err(err: rusqlite::Error, message: impl Into<String>) -> AppError {
    if is_constraint_violation(&err) {
        AppError::new(ErrorKind::Conflict, message)
    } else {
        map_sqlite_err(err)
    }
}

fn is_constraint_violation(err: &rusqlite::Error) -> bool {
    err.sqlite_error_code() == Some(ErrorCode::ConstraintViolation)
}

fn map_sqlite_err(err: rusqlite::Error) -> AppError {
    AppError::new(ErrorKind::Internal, format!("sqlite error: {err}"))
}

#[cfg(test)]
mod tests;
