use std::path::Path;

use crate::output::{AppError, ErrorKind, Payload};

/// Lists board items. Every filter is optional and they compose; `label`
/// matches a label key, not a value.
///
/// Unlike `next`, this only looks — it never claims.
pub(crate) fn run_list(
    db_path: &Path,
    project: Option<&str>,
    state: Option<&str>,
    label: Option<&str>,
) -> Result<Payload, AppError> {
    let _ = (db_path, project, state, label);
    Err(AppError::new(
        ErrorKind::Internal,
        "list is not implemented yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_reports_itself_unimplemented_instead_of_an_empty_board() {
        // An empty `items: []` would read as "the board is empty" — the one
        // wrong answer a listing stub could give.
        let err = run_list(Path::new("/tmp/kanban.db"), None, None, None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "list is not implemented yet");
    }
}
