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
    use crate::commands::testutil::TempBoard;

    #[test]
    fn first_init_creates_the_board_and_reports_created() {
        let board = TempBoard::new("init-first");
        let payload = run_init(board.db_path()).unwrap();
        match payload {
            Payload::Init(data) => {
                assert!(data.created);
                assert_eq!(data.path, board.db_path().display().to_string());
            }
            other => panic!("expected Payload::Init, got {other:?}"),
        }
        assert!(board.db_path().exists());
    }

    #[test]
    fn second_init_on_the_same_path_reports_not_created() {
        let board = TempBoard::new("init-second");
        run_init(board.db_path()).unwrap();
        let payload = run_init(board.db_path()).unwrap();
        match payload {
            Payload::Init(data) => assert!(!data.created),
            other => panic!("expected Payload::Init, got {other:?}"),
        }
    }
}
