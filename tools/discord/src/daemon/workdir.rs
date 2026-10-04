//! Channel -> working directory policy for the `on_message` hook. Settings
//! validation happens at daemon start; existence is re-checked on every
//! publish because a directory can disappear while the daemon runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::common::config::{Config, resolve_channel};
use crate::common::error::{AppError, ErrorKind};

/// Validated `workdirs` / `default_workdir` settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Workdirs {
    by_channel: HashMap<String, PathBuf>,
    default: Option<PathBuf>,
}

impl Workdirs {
    /// Aliases in `workdirs` keys resolve through `channels`. `home` is only
    /// needed for `~/` entries. Rejects relative paths, several keys resolving to one channel, and a setup with no
    /// directory at all, since no message could ever run.
    pub(super) fn from_config(config: &Config, home: Option<&Path>) -> Result<Self, AppError> {
        if config.workdirs.is_empty() && config.default_workdir.is_none() {
            return Err(AppError::new(
                ErrorKind::Config,
                "on_message needs a directory to run in: set workdirs and/or default_workdir",
            ));
        }
        // Sorted so validation (and the conflict message) never depends on
        // HashMap iteration order.
        let mut entries: Vec<_> = config.workdirs.iter().collect();
        entries.sort();
        let mut by_channel = HashMap::new();
        let mut key_of: HashMap<String, &str> = HashMap::new();
        for (key, raw) in entries {
            let channel = resolve_channel(key, &config.channels);
            if let Some(first) = key_of.insert(channel.clone(), key) {
                return Err(AppError::new(
                    ErrorKind::Config,
                    format!(
                        "workdirs[{first}] and workdirs[{key}] both resolve to channel {channel}: keep only one"
                    ),
                ));
            }
            by_channel.insert(channel, expand(raw, home, &format!("workdirs[{key}]"))?);
        }
        let default = config
            .default_workdir
            .as_deref()
            .map(|raw| expand(raw, home, "default_workdir"))
            .transpose()?;
        Ok(Self {
            by_channel,
            default,
        })
    }

    /// Picks the directory for a message and returns its canonical absolute
    /// path. A mapped directory that is missing is an error — it never falls
    /// back to `default_workdir`, which could run the hook in the wrong repo.
    pub(super) fn resolve(
        &self,
        channel_id: &str,
        parent_channel_id: Option<&str>,
    ) -> Result<String, AppError> {
        let key = parent_channel_id.unwrap_or(channel_id);
        let dir = self
            .by_channel
            .get(key)
            .or(self.default.as_ref())
            .ok_or_else(|| {
                AppError::new(
                    ErrorKind::Config,
                    format!("no workdir for channel {key} and no default_workdir, not running"),
                )
            })?;
        let canonical = std::fs::canonicalize(dir).map_err(|e| {
            AppError::new(
                ErrorKind::Config,
                format!(
                    "workdir {} for channel {key} is unusable: {e}",
                    dir.display()
                ),
            )
        })?;
        if !canonical.is_dir() {
            return Err(AppError::new(
                ErrorKind::Config,
                format!(
                    "workdir {} for channel {key} is not a directory",
                    dir.display()
                ),
            ));
        }
        canonical.into_os_string().into_string().map_err(|p| {
            AppError::new(
                ErrorKind::Config,
                format!("workdir {p:?} for channel {key} is not valid UTF-8"),
            )
        })
    }
}

fn expand(raw: &str, home: Option<&Path>, label: &str) -> Result<PathBuf, AppError> {
    if let Some(rest) = raw.strip_prefix("~/") {
        let home = home.ok_or_else(|| {
            AppError::new(
                ErrorKind::Config,
                format!("{label}: cannot expand ~/ because HOME is not set"),
            )
        })?;
        return Ok(home.join(rest));
    }
    if Path::new(raw).is_absolute() {
        return Ok(PathBuf::from(raw));
    }
    Err(AppError::new(
        ErrorKind::Config,
        format!("{label}: {raw:?} must be an absolute path or start with ~/"),
    ))
}

#[cfg(test)]
mod tests;
