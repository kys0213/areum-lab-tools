use std::path::Path;

use crate::output::{AppError, ErrorKind, Payload};

/// Creates the board directory and SQLite database at `db_path`, applying the
/// schema. Answers `Payload::Init` with `created: false` when the board was
/// already there.
pub(crate) fn run_init(db_path: &Path) -> Result<Payload, AppError> {
    let _ = db_path;
    Err(AppError::new(
        ErrorKind::Internal,
        "init is not implemented yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_reports_itself_unimplemented_instead_of_faking_a_board() {
        let err = run_init(Path::new("/tmp/kanban.db")).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "init is not implemented yet");
    }
}
