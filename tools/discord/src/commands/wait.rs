use std::time::Duration;

use crate::api::DiscordApi;
use crate::output::{AppError, ErrorKind, Payload, WaitData};

use super::cursor::{newest_cursor, sort_ascending_by_id, validate_limit};

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

/// Polls until a new message arrives or the poll budget is exhausted. T2 owns
/// the loop (`max_polls = ceil(timeout / interval)`), sleeping via `sleeper`
/// between polls and setting `timed_out` on exhaustion.
pub(crate) async fn run_wait(
    api: &impl DiscordApi,
    sleeper: &impl Sleeper,
    channel_id: &str,
    after: Option<&str>,
    timeout: u64,
    interval: u64,
    limit: u8,
) -> Result<Payload, AppError> {
    validate_limit(limit)?;
    if interval == 0 {
        return Err(AppError::new(
            ErrorKind::Usage,
            "interval must be greater than 0",
        ));
    }
    // Poll-count driven, not wall-clock: keeps `wait` deterministic under test.
    let max_polls = timeout.div_ceil(interval).max(1);

    for poll in 0..max_polls {
        let messages = api.get_messages(channel_id, after, limit).await?;
        if !messages.is_empty() {
            let sorted = sort_ascending_by_id(messages)?;
            let cursor = newest_cursor(&sorted, after);
            return Ok(Payload::Wait(WaitData {
                channel_id: channel_id.to_owned(),
                count: sorted.len(),
                cursor,
                timed_out: false,
                messages: sorted,
            }));
        }
        let is_last_poll = poll + 1 == max_polls;
        if !is_last_poll {
            sleeper.sleep(Duration::from_secs(interval)).await;
        }
    }

    // Exhausting the poll budget with no new message is a normal outcome
    // (exit 0), not an error.
    Ok(Payload::Wait(WaitData {
        channel_id: channel_id.to_owned(),
        count: 0,
        cursor: after.map(str::to_owned),
        timed_out: true,
        messages: Vec::new(),
    }))
}

#[cfg(test)]
mod tests;
