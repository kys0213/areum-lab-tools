use super::*;
use crate::commands::testutil::*;
use crate::common::api::{Attachment, SentMessage};

// ---- resolve_send_content ----

#[test]
fn resolve_send_content_uses_text_when_provided() {
    let content = resolve_send_content(None, Some("hi"), false, unreachable_stdin).unwrap();
    assert_eq!(content, "hi");
}

#[test]
fn resolve_send_content_uses_body_when_provided() {
    let content = resolve_send_content(Some("hello"), None, false, unreachable_stdin).unwrap();
    assert_eq!(content, "hello");
}

#[test]
fn resolve_send_content_reads_stdin_when_body_is_dash() {
    let content =
        resolve_send_content(Some("-"), None, false, || Ok("from stdin".to_owned())).unwrap();
    assert_eq!(content, "from stdin");
}

#[test]
fn resolve_send_content_reads_stdin_when_body_and_text_absent() {
    let content = resolve_send_content(None, None, false, || Ok("from stdin".to_owned())).unwrap();
    assert_eq!(content, "from stdin");
}

#[test]
fn resolve_send_content_preserves_trailing_newline_from_stdin() {
    let content = resolve_send_content(None, None, false, || Ok("line\n".to_owned())).unwrap();
    assert_eq!(content, "line\n");
}

