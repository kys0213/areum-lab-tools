use std::path::Path;

use crate::output::{AppError, ErrorKind, Payload};

/// Returns one item in full, including its body and labels. An unknown id is
/// `not_found`.
pub(crate) fn run_show(db_path: &Path, id: &str) -> Result<Payload, AppError> {
    let _ = (db_path, id);
    Err(AppError::new(
        ErrorKind::Internal,
        "show is not implemented yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_reports_itself_unimplemented_rather_than_not_found() {
        // `not_found` would be a lie about the board's contents; the gap is
        // in the tool, not in the data.
        let err = run_show(Path::new("/tmp/kanban.db"), "itm-000017").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "show is not implemented yet");
    }
}
