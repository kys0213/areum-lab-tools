//! `daemon start|stop|status` — lifecycle management for the resident gateway
//! process. The CLI and daemon never talk directly; this module only manages
//! the OS process (spawn/detach, SIGTERM) and the pidfile that records it.
//!
//! Pidfile ownership: only the foreground runtime ([`run_foreground`]) ever
//! writes the pidfile, via the atomic [`acquire_pidfile`] guard — whether it
//! is running because the user passed `--foreground` directly, or because
//! [`start_background`] spawned it as a detached child. The background
//! spawner itself never writes the pidfile; its pre-spawn liveness check is
//! a fast, best-effort UX rejection only. This makes `acquire_pidfile` the
//! single defense against two daemons running at once, closing the
//! read-check-then-write race a spawner-side write would have.
//!
//! Unix-only: liveness and shutdown use `kill(2)` (`libc`). The heavy I/O
//! (spawn, signal, gateway loop) is exercised end-to-end rather than
//! unit-tested; the pure seams (pid parsing, pidfile round-trip, liveness of a
//! known pid, and the `acquire_pidfile`/`release_own_pidfile` guard) are
//! tested inline.

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

/// How long [`start_background`] waits for its spawned child to reach
/// [`run_foreground`]'s `acquire_pidfile` before reporting a startup failure.
const START_CONFIRM_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);
const START_CONFIRM_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

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
        run_foreground(db_path, pid_path, token).await
    } else {
        start_background(pid_path, config_flag, &token).await
    }
}

/// The daemon runtime's single entry point regardless of how it was launched
/// (`--foreground` directly, or as [`start_background`]'s detached child).
/// Atomically claims the pidfile before touching the store or gateway, so a
/// second instance is rejected here rather than racing a spawner-side check.
async fn run_foreground(
    db_path: &Path,
    pid_path: &Path,
    token: String,
) -> Result<Payload, AppError> {
    acquire_pidfile(pid_path)?;
    let api = HttpDiscordApi::new(token.clone());
    let store = match AskStore::open(db_path) {
        Ok(store) => store,
        Err(err) => {
            release_own_pidfile(pid_path);
            return Err(err);
        }
    };
    // Blocks until SIGTERM (graceful) or a fatal gateway close (Err). Either
    // way this process is the pidfile's sole owner until now, so release it
    // on both outcomes before propagating.
    let run_result = crate::daemon::run(&api, &store, token).await;
    release_own_pidfile(pid_path);
    run_result?;
    Ok(Payload::DaemonStart(DaemonStartData {
        pid: std::process::id(),
        foreground: true,
    }))
}

/// Spawns the detached child and waits for it to confirm it actually reached
/// a healthy state before reporting success; it never writes the pidfile
/// itself (see module doc). The pre-spawn liveness check here is a fast,
/// best-effort rejection for obvious duplicate calls — not the real guard,
/// which is the child's own [`acquire_pidfile`] once it reaches
/// [`run_foreground`].
async fn start_background(
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
    let mut child = spawn_detached(config_flag, token)?;
    let child_pid = child.id();
    confirm_child_started(
        pid_path,
        &mut child,
        child_pid,
        START_CONFIRM_BUDGET,
        START_CONFIRM_POLL_INTERVAL,
    )
    .await?;
    Ok(Payload::DaemonStart(DaemonStartData {
        pid: child_pid,
        foreground: false,
    }))
}

/// Waits up to `budget` (polled every `poll_interval`) for the spawned child
/// to claim the pidfile via its own `acquire_pidfile` call in
/// [`run_foreground`] — the spawner's only feedback that the child reached a
/// healthy state rather than dying immediately (bad token, config error).
/// Uses `child.try_wait()` (which reaps the child) rather than `kill(pid, 0)`
/// to detect an early exit: a child we never `wait()` on stays a zombie and
/// would otherwise still answer "alive" to a signal-0 probe.
async fn confirm_child_started(
    pid_path: &Path,
    child: &mut std::process::Child,
    child_pid: u32,
    budget: std::time::Duration,
    poll_interval: std::time::Duration,
) -> Result<(), AppError> {
    let deadline = std::time::Instant::now() + budget;
    loop {
        if read_pidfile(pid_path)?.is_some_and(|pid| pid == child_pid as i32) {
            return Ok(());
        }
        if let Some(status) = child.try_wait().map_err(|e| {
            AppError::new(
                ErrorKind::Internal,
                format!("failed to check daemon child status: {e}"),
            )
        })? {
            return Err(AppError::new(
                ErrorKind::Internal,
                format!("daemon failed to start (child process exited early: {status})"),
            ));
        }
        if std::time::Instant::now() >= deadline {
            return Err(AppError::new(
                ErrorKind::Internal,
                format!("daemon did not confirm startup within {budget:?}"),
            ));
        }
        tokio::time::sleep(poll_interval).await;
    }
}

