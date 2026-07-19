use crate::common::api::{CreateThreadRequest, DiscordApi};
use crate::output::{AppError, Payload, ThreadData};

/// Creates a thread on a channel, or on a specific message when
/// `from_message` is given. Discord itself decides thread_id semantics (a
/// message-derived thread's id equals the message id) — this function does
/// not special-case that, it just reports what the API returns.
///
/// THREAD_ALREADY_CREATED (re-request on a message that already has a
/// thread) is not papered over with a synthesized success: the API error
/// propagates as-is (fail-fast, see `rust-coding.md`).
pub(crate) async fn run_thread_create(
    api: &impl DiscordApi,
    channel_id: &str,
    name: &str,
    from_message: Option<&str>,
) -> Result<Payload, AppError> {
    let req = CreateThreadRequest {
        channel_id: channel_id.to_owned(),
        name: name.to_owned(),
        from_message_id: from_message.map(str::to_owned),
    };
    let thread = api.create_thread(&req).await?;
    Ok(Payload::Thread(ThreadData {
        thread_id: thread.id,
        name: thread.name,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::MockDiscordApi;
    use crate::common::api::CreatedThread;
    use crate::output::ErrorKind;

    #[tokio::test]
    async fn channel_thread_creation_maps_to_thread_data() {
        let api = MockDiscordApi::with_thread_responses(vec![Ok(CreatedThread {
            id: "111".into(),
            name: "discussion".into(),
        })]);

        let payload = run_thread_create(&api, "chan1", "discussion", None)
            .await
            .unwrap();

        match payload {
            Payload::Thread(data) => {
                assert_eq!(data.thread_id, "111");
                assert_eq!(data.name, "discussion");
            }
            other => panic!("expected Thread, got {other:?}"),
        }
        assert_eq!(api.thread_calls.borrow().len(), 1);
        assert_eq!(api.thread_calls.borrow()[0].from_message_id, None);
    }

    #[tokio::test]
    async fn message_derived_thread_propagates_message_id_to_the_api_call() {
        let api = MockDiscordApi::with_thread_responses(vec![Ok(CreatedThread {
            id: "999".into(),
            name: "from-msg".into(),
        })]);

        let payload = run_thread_create(&api, "chan1", "from-msg", Some("999"))
            .await
            .unwrap();

        match payload {
            Payload::Thread(data) => {
                assert_eq!(data.thread_id, "999");
                assert_eq!(data.name, "from-msg");
            }
            other => panic!("expected Thread, got {other:?}"),
        }
        let calls = api.thread_calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].channel_id, "chan1");
        assert_eq!(calls[0].name, "from-msg");
        assert_eq!(calls[0].from_message_id.as_deref(), Some("999"));
    }

    #[tokio::test]
    async fn api_error_propagates_as_is() {
        // THREAD_ALREADY_CREATED and any other Discord API error must surface
        // unchanged, not be swallowed or converted into a synthesized success.
        let api = MockDiscordApi::with_thread_responses(vec![Err(AppError {
            kind: ErrorKind::Api,
            message: "Discord API returned 400: THREAD_ALREADY_CREATED".into(),
            http_status: Some(400),
            retry_after_ms: None,
        })]);

        let err = run_thread_create(&api, "chan1", "dup", Some("999"))
            .await
            .unwrap_err();

        assert_eq!(err.kind, ErrorKind::Api);
        assert!(err.message.contains("THREAD_ALREADY_CREATED"));
    }
}
