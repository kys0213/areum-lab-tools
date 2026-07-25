use serde::{Deserialize, Serialize};

/// Error taxonomy that drives both the JSON `error.kind` and the process
/// exit code (see [`exit_code`]). Kinds shared with the `discord` tool keep
/// its exit codes; `not_found`/`conflict` are kanban-specific additions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Usage,
    Config,
    Api,
    RateLimit,
    Network,
    NotFound,
    Conflict,
    Internal,
}

impl ErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Usage => "usage",
            ErrorKind::Config => "config",
            ErrorKind::Api => "api",
            ErrorKind::RateLimit => "rate_limit",
            ErrorKind::Network => "network",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Conflict => "conflict",
            ErrorKind::Internal => "internal",
        }
    }
}

/// Structured failure carried through the whole app and rendered into the
/// error envelope. `http_status`/`retry_after_ms` are omitted from JSON when
/// absent; they carry the LLM endpoint's response detail for the `api`,
/// `rate_limit`, and `network` kinds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            http_status: None,
            retry_after_ms: None,
        }
    }

    // Called from `crate::output::render`, outside this module's subtree.
    pub(crate) fn to_human(&self) -> String {
        let mut out = format!("error [{}]: {}", self.kind.as_str(), self.message);
        if let Some(status) = self.http_status {
            out.push_str(&format!(" (http {status})"));
        }
        if let Some(ms) = self.retry_after_ms {
            out.push_str(&format!(" (retry after {ms}ms)"));
        }
        out
    }
}

/// Maps a failure to the process exit code documented in the CLI contract
/// (docs/kanban-board.md §8).
pub fn exit_code(err: &AppError) -> i32 {
    match err.kind {
        ErrorKind::Internal => 1,
        ErrorKind::Usage => 2,
        ErrorKind::Config => 3,
        ErrorKind::Api => 4,
        ErrorKind::RateLimit => 5,
        ErrorKind::Network => 6,
        ErrorKind::NotFound => 7,
        ErrorKind::Conflict => 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_maps_every_kind() {
        assert_eq!(exit_code(&AppError::new(ErrorKind::Internal, "")), 1);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Usage, "")), 2);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Config, "")), 3);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Api, "")), 4);
        assert_eq!(exit_code(&AppError::new(ErrorKind::RateLimit, "")), 5);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Network, "")), 6);
        assert_eq!(exit_code(&AppError::new(ErrorKind::NotFound, "")), 7);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Conflict, "")), 8);
    }

    #[test]
    fn every_kind_serializes_as_its_documented_snake_case_name() {
        let pairs = [
            (ErrorKind::Internal, "internal"),
            (ErrorKind::Usage, "usage"),
            (ErrorKind::Config, "config"),
            (ErrorKind::Api, "api"),
            (ErrorKind::RateLimit, "rate_limit"),
            (ErrorKind::Network, "network"),
            (ErrorKind::NotFound, "not_found"),
            (ErrorKind::Conflict, "conflict"),
        ];
        for (kind, name) in pairs {
            assert_eq!(kind.as_str(), name);
            assert_eq!(
                serde_json::to_string(&kind).expect("ErrorKind serialization is infallible"),
                format!("\"{name}\"")
            );
        }
    }

    #[test]
    fn human_error_appends_present_optionals_only() {
        let plain = AppError::new(ErrorKind::NotFound, "no item itm-000017");
        assert_eq!(plain.to_human(), "error [not_found]: no item itm-000017");

        let detailed = AppError {
            kind: ErrorKind::RateLimit,
            message: "rate limited".into(),
            http_status: Some(429),
            retry_after_ms: Some(1200),
        };
        assert_eq!(
            detailed.to_human(),
            "error [rate_limit]: rate limited (http 429) (retry after 1200ms)"
        );
    }
}
