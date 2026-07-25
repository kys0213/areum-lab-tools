use std::path::Path;

use crate::output::{AppError, ErrorKind, Payload};

/// Atomically claims the project's highest-priority backlog item
/// (`backlog → running`), recording `session`/`agent`/`claimed_at` in the same
/// statement that selects it — two agents calling at once must never receive
/// the same item.
///
/// An empty backlog is not an error: it answers `{"id": null}`.
pub(crate) fn run_next(
    db_path: &Path,
    project: &str,
    session: &str,
    agent: &str,
) -> Result<Payload, AppError> {
    let _ = (db_path, project, session, agent);
    Err(AppError::new(
        ErrorKind::Internal,
        "next is not implemented yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_reports_itself_unimplemented_instead_of_an_empty_backlog() {
        // `{"id": null}` is a normal answer agents poll on — returning it
        // from a stub would make the whole board look permanently empty.
        let err = run_next(Path::new("/tmp/kanban.db"), "belt", "sess-abc", "claude").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "next is not implemented yet");
    }
}
