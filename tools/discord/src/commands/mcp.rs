use std::net::SocketAddr;

use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};

use crate::common::error::{AppError, ErrorKind};
use crate::common::http::API_BASE;
use crate::mcp;
use crate::output::{McpServeData, Payload};

/// `mcp serve`: runs the MCP server in the foreground until SIGINT/SIGTERM.
pub(crate) async fn run_mcp_serve(listen: &str) -> Result<Payload, AppError> {
    let addr = parse_listen_addr(listen)?;
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| internal(format!("failed to listen on {addr}: {e}")))?;
    let url = format!(
        "http://{}{}",
        listener.local_addr().map_err(|e| internal(e.to_string()))?,
        mcp::MCP_PATH
    );
    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| internal(format!("failed to install SIGTERM handler: {e}")))?;
    let shutdown = async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    };
    eprintln!("discord mcp: serving {url}");
    mcp::serve(listener, API_BASE, shutdown)
        .await
        .map_err(|e| internal(format!("MCP server failed: {e}")))?;
    Ok(Payload::McpServe(McpServeData { url }))
}

/// Only loopback addresses are accepted: the bot token arrives in a plain
/// HTTP header, so the port must not be reachable from other machines. There
/// is deliberately no override flag — remote use needs TLS in front, which
/// this server does not do.
fn parse_listen_addr(listen: &str) -> Result<SocketAddr, AppError> {
    let addr: SocketAddr = listen.parse().map_err(|_| {
        AppError::new(
            ErrorKind::Usage,
            format!("--listen `{listen}` must be an IP:port such as 127.0.0.1:8765"),
        )
    })?;
    if !addr.ip().is_loopback() {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!(
                "--listen {addr} is not a loopback address; the bot token travels in plain HTTP, so only 127.0.0.1 or [::1] is allowed"
            ),
        ));
    }
    Ok(addr)
}

fn internal(message: String) -> AppError {
    AppError::new(ErrorKind::Internal, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_addresses_are_accepted() {
        for listen in ["127.0.0.1:8765", "[::1]:8765", "127.0.0.1:0"] {
            assert_eq!(parse_listen_addr(listen).unwrap(), listen.parse().unwrap());
        }
    }

    #[test]
    fn non_loopback_or_unparsable_addresses_are_usage_errors() {
        for listen in [
            "0.0.0.0:8765",
            "192.168.0.10:8765",
            "[::]:8765",
            "localhost:8765",
            "8765",
        ] {
            let err = parse_listen_addr(listen).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Usage, "{listen}");
        }
    }
}
