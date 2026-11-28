use crate::common::api::DiscordApi;
use crate::common::config;
use crate::common::error::{AppError, ErrorKind};

use super::request_headers::RequestHeaders;

/// Turns a tool's `channel` argument into the channel id to call, enforcing
/// `X-Discord-Allowed-Channels`.
///
/// A thread is allowed when its parent channel is listed: threads are created
/// on the fly (by `thread_create`, or by people), so their ids cannot be in a
/// list written ahead of time, and a thread only reaches members of its parent
/// channel anyway. Checking that costs one `GET /channels/{id}`, made only for
/// ids that are not listed directly.
pub(super) async fn resolve_allowed_channel(
    api: &impl DiscordApi,
    headers: &RequestHeaders,
    channel: &str,
) -> Result<String, AppError> {
    if channel.trim().is_empty() {
        return Err(AppError::new(ErrorKind::Usage, "channel must not be empty"));
    }
    let channel_id = config::resolve_channel(channel, &headers.channels);
    let Some(allowed) = &headers.allowed_channels else {
        return Ok(channel_id);
    };
    if allowed.contains(&channel_id) {
        return Ok(channel_id);
    }
    let info = api.get_channel(&channel_id).await?;
    let parent_allowed = info.is_thread()
        && info
            .parent_id
            .as_ref()
            .is_some_and(|parent| allowed.contains(parent));
    if parent_allowed {
        return Ok(channel_id);
    }
    Err(AppError::new(
        ErrorKind::Auth,
        format!("channel {channel_id} is not in X-Discord-Allowed-Channels"),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::commands::testutil::MockDiscordApi;
    use crate::common::api::ChannelInfo;

    fn headers(allowed: Option<&[&str]>) -> RequestHeaders {
        RequestHeaders {
            token: "t".into(),
            channels: HashMap::from([("dev".to_owned(), "111".to_owned())]),
            allowed_channels: allowed.map(|ids| ids.iter().map(|s| (*s).to_owned()).collect()),
        }
    }

    fn channel(id: &str, kind: u8, parent: Option<&str>) -> ChannelInfo {
        ChannelInfo {
            id: id.into(),
            kind,
            parent_id: parent.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn alias_resolves_without_allow_list_and_without_lookup() {
        let api = MockDiscordApi::new();
        let id = resolve_allowed_channel(&api, &headers(None), "dev")
            .await
            .unwrap();
        assert_eq!(id, "111");
        let id = resolve_allowed_channel(&api, &headers(None), "999")
            .await
            .unwrap();
        assert_eq!(id, "999");
        assert!(api.channel_calls.borrow().is_empty());
    }

    #[tokio::test]
    async fn listed_channel_is_checked_after_alias_resolution_without_lookup() {
        let api = MockDiscordApi::new();
        let id = resolve_allowed_channel(&api, &headers(Some(&["111"])), "dev")
            .await
            .unwrap();
        assert_eq!(id, "111");
        assert!(api.channel_calls.borrow().is_empty());
    }

    #[tokio::test]
    async fn thread_under_a_listed_parent_is_allowed() {
        let api = MockDiscordApi::new();
        api.channel_responses
            .borrow_mut()
            .push_back(Ok(channel("555", 11, Some("111"))));
        let id = resolve_allowed_channel(&api, &headers(Some(&["111"])), "555")
            .await
            .unwrap();
        assert_eq!(id, "555");
        assert_eq!(*api.channel_calls.borrow(), vec!["555".to_owned()]);
    }

    #[tokio::test]
    async fn unlisted_channel_or_thread_under_unlisted_parent_is_denied() {
        for info in [
            channel("555", 0, Some("111")),
            channel("555", 11, Some("222")),
        ] {
            let api = MockDiscordApi::new();
            api.channel_responses.borrow_mut().push_back(Ok(info));
            let err = resolve_allowed_channel(&api, &headers(Some(&["111"])), "555")
                .await
                .unwrap_err();
            assert_eq!(err.kind, ErrorKind::Auth);
            assert!(err.message.contains("555"), "{}", err.message);
        }
    }

    #[tokio::test]
    async fn lookup_failure_propagates() {
        let api = MockDiscordApi::new();
        let missing = AppError::new(ErrorKind::Api, "Unknown Channel");
        api.channel_responses
            .borrow_mut()
            .push_back(Err(missing.clone()));
        let err = resolve_allowed_channel(&api, &headers(Some(&["111"])), "555")
            .await
            .unwrap_err();
        assert_eq!(err, missing);
    }

    #[tokio::test]
    async fn empty_channel_is_a_usage_error() {
        let api = MockDiscordApi::new();
        let err = resolve_allowed_channel(&api, &headers(Some(&["111"])), " ")
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage);
    }
}
