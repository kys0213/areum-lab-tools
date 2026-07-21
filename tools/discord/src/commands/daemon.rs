//! `daemon start|stop|status` — lifecycle management for the resident gateway
//! process. The CLI and daemon never talk directly; this module only manages
//! the OS process (spawn/detach, SIGTERM) and the pidfile that records it.
//!
//! Unix-only: liveness and shutdown use `kill(2)` (`libc`). The heavy I/O
//! (spawn, signal, gateway loop) is exercised end-to-end rather than
//! unit-tested; the pure seams (pid parsing, pidfile round-trip, liveness of a
//! known pid) are tested inline.

use std::path::Path;
use std::process::Stdio;

use std::os::unix::process::CommandExt;

use crate::common::http::HttpDiscordApi;
use crate::common::store::AskStore;
use crate::output::{
    AppError, DaemonStartData, DaemonStatusData, DaemonStopData, ErrorKind, Payload,
};

/// How long `stop` waits for the daemon to exit after SIGTERM before reporting
/// failure, polled at [`STOP_POLL_INTERVAL`].
const STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const STOP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Starts the daemon. With `foreground`, runs the gateway loop in this process
/// until SIGTERM; otherwise detaches a child running `--foreground` and records
/// its pid. `token` is already resolved (fail-fast happened in `main`) and is
/// handed to the child via the environment rather than the command line.
pub(crate) async fn run_daemon_start(
    db_path: &Path,
    pid_path: &Path,
    config_flag: Option<&str>,
    token: String,
    foreground: bool,
) -> Result<Payload, AppError> {
    if foreground {
        run_foreground(db_path, token).await
    } else {
        start_background(pid_path, config_flag, &token)
    }
}

async fn run_foreground(db_path: &Path, token: String) -> Result<Payload, AppError> {
    let api = HttpDiscordApi::new(token.clone());
    let store = AskStore::open(db_path)?;
    // Blocks until SIGTERM (graceful) or a fatal gateway close (Err).
    crate::daemon::run(&api, &store, token).await?;
    Ok(Payload::DaemonStart(DaemonStartData {
        pid: std::process::id(),
        foreground: true,
    }))
}

fn start_background(
    pid_path: &Path,
    config_flag: Option<&str>,
    token: &str,
) -> Result<Payload, AppError> {
    if let Some(pid) = read_pidfile(pid_path)?
        && pid_is_alive(pid)
    {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!("daemon is already running (pid {pid})"),
        ));
    }
    let child_pid = spawn_detached(config_flag, token)?;
    write_pidfile(pid_path, child_pid as i32)?;
    Ok(Payload::DaemonStart(DaemonStartData {
        pid: child_pid,
        foreground: false,
    }))
}

/// Re-execs this binary as `daemon start --foreground`, detached: null stdio
/// and its own process group so it outlives the launching shell and its job
/// control. `--config` is forwarded; the token rides in the environment to keep
/// it out of the process table / shell history.
fn spawn_detached(config_flag: Option<&str>, token: &str) -> Result<u32, AppError> {
    let exe = std::env::current_exe().map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("cannot locate current executable to spawn daemon: {e}"),
        )
    })?;
    let mut cmd = std::process::Command::new(exe);
    if let Some(cfg) = config_flag {
        cmd.arg("--config").arg(cfg);
    }
    cmd.arg("daemon").arg("start").arg("--foreground");
    cmd.env("DISCORD_BOT_TOKEN", token);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // pgid 0 => the child leads a fresh process group, detaching it from the
    // launcher's terminal job control.
    cmd.process_group(0);
    let child = cmd.spawn().map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("failed to spawn background daemon: {e}"),
        )
    })?;
    Ok(child.id())
}

/// Stops the running daemon: SIGTERM, wait for exit, then remove the pidfile.
/// Errors if no live daemon is recorded, or if it does not exit within
/// [`STOP_TIMEOUT`] (in which case the pidfile is left for diagnosis).
pub(crate) async fn run_daemon_stop(pid_path: &Path) -> Result<Payload, AppError> {
    let pid = read_pidfile(pid_path)?
        .filter(|&p| pid_is_alive(p))
        .ok_or_else(|| AppError::new(ErrorKind::Usage, "daemon is not running"))?;

    send_sigterm(pid)?;
    wait_for_exit(pid).await?;
    remove_pidfile(pid_path)?;
    Ok(Payload::DaemonStop(DaemonStopData {
        pid: pid as u32,
        stopped: true,
    }))
}