/// Atomically claims the pidfile for this process via `O_CREAT|O_EXCL`, so at
/// most one process can win the create. If the file already exists, its
/// recorded pid decides the outcome: alive means a real duplicate (usage
/// error); dead — or still unparsable after [`read_pid_settling`]'s wait —
/// means a stale leftover, which is removed before retrying the create
/// exactly once. A second failure after clearing a stale entry means we lost
/// a genuine race to another acquirer (see [`second_create_failure_error`]).
fn acquire_pidfile(path: &Path) -> Result<(), AppError> {
    match try_create_pidfile(path) {
        Ok(()) => Ok(()),
        Err(PidfileCreateError::AlreadyExists) => match read_pid_settling(path)? {
            Some(pid) if pid_is_alive(pid) => Err(AppError::new(
                ErrorKind::Usage,
                format!("daemon is already running (pid {pid})"),
            )),
            _ => {
                remove_pidfile(path)?;
                try_create_pidfile(path).map_err(|e| second_create_failure_error(path, e))
            }
        },
        Err(PidfileCreateError::Io(e)) => Err(AppError::new(
            ErrorKind::Internal,
            format!("failed to create pidfile {}: {e}", path.display()),
        )),
    }
}

/// How long a losing acquirer waits for the winning acquirer to finish
/// writing its pid before declaring the pidfile a stale leftover.
const PID_SETTLE_BUDGET: std::time::Duration = std::time::Duration::from_millis(500);
const PID_SETTLE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// Reads the pid of an existing pidfile after losing the `O_CREAT|O_EXCL`
/// race, tolerating the winner's create-then-write window: `try_create_pidfile`
/// creates the file and *then* writes the pid, so a loser that reads
/// immediately can observe an existing-but-empty file. Treating that as stale
/// would delete the winner's claim and let both acquirers succeed (two
/// daemons) — the exact TOCTOU this guard exists to close. So an
/// existing-but-unparsable file is re-read on a short budget until a pid
/// appears; only a file that *stays* unparsable past the budget is reported
/// as `None` (a genuine crash leftover, safe to reclaim). A file that
/// disappears mid-wait means the holder released — also `None`, immediately,
/// so the caller retries the create without burning the budget.
fn read_pid_settling(path: &Path) -> Result<Option<i32>, AppError> {
    let deadline = std::time::Instant::now() + PID_SETTLE_BUDGET;
    loop {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                if let Some(pid) = parse_pid(&contents) {
                    return Ok(Some(pid));
                }
                // Exists but no pid yet — likely the winner mid-write.
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(AppError::new(
                    ErrorKind::Internal,
                    format!("failed to read pidfile {}: {e}", path.display()),
                ));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(PID_SETTLE_POLL_INTERVAL);
    }
}

/// Classifies a failure on the second (post-stale-clear) `try_create_pidfile`
/// attempt. A second `AlreadyExists` means another acquirer won a genuine
/// race in the gap between our `remove_pidfile` and retry — if its pid is
/// alive, that is the ordinary "already running" outcome, not our fault, so
/// it gets the same usage error as the first-attempt live-holder case rather
/// than a generic internal one. Everything else (unreadable/dead pid despite
/// losing the create, or a real I/O error) has no such benign explanation and
/// stays `Internal`.
fn second_create_failure_error(path: &Path, err: PidfileCreateError) -> AppError {
    match err {
        PidfileCreateError::AlreadyExists => match read_pidfile(path) {
            Ok(Some(pid)) if pid_is_alive(pid) => AppError::new(
                ErrorKind::Usage,
                format!("daemon is already running (pid {pid})"),
            ),
            _ => AppError::new(
                ErrorKind::Internal,
                format!(
                    "failed to acquire pidfile {} after clearing a stale entry",
                    path.display()
                ),
            ),
        },
        PidfileCreateError::Io(e) => AppError::new(
            ErrorKind::Internal,
            format!(
                "failed to acquire pidfile {} after clearing a stale entry: {e}",
                path.display()
            ),
        ),
    }
}

