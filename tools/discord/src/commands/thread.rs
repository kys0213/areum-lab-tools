use crate::common::api::{CreateThreadRequest, DiscordApi};
use crate::output::{AppError, ErrorKind, Payload, ThreadData};

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
    if name.is_empty() {
        return Err(AppError::new(ErrorKind::Usage, "--name must not be empty"));
    }
    // An explicit empty --from-message is a caller mistake, not "no message":
    // reject it rather than assembling ".../messages//threads".
    if from_message == Some("") {
        return Err(AppError::new(
            ErrorKind::Usage,
            "--from-message must not be an empty string",
        ));
    }
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
    async fn run_thread_create_rejects_empty_name() {
        // Mirrors send's "--reply-to must not be an empty string" / "message
        // body must not be empty" local-validation convention: an empty
        // --name is a caller mistake, not something to forward to Discord.
        let api = MockDiscordApi::new();
        let err = run_thread_create(&api, "chan1", "", None)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage);
        assert!(api.thread_calls.borrow().is_empty());
    }

    #[tokio::test]
    async fn run_thread_create_rejects_empty_from_message() {
        // An explicit empty --from-message would otherwise reach
        // `create_thread_url` and assemble ".../messages//threads" — reject
        // it locally instead, same as send's empty --reply-to check.
        let api = MockDiscordApi::new();
        let err = run_thread_create(&api, "chan1", "topic", Some(""))
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage);
        assert!(api.thread_calls.borrow().is_empty());
    }

    #[tokio::test]
    async fn thread_id_can_be_used_as_send_channel_id() {
        // Acceptance criterion 2: the thread id returned by `thread create`
        // must work as the channel id for a subsequent `send --file` call.
        // Both channel_id and thread_id are opaque Strings throughout this
        // crate (SendRequest.channel_id: String, ThreadData.thread_id:
        // String) — this pins that no special-casing breaks the handoff.
        use crate::commands::run_send;
        use crate::commands::testutil::unreachable_stdin;
        use crate::common::api::SentMessage;

        let thread_api = MockDiscordApi::with_thread_responses(vec![Ok(CreatedThread {
            id: "222333".into(),
            name: "from-msg".into(),
        })]);
        let payload = run_thread_create(&thread_api, "chan1", "from-msg", Some("111"))
            .await
            .unwrap();
        let thread_id = match payload {
            Payload::Thread(data) => data.thread_id,
            other => panic!("expected Thread, got {other:?}"),
        };

        let send_api = MockDiscordApi::new();
        send_api
            .send_responses
            .borrow_mut()
            .push_back(Ok(SentMessage {
                id: "1".into(),
                channel_id: thread_id.clone(),
                timestamp: "2024-01-01T00:00:00Z".into(),
                attachments: vec![],
            }));
        run_send(
            &send_api,
            &thread_id,
            None,
            None,
            None,
            &["/tmp/photo.png".to_owned()],
            unreachable_stdin,
            |_| Ok(vec![1, 2, 3]),
        )
        .await
        .unwrap();

        let calls = send_api.send_calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].channel_id, thread_id);
        assert_eq!(calls[0].files.len(), 1);
        assert_eq!(calls[0].files[0].filename, "photo.png");
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
