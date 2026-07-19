use std::path::Path;

use crate::common::config::Config;
use crate::output::{AppError, ErrorKind, InitData, Payload};

use super::send::read_stdin_to_string;

/// Creates or overwrites the local config file with a bot token. Network-free
/// (init is local setup only). I/O is injected (existence check, existing-file
/// load, and the write itself) so the whole flow is blackbox-testable without
/// touching the real filesystem — same seam pattern as `read_stdin`/`read_file`
/// in [`run_send`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_init(
    path: &Path,
    token_flag: Option<&str>,
    force: bool,
    read_stdin: impl FnOnce() -> std::io::Result<String>,
    exists: impl FnOnce() -> bool,
    load_existing: impl FnOnce() -> Result<Option<Config>, AppError>,
    write: impl FnOnce(&Config) -> Result<(), AppError>,
) -> Result<Payload, AppError> {
    let token = resolve_init_token(token_flag, read_stdin)?;

    let already_exists = exists();
    if already_exists && !force {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!(
                "config already exists at {} (use --force to overwrite)",
                path.display()
            ),
        ));
    }

    // Preserve `channels` from a --force overwrite rather than discarding it;
    // a malformed existing file fails fast here instead of being silently
    // dropped.
    let channels = if already_exists {
        load_existing()?.map(|c| c.channels).unwrap_or_default()
    } else {
        Default::default()
    };

    let new_config = Config {
        token: Some(token),
        channels,
    };
    write(&new_config)?;

    Ok(Payload::Init(InitData {
        path: path.display().to_string(),
        created: !already_exists,
    }))
}

/// Resolves and trims the init token: `--token` if given, otherwise the full
/// stdin. Empty after trim is a caller mistake (usage error) — init never
/// silently proceeds with a blank token.
fn resolve_init_token(
    flag: Option<&str>,
    read_stdin: impl FnOnce() -> std::io::Result<String>,
) -> Result<String, AppError> {
    let raw = match flag {
        Some(t) => t.to_owned(),
        None => read_stdin_to_string(read_stdin)?,
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::new(ErrorKind::Usage, "token must not be empty"));
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests;