enum PidfileCreateError {
    AlreadyExists,
    Io(std::io::Error),
}

/// The `O_CREAT|O_EXCL` primitive `acquire_pidfile` builds its retry-once
/// policy on: a bare create-and-write with no read-then-write window.
fn try_create_pidfile(path: &Path) -> Result<(), PidfileCreateError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(PidfileCreateError::Io)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                PidfileCreateError::AlreadyExists
            } else {
                PidfileCreateError::Io(e)
            }
        })?;
    use std::io::Write;
    write!(file, "{}", std::process::id()).map_err(PidfileCreateError::Io)
}

/// Removes the pidfile only if it currently records *this* process's pid.
/// Never deletes another instance's entry — the acquire/release pair must
/// stay symmetric per-owner, not "whoever exits last wins".
fn release_own_pidfile(path: &Path) {
    let own_pid = std::process::id() as i32;
    match read_pidfile(path) {
        Ok(Some(pid)) if pid == own_pid => {
            if let Err(err) = remove_pidfile(path) {
                eprintln!(
                    "daemon: failed to remove pidfile on exit: {}",
                    err.to_human()
                );
            }
        }
        _ => {}
    }
}

/// Re-execs this binary as `daemon start --foreground`, detached: null stdio
/// and its own process group so it outlives the launching shell and its job
/// control. `--config` is forwarded; the token rides in the environment to keep
/// it out of the process table / shell history.
///
/// Returns the live [`std::process::Child`] handle (not just its pid) so the
/// caller can `try_wait()` on it — [`confirm_child_started`] needs that to
/// reliably detect an early exit rather than a zombie still answering
/// "alive" to a signal-0 probe.
fn spawn_detached(config_flag: Option<&str>, token: &str) -> Result<std::process::Child, AppError> {
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
    cmd.spawn().map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("failed to spawn background daemon: {e}"),
        )
    })
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

