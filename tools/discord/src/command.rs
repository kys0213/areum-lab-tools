use std::time::Duration;

use crate::api::{DiscordApi, FilePart, Message, SendRequest};
use crate::output::{AppError, ErrorKind, Payload, ReadData, SendData, WaitData};

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

/// Sends a message (optionally with file attachments) to a channel and reports
/// the created message.
///
/// Over clippy's arg ceiling by one: the extra parameters are the two I/O
/// seams (`read_stdin`, `read_file`) kept injectable for black-box tests
/// rather than reaching for `std` directly — bundling them into a struct would
/// obscure that intent.
#[allow(clippy::too_many_arguments)]
pub async fn run_send(
    api: &impl DiscordApi,
    channel_id: &str,
    body: Option<&str>,
    text: Option<&str>,
    reply_to: Option<&str>,
    file_paths: &[String],
    read_stdin: impl FnOnce() -> std::io::Result<String>,
    read_file: impl Fn(&str) -> std::io::Result<Vec<u8>>,
) -> Result<Payload, AppError> {
    // An explicit empty --reply-to is a caller mistake, not "no reply": reject
    // it rather than sending message_reference with an empty message_id.
    if reply_to == Some("") {
        return Err(AppError::new(
            ErrorKind::Usage,
            "--reply-to must not be an empty string",
        ));
    }
    let files = resolve_files(file_paths, read_file)?;
    let content = resolve_send_content(body, text, !files.is_empty(), read_stdin)?;
    let req = SendRequest {
        channel_id: channel_id.to_owned(),
        content,
        reply_to: reply_to.map(str::to_owned),
        files,
    };
    let sent = api.send_message(&req).await?;
    Ok(Payload::Send(SendData {
        message_id: sent.id,
        channel_id: sent.channel_id,
        timestamp: sent.timestamp,
        attachments: sent.attachments,
    }))
}

/// Discord rejects empty messages and caps content at 2000 codepoints.
const MAX_BODY_CHARS: usize = 2000;

/// Discord allows at most 10 attachments per message.
const MAX_FILES: usize = 10;

/// Files are read fully into memory before upload, so this bounds how much a
/// single file can pull into RAM before Discord's own 413 would reject it. It
/// is a sanity ceiling, not a spec match — it aligns with Discord's highest
/// boost-tier per-file limit (100 MiB).
const MAX_FILE_BYTES: usize = 100 * 1024 * 1024;

/// Resolves the outgoing caption. BODY vs --text stay mutually exclusive and
/// the 2000-codepoint cap always applies. Emptiness handling depends on files:
/// with attachments present an empty caption is allowed and stdin is NOT read;
/// without attachments the pre-file behaviour is unchanged (empty rejected,
/// stdin read when BODY is `None` or `"-"`).
fn resolve_send_content(
    body: Option<&str>,
    text: Option<&str>,
    has_files: bool,
    read_stdin: impl FnOnce() -> std::io::Result<String>,
) -> Result<String, AppError> {
    if body.is_some() && text.is_some() {
        return Err(AppError::new(
            ErrorKind::Usage,
            "BODY and --text are mutually exclusive; pass only one",
        ));
    }

    let content = match (body, text) {
        (_, Some(text)) => text.to_owned(),
        (Some("-"), None) => read_stdin_to_string(read_stdin)?,
        // Files present with no explicit caption: send an empty caption rather
        // than blocking on stdin (the agent isn't piping one in).
        (None, None) if has_files => String::new(),
        (None, None) => read_stdin_to_string(read_stdin)?,
        (Some(body), None) => body.to_owned(),
    };

    validate_body(&content, has_files)?;
    Ok(content)
}

fn read_stdin_to_string(
    read_stdin: impl FnOnce() -> std::io::Result<String>,
) -> Result<String, AppError> {
    read_stdin()
        .map_err(|e| AppError::new(ErrorKind::Internal, format!("failed to read stdin: {e}")))
}

