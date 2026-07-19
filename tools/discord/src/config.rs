use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::output::{AppError, ErrorKind};

/// On-disk config at `~/.areum/discord/config.json`. Both fields optional.
/// `skip_serializing_if` keeps a fresh `init` write minimal (`{"token":"..."}`)
/// rather than always emitting an empty `channels` map.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub channels: HashMap<String, String>,
}

/// Resolves the bot token by precedence: flag > env > config. Fails fast when
/// none is present rather than proceeding token-less.
pub fn resolve_token(
    flag: Option<&str>,
    env: Option<&str>,
    config_token: Option<&str>,
) -> Result<String, AppError> {
    flag.or(env)
        .or(config_token)
        .map(str::to_owned)
        .ok_or_else(|| {
            AppError::new(
                ErrorKind::Config,
                "no bot token: set --token, export DISCORD_BOT_TOKEN, or create \
                 ~/.areum/discord/config.json (chmod 600) with {\"token\":\"...\"}",
            )
        })
}

/// Substitutes a config alias with its channel id; passes an unknown value
/// through unchanged (treated as a raw channel id).
pub fn resolve_channel(input: &str, channels: &HashMap<String, String>) -> String {
    channels
        .get(input)
        .cloned()
        .unwrap_or_else(|| input.to_owned())
}

/// Parses config JSON, failing fast on malformed input.
pub fn parse_config(json: &str) -> Result<Config, AppError> {
    serde_json::from_str(json)
        .map_err(|e| AppError::new(ErrorKind::Config, format!("failed to parse config: {e}")))
}

fn home_dir() -> Result<PathBuf, AppError> {
    std::env::var("HOME").map(PathBuf::from).map_err(|_| {
        AppError::new(
            ErrorKind::Config,
            "HOME is not set; cannot locate ~/.areum/discord/config.json",
        )
    })
}

pub fn default_config_path() -> Result<PathBuf, AppError> {
    Ok(home_dir()?.join(".areum/discord/config.json"))
}

/// Loads the config file if it exists. A missing file is not an error (the
/// token may come from flag/env). This tool never creates the file.
pub fn load_config(path: &Path) -> Result<Option<Config>, AppError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            warn_if_insecure_permissions(path);
            Ok(Some(parse_config(&contents)?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AppError::new(
            ErrorKind::Config,
            format!("failed to read config {}: {e}", path.display()),
        )),
    }
}

/// Writes `config` to `path` as compact JSON, creating the parent directory
/// (mode 700) if it doesn't exist and setting the file to mode 600. `init` is
/// the only caller — this tool never creates config implicitly otherwise.
pub fn write_config_file(path: &Path, config: &Config) -> Result<(), AppError> {
    let contents = serde_json::to_string(config).map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("failed to serialize config: {e}"),
        )
    })?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AppError::new(
                ErrorKind::Internal,
                format!(
                    "failed to create config directory {}: {e}",
                    parent.display()
                ),
            )
        })?;
        set_dir_permissions_700(parent)?;
    }

    std::fs::write(path, &contents).map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("failed to write config {}: {e}", path.display()),
        )
    })?;
    set_file_permissions_600(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_dir_permissions_700(path: &Path) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("failed to set permissions on {}: {e}", path.display()),
        )
    })
}

#[cfg(not(unix))]
fn set_dir_permissions_700(_path: &Path) -> Result<(), AppError> {
    Ok(())
}

#[cfg(unix)]
fn set_file_permissions_600(path: &Path) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("failed to set permissions on {}: {e}", path.display()),
        )
    })
}

#[cfg(not(unix))]
fn set_file_permissions_600(_path: &Path) -> Result<(), AppError> {
    Ok(())
}

#[cfg(unix)]
fn warn_if_insecure_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o600 {
            eprintln!(
                "warning: config {} is mode {mode:o}, expected 600 (run: chmod 600 {})",
                path.display(),
                path.display()
            );
        }
    }
}

#[cfg(not(unix))]
fn warn_if_insecure_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn channels() -> HashMap<String, String> {
        HashMap::from([("general".to_owned(), "111".to_owned())])
    }

    #[test]
    fn token_prefers_flag_over_env_and_config() {
        let token = resolve_token(Some("flag"), Some("env"), Some("cfg")).unwrap();
        assert_eq!(token, "flag");
    }

    #[test]
    fn token_prefers_env_over_config() {
        let token = resolve_token(None, Some("env"), Some("cfg")).unwrap();
        assert_eq!(token, "env");
    }

    #[test]
    fn token_falls_back_to_config() {
        let token = resolve_token(None, None, Some("cfg")).unwrap();
        assert_eq!(token, "cfg");
    }

    #[test]
    fn token_absent_is_config_error() {
        let err = resolve_token(None, None, None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Config);
    }

    #[test]
    fn channel_alias_is_substituted() {
        assert_eq!(resolve_channel("general", &channels()), "111");
    }

    #[test]
    fn channel_unknown_passes_through_as_raw_id() {
        assert_eq!(resolve_channel("999", &channels()), "999");
    }

    #[test]
    fn parse_config_reads_both_fields() {
        let cfg = parse_config(r#"{"token":"t","channels":{"a":"1"}}"#).unwrap();
        assert_eq!(cfg.token.as_deref(), Some("t"));
        assert_eq!(cfg.channels.get("a").map(String::as_str), Some("1"));
    }

    #[test]
    fn parse_config_allows_empty_object() {
        let cfg = parse_config("{}").unwrap();
        assert!(cfg.token.is_none());
        assert!(cfg.channels.is_empty());
    }

    #[test]
    fn parse_config_rejects_malformed_json() {
        let err = parse_config("{not json").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Config);
    }

    #[test]
    fn load_config_missing_file_returns_none_not_error() {
        // Unique path under the OS temp dir that is guaranteed not to exist;
        // a missing config file is a valid state (token may come from flag/env).
        let path = std::env::temp_dir().join(format!(
            "areum-discord-missing-config-{}.json",
            std::process::id()
        ));
        assert!(!path.exists());

        let result = load_config(&path).unwrap();
        assert!(result.is_none());
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "areum-discord-config-test-{}-{label}",
            std::process::id()
        ))
    }

    #[test]
    fn serialize_config_omits_empty_channels() {
        let cfg = Config {
            token: Some("t".to_owned()),
            channels: HashMap::new(),
        };
        assert_eq!(serde_json::to_string(&cfg).unwrap(), r#"{"token":"t"}"#);
    }

    #[test]
    fn serialize_config_includes_non_empty_channels() {
        let cfg = Config {
            token: Some("t".to_owned()),
            channels: channels(),
        };
        assert_eq!(
            serde_json::to_string(&cfg).unwrap(),
            r#"{"token":"t","channels":{"general":"111"}}"#
        );
    }

    #[test]
    fn write_config_file_creates_parent_dir_and_writes_minimal_json() {
        let dir = unique_temp_dir("write-fresh");
        let path = dir.join("config.json");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!dir.exists());

        let cfg = Config {
            token: Some("mytoken".to_owned()),
            channels: HashMap::new(),
        };
        write_config_file(&path, &cfg).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, r#"{"token":"mytoken"}"#);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn write_config_file_sets_dir_700_and_file_600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = unique_temp_dir("write-perms");
        let path = dir.join("config.json");
        let _ = std::fs::remove_dir_all(&dir);

        let cfg = Config {
            token: Some("mytoken".to_owned()),
            channels: HashMap::new(),
        };
        write_config_file(&path, &cfg).unwrap();

        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600);

        std::fs::remove_dir_all(&dir).ok();
    }
}
