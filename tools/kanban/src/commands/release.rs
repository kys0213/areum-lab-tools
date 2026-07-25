use std::path::Path;

use crate::output::{AppError, ErrorKind, Payload};

/// Drops a claim (`running → backlog`), clearing `session_id`/`agent`/
/// `claimed_at`. Failure is a released claim, never a state — this is the only
/// recovery path for an item whose agent died.
pub(crate) fn run_release(db_path: &Path, id: &str, reason: &str) -> Result<Payload, AppError> {
    let _ = (db_path, id, reason);
    Err(AppError::new(
        ErrorKind::Internal,
        "release is not implemented yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_reports_itself_unimplemented_instead_of_freeing_a_claim() {
        let err =
            run_release(Path::new("/tmp/kanban.db"), "itm-000017", "build failed").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "release is not implemented yet");
    }
}
