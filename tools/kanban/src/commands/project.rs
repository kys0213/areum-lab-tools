use std::path::Path;

use crate::output::{AppError, ErrorKind, Payload};

/// Registers a project. `description` is the classification rationale, so an
/// empty one is a usage error rather than a project the classifier cannot
/// reason about.
pub(crate) fn run_project_add(
    db_path: &Path,
    name: &str,
    description: &str,
) -> Result<Payload, AppError> {
    let _ = (db_path, name, description);
    Err(AppError::new(
        ErrorKind::Internal,
        "project add is not implemented yet",
    ))
}

/// Lists registered projects with their descriptions.
pub(crate) fn run_project_list(db_path: &Path) -> Result<Payload, AppError> {
    let _ = db_path;
    Err(AppError::new(
        ErrorKind::Internal,
        "project list is not implemented yet",
    ))
}

/// Removes a project. A project still referenced by items is rejected with
/// `conflict` — the foreign key decides, not a pre-check.
pub(crate) fn run_project_rm(db_path: &Path, name: &str) -> Result<Payload, AppError> {
    let _ = (db_path, name);
    Err(AppError::new(
        ErrorKind::Internal,
        "project rm is not implemented yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_project_action_reports_itself_unimplemented() {
        let path = Path::new("/tmp/kanban.db");
        for (err, expected) in [
            (
                run_project_add(path, "belt", "conveyor").unwrap_err(),
                "project add is not implemented yet",
            ),
            (
                run_project_list(path).unwrap_err(),
                "project list is not implemented yet",
            ),
            (
                run_project_rm(path, "belt").unwrap_err(),
                "project rm is not implemented yet",
            ),
        ] {
            assert_eq!(err.kind, ErrorKind::Internal);
            assert_eq!(err.message, expected);
        }
    }
}
