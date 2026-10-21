use std::path::Path;

use crate::common::store::Store;
use crate::output::{AppError, InitData, Payload};

/// Creates `~/.areum/doppel/` and the database, and registers the seed
/// categories that are missing. Re-running is safe: it reports
/// `created: false` and seeds nothing new.
pub(crate) fn run_init(db_path: &Path) -> Result<Payload, AppError> {
    let (store, created) = Store::open_reporting_created(db_path)?;
    let seeded_categories = store.seed_categories()?;
    Ok(Payload::Init(InitData {
        path: db_path.display().to_string(),
        created,
        seeded_categories,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::TempDir;

    #[test]
    fn first_init_creates_the_database_and_seeds() {
        let dir = TempDir::new("init-first");
        match run_init(&dir.db_path()).unwrap() {
            Payload::Init(data) => {
                assert!(data.created);
                assert_eq!(
                    data.seeded_categories,
                    ["design-scope", "naming-api-test", "dependency-review"]
                );
            }
            other => panic!("expected Payload::Init, got {other:?}"),
        }
        assert!(dir.db_path().exists());
    }

    #[test]
    fn second_init_reports_nothing_new() {
        let dir = TempDir::new("init-second");
        run_init(&dir.db_path()).unwrap();
        match run_init(&dir.db_path()).unwrap() {
            Payload::Init(data) => {
                assert!(!data.created);
                assert!(data.seeded_categories.is_empty());
            }
            other => panic!("expected Payload::Init, got {other:?}"),
        }
    }
}
