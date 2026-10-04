//! Runs the configured `on_message` command for a detected message: spawns the
//! argv without a shell, writes one JSON line to its stdin, and never waits on
//! the daemon's event loop. A detached reaper thread writes stdin, closes it,
//! and `wait()`s the child so finished hooks do not linger as zombies.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::common::error::{AppError, ErrorKind};

/// Seam for the external process launch so the message trigger can be tested
/// with a fake. `Err` means the command could not be started at all; anything
/// that goes wrong after a successful start (stdin write, non-zero exit) is
/// reported asynchronously on stderr by the implementation.
pub(super) trait HookRunner {
    fn run(&self, argv: &[String], stdin_line: String) -> Result<(), AppError>;
}

/// Real [`HookRunner`] backed by `std::process`. stdout is discarded so a
/// chatty hook cannot interleave with the daemon's own stdout output; stderr
/// is inherited so hook diagnostics land in the daemon log.
pub(super) struct ProcessHookRunner;

impl HookRunner for ProcessHookRunner {
    fn run(&self, argv: &[String], stdin_line: String) -> Result<(), AppError> {
        let (program, args) = argv.split_first().ok_or_else(|| {
            AppError::new(ErrorKind::Config, "on_message must not be an empty array")
        })?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                AppError::new(
                    ErrorKind::Internal,
                    format!("failed to spawn on_message command {program:?}: {e}"),
                )
            })?;
        let mut stdin = child
            .stdin
            .take()
            .expect("stdin was configured as Stdio::piped() above");
        let label = program.clone();
        // A dedicated thread (not the event loop) owns the blocking stdin write
        // and `wait()`: a hook that never reads stdin must not stall the daemon.
        std::thread::Builder::new()
            .name("on-message-reaper".to_owned())
            .spawn(move || {
                if let Err(e) = stdin.write_all(stdin_line.as_bytes()) {
                    eprintln!("daemon: on_message {label:?}: failed to write stdin: {e}");
                }
                drop(stdin);
                match child.wait() {
                    Ok(status) if !status.success() => {
                        eprintln!("daemon: on_message {label:?} exited with {status}");
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("daemon: on_message {label:?}: wait failed: {e}"),
                }
            })
            .map(|_| ())
            .map_err(|e| {
                AppError::new(
                    ErrorKind::Internal,
                    format!("failed to start on_message reaper thread: {e}"),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "areum-discord-hook-runner-{}-{label}",
            std::process::id()
        ))
    }

    fn wait_for_file(path: &std::path::Path) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok(contents) = std::fs::read_to_string(path)
                && contents.ends_with('\n')
            {
                return contents;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "hook output never appeared at {}",
                path.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn run_delivers_stdin_line_to_the_command_without_a_shell_wrapper() {
        let out = unique_path("stdin");
        let _ = std::fs::remove_file(&out);
        let script = format!("cat > {}", out.display());

        ProcessHookRunner
            .run(&argv(&["sh", "-c", &script]), "{\"a\":1}\n".to_owned())
            .expect("a valid command must start");

        assert_eq!(wait_for_file(&out), "{\"a\":1}\n");
        std::fs::remove_file(&out).ok();
    }

    #[test]
    fn run_returns_err_when_the_program_cannot_be_spawned() {
        let err = ProcessHookRunner
            .run(
                &argv(&["/nonexistent/areum-discord-hook"]),
                "{}\n".to_owned(),
            )
            .expect_err("a missing program must be reported, not panic");
        assert_eq!(err.kind, ErrorKind::Internal);
        assert!(err.message.contains("failed to spawn"));
    }

    #[test]
    fn run_returns_ok_without_waiting_when_the_command_later_exits_non_zero() {
        // The non-zero exit is only logged by the reaper thread; the caller
        // (the event loop) sees a successful start and keeps going.
        ProcessHookRunner
            .run(&argv(&["sh", "-c", "exit 3"]), "{}\n".to_owned())
            .expect("a started command is Ok even if it fails later");
    }

    #[test]
    fn run_does_not_block_on_a_long_running_command() {
        let started = std::time::Instant::now();
        ProcessHookRunner
            .run(&argv(&["sleep", "2"]), "{}\n".to_owned())
            .unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn run_rejects_an_empty_argv() {
        let err = ProcessHookRunner.run(&[], "{}\n".to_owned()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Config);
    }
}
