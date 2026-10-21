//! Binding and serving. The Unix socket doubles as the single-instance lock:
//! a socket that accepts a connection belongs to a running daemon; one that
//! refuses is left over from a daemon that died, and is replaced.

use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use axum::middleware;
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::sync::oneshot;

use crate::common::error::{AppError, ErrorKind};
use crate::common::store::Store;
use crate::daemon::brief::BriefConfig;
use crate::daemon::guard::loopback_only;
use crate::daemon::routes::{AppState, shared_routes, socket_routes, stopped};
use crate::daemon::state::Thresholds;

pub(crate) struct DaemonConfig {
    pub(crate) socket: PathBuf,
    /// `None` serves no UI; `Some(0)` lets the OS pick the port.
    pub(crate) ui_port: Option<u16>,
    pub(crate) thresholds: Thresholds,
    /// `None` writes no briefs.
    pub(crate) brief: Option<BriefConfig>,
}

/// Where the daemon ended up listening, reported once both listeners are
/// bound.
#[derive(Debug, Clone)]
pub(crate) struct Bound {
    pub(crate) socket: PathBuf,
    pub(crate) ui: Option<SocketAddr>,
}

/// Serves until `/v1/shutdown`, SIGINT or SIGTERM, then removes the socket.
pub(crate) async fn serve(
    store: Store,
    config: DaemonConfig,
    ready: Option<oneshot::Sender<Bound>>,
) -> Result<(), AppError> {
    let unix = bind_socket(&config.socket).await?;
    let tcp = match config.ui_port {
        Some(port) => Some(
            TcpListener::bind(("127.0.0.1", port))
                .await
                .map_err(|e| internal(format!("cannot listen on 127.0.0.1:{port}: {e}")))?,
        ),
        None => None,
    };
    let ui_addr = tcp
        .as_ref()
        .map(|l| l.local_addr())
        .transpose()
        .map_err(|e| internal(format!("cannot read the UI address: {e}")))?;

    let app = AppState::new(store, config.thresholds, config.brief.clone());
    tokio::spawn(stop_on_signal(app.clone()));

    let socket_server = axum::serve(unix, socket_routes().with_state(app.clone()))
        .with_graceful_shutdown(stopped(app.stop.subscribe()));
    if let Some(ready) = ready {
        let _ = ready.send(Bound {
            socket: config.socket.clone(),
            ui: ui_addr,
        });
    }
    let result = match (tcp, ui_addr) {
        (Some(tcp), Some(addr)) => {
            let ui_router = shared_routes()
                .layer(middleware::from_fn_with_state(addr.port(), loopback_only))
                .with_state(app.clone());
            let ui_server =
                axum::serve(tcp, ui_router).with_graceful_shutdown(stopped(app.stop.subscribe()));
            tokio::try_join!(socket_server.into_future(), ui_server.into_future()).map(|_| ())
        }
        _ => socket_server.await,
    };
    let _ = std::fs::remove_file(&config.socket);
    result.map_err(|e| internal(format!("server stopped with an error: {e}")))
}

async fn stop_on_signal(app: AppState) {
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate => {}
        _ = stopped(app.stop.subscribe()) => return,
    }
    app.stop.send_replace(true);
}

/// Binds the socket with owner-only permissions — the file mode is the
/// authentication for every local client.
async fn bind_socket(path: &Path) -> Result<UnixListener, AppError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| internal(format!("cannot create {}: {e}", parent.display())))?;
    }
    if path.exists() {
        if UnixStream::connect(path).await.is_ok() {
            return Err(AppError::new(
                ErrorKind::Conflict,
                format!("a doppel daemon is already listening on {}", path.display()),
            ));
        }
        std::fs::remove_file(path).map_err(|e| {
            internal(format!(
                "cannot remove stale socket {}: {e}",
                path.display()
            ))
        })?;
    }
    let listener = UnixListener::bind(path)
        .map_err(|e| internal(format!("cannot listen on {}: {e}", path.display())))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| internal(format!("cannot restrict {}: {e}", path.display())))?;
    Ok(listener)
}

fn internal(message: String) -> AppError {
    AppError::new(ErrorKind::Internal, message)
}
