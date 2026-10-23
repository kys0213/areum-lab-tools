use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use crate::cli::DaemonOptions;
use crate::commands::runtime::block_on;
use crate::common::client::request;
use crate::common::error::{AppError, ErrorKind};
use crate::common::store::Store;
use crate::daemon::api::Health;
use crate::daemon::auto::{AutoConfig, TriageGate};
use crate::daemon::launch::LaunchConfig;
use crate::daemon::listen::{Bound, DaemonConfig, serve};
use crate::daemon::model_call::ModelCommand;
use crate::daemon::state::Thresholds;
use crate::output::{DaemonData, Payload};

/// How long `start` waits for the detached daemon to answer, and `stop` for
/// it to let go of the socket.
const SETTLE_BUDGET: Duration = Duration::from_secs(5);
const SETTLE_POLL: Duration = Duration::from_millis(100);

fn brief_config(options: &DaemonOptions) -> Option<ModelCommand> {
    let timeout = Duration::from_secs(options.brief_timeout_secs);
    match &options.brief_command {
        Some(command) => ModelCommand::from_command(command, timeout),
        None if options.brief => {
            ModelCommand::from_command(crate::daemon::brief::DEFAULT_COMMAND, timeout)
        }
        None => None,
    }
}

fn proposal_config(options: &DaemonOptions) -> Option<ModelCommand> {
    let timeout = Duration::from_secs(options.propose_timeout_secs);
    match &options.propose_command {
        Some(command) => ModelCommand::from_command(command, timeout),
        None if options.propose => {
            ModelCommand::from_command(crate::daemon::proposal::DEFAULT_COMMAND, timeout)
        }
        None => None,
    }
}

fn auto_config(options: &DaemonOptions) -> AutoConfig {
    AutoConfig {
        triage: options.auto_triage.then_some(TriageGate {
            min_judged: options.auto_triage_min_judged,
            min_acceptance: options.auto_triage_min_acceptance,
        }),
        dispatch: options.auto_dispatch,
    }
}

fn launch_config(socket: &Path, options: &DaemonOptions) -> Option<LaunchConfig> {
    let command = options
        .launch_command
        .as_deref()
        .unwrap_or(LaunchConfig::DEFAULT_COMMAND);
    LaunchConfig::from_command(command, options.max_running, socket.to_path_buf())
}

fn config(socket: &Path, options: &DaemonOptions) -> DaemonConfig {
    DaemonConfig {
        brief: brief_config(options),
        proposal: proposal_config(options),
        launch: launch_config(socket, options),
        auto: auto_config(options),
        socket: socket.to_path_buf(),
        ui_port: (!options.no_ui).then_some(options.ui_port),
        thresholds: Thresholds {
            stall_after_secs: options.stall_after_secs,
            fail_streak: options.fail_streak,
            heartbeat_timeout_secs: options.heartbeat_timeout_secs,
        },
    }
}

fn ui_url(options: &DaemonOptions) -> Option<String> {
    (!options.no_ui).then(|| format!("http://127.0.0.1:{}/", options.ui_port))
}

/// Serves in this process until stopped. Diagnostics go to stderr; stdout
/// only carries the final result.
pub(crate) fn run_daemon_run(
    db_path: &Path,
    socket: &Path,
    options: &DaemonOptions,
) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let config = config(socket, options);
    block_on(async move {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Bound>();
        tokio::spawn(async move {
            if let Ok(bound) = ready_rx.await {
                let ui = bound
                    .ui
                    .map(|a| format!(", web UI http://{a}/"))
                    .unwrap_or_default();
                eprintln!("doppel daemon listening on {}{ui}", bound.socket.display());
            }
        });
        serve(store, config, Some(ready_tx)).await
    })?;
    Ok(Payload::DaemonRun(DaemonData {
        running: false,
        socket: socket.display().to_string(),
        ui_url: None,
        pid: None,
        version: None,
    }))
}

