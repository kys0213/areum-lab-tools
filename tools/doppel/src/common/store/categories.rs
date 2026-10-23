//! `categories` — the kinds of decision the ledger files under. Rows are never
//! deleted: a retired category keeps its filings and precedents but accepts no
//! new ones.

use rusqlite::{OptionalExtension, Row, params};

use super::{Store, conflict, constraint_err, map_sqlite_err, not_found};
use crate::common::error::AppError;

/// The seed set `init` registers (docs/doppel-judgment.md §2).
pub(crate) const SEED_CATEGORIES: [(&str, &str); 4] = [
    (
        "design-scope",
        "설계 승인·범위 자르기 — plan 승인·거절, 범위 밖 판정, 버린 선택지",
    ),
    (
        "naming-api-test",
        "네이밍·API·테스트 전략 — 이름, 인터페이스 모양, 테스트 방식",
    ),
    (
        "dependency-review",
        "의존성·리뷰 판정 — 라이브러리 선택, 리뷰 finding 수용·거절",
    ),
    ("triage", "작업 배정 — 들어온 작업의 프로젝트·우선순위"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CategoryRecord {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) state: String,
    pub(crate) created_at: String,
}

impl Store {
    /// Registers the seed categories that are not there yet and answers the
    /// names it added. Re-running `init` adds nothing and changes nothing —
    /// a seed the user retired stays retired.
    pub(crate) fn seed_categories(&self) -> Result<Vec<String>, AppError> {
        let now = self.clock.now();
        let mut added = Vec::new();
        for (name, description) in SEED_CATEGORIES {
            let changed = self
                .conn
                .execute(
                    "INSERT INTO categories (name, description, created_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(name) DO NOTHING",
                    params![name, description, now],
                )
                .map_err(map_sqlite_err)?;
            if changed == 1 {
                added.push(name.to_owned());
            }
        }
        Ok(added)
    }

    pub(crate) fn add_category(
        &self,
        name: &str,
        description: &str,
    ) -> Result<CategoryRecord, AppError> {
        let now = self.clock.now();
        self.conn
            .execute(
                "INSERT INTO categories (name, description, created_at) VALUES (?1, ?2, ?3)",
                params![name, description, now],
            )
            .map_err(|e| constraint_err(e, format!("category {name} already exists")))?;
        self.get_category(name)
    }

    pub(crate) fn list_categories(&self) -> Result<Vec<CategoryRecord>, AppError> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, description, state, created_at FROM categories ORDER BY name")
            .map_err(map_sqlite_err)?;
        stmt.query_map([], row_to_category)
            .map_err(map_sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite_err)
    }

    pub(crate) fn retire_category(&self, name: &str) -> Result<CategoryRecord, AppError> {
        let current = self.get_category(name)?;
        if current.state == "retired" {
            return Err(conflict(format!("category {name} is already retired")));
        }
        self.conn
            .execute(
                "UPDATE categories SET state = 'retired' WHERE name = ?1",
                params![name],
            )
            .map_err(map_sqlite_err)?;
        self.get_category(name)
    }

    pub(crate) fn get_category(&self, name: &str) -> Result<CategoryRecord, AppError> {
        self.conn
            .query_row(
                "SELECT name, description, state, created_at FROM categories WHERE name = ?1",
                params![name],
                row_to_category,
            )
            .optional()
            .map_err(map_sqlite_err)?
            .ok_or_else(|| not_found(format!("no category {name}")))
    }

    /// The category a new filing may point at: it must exist and be active.
    pub(super) fn require_active_category(&self, name: &str) -> Result<(), AppError> {
        let category = self.get_category(name)?;
        if category.state == "retired" {
            return Err(conflict(format!(
                "category {name} is retired and accepts no new filings"
            )));
        }
        Ok(())
    }
}

fn row_to_category(row: &Row<'_>) -> rusqlite::Result<CategoryRecord> {
    Ok(CategoryRecord {
        name: row.get(0)?,
        description: row.get(1)?,
        state: row.get(2)?,
        created_at: row.get(3)?,
    })
}
