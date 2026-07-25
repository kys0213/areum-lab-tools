use std::path::Path;

use crate::output::{AppError, ErrorKind, Payload};

/// Assigns an item to a project by hand (`inbox`/`unmatched` → `backlog`),
/// optionally correcting the priority in the same move. An unknown project is
/// `not_found` rather than a nearest-name match.
pub(crate) fn run_assign(
    db_path: &Path,
    id: &str,
    project: &str,
    priority: Option<&str>,
) -> Result<Payload, AppError> {
    let _ = (db_path, id, project, priority);
    Err(AppError::new(
        ErrorKind::Internal,
        "assign is not implemented yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assign_reports_itself_unimplemented_instead_of_moving_the_item() {
        let err = run_assign(
            Path::new("/tmp/kanban.db"),
            "itm-000021",
            "belt",
            Some("P1"),
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "assign is not implemented yet");
    }
}
