//! `move` is a Rust keyword, so the escape-hatch transition lives in
//! `move_item.rs` while the subcommand keeps its documented name.

use std::path::Path;

use crate::output::{AppError, ErrorKind, Payload};

/// Forces an item into `state` — the escape hatch for transitions the regular
/// commands refuse. The schema's CHECK constraints still apply, so a move that
/// would leave an inconsistent row (a `running` item with no owner, say) is
/// rejected as a `conflict`.
pub(crate) fn run_move(db_path: &Path, id: &str, state: &str) -> Result<Payload, AppError> {
    let _ = (db_path, id, state);
    Err(AppError::new(
        ErrorKind::Internal,
        "move is not implemented yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_reports_itself_unimplemented_instead_of_confirming_a_transition() {
        let err = run_move(Path::new("/tmp/kanban.db"), "itm-000017", "done").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "move is not implemented yet");
    }
}
