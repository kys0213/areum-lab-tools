//! Location resolution for the database and the transcript root. `home` is
//! passed in so resolution is testable without touching the process
//! environment.

use std::path::PathBuf;

use crate::common::error::{AppError, ErrorKind};

/// The `--db` override when given, else `~/.areum/doppel/doppel.db` per the
/// tool-crate config-location convention.
pub(crate) fn resolve_db_path(flag: Option<&str>, home: Option<&str>) -> Result<PathBuf, AppError> {
    if let Some(path) = flag {
        return Ok(PathBuf::from(path));
    }
    home.map(|h| PathBuf::from(h).join(".areum/doppel/doppel.db"))
        .ok_or_else(|| {
            AppError::new(
                ErrorKind::Config,
                "HOME is not set; cannot locate ~/.areum/doppel/doppel.db (pass --db)",
            )
        })
}

/// The `--root` override when given, else Claude Code's own transcript
/// directory `~/.claude/projects`.
pub(crate) fn resolve_transcript_root(
    flag: Option<&str>,
    home: Option<&str>,
) -> Result<PathBuf, AppError> {
    if let Some(path) = flag {
        return Ok(PathBuf::from(path));
    }
    home.map(|h| PathBuf::from(h).join(".claude/projects"))
        .ok_or_else(|| {
            AppError::new(
                ErrorKind::Config,
                "HOME is not set; cannot locate ~/.claude/projects (pass --root)",
            )
        })
}

/// The `--socket` flag, else `$DOPPEL_SOCKET` (what the plugin reads too),
/// else `~/.areum/doppel/doppel.sock`.
pub(crate) fn resolve_socket_path(
    flag: Option<&str>,
    env: Option<&str>,
    home: Option<&str>,
) -> Result<PathBuf, AppError> {
    if let Some(path) = flag.or(env).filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    home.map(|h| PathBuf::from(h).join(".areum/doppel/doppel.sock"))
        .ok_or_else(|| {
            AppError::new(
                ErrorKind::Config,
                "HOME is not set; cannot locate ~/.areum/doppel/doppel.sock (pass --socket)",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_flag_wins_over_the_home_default() {
        let path = resolve_db_path(Some("/tmp/custom.db"), Some("/home/user")).unwrap();
        assert_eq!(path, PathBuf::from("/tmp/custom.db"));
    }

    #[test]
    fn default_db_path_lives_under_the_areum_tool_directory() {
        let path = resolve_db_path(None, Some("/home/user")).unwrap();
        assert_eq!(path, PathBuf::from("/home/user/.areum/doppel/doppel.db"));
    }

    #[test]
    fn missing_home_without_db_flag_is_a_config_error() {
        // Fail fast rather than silently creating a database relative to cwd.
        let err = resolve_db_path(None, None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Config);
        assert!(err.message.contains("--db"));
    }

    #[test]
    fn default_transcript_root_is_claude_projects() {
        let path = resolve_transcript_root(None, Some("/home/user")).unwrap();
        assert_eq!(path, PathBuf::from("/home/user/.claude/projects"));
    }

    #[test]
    fn missing_home_without_root_flag_is_a_config_error() {
        let err = resolve_transcript_root(None, None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Config);
        assert!(err.message.contains("--root"));
    }

    #[test]
    fn socket_flag_beats_env_beats_home() {
        assert_eq!(
            resolve_socket_path(Some("/a.sock"), Some("/b.sock"), Some("/home/u")).unwrap(),
            PathBuf::from("/a.sock")
        );
        assert_eq!(
            resolve_socket_path(None, Some("/b.sock"), Some("/home/u")).unwrap(),
            PathBuf::from("/b.sock")
        );
        assert_eq!(
            resolve_socket_path(None, None, Some("/home/u")).unwrap(),
            PathBuf::from("/home/u/.areum/doppel/doppel.sock")
        );
        assert_eq!(
            resolve_socket_path(None, None, None).unwrap_err().kind,
            ErrorKind::Config
        );
    }
}
