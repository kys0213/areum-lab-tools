//! `projects` — where cards are worked on: a name the board uses and the
//! local directory doppel launches sessions in.

use rusqlite::{OptionalExtension, Row, params};

use super::{Store, constraint_err, map_sqlite_err, not_found};
use crate::common::error::{AppError, ErrorKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectRecord {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) description: String,
    pub(crate) created_at: String,
}

impl Store {
    /// The path must be an existing directory now; whether it still is when
    /// a card is dispatched is checked again then.
    pub(crate) fn add_project(
        &self,
        name: &str,
        path: &str,
        description: &str,
    ) -> Result<ProjectRecord, AppError> {
        if !std::path::Path::new(path).is_dir() {
            return Err(AppError::new(
                ErrorKind::Usage,
                format!("project path {path} is not a directory"),
            ));
        }
        let now = self.clock.now();
        self.conn
            .execute(
                "INSERT INTO projects (name, path, description, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![name, path, description, now],
            )
            .map_err(|e| constraint_err(e, format!("project {name} already exists")))?;
        self.get_project(name)
    }

    pub(crate) fn get_project(&self, name: &str) -> Result<ProjectRecord, AppError> {
        self.conn
            .query_row(
                "SELECT name, path, description, created_at FROM projects WHERE name = ?1",
                params![name],
                row_to_project,
            )
            .optional()
            .map_err(map_sqlite_err)?
            .ok_or_else(|| not_found(format!("no project {name}")))
    }

    pub(crate) fn list_projects(&self) -> Result<Vec<ProjectRecord>, AppError> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, path, description, created_at FROM projects ORDER BY name")
            .map_err(map_sqlite_err)?;
        stmt.query_map([], row_to_project)
            .map_err(map_sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite_err)
    }
}

fn row_to_project(row: &Row<'_>) -> rusqlite::Result<ProjectRecord> {
    Ok(ProjectRecord {
        name: row.get(0)?,
        path: row.get(1)?,
        description: row.get(2)?,
        created_at: row.get(3)?,
    })
}
