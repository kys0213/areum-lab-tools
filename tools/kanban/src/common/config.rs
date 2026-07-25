//! Board location resolution. Shared by `main.rs` (which resolves the path
//! before dispatching) and the command bodies that open the store at it.

use std::path::PathBuf;

use crate::common::error::{AppError, ErrorKind};

/// Resolves the board database path: the `--db` override when given, else
/// `~/.areum/kanban/kanban.db` per the tool-crate config-location convention.
/// `home` is passed in so the resolution is testable without touching the
/// process environment.
pub(crate) fn resolve_db_path(flag: Option<&str>, home: Option<&str>) -> Result<PathBuf, AppError> {
    if let Some(path) = flag {
        return Ok(PathBuf::from(path));
    }
    home.map(|h| PathBuf::from(h).join(".areum/kanban/kanban.db"))
        .ok_or_else(|| {
            AppError::new(
                ErrorKind::Config,
                "HOME is not set; cannot locate ~/.areum/kanban/kanban.db (pass --db)",
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
        assert_eq!(path, PathBuf::from("/home/user/.areum/kanban/kanban.db"));
    }

    #[test]
    fn missing_home_without_db_flag_is_a_config_error() {
        // Fail fast rather than falling back to a relative path that would
        // silently create a stray database in the working directory.
        let err = resolve_db_path(None, None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Config);
        assert!(err.message.contains("--db"));
    }

    #[test]
    fn missing_home_is_tolerated_when_db_is_given() {
        let path = resolve_db_path(Some("/tmp/custom.db"), None).unwrap();
        assert_eq!(path, PathBuf::from("/tmp/custom.db"));
    }
}
