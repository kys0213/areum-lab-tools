use crate::common::api::DiscordApi;
use crate::output::{AppError, ErrorKind, Payload, ReactData};

/// Adds the bot's reaction to a message, or removes it with `remove`.
/// Discord treats a repeated add (or removing an absent reaction) as success,
/// so the call is idempotent and needs no state.
pub(crate) async fn run_react(
    api: &impl DiscordApi,
    channel_id: &str,
    message_id: &str,
    emoji: &str,
    remove: bool,
) -> Result<Payload, AppError> {
    if message_id.is_empty() {
        return Err(AppError::new(
            ErrorKind::Usage,
            "MESSAGE_ID must not be empty",
        ));
    }
    if emoji.trim().is_empty() {
        return Err(AppError::new(ErrorKind::Usage, "EMOJI must not be empty"));
    }
    if remove {
        api.remove_reaction(channel_id, message_id, emoji).await?;
    } else {
        api.add_reaction(channel_id, message_id, emoji).await?;
    }
    Ok(Payload::React(ReactData {
        channel_id: channel_id.to_owned(),
        message_id: message_id.to_owned(),
        emoji: emoji.to_owned(),
        removed: remove,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::{MockDiscordApi, ReactionCall};

    fn call(remove: bool, emoji: &str) -> ReactionCall {
        ReactionCall {
            remove,
            channel_id: "chan1".into(),
            message_id: "msg1".into(),
            emoji: emoji.into(),
        }
    }

    #[tokio::test]
    async fn adds_the_reaction_and_reports_it() {
        let api = MockDiscordApi::new();
        let payload = run_react(&api, "chan1", "msg1", "✅", false).await.unwrap();
        assert_eq!(
            payload,
            Payload::React(ReactData {
                channel_id: "chan1".into(),
                message_id: "msg1".into(),
                emoji: "✅".into(),
                removed: false,
            })
        );
        assert_eq!(*api.reaction_calls.borrow(), vec![call(false, "✅")]);
    }

    #[tokio::test]
    async fn remove_flag_removes_the_reaction() {
        let api = MockDiscordApi::new();
        let payload = run_react(&api, "chan1", "msg1", "party:123", true)
            .await
            .unwrap();
        match payload {
            Payload::React(data) => assert!(data.removed),
            other => panic!("expected React, got {other:?}"),
        }
        assert_eq!(*api.reaction_calls.borrow(), vec![call(true, "party:123")]);
    }

    #[tokio::test]
    async fn api_failure_propagates_unchanged() {
        let api = MockDiscordApi::new();
        let denied = AppError {
            kind: ErrorKind::Auth,
            message: "Discord API returned 403: Missing Permissions".into(),
            http_status: Some(403),
            retry_after_ms: None,
        };
        api.reaction_responses
            .borrow_mut()
            .push_back(Err(denied.clone()));
        let err = run_react(&api, "chan1", "msg1", "✅", false)
            .await
            .unwrap_err();
        assert_eq!(err, denied);
    }

    #[tokio::test]
    async fn empty_message_id_or_emoji_is_a_usage_error_without_a_call() {
        let api = MockDiscordApi::new();
        for (message_id, emoji) in [("", "✅"), ("msg1", ""), ("msg1", "  ")] {
            let err = run_react(&api, "chan1", message_id, emoji, false)
                .await
                .unwrap_err();
            assert_eq!(err.kind, ErrorKind::Usage, "{message_id:?} {emoji:?}");
        }
        assert!(api.reaction_calls.borrow().is_empty());
    }
}
