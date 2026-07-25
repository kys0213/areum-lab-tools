use std::path::Path;

use crate::common::store::Store;
use crate::output::{AppError, InitData, Payload};

/// Creates the board directory and SQLite database at `db_path`, applying the
/// schema. Answers `Payload::Init` with `created: false` when the board was
/// already there.
pub(crate) fn run_init(db_path: &Path) -> Result<Payload, AppError> {
    let (_store, created) = Store::open_reporting_created(db_path)?;
    Ok(Payload::Init(InitData {
        path: db_path.display().to_string(),
        created,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A unique, not-yet-existing db path under the OS temp dir, so tests
    /// running in the same process do not collide on the same file.
    fn unique_db_path(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "areum-kanban-init-test-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("kanban.db")
    }

    #[test]
    fn first_init_creates_the_board_and_reports_created() {
        let path = unique_db_path("first");
        let payload = run_init(&path).unwrap();
        match payload {
            Payload::Init(data) => {
                assert!(data.created);
                assert_eq!(data.path, path.display().to_string());
            }
            other => panic!("expected Payload::Init, got {other:?}"),
        }
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn second_init_on_the_same_path_reports_not_created() {
        let path = unique_db_path("second");
        run_init(&path).unwrap();
        let payload = run_init(&path).unwrap();
        match payload {
            Payload::Init(data) => assert!(!data.created),
            other => panic!("expected Payload::Init, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
