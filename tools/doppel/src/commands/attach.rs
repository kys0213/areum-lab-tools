use std::io::Write;
use std::path::Path;

use crate::commands::runtime::block_on;
use crate::common::client::attach;
use crate::output::{AppError, AttachData, Payload};

/// Writes each message as one line and flushes it at once — the plugin reads
/// the lines as a stream while this process is still running.
pub(crate) fn run_attach(socket: &Path, session_id: &str) -> Result<Payload, AppError> {
    let mut messages = 0;
    block_on(attach(socket, session_id, |line| {
        let mut out = std::io::stdout().lock();
        // A closed stdout means the plugin is gone; the next message ends
        // the loop through the daemon dropping us, so the error is moot.
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
        messages += 1;
    }))?;
    Ok(Payload::Attach(AttachData {
        session_id: session_id.to_owned(),
        messages,
    }))
}
