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
    format!(
        "[{}] {}{}: {}",
        message.timestamp, message.author.username, bot, message.content
    )
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

/// Renders a command result to the string printed on stdout plus the process
/// exit code. JSON by default; text when `human` is set.
pub fn render(command: &str, result: &Result<Payload, AppError>, human: bool) -> (String, i32) {
    match result {
        Ok(payload) => {
            let text = if human {
                payload.to_human()
            } else {
                success_json(command, payload)
            };
            (text, 0)
        }
        Err(err) => {
            let text = if human {
                err.to_human()
            } else {
                error_json(command, err)
            };
            (text, exit_code(err))
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
        let (json, code) = render("send", &Ok(payload), false);
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
        let (json, code) = render("send", &Ok(payload), false);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"send","data":{"message_id":"123","channel_id":"456","timestamp":"2024-01-01T00:00:00Z","attachments":[{"id":"a1","filename":"photo.png","size":1024,"url":"https://cdn.discordapp.com/attachments/1/a1/photo.png","content_type":"image/png"}]}}"#
        );
    }

    #[test]
    fn read_success_has_no_timed_out_field() {
        let payload = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("10".into()),
            messages: vec![sample_message()],
        });
        let (json, _) = render("read", &Ok(payload), false);
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
        let (json, _) = render("read", &Ok(payload), false);
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
        let (json, _) = render("read", &Ok(payload), false);
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
        let (json, code) = render("wait", &Ok(payload), false);
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
        let (json, _) = render("wait", &Ok(payload), false);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"wait","data":{"channel_id":"c","count":1,"cursor":"20","timed_out":false,"messages":[{"id":"10","channel_id":"chan","author":{"id":"u1","username":"alice","bot":false},"content":"hi","timestamp":"2024-01-01T00:00:00Z","attachments":[{"id":"a1","filename":"photo.png","size":1024,"url":"https://cdn.discordapp.com/attachments/1/a1/photo.png","content_type":"image/png"}]}]}}"#
        );
    }

    #[test]
    fn error_omits_absent_optionals() {
        let err = AppError::new(ErrorKind::Config, "no bot token");
        let (json, code) = render("send", &Err(err), false);
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
        let (json, code) = render("read", &Err(err), false);
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
        let (text, code) = render("send", &Err(err), true);
        assert_eq!(code, 3);
        assert_eq!(text, "error [auth]: invalid token (http 401)");
    }

    #[test]
    fn human_send_success_renders_readable_text() {
        let payload = Payload::Send(SendData {
            message_id: "123".into(),
            channel_id: "456".into(),
            timestamp: "2024-01-01T00:00:00Z".into(),
            attachments: vec![],
        });
        let (text, code) = render("send", &Ok(payload), true);
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
        let (text, code) = render("send", &Ok(payload), true);
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
        let (text, code) = render("read", &Ok(payload), true);
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
        let (text, code) = render("wait", &Ok(payload), true);
        assert_eq!(code, 0);
        assert_eq!(
            text,
            "channel c: 1 new message(s)\n[2024-01-01T00:00:00Z] alice: hi"
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
        let (text, code) = render("wait", &Ok(payload), true);
        assert_eq!(code, 0);
        assert_eq!(text, "channel c: timed out, no new messages");
    }
}