#[test]
fn resolve_send_content_rejects_body_and_text_together() {
    let err = resolve_send_content(Some("a"), Some("b"), false, unreachable_stdin).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn resolve_send_content_rejects_empty_body() {
    let err = resolve_send_content(Some(""), None, false, unreachable_stdin).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn resolve_send_content_stdin_failure_is_internal_error() {
    let err =
        resolve_send_content(None, None, false, || Err(std::io::Error::other("boom"))).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Internal);
}

#[test]
fn resolve_send_content_allows_2000_chars() {
    let text = "a".repeat(2000);
    let content = resolve_send_content(None, Some(&text), false, unreachable_stdin).unwrap();
    assert_eq!(content.chars().count(), 2000);
}

#[test]
fn resolve_send_content_rejects_2001_chars() {
    let text = "a".repeat(2001);
    let err = resolve_send_content(None, Some(&text), false, unreachable_stdin).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn resolve_send_content_counts_multibyte_chars_not_bytes() {
    // 2001 Korean syllables is 6003 UTF-8 bytes but must fail on the
    // 2001-char limit, not a byte-length check.
    let too_long = "가".repeat(2001);
    assert_eq!(too_long.len(), 6003);
    let err = resolve_send_content(None, Some(&too_long), false, unreachable_stdin).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);

    let exactly_at_limit = "가".repeat(2000);
    let content =
        resolve_send_content(None, Some(&exactly_at_limit), false, unreachable_stdin).unwrap();
    assert_eq!(content.chars().count(), 2000);
}

// ---- caption rules with files present ----

#[test]
fn resolve_send_content_files_present_no_body_gives_empty_caption_without_reading_stdin() {
    // read_stdin panics if touched: files-present + no caption must not
    // fall back to stdin (that would block an agent that pipes nothing).
    let content = resolve_send_content(None, None, true, unreachable_stdin).unwrap();
    assert_eq!(content, "");
}

#[test]
fn resolve_send_content_files_present_body_dash_reads_stdin() {
    let content =
        resolve_send_content(Some("-"), None, true, || Ok("piped caption".to_owned())).unwrap();
    assert_eq!(content, "piped caption");
}

#[test]
fn resolve_send_content_files_present_body_is_used_as_caption() {
    let content = resolve_send_content(Some("look"), None, true, unreachable_stdin).unwrap();
    assert_eq!(content, "look");
}

#[test]
fn resolve_send_content_files_present_still_enforces_2000_char_cap() {
    let text = "a".repeat(2001);
    let err = resolve_send_content(None, Some(&text), true, unreachable_stdin).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn resolve_send_content_files_present_text_flag_is_used_as_caption() {
    // Distinct from the BODY-positional case above: exercises the
    // `--text` source specifically with files present.
    let content = resolve_send_content(None, Some("caption"), true, unreachable_stdin).unwrap();
    assert_eq!(content, "caption");
}

#[test]
fn resolve_send_content_files_present_allows_2000_chars() {
    // The 2000-char boundary pass, not just the 2001 reject above, must
    // also hold when files are present.
    let text = "a".repeat(2000);
    let content = resolve_send_content(None, Some(&text), true, unreachable_stdin).unwrap();
    assert_eq!(content.chars().count(), 2000);
}

// ---- resolve_files ----

#[test]
fn resolve_file_uses_last_path_component_as_filename() {
    let part = resolve_file("/home/user/pics/photo.png", |_| Ok(vec![1, 2, 3])).unwrap();
    assert_eq!(part.filename, "photo.png");
}

#[test]
fn resolve_file_infers_mime_from_extension() {
    let part = resolve_file("photo.png", |_| Ok(vec![1])).unwrap();
    assert_eq!(part.content_type, "image/png");
}

#[test]
fn resolve_file_falls_back_to_octet_stream_without_extension() {
    let part = resolve_file("noext", |_| Ok(vec![1])).unwrap();
    assert_eq!(part.content_type, "application/octet-stream");
}

#[test]
fn resolve_file_falls_back_to_octet_stream_for_unknown_extension() {
    // Distinct from the no-extension case above: a dotted extension that
    // mime_guess does not recognize must also fall back, not error.
    let part = resolve_file("data.notarealext", |_| Ok(vec![1])).unwrap();
    assert_eq!(part.content_type, "application/octet-stream");
}

#[test]
fn resolve_file_read_error_is_usage_with_path() {
    let err = resolve_file("missing.txt", |_| Err(std::io::Error::other("nope"))).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(err.message.contains("missing.txt"));
}

#[test]
fn resolve_file_rejects_empty_file() {
    let err = resolve_file("empty.txt", |_| Ok(vec![])).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(err.message.contains("empty"));
}

#[test]
fn resolve_file_rejects_file_over_max_bytes() {
    let err = resolve_file("huge.bin", |_| Ok(vec![0u8; MAX_FILE_BYTES + 1])).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn resolve_file_accepts_exactly_max_bytes() {
    // Boundary pass complementing the over-limit reject above.
    let part = resolve_file("max.bin", |_| Ok(vec![0u8; MAX_FILE_BYTES])).unwrap();
    assert_eq!(part.bytes.len(), MAX_FILE_BYTES);
}

#[test]
fn resolve_files_rejects_more_than_ten() {
    let paths: Vec<String> = (0..11).map(|i| format!("f{i}.txt")).collect();
    let err = resolve_files(&paths, |_| Ok(vec![1])).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn resolve_files_accepts_exactly_ten() {
    let paths: Vec<String> = (0..10).map(|i| format!("f{i}.txt")).collect();
    let parts = resolve_files(&paths, |_| Ok(vec![1])).unwrap();
    assert_eq!(parts.len(), 10);
}

// ---- run_send ----

#[tokio::test]
async fn run_send_maps_sent_message_fields_to_send_data() {
    let api = MockDiscordApi::new();
    api.send_responses.borrow_mut().push_back(Ok(SentMessage {
        id: "1".into(),
        channel_id: "c".into(),
        timestamp: "2024-01-01T00:00:00Z".into(),
        attachments: vec![],
    }));

    let payload = run_send(
        &api,
        "c",
        None,
        Some("hi"),
        None,
        &[],
        unreachable_stdin,
        unreachable_read_file,
    )
    .await
    .unwrap();

    match payload {
        Payload::Send(data) => {
            assert_eq!(data.message_id, "1");
            assert_eq!(data.channel_id, "c");
            assert_eq!(data.timestamp, "2024-01-01T00:00:00Z");
            assert!(data.attachments.is_empty());
        }
        other => panic!("expected Send, got {other:?}"),
    }
}

#[tokio::test]
async fn run_send_maps_sent_message_attachments_to_send_data() {
    let api = MockDiscordApi::new();
    api.send_responses.borrow_mut().push_back(Ok(SentMessage {
        id: "1".into(),
        channel_id: "c".into(),
        timestamp: "2024-01-01T00:00:00Z".into(),
        attachments: vec![Attachment {
            id: "a1".into(),
            filename: "photo.png".into(),
            size: 1024,
            url: "https://cdn.discordapp.com/attachments/1/a1/photo.png".into(),
            content_type: Some("image/png".into()),
        }],
    }));

    let payload = run_send(
        &api,
        "c",
        None,
        Some("hi"),
        None,
        &[],
        unreachable_stdin,
        unreachable_read_file,
    )
    .await
    .unwrap();

    match payload {
        Payload::Send(data) => {
            assert_eq!(data.attachments.len(), 1);
            assert_eq!(data.attachments[0].filename, "photo.png");
        }
        other => panic!("expected Send, got {other:?}"),
    }
}

#[tokio::test]
async fn run_send_passes_reply_to_some_into_send_request() {
    let api = MockDiscordApi::new();
    api.send_responses.borrow_mut().push_back(Ok(SentMessage {
        id: "1".into(),
        channel_id: "c".into(),
        timestamp: "2024-01-01T00:00:00Z".into(),
        attachments: vec![],
    }));

    run_send(
        &api,
        "c",
        None,
        Some("hi"),
        Some("99"),
        &[],
        unreachable_stdin,
        unreachable_read_file,
    )
    .await
    .unwrap();

    let calls = api.send_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].channel_id, "c");
    assert_eq!(calls[0].content, "hi");
    assert_eq!(calls[0].reply_to.as_deref(), Some("99"));
}

#[tokio::test]
async fn run_send_passes_reply_to_none_into_send_request() {
    let api = MockDiscordApi::new();
    api.send_responses.borrow_mut().push_back(Ok(SentMessage {
        id: "1".into(),
        channel_id: "c".into(),
        timestamp: "2024-01-01T00:00:00Z".into(),
        attachments: vec![],
    }));

    run_send(
        &api,
        "c",
        None,
        Some("hi"),
        None,
        &[],
        unreachable_stdin,
        unreachable_read_file,
    )
    .await
    .unwrap();

    let calls = api.send_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].content, "hi");
    assert_eq!(calls[0].reply_to, None);
}

