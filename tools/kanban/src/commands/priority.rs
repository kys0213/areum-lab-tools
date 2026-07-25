use std::path::Path;

use crate::output::{AppError, ErrorKind, Payload};

/// Corrects an item's priority without touching its state. `priority` is
/// already validated against `P0`~`P3` at the CLI boundary.
pub(crate) fn run_priority(db_path: &Path, id: &str, priority: &str) -> Result<Payload, AppError> {
    let _ = (db_path, id, priority);
    Err(AppError::new(
        ErrorKind::Internal,
        "priority is not implemented yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_reports_itself_unimplemented_instead_of_confirming_a_change() {
        let err = run_priority(Path::new("/tmp/kanban.db"), "itm-000017", "P0").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "priority is not implemented yet");
    }
}
