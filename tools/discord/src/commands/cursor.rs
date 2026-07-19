use crate::api::Message;
use crate::output::{AppError, ErrorKind};

/// Discord caps `limit` at 100; pushing the bound down to a usage error keeps
/// the failure in the same envelope class as other client-side mistakes
/// instead of surfacing as an opaque 400 from the API.
pub(crate) fn validate_limit(limit: u8) -> Result<(), AppError> {
    if (1..=100).contains(&limit) {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorKind::Usage,
            format!("limit must be 1..=100 (got {limit})"),
        ))
    }
}

/// Discord snowflake ids sort correctly only by numeric value, not lexically
/// ("9" > "10" as strings). Parse failure means the API returned something
/// that isn't a snowflake, which is an API contract violation, not a usage
/// error.
pub(crate) fn parse_snowflake(id: &str) -> Result<u64, AppError> {
    id.parse::<u64>().map_err(|_| {
        AppError::new(
            ErrorKind::Api,
            format!("message id '{id}' is not a valid u64 snowflake"),
        )
    })
}

/// Discord returns messages newest-first; the CLI contract is oldest-first.
pub(crate) fn sort_ascending_by_id(messages: Vec<Message>) -> Result<Vec<Message>, AppError> {
    let mut keyed = messages
        .into_iter()
        .map(|m| parse_snowflake(&m.id).map(|key| (key, m)))
        .collect::<Result<Vec<_>, _>>()?;
    keyed.sort_by_key(|(key, _)| *key);
    Ok(keyed.into_iter().map(|(_, m)| m).collect())
}

/// Cursor for the next `--after`: the newest (numerically largest) id when
/// messages came back, otherwise the input `after` echoed unchanged so a
/// caller can retry the same window.
pub(crate) fn newest_cursor(sorted_ascending: &[Message], after: Option<&str>) -> Option<String> {
    match sorted_ascending.last() {
        Some(newest) => Some(newest.id.clone()),
        None => after.map(str::to_owned),
    }
}
