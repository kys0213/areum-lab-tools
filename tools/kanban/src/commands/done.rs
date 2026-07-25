use std::path::Path;

use crate::output::{AppError, ErrorKind, Payload};

/// Completes an item (`running → done`), **keeping** `session_id`/`agent` so
/// the record of who did the work survives completion (docs §4). Calling it
/// on an item that is not claimed is a `conflict`, not a silent success.
pub(crate) fn run_done(db_path: &Path, id: &str) -> Result<Payload, AppError> {
    let _ = (db_path, id);
    Err(AppError::new(
        ErrorKind::Internal,
        "done is not implemented yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_reports_itself_unimplemented_instead_of_claiming_completion() {
        let err = run_done(Path::new("/tmp/kanban.db"), "itm-000017").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "done is not implemented yet");
    }
}
