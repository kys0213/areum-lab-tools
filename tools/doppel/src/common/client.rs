//! The CLI's side of the daemon's Unix socket: one HTTP/1 request per call,
//! and the attach WebSocket. Shared by every command that talks to a running
//! daemon (`daemon`, `ask`, `session`, `attach`).

use std::path::Path;

use futures_util::StreamExt;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::net::UnixStream;
use tokio_tungstenite::tungstenite::Message;

use crate::common::error::{AppError, ErrorKind};

/// The host part of every request URL. The socket path decides where the
/// request goes; the daemon ignores the host on its socket listener.
const HOST: &str = "doppel";

fn unreachable(socket: &Path, cause: impl std::fmt::Display) -> AppError {
    AppError::new(
        ErrorKind::Network,
        format!(
            "doppel daemon is not reachable at {} ({cause}); start it with `doppel daemon start`",
            socket.display()
        ),
    )
}

/// Sends one request and decodes the JSON answer. A daemon error envelope
/// comes back as the `AppError` it carries, so the CLI's exit code is the
/// daemon's verdict.
pub(crate) async fn request(
    socket: &Path,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, AppError> {
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|e| unreachable(socket, e))?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| unreachable(socket, e))?;
    tokio::spawn(connection);

    let payload = body.map(Value::to_string).unwrap_or_default();
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", HOST)
        .header("content-type", "application/json")
        .header("x-doppel-client", "cli")
        .body(Full::new(Bytes::from(payload)))
        .map_err(|e| AppError::new(ErrorKind::Internal, format!("cannot build request: {e}")))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| unreachable(socket, e))?;
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| unreachable(socket, e))?
        .to_bytes();
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("daemon answered {status} with a body that is not JSON: {e}"),
        )
    })?;
    if status.is_success() {
        return Ok(value);
    }
    Err(
        serde_json::from_value::<DaemonError>(value["error"].clone())
            .map(DaemonError::into_app_error)
            .unwrap_or_else(|_| {
                AppError::new(ErrorKind::Internal, format!("daemon answered {status}"))
            }),
    )
}

#[derive(serde::Deserialize)]
struct DaemonError {
    kind: String,
    message: String,
}

impl DaemonError {
    fn into_app_error(self) -> AppError {
        let kind = match self.kind.as_str() {
            "usage" => ErrorKind::Usage,
            "config" => ErrorKind::Config,
            "not_found" => ErrorKind::NotFound,
            "conflict" => ErrorKind::Conflict,
            _ => ErrorKind::Internal,
        };
        AppError::new(kind, self.message)
    }
}

/// Opens `/v1/attach/{session_id}` and hands every text message to `on_line`
/// until the daemon closes the connection.
pub(crate) async fn attach(
    socket: &Path,
    session_id: &str,
    mut on_line: impl FnMut(&str),
) -> Result<(), AppError> {
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|e| unreachable(socket, e))?;
    let url = format!("ws://{HOST}/v1/attach/{session_id}");
    let (mut ws, _response) = tokio_tungstenite::client_async(url, stream)
        .await
        .map_err(|e| unreachable(socket, e))?;
    while let Some(message) = ws.next().await {
        match message {
            Ok(Message::Text(text)) => on_line(text.as_str()),
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    Ok(())
}
