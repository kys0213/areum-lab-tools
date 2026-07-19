use crate::api::DiscordApi;
use crate::output::{AppError, Payload, ReadData};

use super::cursor::{newest_cursor, sort_ascending_by_id, validate_limit};

/// Reads recent messages. T2 owns cursor derivation and the `Payload::Read`
/// assembly around the fetched messages.
pub(crate) async fn run_read(
    api: &impl DiscordApi,
    channel_id: &str,
    after: Option<&str>,
    limit: u8,
) -> Result<Payload, AppError> {
    validate_limit(limit)?;
    let messages = api.get_messages(channel_id, after, limit).await?;
    let sorted = sort_ascending_by_id(messages)?;
    let cursor = newest_cursor(&sorted, after);
    Ok(Payload::Read(ReadData {
        channel_id: channel_id.to_owned(),
        count: sorted.len(),
        cursor,
        messages: sorted,
    }))
}

#[cfg(test)]
mod tests;