#[tokio::test]
async fn run_send_body_positional_content_is_independent_of_reply_to() {
    // Content resolution (BODY/--text/stdin) and reply_to are orthogonal
    // inputs; this exercises the BODY positional source together with
    // reply_to Some to prove neither leaks into the other.
    let api = MockDiscordApi::new();
    api.send_responses.borrow_mut().push_back(Ok(SentMessage {
        id: "1".into(),
        channel_id: "c".into(),
        timestamp: "2024-01-01T00:00:00Z".into(),
        attachments: vec![],
    }));

    run_send(
        &api,
        "c",
        Some("body-text"),
        None,
        Some("55"),
        &[],
        unreachable_stdin,
        unreachable_read_file,
    )
    .await
    .unwrap();

    let calls = api.send_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].content, "body-text");
    assert_eq!(calls[0].reply_to.as_deref(), Some("55"));
}

#[tokio::test]
async fn run_send_stdin_content_is_independent_of_reply_to() {
    // Same orthogonality check as the BODY-positional case above, but for
    // the stdin-read source (BODY and --text both absent).
    let api = MockDiscordApi::new();
    api.send_responses.borrow_mut().push_back(Ok(SentMessage {
        id: "1".into(),
        channel_id: "c".into(),
        timestamp: "2024-01-01T00:00:00Z".into(),
        attachments: vec![],
    }));

    run_send(
        &api,
        "c",
        None,
        None,
        Some("77"),
        &[],
        || Ok("from-stdin".to_owned()),
        unreachable_read_file,
    )
    .await
    .unwrap();

    let calls = api.send_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].content, "from-stdin");
    assert_eq!(calls[0].reply_to.as_deref(), Some("77"));
}

#[tokio::test]
async fn run_send_rejects_empty_reply_to() {
    let api = MockDiscordApi::new();
    let err = run_send(
        &api,
        "c",
        None,
        Some("hi"),
        Some(""),
        &[],
        unreachable_stdin,
        unreachable_read_file,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[tokio::test]
async fn run_send_rejects_empty_reply_to_before_touching_files() {
    // reply_to validation must short-circuit before file resolution:
    // unreachable_read_file panics if resolve_files is ever reached.
    let api = MockDiscordApi::new();
    let err = run_send(
        &api,
        "c",
        None,
        Some("hi"),
        Some(""),
        &["/tmp/photo.png".to_owned()],
        unreachable_stdin,
        unreachable_read_file,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(api.send_calls.borrow().is_empty());
}

#[tokio::test]
async fn run_send_passes_files_and_reply_to_together() {
    // Files and reply_to are independent inputs; both must land on the
    // same SendRequest without one clobbering the other.
    let api = MockDiscordApi::new();
    api.send_responses.borrow_mut().push_back(Ok(SentMessage {
        id: "1".into(),
        channel_id: "c".into(),
        timestamp: "2024-01-01T00:00:00Z".into(),
        attachments: vec![],
    }));

    run_send(
        &api,
        "c",
        None,
        Some("look"),
        Some("42"),
        &["/tmp/photo.png".to_owned()],
        unreachable_stdin,
        |_| Ok(vec![1, 2, 3]),
    )
    .await
    .unwrap();

    let calls = api.send_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].reply_to.as_deref(), Some("42"));
    assert_eq!(calls[0].files.len(), 1);
    assert_eq!(calls[0].files[0].filename, "photo.png");
    assert_eq!(calls[0].content, "look");
}

#[tokio::test]
async fn run_send_passes_resolved_files_into_send_request() {
    let api = MockDiscordApi::new();
    api.send_responses.borrow_mut().push_back(Ok(SentMessage {
        id: "1".into(),
        channel_id: "c".into(),
        timestamp: "2024-01-01T00:00:00Z".into(),
        attachments: vec![],
    }));

    run_send(
        &api,
        "c",
        None,
        None,
        None,
        &["/tmp/photo.png".to_owned()],
        unreachable_stdin,
        |_| Ok(vec![1, 2, 3]),
    )
    .await
    .unwrap();

    let calls = api.send_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].files.len(), 1);
    assert_eq!(calls[0].files[0].filename, "photo.png");
    assert_eq!(calls[0].files[0].content_type, "image/png");
    assert_eq!(calls[0].files[0].bytes.as_ref(), &[1, 2, 3]);
    // Files present with no BODY/--text yields an empty caption.
    assert_eq!(calls[0].content, "");
}