/// Reports whether the daemon is running (pidfile records a live pid) and how
/// many asks are pending.
pub(crate) fn run_daemon_status(pid_path: &Path, db_path: &Path) -> Result<Payload, AppError> {
    let pid = read_pidfile(pid_path)?.filter(|&p| pid_is_alive(p));
    let pending = AskStore::open(db_path)?.count_pending()?;
    Ok(Payload::DaemonStatus(DaemonStatusData {
        running: pid.is_some(),
        pid: pid.map(|p| p as u32),
        pending,
    }))
}

async fn wait_for_exit(pid: i32) -> Result<(), AppError> {
    let deadline = std::time::Instant::now() + STOP_TIMEOUT;
    while std::time::Instant::now() < deadline {
        if !pid_is_alive(pid) {
            return Ok(());
        }
        tokio::time::sleep(STOP_POLL_INTERVAL).await;
    }
    Err(AppError::new(
        ErrorKind::Internal,
        format!("daemon (pid {pid}) did not exit within {STOP_TIMEOUT:?} of SIGTERM"),
    ))
}

/// Probes process existence with `kill(pid, 0)`: `0` = alive, `EPERM` = alive
/// but owned by another user (still running), `ESRCH` = gone.
fn pid_is_alive(pid: i32) -> bool {
    let ret = unsafe { libc::kill(pid, 0) };
    if ret == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn send_sigterm(pid: i32) -> Result<(), AppError> {
    let ret = unsafe { libc::kill(pid, libc::SIGTERM) };
    if ret == 0 {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorKind::Internal,
            format!(
                "failed to signal daemon pid {pid}: {}",
                std::io::Error::last_os_error()
            ),
        ))
    }
}

fn read_pidfile(path: &Path) -> Result<Option<i32>, AppError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(parse_pid(&contents)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AppError::new(
            ErrorKind::Internal,
            format!("failed to read pidfile {}: {e}", path.display()),
        )),
    }
}

/// A pidfile with unreadable/garbage contents is treated as stale (no daemon),
/// so a fresh `start` can proceed and overwrite it.
fn parse_pid(contents: &str) -> Option<i32> {
    contents.trim().parse::<i32>().ok().filter(|&p| p > 0)
}

fn write_pidfile(path: &Path, pid: i32) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AppError::new(
                ErrorKind::Internal,
                format!(
                    "failed to create pidfile directory {}: {e}",
                    parent.display()
                ),
            )
        })?;
    }
    std::fs::write(path, pid.to_string()).map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("failed to write pidfile {}: {e}", path.display()),
        )
    })
}

fn remove_pidfile(path: &Path) -> Result<(), AppError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::new(
            ErrorKind::Internal,
            format!("failed to remove pidfile {}: {e}", path.display()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pid_accepts_positive_trimmed_number() {
        assert_eq!(parse_pid("1234\n"), Some(1234));
        assert_eq!(parse_pid("  42  "), Some(42));
    }

    #[test]
    fn parse_pid_rejects_garbage_and_nonpositive() {
        assert_eq!(parse_pid(""), None);
        assert_eq!(parse_pid("not-a-pid"), None);
        assert_eq!(parse_pid("0"), None);
        assert_eq!(parse_pid("-5"), None);
    }

    #[test]
    fn pid_is_alive_true_for_self() {
        assert!(pid_is_alive(std::process::id() as i32));
    }

    #[test]
    fn pid_is_alive_false_for_unused_pid() {
        // A very high pid is exceedingly unlikely to be assigned.
        assert!(!pid_is_alive(1_000_000_000));
    }

    #[test]
    fn pidfile_roundtrips_and_missing_reads_none() {
        let dir = std::env::temp_dir().join(format!(
            "areum-discord-daemon-pidtest-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("daemon.pid");

        assert_eq!(read_pidfile(&path).unwrap(), None);
        write_pidfile(&path, 4321).unwrap();
        assert_eq!(read_pidfile(&path).unwrap(), Some(4321));
        remove_pidfile(&path).unwrap();
        assert_eq!(read_pidfile(&path).unwrap(), None);

        std::fs::remove_dir_all(&dir).ok();
    }
}
