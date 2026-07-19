use serde::{Deserialize, Serialize};

/// Error taxonomy that drives both the JSON `error.kind` and the process
/// exit code (see [`exit_code`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Usage,
    Config,
    Auth,
    Api,
    RateLimit,
    Network,
    Internal,
}

impl ErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Usage => "usage",
            ErrorKind::Config => "config",
            ErrorKind::Auth => "auth",
            ErrorKind::Api => "api",
            ErrorKind::RateLimit => "rate_limit",
            ErrorKind::Network => "network",
            ErrorKind::Internal => "internal",
        }
    }
}

/// Structured failure carried through the whole app and rendered into the
/// error envelope. `http_status`/`retry_after_ms` are omitted from JSON when
/// absent.
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

/// Maps a failure to the process exit code documented in the CLI contract.
pub fn exit_code(err: &AppError) -> i32 {
    match err.kind {
        ErrorKind::Usage => 2,
        ErrorKind::Config | ErrorKind::Auth => 3,
        ErrorKind::Api => 4,
        ErrorKind::RateLimit => 5,
        ErrorKind::Network => 6,
        ErrorKind::Internal => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_maps_every_kind() {
        assert_eq!(exit_code(&AppError::new(ErrorKind::Usage, "")), 2);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Config, "")), 3);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Auth, "")), 3);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Api, "")), 4);
        assert_eq!(exit_code(&AppError::new(ErrorKind::RateLimit, "")), 5);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Network, "")), 6);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Internal, "")), 1);
    }
}
