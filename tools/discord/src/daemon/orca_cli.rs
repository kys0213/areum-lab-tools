//! Runs the local `orca` CLI for the Orca forward and waits for its stdout.
//! Unlike `on_message` (fire-and-forget through
//! [`super::hook_runner::HookRunner`]), the forward needs the CLI's answer:
//! a new chat's id is only known from the send result.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::common::error::{AppError, ErrorKind};

/// The CLI does not depend on its working directory, so it runs in a fixed
/// one and never waits on `workdirs` resolution.
const ORCA_DIR: &str = "/";

/// Upper bound for one CLI call. The call runs on the gateway event loop, so
/// a hung Orca app must not stall every other event for long. A normal send
/// takes about 0.1s.
const ORCA_TIMEOUT: Duration = Duration::from_secs(10);

/// Seam for the `orca` CLI so the forward logic is testable without Orca.
pub(super) trait OrcaCli {
    /// Runs `argv` with `body` on stdin. `Ok` carries stdout of a zero exit;
    /// a non-zero exit, spawn failure or timeout is `Err`.
    async fn run(&self, argv: &[String], body: &str) -> Result<String, AppError>;
}

/// Real [`OrcaCli`] backed by `tokio::process`.
pub(super) struct ProcessOrcaCli;

impl OrcaCli for ProcessOrcaCli {
    async fn run(&self, argv: &[String], body: &str) -> Result<String, AppError> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| AppError::new(ErrorKind::Config, "orca argv must not be empty"))?;
        // Same rule as `on_message`: the bot token stays inside the daemon.
        let mut child = Command::new(program)
            .args(args)
            .env_remove("DISCORD_BOT_TOKEN")
            .current_dir(ORCA_DIR)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                AppError::new(
                    ErrorKind::Internal,
                    format!("failed to spawn {program:?}: {e}"),
                )
            })?;
        let mut stdin = child
            .stdin
            .take()
            .expect("stdin was configured as Stdio::piped() above");
        let finished = async {
            // A CLI that exits without reading stdin closes the pipe; its exit
            // status below is the meaningful result, not the write error.
            let _ = stdin.write_all(body.as_bytes()).await;
            drop(stdin);
            child.wait_with_output().await
        };
        let output = tokio::time::timeout(ORCA_TIMEOUT, finished)
            .await
            .map_err(|_| {
                AppError::new(
                    ErrorKind::Network,
                    format!("{program:?} timed out after {ORCA_TIMEOUT:?}"),
                )
            })?
            .map_err(|e| {
                AppError::new(
                    ErrorKind::Internal,
                    format!("failed to wait for {program:?}: {e}"),
                )
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            return Ok(stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(AppError::new(
            ErrorKind::Internal,
            format!(
                "{program:?} exited with {}: {}",
                output.status,
                one_line(if stderr.trim().is_empty() {
                    &stdout
                } else {
                    &stderr
                })
            ),
        ))
    }
}

/// Collapses CLI output into one log line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_owned()).collect()
    }

    #[tokio::test]
    async fn run_returns_stdout_and_feeds_the_body_on_stdin() {
        let out = ProcessOrcaCli
            .run(&argv(&["sh", "-c", "echo got:; cat"]), "hello body")
            .await
            .unwrap();

        assert_eq!(out, "got:\nhello body");
    }

    #[tokio::test]
    async fn run_reports_a_non_zero_exit_with_its_stderr() {
        let err = ProcessOrcaCli
            .run(&argv(&["sh", "-c", "echo 'no chat x' >&2; exit 1"]), "body")
            .await
            .unwrap_err();

        assert!(err.message.contains("no chat x"), "{}", err.message);
    }

    #[tokio::test]
    async fn run_reports_a_missing_program() {
        let err = ProcessOrcaCli
            .run(&argv(&["/nonexistent/orca"]), "body")
            .await
            .unwrap_err();

        assert!(err.message.contains("failed to spawn"), "{}", err.message);
    }

    #[tokio::test]
    async fn run_does_not_pass_the_bot_token_environment() {
        // SAFETY: see hook_runner's token test; tests touching this variable
        // only read it through child processes.
        unsafe { std::env::set_var("DISCORD_BOT_TOKEN", "leaked-token") };
        let out = ProcessOrcaCli
            .run(
                &argv(&["sh", "-c", "printf %s \"${DISCORD_BOT_TOKEN:-absent}\""]),
                "",
            )
            .await;
        unsafe { std::env::remove_var("DISCORD_BOT_TOKEN") };

        assert_eq!(out.unwrap(), "absent");
    }
}