fn validate_body(content: &str, has_files: bool) -> Result<(), AppError> {
    if content.is_empty() && !has_files {
        return Err(AppError::new(
            ErrorKind::Usage,
            "message body must not be empty",
        ));
    }
    let len = content.chars().count();
    if len > MAX_BODY_CHARS {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!("message body exceeds {MAX_BODY_CHARS} characters (got {len})"),
        ));
    }
    Ok(())
}

/// Reads and validates each attachment path into a [`FilePart`]. All failures
/// map to usage errors (exit 2) with the offending path in the message, since
/// they are caller-side mistakes caught before any network call.
fn resolve_files(
    file_paths: &[String],
    read_file: impl Fn(&str) -> std::io::Result<Vec<u8>>,
) -> Result<Vec<FilePart>, AppError> {
    if file_paths.len() > MAX_FILES {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!(
                "at most {MAX_FILES} files per message (got {})",
                file_paths.len()
            ),
        ));
    }
    file_paths
        .iter()
        .map(|path| resolve_file(path, &read_file))
        .collect()
}

fn resolve_file(
    path: &str,
    read_file: impl Fn(&str) -> std::io::Result<Vec<u8>>,
) -> Result<FilePart, AppError> {
    let bytes = read_file(path)
        .map_err(|e| AppError::new(ErrorKind::Usage, format!("cannot read file '{path}': {e}")))?;
    if bytes.is_empty() {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!("file '{path}' is empty; nothing to upload"),
        ));
    }
    if bytes.len() > MAX_FILE_BYTES {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!(
                "file '{path}' exceeds {MAX_FILE_BYTES} bytes (got {})",
                bytes.len()
            ),
        ));
    }
    let filename = file_name_of(path);
    let content_type = mime_guess::from_path(&filename)
        .first_or_octet_stream()
        .to_string();
    Ok(FilePart {
        filename,
        bytes: bytes::Bytes::from(bytes),
        content_type,
    })
}

/// The last path component — Discord (and other viewers) should see the file's
/// name, not the caller's full local path.
fn file_name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