/// Re-runs this binary as `daemon run` in its own process group, detached
/// from the terminal, logging to `daemon.log` next to the database.
pub(crate) fn run_daemon_start(
    db_path: &Path,
    socket: &Path,
    options: &DaemonOptions,
) -> Result<Payload, AppError> {
    if block_on(health(socket)).is_ok() {
        return Err(AppError::new(
            ErrorKind::Conflict,
            format!("a doppel daemon is already running on {}", socket.display()),
        ));
    }
    let log_path = db_path.with_file_name("daemon.log");
    if let Some(parent) = log_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| internal(format!("cannot create {}: {e}", parent.display())))?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| internal(format!("cannot open {}: {e}", log_path.display())))?;
    let log_err = log
        .try_clone()
        .map_err(|e| internal(format!("cannot open {}: {e}", log_path.display())))?;
    let exe =
        std::env::current_exe().map_err(|e| internal(format!("cannot locate doppel: {e}")))?;

    let mut command = std::process::Command::new(exe);
    command
        .arg("--db")
        .arg(db_path)
        .arg("--socket")
        .arg(socket)
        .args(["daemon", "run"])
        .args(["--ui-port", &options.ui_port.to_string()])
        .args(["--stall-after-secs", &options.stall_after_secs.to_string()])
        .args(["--fail-streak", &options.fail_streak.to_string()])
        .args([
            "--heartbeat-timeout-secs",
            &options.heartbeat_timeout_secs.to_string(),
        ])
        .args([
            "--brief-timeout-secs",
            &options.brief_timeout_secs.to_string(),
        ])
        .args(["--max-running", &options.max_running.to_string()])
        .args([
            "--propose-timeout-secs",
            &options.propose_timeout_secs.to_string(),
        ])
        .args([
            "--auto-triage-min-judged",
            &options.auto_triage_min_judged.to_string(),
        ])
        .args([
            "--auto-triage-min-acceptance",
            &options.auto_triage_min_acceptance.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err);
    if options.no_ui {
        command.arg("--no-ui");
    }
    if options.brief {
        command.arg("--brief");
    }
    if let Some(brief_command) = &options.brief_command {
        command.args(["--brief-command", brief_command]);
    }
    if let Some(launch_command) = &options.launch_command {
        command.args(["--launch-command", launch_command]);
    }
    if options.propose {
        command.arg("--propose");
    }
    if let Some(propose_command) = &options.propose_command {
        command.args(["--propose-command", propose_command]);
    }
    if options.auto_triage {
        command.arg("--auto-triage");
    }
    if options.auto_dispatch {
        command.arg("--auto-dispatch");
    }
    detach(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| internal(format!("cannot start the daemon: {e}")))?;

    let started = block_on(async {
        let deadline = tokio::time::Instant::now() + SETTLE_BUDGET;
        loop {
            if let Ok(h) = health(socket).await {
                return Ok(h);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(internal(format!(
                    "the daemon did not answer within {}s; see {}",
                    SETTLE_BUDGET.as_secs(),
                    log_path.display()
                )));
            }
            tokio::time::sleep(SETTLE_POLL).await;
        }
    });
    let health = match started {
        Ok(h) => h,
        Err(err) => {
            // Exit status says why when it died; a live child that never
            // answered is left to its log.
            if let Ok(Some(status)) = child.try_wait() {
                return Err(internal(format!(
                    "the daemon exited ({status}) before answering; see {}",
                    log_path.display()
                )));
            }
            return Err(err);
        }
    };
    Ok(Payload::DaemonStart(DaemonData {
        running: true,
        socket: socket.display().to_string(),
        ui_url: ui_url(options),
        pid: Some(health.pid),
        version: Some(health.version),
    }))
}

#[cfg(unix)]
fn detach(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // Its own process group: the terminal's Ctrl-C does not reach it.
    command.process_group(0);
}

pub(crate) fn run_daemon_stop(socket: &Path) -> Result<Payload, AppError> {
    block_on(async {
        request(socket, "POST", "/v1/shutdown", None).await?;
        let deadline = tokio::time::Instant::now() + SETTLE_BUDGET;
        while health(socket).await.is_ok() {
            if tokio::time::Instant::now() >= deadline {
                return Err(internal(format!(
                    "the daemon was asked to stop but still answers after {}s",
                    SETTLE_BUDGET.as_secs()
                )));
            }
            tokio::time::sleep(SETTLE_POLL).await;
        }
        Ok(())
    })?;
    Ok(Payload::DaemonStop(DaemonData {
        running: false,
        socket: socket.display().to_string(),
        ui_url: None,
        pid: None,
        version: None,
    }))
}

/// A daemon that does not answer is a valid status, not an error.
pub(crate) fn run_daemon_status(socket: &Path) -> Result<Payload, AppError> {
    let data = match block_on(health(socket)) {
        Ok(h) => DaemonData {
            running: true,
            socket: socket.display().to_string(),
            ui_url: None,
            pid: Some(h.pid),
            version: Some(h.version),
        },
        Err(err) if err.kind == ErrorKind::Network => DaemonData {
            running: false,
            socket: socket.display().to_string(),
            ui_url: None,
            pid: None,
            version: None,
        },
        Err(err) => return Err(err),
    };
    Ok(Payload::DaemonStatus(data))
}

async fn health(socket: &Path) -> Result<Health, AppError> {
    let value = request(socket, "GET", "/v1/health", None).await?;
    serde_json::from_value(value).map_err(|e| internal(format!("unexpected health answer: {e}")))
}

fn internal(message: String) -> AppError {
    AppError::new(ErrorKind::Internal, message)
}
