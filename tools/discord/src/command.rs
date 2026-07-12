use std::time::Duration;

use crate::api::DiscordApi;
use crate::output::{AppError, Payload, SendData};

/// Clock seam so `wait` polling is testable without real time. `wait` drives a
/// fixed number of polls (`ceil(timeout / interval)`), not wall-clock elapsed.
pub(crate) trait Sleeper {
    async fn sleep(&self, dur: Duration);
}

/// Production clock backed by tokio (requires the tokio "time" feature).
pub struct TokioSleeper;

impl Sleeper for TokioSleeper {
    async fn sleep(&self, dur: Duration) {
        tokio::time::sleep(dur).await;
    }
}

/// Sends a message to a channel and reports the created message. T2 fills
/// [`resolve_send_content`]; the send + envelope mapping is the settled contract.
pub async fn run_send(
    api: &impl DiscordApi,
    channel_id: &str,
    body: Option<&str>,
    text: Option<&str>,
    read_stdin: impl FnOnce() -> std::io::Result<String>,
) -> Result<Payload, AppError> {
    let content = resolve_send_content(body, text, read_stdin)?;
    let sent = api.send_message(channel_id, &content).await?;
    Ok(Payload::Send(SendData {
        message_id: sent.id,
        channel_id: sent.channel_id,
        timestamp: sent.timestamp,
    }))
}

/// Resolves the outgoing message content. T2 owns: BODY vs --text mutual
/// exclusion (usage error), stdin read when `body` is `None` or `"-"`, and the
/// 2000-codepoint limit (`chars()`, usage error).
fn resolve_send_content(
    body: Option<&str>,
    text: Option<&str>,
    read_stdin: impl FnOnce() -> std::io::Result<String>,
) -> Result<String, AppError> {
    let _ = (body, text, read_stdin);
    todo!("T2: resolve content from body/text/stdin and validate usage")
}

/// Reads recent messages. T2 owns cursor derivation and the `Payload::Read`
/// assembly around the fetched messages.
pub async fn run_read(
    api: &impl DiscordApi,
    channel_id: &str,
    after: Option<&str>,
    limit: u8,
) -> Result<Payload, AppError> {
    let _messages = api.get_messages(channel_id, after, limit).await?;
    todo!("T2: compute cursor and build Payload::Read")
}

/// Polls until a new message arrives or the poll budget is exhausted. T2 owns
/// the loop (`max_polls = ceil(timeout / interval)`), sleeping via `sleeper`
/// between polls and setting `timed_out` on exhaustion.
pub async fn run_wait(
    api: &impl DiscordApi,
    sleeper: &impl Sleeper,
    channel_id: &str,
    after: Option<&str>,
    timeout: u64,
    interval: u64,
    limit: u8,
) -> Result<Payload, AppError> {
    let _ = timeout;
    let _messages = api.get_messages(channel_id, after, limit).await?;
    sleeper.sleep(Duration::from_secs(interval)).await;
    todo!("T2: poll loop building Payload::Wait (timed_out on exhaustion)")
}
