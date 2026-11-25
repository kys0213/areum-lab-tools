//! One call to the daemon API, decoded into the shape the command expects.

use std::path::Path;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::commands::runtime::block_on;
use crate::common::client::request;
use crate::common::error::{AppError, ErrorKind};

pub(super) fn call<T: DeserializeOwned>(
    socket: &Path,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<T, AppError> {
    let value = block_on(request(socket, method, path, body))?;
    serde_json::from_value(value).map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("unexpected daemon answer: {e}"),
        )
    })
}
