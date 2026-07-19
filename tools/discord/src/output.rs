use serde::{Deserialize, Serialize};

use crate::api::{Attachment, Message};

/// Error taxonomy that drives both the JSON `error.kind` and the process
/// exit code (see [`exit_code`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Usage,
    Config,
    Auth,
    Api,
    RateLimit,
    Network,
    Internal,
}

impl ErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Usage => "usage",
            ErrorKind::Config => "config",
            ErrorKind::Auth => "auth",
            ErrorKind::Api => "api",
            ErrorKind::RateLimit => "rate_limit",
            ErrorKind::Network => "network",
            ErrorKind::Internal => "internal",
        }
    }
}

/// Structured failure carried through the whole app and rendered into the
/// error envelope. `http_status`/`retry_after_ms` are omitted from JSON when
/// absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            http_status: None,
            retry_after_ms: None,
        }
    }

    fn to_human(&self) -> String {
        let mut out = format!("error [{}]: {}", self.kind.as_str(), self.message);
        if let Some(status) = self.http_status {
            out.push_str(&format!(" (http {status})"));
        }
        if let Some(ms) = self.retry_after_ms {
            out.push_str(&format!(" (retry after {ms}ms)"));
        }
        out
    }
}

/// Maps a failure to the process exit code documented in the CLI contract.
pub fn exit_code(err: &AppError) -> i32 {
    match err.kind {
        ErrorKind::Usage => 2,
        ErrorKind::Config | ErrorKind::Auth => 3,
        ErrorKind::Api => 4,
        ErrorKind::RateLimit => 5,
        ErrorKind::Network => 6,
        ErrorKind::Internal => 1,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendData {
    pub message_id: String,
    pub channel_id: String,
    pub timestamp: String,
    /// Always serialized, even empty — agents should see the same shape on
    /// every send response regardless of whether attachments were sent.
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadData {
    pub channel_id: String,
    pub count: usize,
    /// Value to pass as `--after` on the next call. `null` when there is no
    /// newer cursor; never omitted.
    pub cursor: Option<String>,
    pub messages: Vec<Message>,
}

/// `init` never echoes the token — only the path it wrote and whether the
/// file was newly created vs. overwritten with `--force`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitData {
    pub path: String,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitData {
    pub channel_id: String,
    pub count: usize,
    pub cursor: Option<String>,
    /// True when the poll budget elapsed with no new message (still exit 0).
    pub timed_out: bool,
    pub messages: Vec<Message>,
}

/// Command result payload. `untagged` so each variant serializes as its inner
/// object directly under the envelope `data` key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Payload {
    Send(SendData),
    Init(InitData),
    Read(ReadData),
    Wait(WaitData),
}

impl Payload {
    fn to_human(&self) -> String {
        match self {
            Payload::Send(d) => {
                let mut text = format!(
                    "sent message {} to channel {} at {}",
                    d.message_id, d.channel_id, d.timestamp
                );
                if !d.attachments.is_empty() {
                    text.push('\n');
                    text.push_str(&format_attachment_filenames(&d.attachments));
                }
                text
            }
            Payload::Init(d) => format!("config written: {}", d.path),
            Payload::Read(d) => format!(
                "channel {}: {} message(s){}\n{}",
                d.channel_id,
                d.count,
                cursor_hint(&d.cursor),
                format_messages(&d.messages)
            ),
            Payload::Wait(d) if d.timed_out => {
                format!("channel {}: timed out, no new messages", d.channel_id)
            }
            Payload::Wait(d) => format!(
                "channel {}: {} new message(s){}\n{}",
                d.channel_id,
                d.count,
                cursor_hint(&d.cursor),
                format_messages(&d.messages)
            ),
        }
    }
}

fn cursor_hint(cursor: &Option<String>) -> String {
    match cursor {
        Some(c) => format!(" (next --after {c})"),
        None => String::new(),
    }
}

fn format_messages(messages: &[Message]) -> String {
    messages
        .iter()
        .map(format_message)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Human output lists attachment filenames only — the CDN `url` is exposed
/// in JSON output alone (see docs §5).
fn format_attachment_filenames(attachments: &[Attachment]) -> String {
    let names: Vec<&str> = attachments.iter().map(|a| a.filename.as_str()).collect();
    format!("attachments: {}", names.join(", "))
}

fn format_message(message: &Message) -> String {
    let bot = if message.author.bot { " (bot)" } else { "" };
    let mut line = format!(
        "[{}] {}{}: {}",
        message.timestamp, message.author.username, bot, message.content
    );
    if !message.attachments.is_empty() {
        line.push('\n');
        line.push_str(&format_attachment_filenames(&message.attachments));
    }
    line
}

#[derive(Serialize)]
struct SuccessEnvelope<'a> {
    ok: bool,
    command: &'a str,
    data: &'a Payload,
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    ok: bool,
    command: &'a str,
    error: &'a AppError,
}

/// Which stream a rendered line belongs on. Human-mode errors are
/// diagnostics and must not pollute stdout; every other case is a result
/// payload and belongs on stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sink {
    Stdout,
    Stderr,
}

/// Renders a command result to the stream it belongs on, the line to print,
/// and the process exit code. Human text by default; the JSON envelope when
/// `json` is set. In human mode, errors go to stderr and stdout stays empty
/// ("diagnostics on stderr"); the JSON envelope always goes to stdout.
pub fn render(
    command: &str,
    result: &Result<Payload, AppError>,
    json: bool,
) -> (Sink, String, i32) {
    match result {
        Ok(payload) => {
            let text = if json {
                success_json(command, payload)
            } else {
                payload.to_human()
            };
            (Sink::Stdout, text, 0)
        }
        Err(err) => {
            let text = if json {
                error_json(command, err)
            } else {
                err.to_human()
            };
            let sink = if json { Sink::Stdout } else { Sink::Stderr };
            (sink, text, exit_code(err))
        }
    }
}

fn success_json(command: &str, data: &Payload) -> String {
    let envelope = SuccessEnvelope {
        ok: true,
        command,
        data,
    };
    serde_json::to_string(&envelope)
        .expect("success envelope serialization is infallible for plain data")
}

fn error_json(command: &str, error: &AppError) -> String {
    let envelope = ErrorEnvelope {
        ok: false,
        command,
        error,
    };
    serde_json::to_string(&envelope)
        .expect("error envelope serialization is infallible for plain data")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Attachment, Author, Message};

    fn sample_message() -> Message {
        Message {
            id: "10".into(),
            channel_id: "chan".into(),
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

    fn sample_attachment() -> Attachment {
        Attachment {
            id: "a1".into(),
            filename: "photo.png".into(),
            size: 1024,
            url: "https://cdn.discordapp.com/attachments/1/a1/photo.png".into(),
            content_type: Some("image/png".into()),
        }
    }

    #[test]
    fn send_success_matches_contract() {
        let payload = Payload::Send(SendData {
            message_id: "123".into(),
            channel_id: "456".into(),
            timestamp: "2024-01-01T00:00:00Z".into(),
            attachments: vec![],
        });
        let (sink, json, code) = render("send", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"send","data":{"message_id":"123","channel_id":"456","timestamp":"2024-01-01T00:00:00Z","attachments":[]}}"#
        );
    }

    #[test]
    fn send_success_with_attachments_includes_full_fields() {
        let payload = Payload::Send(SendData {
            message_id: "123".into(),
            channel_id: "456".into(),
            timestamp: "2024-01-01T00:00:00Z".into(),
            attachments: vec![sample_attachment()],
        });
        let (sink, json, code) = render("send", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"send","data":{"message_id":"123","channel_id":"456","timestamp":"2024-01-01T00:00:00Z","attachments":[{"id":"a1","filename":"photo.png","size":1024,"url":"https://cdn.discordapp.com/attachments/1/a1/photo.png","content_type":"image/png"}]}}"#
        );
    }

    #[test]
    fn init_success_matches_contract() {
        let payload = Payload::Init(InitData {
            path: "/home/user/.areum/discord/config.json".into(),
            created: true,
        });
        let (sink, json, code) = render("init", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"init","data":{"path":"/home/user/.areum/discord/config.json","created":true}}"#
        );
    }

    #[test]
    fn init_overwrite_reports_created_false() {
        let payload = Payload::Init(InitData {
            path: "/home/user/.areum/discord/config.json".into(),
            created: false,
        });
        let (_, json, _) = render("init", &Ok(payload), true);
        assert!(json.contains(r#""created":false"#));
    }

    #[test]
    fn init_overwrite_success_matches_contract_exactly() {
        // init_overwrite_reports_created_false above only spot-checks the
        // `created` field; this pins the whole envelope for the --force
        // (created:false) branch the same way init_success_matches_contract
        // does for the fresh-create branch.
        let payload = Payload::Init(InitData {
            path: "/home/user/.areum/discord/config.json".into(),
            created: false,
        });
        let (sink, json, code) = render("init", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"init","data":{"path":"/home/user/.areum/discord/config.json","created":false}}"#
        );
    }

    #[test]
    fn init_error_json_matches_contract() {
        let err = AppError::new(
            ErrorKind::Usage,
            "config already exists at /home/user/.areum/discord/config.json (use --force to overwrite)",
        );
        let (sink, json, code) = render("init", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 2);
        assert_eq!(
            json,
            r#"{"ok":false,"command":"init","error":{"kind":"usage","message":"config already exists at /home/user/.areum/discord/config.json (use --force to overwrite)"}}"#
        );
    }

    #[test]
    fn init_error_human_and_json_never_contain_token_value() {
        // Mirrors init_output_never_contains_token_value_in_human_or_json but
        // for the error path: init's AppError messages are built from the
        // path/force state only, never the token, in both render modes.
        let secret = "super-secret-token-value";
        let err = AppError::new(
            ErrorKind::Usage,
            "config already exists at /home/user/.areum/discord/config.json (use --force to overwrite)",
        );
        let (_, human, _) = render("init", &Err(err.clone()), false);
        let (_, json, _) = render("init", &Err(err), true);
        assert!(!human.contains(secret));
        assert!(!json.contains(secret));
    }

    #[test]
    fn human_init_success_renders_readable_text_without_token() {
        let payload = Payload::Init(InitData {
            path: "/home/user/.areum/discord/config.json".into(),
            created: true,
        });
        let (sink, text, code) = render("init", &Ok(payload), false);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            text,
            "config written: /home/user/.areum/discord/config.json"
        );
    }

    #[test]
    fn init_output_never_contains_token_value_in_human_or_json() {
        // InitData structurally carries only path/created — this asserts the
        // no-token-leak contract holds for both render modes.
        fn init_payload() -> Payload {
            Payload::Init(InitData {
                path: "/home/user/.areum/discord/config.json".into(),
                created: true,
            })
        }
        let secret = "super-secret-token-value";
        let (_, human, _) = render("init", &Ok(init_payload()), false);
        let (_, json, _) = render("init", &Ok(init_payload()), true);
        assert!(!human.contains(secret));
        assert!(!json.contains(secret));
    }

    #[test]
    fn read_success_has_no_timed_out_field() {
        let payload = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("10".into()),
            messages: vec![sample_message()],
        });
        let (_, json, _) = render("read", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"read","data":{"channel_id":"c","count":1,"cursor":"10","messages":[{"id":"10","channel_id":"chan","author":{"id":"u1","username":"alice","bot":false},"content":"hi","timestamp":"2024-01-01T00:00:00Z","attachments":[]}]}}"#
        );
    }

    #[test]
    fn read_success_with_attachment_bearing_message_matches_contract() {
        let mut message = sample_message();
        message.attachments = vec![Attachment {
            id: "a2".into(),
            filename: "notes.txt".into(),
            size: 42,
            url: "https://cdn.discordapp.com/attachments/1/a2/notes.txt".into(),
            content_type: None,
        }];
        let payload = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("10".into()),
            messages: vec![message],
        });
        let (_, json, _) = render("read", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"read","data":{"channel_id":"c","count":1,"cursor":"10","messages":[{"id":"10","channel_id":"chan","author":{"id":"u1","username":"alice","bot":false},"content":"hi","timestamp":"2024-01-01T00:00:00Z","attachments":[{"id":"a2","filename":"notes.txt","size":42,"url":"https://cdn.discordapp.com/attachments/1/a2/notes.txt"}]}]}}"#
        );
    }

    #[test]
    fn read_cursor_none_serializes_as_null() {
        let payload = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 0,
            cursor: None,
            messages: vec![],
        });
        let (_, json, _) = render("read", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"read","data":{"channel_id":"c","count":0,"cursor":null,"messages":[]}}"#
        );
    }

    #[test]
    fn wait_success_includes_timed_out() {
        let payload = Payload::Wait(WaitData {
            channel_id: "c".into(),
            count: 0,
            cursor: None,
            timed_out: true,
            messages: vec![],
        });
        let (sink, json, code) = render("wait", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"wait","data":{"channel_id":"c","count":0,"cursor":null,"timed_out":true,"messages":[]}}"#
        );
    }

    #[test]
    fn wait_success_with_attachment_bearing_message_matches_contract() {
        let mut message = sample_message();
        message.attachments = vec![sample_attachment()];
        let payload = Payload::Wait(WaitData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("20".into()),
            timed_out: false,
            messages: vec![message],
        });
        let (_, json, _) = render("wait", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"wait","data":{"channel_id":"c","count":1,"cursor":"20","timed_out":false,"messages":[{"id":"10","channel_id":"chan","author":{"id":"u1","username":"alice","bot":false},"content":"hi","timestamp":"2024-01-01T00:00:00Z","attachments":[{"id":"a1","filename":"photo.png","size":1024,"url":"https://cdn.discordapp.com/attachments/1/a1/photo.png","content_type":"image/png"}]}]}}"#
        );
    }

    #[test]
    fn error_omits_absent_optionals() {
        let err = AppError::new(ErrorKind::Config, "no bot token");
        let (sink, json, code) = render("send", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 3);
        assert_eq!(
            json,
            r#"{"ok":false,"command":"send","error":{"kind":"config","message":"no bot token"}}"#
        );
    }

    #[test]
    fn error_includes_http_status_and_retry() {
        let err = AppError {
            kind: ErrorKind::RateLimit,
            message: "rate limited".into(),
            http_status: Some(429),
            retry_after_ms: Some(1200),
        };
        let (sink, json, code) = render("read", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 5);
        assert_eq!(
            json,
            r#"{"ok":false,"command":"read","error":{"kind":"rate_limit","message":"rate limited","http_status":429,"retry_after_ms":1200}}"#
        );
    }

    #[test]
    fn exit_code_maps_every_kind() {
        assert_eq!(exit_code(&AppError::new(ErrorKind::Usage, "")), 2);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Config, "")), 3);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Auth, "")), 3);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Api, "")), 4);
        assert_eq!(exit_code(&AppError::new(ErrorKind::RateLimit, "")), 5);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Network, "")), 6);
        assert_eq!(exit_code(&AppError::new(ErrorKind::Internal, "")), 1);
    }

    #[test]
    fn human_error_reports_kind_and_status() {
        let err = AppError {
            kind: ErrorKind::Auth,
            message: "invalid token".into(),
            http_status: Some(401),
            retry_after_ms: None,
        };
        let (sink, text, code) = render("send", &Err(err), false);
        assert_eq!(sink, Sink::Stderr);
        assert_eq!(code, 3);
        assert_eq!(text, "error [auth]: invalid token (http 401)");
    }

    #[test]
    fn human_mode_error_routes_to_stderr_not_stdout() {
        // "diagnostics on stderr": a human-mode error must never appear as
        // the stdout line, since agents piping stdout would see nothing.
        let err = AppError::new(ErrorKind::Api, "channel not found");
        let (sink, _, _) = render("read", &Err(err), false);
        assert_eq!(sink, Sink::Stderr);
    }

    #[test]
    fn json_mode_error_still_routes_to_stdout() {
        let err = AppError::new(ErrorKind::Api, "channel not found");
        let (sink, _, _) = render("read", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
    }

    #[test]
    fn human_send_success_renders_readable_text() {
        let payload = Payload::Send(SendData {
            message_id: "123".into(),
            channel_id: "456".into(),
            timestamp: "2024-01-01T00:00:00Z".into(),
            attachments: vec![],
        });
        let (sink, text, code) = render("send", &Ok(payload), false);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            text,
            "sent message 123 to channel 456 at 2024-01-01T00:00:00Z"
        );
    }

    #[test]
    fn human_send_success_with_attachments_lists_filenames_not_urls() {
        let payload = Payload::Send(SendData {
            message_id: "123".into(),
            channel_id: "456".into(),
            timestamp: "2024-01-01T00:00:00Z".into(),
            attachments: vec![
                sample_attachment(),
                Attachment {
                    id: "a2".into(),
                    filename: "notes.txt".into(),
                    size: 42,
                    url: "https://cdn.discordapp.com/attachments/1/a2/notes.txt".into(),
                    content_type: None,
                },
            ],
        });
        let (_, text, code) = render("send", &Ok(payload), false);
        assert_eq!(code, 0);
        assert_eq!(
            text,
            "sent message 123 to channel 456 at 2024-01-01T00:00:00Z\nattachments: photo.png, notes.txt"
        );
        assert!(!text.contains("cdn.discordapp.com"));
    }

    #[test]
    fn human_read_success_renders_readable_text() {
        let payload = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("10".into()),
            messages: vec![sample_message()],
        });
        let (_, text, code) = render("read", &Ok(payload), false);
        assert_eq!(code, 0);
        assert_eq!(
            text,
            "channel c: 1 message(s) (next --after 10)\n[2024-01-01T00:00:00Z] alice: hi"
        );
    }

    #[test]
    fn human_wait_new_messages_renders_readable_text() {
        let payload = Payload::Wait(WaitData {
            channel_id: "c".into(),
            count: 1,
            cursor: None,
            timed_out: false,
            messages: vec![sample_message()],
        });
        let (_, text, code) = render("wait", &Ok(payload), false);
        assert_eq!(code, 0);
        assert_eq!(
            text,
            "channel c: 1 new message(s)\n[2024-01-01T00:00:00Z] alice: hi"
        );
    }

    #[test]
    fn human_read_success_with_attachment_lists_filename_not_url() {
        let mut message = sample_message();
        message.attachments = vec![sample_attachment()];
        let payload = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("10".into()),
            messages: vec![message],
        });
        let (_, text, code) = render("read", &Ok(payload), false);
        assert_eq!(code, 0);
        assert_eq!(
            text,
            "channel c: 1 message(s) (next --after 10)\n[2024-01-01T00:00:00Z] alice: hi\nattachments: photo.png"
        );
        assert!(!text.contains("cdn.discordapp.com"));
    }

    #[test]
    fn human_read_success_without_attachments_is_unchanged() {
        let payload = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("10".into()),
            messages: vec![sample_message()],
        });
        let (_, text, _) = render("read", &Ok(payload), false);
        assert_eq!(
            text,
            "channel c: 1 message(s) (next --after 10)\n[2024-01-01T00:00:00Z] alice: hi"
        );
    }

    #[test]
    fn human_wait_timeout_renders_readable_text() {
        let payload = Payload::Wait(WaitData {
            channel_id: "c".into(),
            count: 0,
            cursor: None,
            timed_out: true,
            messages: vec![],
        });
        let (sink, text, code) = render("wait", &Ok(payload), false);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(text, "channel c: timed out, no new messages");
    }

    #[test]
    fn json_read_success_routes_to_stdout_with_exit_zero() {
        let payload = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("10".into()),
            messages: vec![sample_message()],
        });
        let (sink, _, code) = render("read", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
    }

    #[test]
    fn json_wait_success_with_attachments_routes_to_stdout_with_exit_zero() {
        let mut message = sample_message();
        message.attachments = vec![sample_attachment()];
        let payload = Payload::Wait(WaitData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("20".into()),
            timed_out: false,
            messages: vec![message],
        });
        let (sink, _, code) = render("wait", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
    }

    #[test]
    fn human_success_output_never_contains_json_envelope_marker() {
        // json=false success must be plain text, never the {"ok":...}
        // envelope shape — this is the boundary agents rely on to tell modes
        // apart, so assert its absence directly rather than only asserting
        // the expected human string.
        let send = Payload::Send(SendData {
            message_id: "1".into(),
            channel_id: "c".into(),
            timestamp: "2024-01-01T00:00:00Z".into(),
            attachments: vec![],
        });
        let read = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("10".into()),
            messages: vec![sample_message()],
        });
        let wait = Payload::Wait(WaitData {
            channel_id: "c".into(),
            count: 0,
            cursor: None,
            timed_out: true,
            messages: vec![],
        });
        let init = Payload::Init(InitData {
            path: "/home/user/.areum/discord/config.json".into(),
            created: true,
        });

        for payload in [send, read, wait, init] {
            let (sink, text, code) = render("cmd", &Ok(payload), false);
            assert_eq!(sink, Sink::Stdout);
            assert_eq!(code, 0);
            assert!(
                !text.contains(r#"{"ok"#),
                "human output leaked json envelope: {text}"
            );
        }
    }

    #[test]
    fn human_read_success_routes_to_stdout() {
        let payload = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("10".into()),
            messages: vec![sample_message()],
        });
        let (sink, _, code) = render("read", &Ok(payload), false);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
    }

    #[test]
    fn human_wait_success_routes_to_stdout() {
        let payload = Payload::Wait(WaitData {
            channel_id: "c".into(),
            count: 1,
            cursor: None,
            timed_out: false,
            messages: vec![sample_message()],
        });
        let (sink, _, code) = render("wait", &Ok(payload), false);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
    }

    #[test]
    fn human_wait_success_with_attachment_lists_filename_not_url() {
        // Regression coverage matching the send/read filename-not-url
        // contract, but for wait specifically — no prior test exercised
        // this combination.
        let mut message = sample_message();
        message.attachments = vec![sample_attachment()];
        let payload = Payload::Wait(WaitData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("20".into()),
            timed_out: false,
            messages: vec![message],
        });
        let (_, text, code) = render("wait", &Ok(payload), false);
        assert_eq!(code, 0);
        assert_eq!(
            text,
            "channel c: 1 new message(s) (next --after 20)\n[2024-01-01T00:00:00Z] alice: hi\nattachments: photo.png"
        );
        assert!(!text.contains("cdn.discordapp.com"));
    }

    #[test]
    fn exit_code_matches_across_json_and_human_modes_for_every_kind() {
        // The CLI contract promises identical exit codes regardless of
        // --json; verify render() itself preserves that for every error
        // kind, not just the standalone exit_code() mapping function.
        let kinds = [
            ErrorKind::Usage,
            ErrorKind::Config,
            ErrorKind::Auth,
            ErrorKind::Api,
            ErrorKind::RateLimit,
            ErrorKind::Network,
            ErrorKind::Internal,
        ];
        for kind in kinds {
            let err = AppError::new(kind, "boom");
            let (_, _, json_code) = render("send", &Err(err.clone()), true);
            let (_, _, human_code) = render("send", &Err(err), false);
            assert_eq!(
                json_code, human_code,
                "exit code diverged between modes for {kind:?}"
            );
        }
    }
}