/// Whether a live daemon is recorded in the pidfile — the fail-fast gate
/// `ask create` uses before sending a question nobody can answer (spec §4).
pub(crate) fn is_daemon_running(pid_path: &Path) -> Result<bool, AppError> {
    Ok(read_pidfile(pid_path)?.is_some_and(pid_is_alive))
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

/// Test-only fixture writer: production code only ever writes the pidfile
/// through the atomic [`try_create_pidfile`] inside [`acquire_pidfile`]; this
/// unconditional write lets tests set up pre-existing pidfile states.
#[cfg(test)]
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

    #[test]
    fn is_daemon_running_false_when_pidfile_missing() {
        let dir = unique_daemon_dir("is-running-missing");
        let pid_path = dir.join("daemon.pid");

        assert!(!is_daemon_running(&pid_path).unwrap());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn is_daemon_running_false_when_pidfile_pid_already_exited() {
        let dir = unique_daemon_dir("is-running-stale");
        let pid_path = dir.join("daemon.pid");

        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn a short-lived child to obtain a guaranteed-dead pid");
        let dead_pid = child.id();
        child.wait().unwrap();
        write_pidfile(&pid_path, dead_pid as i32).unwrap();

        assert!(!is_daemon_running(&pid_path).unwrap());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn is_daemon_running_true_for_a_live_pid() {
        let dir = unique_daemon_dir("is-running-live");
        let pid_path = dir.join("daemon.pid");
        write_pidfile(&pid_path, std::process::id() as i32).unwrap();

        assert!(is_daemon_running(&pid_path).unwrap());

        std::fs::remove_dir_all(&dir).ok();
    }

    fn unique_daemon_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "areum-discord-daemon-stoptest-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Graceful stop end-to-end against a *real* OS process standing in for
    /// the daemon (no Discord/gateway dependency needed for this path): a
    /// live pid in the pidfile must receive SIGTERM, the pidfile must be
    /// removed once it exits, and the reported pid must match.
    ///
    /// The child is reaped on a background thread rather than left a zombie:
    /// in production the immediate parent (the `daemon start` invocation)
    /// exits right after spawning, so init reparents and reaps the detached
    /// child. This in-process test still owns the child, and `kill(pid, 0)`
    /// reports a zombie as alive — so without an explicit reaper, `stop`'s
    /// poll loop would spin for the full `STOP_TIMEOUT` instead of the exit
    /// it actually observed.
    #[tokio::test]
    async fn stop_sends_sigterm_waits_for_exit_and_removes_pidfile() {
        let dir = unique_daemon_dir("graceful");
        let pid_path = dir.join("daemon.pid");

        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn a real child process to stand in for the daemon");
        let child_pid = child.id();
        write_pidfile(&pid_path, child_pid as i32).unwrap();
        let reaper = std::thread::spawn(move || {
            let _ = child.wait();
        });

        let result = run_daemon_stop(&pid_path).await.unwrap();
        match result {
            crate::output::Payload::DaemonStop(data) => {
                assert_eq!(data.pid, child_pid);
                assert!(data.stopped);
            }
            other => panic!("expected DaemonStop, got {other:?}"),
        }

        reaper.join().unwrap();
        assert_eq!(
            read_pidfile(&pid_path).unwrap(),
            None,
            "pidfile must be removed after a graceful stop"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A pidfile pointing at a pid that has already exited (e.g. the daemon
    /// crashed without cleaning up, or the machine's pid counter recycled)
    /// must not be treated as a live daemon: `stop` reports "not running"
    /// rather than signalling an unrelated/nonexistent process, and leaves
    /// the stale pidfile in place for `start` to overwrite.
    #[tokio::test]
    async fn stop_errors_when_pidfile_pid_already_exited() {
        let dir = unique_daemon_dir("stale");
        let pid_path = dir.join("daemon.pid");

        // A pid guaranteed dead: spawned, waited on, and reaped.
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn a short-lived child to obtain a guaranteed-dead pid");
        let dead_pid = child.id();
        child.wait().unwrap();
        write_pidfile(&pid_path, dead_pid as i32).unwrap();

        let err = run_daemon_stop(&pid_path)
            .await
            .expect_err("a pidfile recording an already-exited pid must not report success");
        assert_eq!(err.kind, crate::output::ErrorKind::Usage);
        assert_eq!(
            read_pidfile(&pid_path).unwrap(),
            Some(dead_pid as i32),
            "a failed stop must leave the stale pidfile for `start` to overwrite"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `acquire_pidfile` is the guard `run_foreground` uses on every entry
    /// (direct `--foreground` or as the background spawner's child), so a
    /// live holder must reject a second acquirer with a usage error rather
    /// than silently letting a duplicate daemon start.
    #[test]
    fn acquire_pidfile_errors_when_a_live_pid_already_holds_it() {
        let dir = unique_daemon_dir("acquire-live");
        let pid_path = dir.join("daemon.pid");
        // Our own pid stands in for "a live holder" — always alive in-test.
        write_pidfile(&pid_path, std::process::id() as i32).unwrap();

        let err = acquire_pidfile(&pid_path)
            .expect_err("a live pidfile holder must block a second acquirer");
        assert_eq!(err.kind, ErrorKind::Usage);
        assert!(err.message.contains("already running"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A pidfile left behind by a daemon that died without cleaning up (or a
    /// recycled pid) must not block a fresh start: the stale entry is
    /// cleared and the acquirer claims the pidfile for itself.
    #[test]
    fn acquire_pidfile_reclaims_a_stale_pidfile() {
        let dir = unique_daemon_dir("acquire-stale");
        let pid_path = dir.join("daemon.pid");

        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn a short-lived child to obtain a guaranteed-dead pid");
        let dead_pid = child.id();
        child.wait().unwrap();
        write_pidfile(&pid_path, dead_pid as i32).unwrap();

        acquire_pidfile(&pid_path).expect("a stale pidfile must be reclaimable");
        assert_eq!(
            read_pidfile(&pid_path).unwrap(),
            Some(std::process::id() as i32),
            "the acquirer must overwrite the stale entry with its own pid"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A pidfile-less path must let acquisition succeed and record this
    /// process's pid, mirroring the very first daemon start on a machine.
    #[test]
    fn acquire_pidfile_succeeds_when_no_pidfile_exists() {
        let dir = unique_daemon_dir("acquire-fresh");
        let pid_path = dir.join("daemon.pid");

        acquire_pidfile(&pid_path).expect("acquisition must succeed with no existing pidfile");
        assert_eq!(
            read_pidfile(&pid_path).unwrap(),
            Some(std::process::id() as i32)
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The `O_CREAT|O_EXCL` create is the sole arbiter of the race: with two
    /// threads racing `acquire_pidfile` against the same fresh path, exactly
    /// one must observe success (having won the create) and the other must
    /// be rejected as "already running" (it lost the create, then read back
    /// the winner's — our own process's, so always-alive — pid). Neither
    /// outcome may be "both succeed", which is the TOCTOU this guard closes.
    #[test]
    fn acquire_pidfile_is_atomic_under_concurrent_attempts() {
        let dir = unique_daemon_dir("acquire-race");
        let pid_path = dir.join("daemon.pid");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));

        let handles: Vec<_> = (0..2)
            .map(|_| {
                let pid_path = pid_path.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    acquire_pidfile(&pid_path)
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        let ok_count = results.iter().filter(|r| r.is_ok()).count();
        let usage_err_count = results
            .iter()
            .filter(|r| matches!(r, Err(e) if e.kind == ErrorKind::Usage))
            .count();
        assert_eq!(ok_count, 1, "exactly one racer must win the acquire");
        assert_eq!(
            usage_err_count, 1,
            "the loser must be rejected as already-running, not silently succeed too"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `release_own_pidfile` must remove the pidfile when it still records
    /// this process's own pid (the normal graceful/error-exit path).
    #[test]
    fn release_own_pidfile_removes_its_own_entry() {
        let dir = unique_daemon_dir("release-own");
        let pid_path = dir.join("daemon.pid");
        write_pidfile(&pid_path, std::process::id() as i32).unwrap();

        release_own_pidfile(&pid_path);

        assert_eq!(read_pidfile(&pid_path).unwrap(), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `release_own_pidfile` must never delete another instance's entry —
    /// only the pid that currently holds the pidfile may clear it.
    #[test]
    fn release_own_pidfile_preserves_a_foreign_entry() {
        let dir = unique_daemon_dir("release-foreign");
        let pid_path = dir.join("daemon.pid");
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn a real child process to stand in for another daemon instance");
        let other_pid = child.id();
        write_pidfile(&pid_path, other_pid as i32).unwrap();

        release_own_pidfile(&pid_path);

        assert_eq!(
            read_pidfile(&pid_path).unwrap(),
            Some(other_pid as i32),
            "a pidfile owned by another pid must survive our release call"
        );

        child.kill().ok();
        child.wait().ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `confirm_child_started` must succeed as soon as the pidfile appears
    /// recording the expected child pid, even while the (still-alive) child
    /// process keeps running — this is the normal, healthy startup path.
    #[tokio::test]
    async fn confirm_child_started_succeeds_when_pidfile_appears_before_child_exits() {
        let dir = unique_daemon_dir("confirm-success");
        let pid_path = dir.join("daemon.pid");
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("spawn a real child process to stand in for the daemon");
        let child_pid = child.id();

        // Simulates the child reaching `acquire_pidfile` shortly after spawn.
        let write_pid_path = pid_path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            write_pidfile(&write_pid_path, child_pid as i32).unwrap();
        });

        confirm_child_started(
            &pid_path,
            &mut child,
            child_pid,
            std::time::Duration::from_millis(500),
            std::time::Duration::from_millis(10),
        )
        .await
        .expect("must succeed once the pidfile records the child's pid");

        child.kill().ok();
        child.wait().ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A child that exits (e.g. a bad token / config error) before ever
    /// reaching `acquire_pidfile` must be reported as a startup failure
    /// rather than silently reported as a successful background start.
    #[tokio::test]
    async fn confirm_child_started_errors_when_child_exits_before_claiming_pidfile() {
        let dir = unique_daemon_dir("confirm-early-exit");
        let pid_path = dir.join("daemon.pid");
        let mut child = std::process::Command::new("false")
            .spawn()
            .expect("spawn a real child process that exits immediately");
        let child_pid = child.id();

        let err = confirm_child_started(
            &pid_path,
            &mut child,
            child_pid,
            std::time::Duration::from_millis(500),
            std::time::Duration::from_millis(10),
        )
        .await
        .expect_err("an early-exiting child must not be reported as a successful start");
        assert_eq!(err.kind, ErrorKind::Internal);
        assert!(err.message.contains("failed to start"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A child that neither claims the pidfile nor exits within the budget
    /// (e.g. hung on a slow gateway handshake) must fail with a distinct
    /// "did not confirm" error rather than hanging the CLI invocation.
    #[tokio::test]
    async fn confirm_child_started_errors_when_budget_is_exhausted_with_child_still_alive() {
        let dir = unique_daemon_dir("confirm-budget-exhausted");
        let pid_path = dir.join("daemon.pid");
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("spawn a real child process to stand in for a hung daemon");
        let child_pid = child.id();

        let err = confirm_child_started(
            &pid_path,
            &mut child,
            child_pid,
            std::time::Duration::from_millis(50),
            std::time::Duration::from_millis(10),
        )
        .await
        .expect_err("exhausting the confirm budget must be reported, not hang forever");
        assert_eq!(err.kind, ErrorKind::Internal);
        assert!(err.message.contains("did not confirm startup"));

        child.kill().ok();
        child.wait().ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The empty-pidfile race window: `try_create_pidfile` creates the file
    /// and then writes the pid, so a losing acquirer can observe an
    /// existing-but-empty file. It must wait for the winner's pid to land
    /// and then report "already running" — not misjudge the winner's claim
    /// as stale, delete it, and acquire a second time (two daemons).
    #[test]
    fn acquire_pidfile_waits_out_the_winners_write_instead_of_stealing_an_empty_pidfile() {
        let dir = unique_daemon_dir("acquire-empty-window");
        let pid_path = dir.join("daemon.pid");
        // An existing-but-empty pidfile: exactly what a loser sees when it
        // reads inside the winner's create-then-write window.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&pid_path, "").unwrap();

        // The "winner" finishes its write shortly after — well within the
        // settle budget. Our own pid stands in for a live holder.
        let writer_path = pid_path.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            std::fs::write(&writer_path, std::process::id().to_string()).unwrap();
        });

        let err = acquire_pidfile(&pid_path)
            .expect_err("the loser must defer to the winner's claim, not steal it");
        writer.join().unwrap();
        assert_eq!(err.kind, ErrorKind::Usage);
        assert!(err.message.contains("already running"));
        assert_eq!(
            read_pidfile(&pid_path).unwrap(),
            Some(std::process::id() as i32),
            "the winner's pidfile must survive the loser's attempt"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A pidfile that *stays* empty past the settle budget is a genuine
    /// crash leftover (a process that died between create and write) and
    /// must still be reclaimable — the settle wait must not turn real stale
    /// recovery into a permanent lockout.
    #[test]
    fn acquire_pidfile_reclaims_a_pidfile_that_stays_empty_past_the_settle_budget() {
        let dir = unique_daemon_dir("acquire-stays-empty");
        let pid_path = dir.join("daemon.pid");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&pid_path, "").unwrap();

        acquire_pidfile(&pid_path)
            .expect("a permanently-empty pidfile must be reclaimable as stale");
        assert_eq!(
            read_pidfile(&pid_path).unwrap(),
            Some(std::process::id() as i32)
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Q2 minor: a second create failure whose re-read shows a live pid is a
    /// genuine race loss to another acquirer — the ordinary "already
    /// running" usage error, not an opaque internal one.
    #[test]
    fn second_create_failure_error_reports_usage_when_a_live_pid_now_holds_it() {
        let dir = unique_daemon_dir("second-failure-live");
        let pid_path = dir.join("daemon.pid");
        write_pidfile(&pid_path, std::process::id() as i32).unwrap();

        let err = second_create_failure_error(&pid_path, PidfileCreateError::AlreadyExists);
        assert_eq!(err.kind, ErrorKind::Usage);
        assert!(err.message.contains("already running"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A second `AlreadyExists` whose re-read shows no live pid (dead/
    /// unparsable/missing) has no benign explanation and stays `Internal`.
    #[test]
    fn second_create_failure_error_reports_internal_when_no_live_pid_explains_it() {
        let dir = unique_daemon_dir("second-failure-no-live-pid");
        let pid_path = dir.join("daemon.pid");
        // No pidfile at all — an `AlreadyExists` failure with nothing to
        // read back is not a benign race, so it must stay Internal.

        let err = second_create_failure_error(&pid_path, PidfileCreateError::AlreadyExists);
        assert_eq!(err.kind, ErrorKind::Internal);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A genuine I/O error on the second create attempt has no live-pid
    /// explanation to check and must always stay `Internal`.
    #[test]
    fn second_create_failure_error_reports_internal_for_a_genuine_io_error() {
        let dir = unique_daemon_dir("second-failure-io");
        let pid_path = dir.join("daemon.pid");
        let io_err = std::io::Error::other("disk full");

        let err = second_create_failure_error(&pid_path, PidfileCreateError::Io(io_err));
        assert_eq!(err.kind, ErrorKind::Internal);
        assert!(err.message.contains("disk full"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
