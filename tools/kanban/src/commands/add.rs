use std::path::Path;

use crate::output::{AppError, ErrorKind, Payload};

/// Records an inbound issue in `inbox`. `body` of `"-"` means "read the body
/// from stdin", which is why the reader is injected rather than called
/// directly — the '-' rule stays testable without a real stdin.
///
/// A repeat of the same `(source, external_id)` pair is a `conflict`: the
/// UNIQUE constraint decides, so the same Discord message pushed twice never
/// becomes two items.
pub(crate) fn run_add(
    db_path: &Path,
    source: &str,
    external_id: &str,
    title: &str,
    body: &str,
    read_stdin: impl Fn() -> std::io::Result<String>,
) -> Result<Payload, AppError> {
    let _ = (db_path, source, external_id, title, body, read_stdin);
    Err(AppError::new(
        ErrorKind::Internal,
        "add is not implemented yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_reports_itself_unimplemented_instead_of_minting_an_id() {
        let err = run_add(
            Path::new("/tmp/kanban.db"),
            "discord",
            "msg-1",
            "cache drifts",
            "details",
            || Ok(String::new()),
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);
        assert_eq!(err.message, "add is not implemented yet");
    }
}
