use cli_contract::{CommonKind, ExitStatus, HumanText};
use serde::Serialize;

/// Error taxonomy that drives both the JSON `error.kind` and the process
/// exit code (see [`exit_code`]). Every doppel kind is a
/// [`CommonKind`], so names and exit codes match `discord` and `kanban`
/// (docs/doppel.md §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Usage,
    Config,
    /// The daemon could not be reached over its socket.
    Network,
    NotFound,
    Conflict,
    Internal,
}

impl ErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Usage => CommonKind::Usage.name(),
            ErrorKind::Config => CommonKind::Config.name(),
            ErrorKind::Network => CommonKind::Network.name(),
            ErrorKind::NotFound => CommonKind::NotFound.name(),
            ErrorKind::Conflict => CommonKind::Conflict.name(),
            ErrorKind::Internal => CommonKind::Internal.name(),
        }
    }
}

/// Structured failure carried through the whole app and rendered into the
/// error envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
}

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    // Called from `crate::output::render`, outside this module's subtree.
    pub(crate) fn to_human(&self) -> String {
        format!("error [{}]: {}", self.kind.as_str(), self.message)
    }
}

/// Maps a failure to the process exit code documented in the CLI contract
/// (docs/doppel.md §9).
pub fn exit_code(err: &AppError) -> i32 {
    match err.kind {
        ErrorKind::Internal => CommonKind::Internal.exit_code(),
        ErrorKind::Usage => CommonKind::Usage.exit_code(),
        ErrorKind::Config => CommonKind::Config.exit_code(),
        ErrorKind::Network => CommonKind::Network.exit_code(),
        ErrorKind::NotFound => CommonKind::NotFound.exit_code(),
        ErrorKind::Conflict => CommonKind::Conflict.exit_code(),
    }
}

impl HumanText for AppError {
    fn to_human(&self) -> String {
        AppError::to_human(self)
    }
}

impl ExitStatus for AppError {
    fn exit_code(&self) -> i32 {
        exit_code(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_maps_to_its_documented_exit_code() {
        let cases = [
            (ErrorKind::Internal, 1),
            (ErrorKind::Usage, 2),
            (ErrorKind::Config, 3),
            (ErrorKind::Network, 6),
            (ErrorKind::NotFound, 7),
            (ErrorKind::Conflict, 8),
        ];
        for (kind, code) in cases {
            assert_eq!(exit_code(&AppError::new(kind, "x")), code, "{kind:?}");
        }
    }

    #[test]
    fn human_form_names_the_kind() {
        let err = AppError::new(ErrorKind::NotFound, "no decision dec-000009");
        assert_eq!(err.to_human(), "error [not_found]: no decision dec-000009");
    }

    #[test]
    fn json_form_uses_snake_case_kind() {
        let err = AppError::new(ErrorKind::NotFound, "gone");
        assert_eq!(
            serde_json::to_string(&err).unwrap(),
            r#"{"kind":"not_found","message":"gone"}"#
        );
    }
}