/// Reads recent messages. T2 owns cursor derivation and the `Payload::Read`
/// assembly around the fetched messages.
pub async fn run_read(
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

/// Discord caps `limit` at 100; pushing the bound down to a usage error keeps
/// the failure in the same envelope class as other client-side mistakes
/// instead of surfacing as an opaque 400 from the API.
fn validate_limit(limit: u8) -> Result<(), AppError> {
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
fn parse_snowflake(id: &str) -> Result<u64, AppError> {
    id.parse::<u64>().map_err(|_| {
        AppError::new(
            ErrorKind::Api,
            format!("message id '{id}' is not a valid u64 snowflake"),
        )
    })
}

/// Discord returns messages newest-first; the CLI contract is oldest-first.
fn sort_ascending_by_id(messages: Vec<Message>) -> Result<Vec<Message>, AppError> {
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
fn newest_cursor(sorted_ascending: &[Message], after: Option<&str>) -> Option<String> {
    match sorted_ascending.last() {
        Some(newest) => Some(newest.id.clone()),
        None => after.map(str::to_owned),
    }
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
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;
    use crate::api::{Attachment, Author, SentMessage};

    fn unreachable_read_file(_: &str) -> std::io::Result<Vec<u8>> {
        panic!("read_file should not be called")
    }

    type GetCall = (String, Option<String>, u8);

    /// Scripted [`DiscordApi`]: each call pops the next queued response and
    /// records the arguments it was invoked with.
    struct MockDiscordApi {
        send_responses: RefCell<VecDeque<Result<SentMessage, AppError>>>,
        send_calls: RefCell<Vec<SendRequest>>,
        get_responses: RefCell<VecDeque<Result<Vec<Message>, AppError>>>,
        get_calls: RefCell<Vec<GetCall>>,
    }

    impl MockDiscordApi {
        fn new() -> Self {
            Self {
                send_responses: RefCell::new(VecDeque::new()),
                send_calls: RefCell::new(Vec::new()),
                get_responses: RefCell::new(VecDeque::new()),
                get_calls: RefCell::new(Vec::new()),
            }
        }

        fn with_get_responses(responses: Vec<Result<Vec<Message>, AppError>>) -> Self {
            let api = Self::new();
            *api.get_responses.borrow_mut() = responses.into_iter().collect();
            api
        }
    }

    impl DiscordApi for MockDiscordApi {
        async fn send_message(&self, req: &SendRequest) -> Result<SentMessage, AppError> {
            self.send_calls.borrow_mut().push(req.clone());
            self.send_responses
                .borrow_mut()
                .pop_front()
                .expect("test must queue a send response before calling send_message")
        }

        async fn get_messages(
            &self,
            channel_id: &str,
            after: Option<&str>,
            limit: u8,
        ) -> Result<Vec<Message>, AppError> {
            self.get_calls.borrow_mut().push((
                channel_id.to_owned(),
                after.map(str::to_owned),
                limit,
            ));
            self.get_responses
                .borrow_mut()
                .pop_front()
                .expect("test must queue a get_messages response before calling get_messages")
        }
    }

    /// Records sleep durations instead of actually waiting, so `wait` tests
    /// run instantly and assert poll cadence deterministically.
    struct FakeSleeper {
        calls: RefCell<Vec<Duration>>,
    }

    impl FakeSleeper {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl Sleeper for FakeSleeper {
        async fn sleep(&self, dur: Duration) {
            self.calls.borrow_mut().push(dur);
        }
    }

    fn message(id: &str) -> Message {
        Message {
            id: id.to_owned(),
            channel_id: "c".into(),
            author: Author {
                id: "u1".into(),
                username: "alice".into(),
                bot: false,
            },
            content: "hi".into(),
            timestamp: "2024-01-01T00:00:00Z".into(),
            attachments: vec![],
        }
    }

    fn unreachable_stdin() -> std::io::Result<String> {
        panic!("stdin should not be read")
    }

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
        let content =
            resolve_send_content(None, None, false, || Ok("from stdin".to_owned())).unwrap();
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
        let err = resolve_send_content(None, None, false, || Err(std::io::Error::other("boom")))
            .unwrap_err();
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
        let err =
            resolve_send_content(None, Some(&too_long), false, unreachable_stdin).unwrap_err();
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

    // ---- run_read ----

    #[tokio::test]
    async fn run_read_sorts_descending_input_to_ascending_output() {
        let api = MockDiscordApi::with_get_responses(vec![Ok(vec![
            message("30"),
            message("10"),
            message("20"),
        ])]);

        let payload = run_read(&api, "c", None, 50).await.unwrap();

        match payload {
            Payload::Read(data) => {
                let ids: Vec<&str> = data.messages.iter().map(|m| m.id.as_str()).collect();
                assert_eq!(ids, vec!["10", "20", "30"]);
                assert_eq!(data.count, 3);
                assert_eq!(data.cursor.as_deref(), Some("30"));
            }
            other => panic!("expected Read, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_read_cursor_is_numeric_max_not_lexical_max() {
        // Lexical comparison would pick "9" over "10"; numeric must pick "10".
        let api = MockDiscordApi::with_get_responses(vec![Ok(vec![message("9"), message("10")])]);

        let payload = run_read(&api, "c", None, 50).await.unwrap();

        match payload {
            Payload::Read(data) => {
                let ids: Vec<&str> = data.messages.iter().map(|m| m.id.as_str()).collect();
                assert_eq!(ids, vec!["9", "10"]);
                assert_eq!(data.cursor.as_deref(), Some("10"));
            }
            other => panic!("expected Read, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_read_empty_result_echoes_input_after_when_some() {
        let api = MockDiscordApi::with_get_responses(vec![Ok(vec![])]);

        let payload = run_read(&api, "c", Some("42"), 50).await.unwrap();

        match payload {
            Payload::Read(data) => {
                assert_eq!(data.count, 0);
                assert_eq!(data.cursor.as_deref(), Some("42"));
            }
            other => panic!("expected Read, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_read_empty_result_with_no_after_has_none_cursor() {
        let api = MockDiscordApi::with_get_responses(vec![Ok(vec![])]);

        let payload = run_read(&api, "c", None, 50).await.unwrap();

        match payload {
            Payload::Read(data) => assert_eq!(data.cursor, None),
            other => panic!("expected Read, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_read_rejects_limit_zero() {
        let api = MockDiscordApi::new();
        let err = run_read(&api, "c", None, 0).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage);
    }

    #[tokio::test]
    async fn run_read_rejects_limit_over_100() {
        let api = MockDiscordApi::new();
        let err = run_read(&api, "c", None, 101).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage);
    }

    #[tokio::test]
    async fn run_read_accepts_limit_boundaries_1_and_100() {
        let api = MockDiscordApi::with_get_responses(vec![Ok(vec![]), Ok(vec![])]);

        assert!(run_read(&api, "c", None, 1).await.is_ok());
        assert!(run_read(&api, "c", None, 100).await.is_ok());
    }

    #[tokio::test]
    async fn run_read_rejects_non_numeric_message_id_as_api_error() {
        // A non-snowflake id in the response is an API contract violation,
        // not a client-side usage mistake.
        let api = MockDiscordApi::with_get_responses(vec![Ok(vec![message("not-a-number")])]);

        let err = run_read(&api, "c", None, 50).await.unwrap_err();

        assert_eq!(err.kind, ErrorKind::Api);
    }

    #[tokio::test]
    async fn run_read_passes_channel_after_and_limit_to_api() {
        let api = MockDiscordApi::with_get_responses(vec![Ok(vec![])]);

        run_read(&api, "chan-1", Some("42"), 7).await.unwrap();

        let calls = api.get_calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], ("chan-1".to_owned(), Some("42".to_owned()), 7));
    }

    // ---- run_wait ----

    #[tokio::test]
    async fn run_wait_returns_immediately_when_first_poll_has_messages() {
        let api = MockDiscordApi::with_get_responses(vec![Ok(vec![message("10")])]);
        let sleeper = FakeSleeper::new();

        let payload = run_wait(&api, &sleeper, "c", None, 30, 5, 50)
            .await
            .unwrap();

        match payload {
            Payload::Wait(data) => {
                assert!(!data.timed_out);
                assert_eq!(data.count, 1);
                assert_eq!(data.cursor.as_deref(), Some("10"));
            }
            other => panic!("expected Wait, got {other:?}"),
        }
        assert_eq!(sleeper.calls.borrow().len(), 0);
    }

    #[tokio::test]
    async fn run_wait_sorts_and_derives_cursor_same_as_read_when_found() {
        // Wait reuses sort_ascending_by_id/newest_cursor; verify the found
        // path applies the same ordering and cursor rule as `read`.
        let api = MockDiscordApi::with_get_responses(vec![Ok(vec![
            message("30"),
            message("10"),
            message("20"),
        ])]);
        let sleeper = FakeSleeper::new();

        let payload = run_wait(&api, &sleeper, "c", None, 30, 5, 50)
            .await
            .unwrap();

        match payload {
            Payload::Wait(data) => {
                let ids: Vec<&str> = data.messages.iter().map(|m| m.id.as_str()).collect();
                assert_eq!(ids, vec!["10", "20", "30"]);
                assert_eq!(data.count, 3);
                assert_eq!(data.cursor.as_deref(), Some("30"));
            }
            other => panic!("expected Wait, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_wait_sleeps_n_minus_1_times_before_nth_poll_has_messages() {
        let api = MockDiscordApi::with_get_responses(vec![
            Ok(vec![]),
            Ok(vec![]),
            Ok(vec![message("5")]),
        ]);
        let sleeper = FakeSleeper::new();

        let payload = run_wait(&api, &sleeper, "c", None, 20, 5, 50)
            .await
            .unwrap();

        match payload {
            Payload::Wait(data) => assert!(!data.timed_out),
            other => panic!("expected Wait, got {other:?}"),
        }
        assert_eq!(sleeper.calls.borrow().len(), 2);
    }

    #[tokio::test]
    async fn run_wait_times_out_after_ceil_polls_all_empty() {
        // timeout 10 / interval 3 -> ceil(10/3) = 4 polls, 3 sleeps between them.
        let api = MockDiscordApi::with_get_responses(vec![
            Ok(vec![]),
            Ok(vec![]),
            Ok(vec![]),
            Ok(vec![]),
        ]);
        let sleeper = FakeSleeper::new();

        let payload = run_wait(&api, &sleeper, "c", Some("7"), 10, 3, 50)
            .await
            .unwrap();

        match payload {
            Payload::Wait(data) => {
                assert!(data.timed_out);
                assert_eq!(data.count, 0);
                assert!(data.messages.is_empty());
                assert_eq!(data.cursor.as_deref(), Some("7"));
            }
            other => panic!("expected Wait, got {other:?}"),
        }
        assert_eq!(api.get_calls.borrow().len(), 4);
        assert_eq!(sleeper.calls.borrow().len(), 3);
    }

    #[tokio::test]
    async fn run_wait_poll_count_is_exact_when_timeout_divides_interval_evenly() {
        // timeout 10 / interval 5 -> ceil(10/5) = 2 polls, 1 sleep between them.
        let api = MockDiscordApi::with_get_responses(vec![Ok(vec![]), Ok(vec![])]);
        let sleeper = FakeSleeper::new();

        let payload = run_wait(&api, &sleeper, "c", None, 10, 5, 50)
            .await
            .unwrap();

        match payload {
            Payload::Wait(data) => assert!(data.timed_out),
            other => panic!("expected Wait, got {other:?}"),
        }
        assert_eq!(api.get_calls.borrow().len(), 2);
        assert_eq!(sleeper.calls.borrow().len(), 1);
    }

    #[tokio::test]
    async fn run_wait_polls_at_least_once_when_timeout_is_zero() {
        // timeout 0 / interval 5 -> ceil(0/5) = 0, but max_polls floors at 1
        // poll so `wait` always checks at least once.
        let api = MockDiscordApi::with_get_responses(vec![Ok(vec![])]);
        let sleeper = FakeSleeper::new();

        let payload = run_wait(&api, &sleeper, "c", None, 0, 5, 50).await.unwrap();

        match payload {
            Payload::Wait(data) => assert!(data.timed_out),
            other => panic!("expected Wait, got {other:?}"),
        }
        assert_eq!(api.get_calls.borrow().len(), 1);
        assert_eq!(sleeper.calls.borrow().len(), 0);
    }

    #[tokio::test]
    async fn run_wait_rejects_interval_zero() {
        let api = MockDiscordApi::new();
        let sleeper = FakeSleeper::new();

        let err = run_wait(&api, &sleeper, "c", None, 10, 0, 50)
            .await
            .unwrap_err();

        assert_eq!(err.kind, ErrorKind::Usage);
    }

    #[tokio::test]
    async fn run_wait_rejects_limit_out_of_range() {
        let api = MockDiscordApi::new();
        let sleeper = FakeSleeper::new();

        let err = run_wait(&api, &sleeper, "c", None, 10, 5, 0)
            .await
            .unwrap_err();

        assert_eq!(err.kind, ErrorKind::Usage);
    }

    #[tokio::test]
    async fn run_wait_propagates_api_error_immediately() {
        let api =
            MockDiscordApi::with_get_responses(vec![Err(AppError::new(ErrorKind::Api, "boom"))]);
        let sleeper = FakeSleeper::new();

        let err = run_wait(&api, &sleeper, "c", None, 30, 5, 50)
            .await
            .unwrap_err();

        assert_eq!(err.kind, ErrorKind::Api);
        assert_eq!(sleeper.calls.borrow().len(), 0);
    }
}
