use std::path::Path;

use crate::commands::runtime::block_on;
use crate::common::client::request;
use crate::common::error::{AppError, ErrorKind};
use crate::daemon::api::SessionList;
use crate::output::{Payload, SessionListData};

pub(crate) fn run_session_list(socket: &Path) -> Result<Payload, AppError> {
    let value = block_on(request(socket, "GET", "/v1/sessions", None))?;
    let list: SessionList = serde_json::from_value(value).map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("unexpected sessions answer: {e}"),
        )
    })?;
    Ok(Payload::SessionList(SessionListData {
        count: list.sessions.len(),
        sessions: list.sessions,
    }))
}
