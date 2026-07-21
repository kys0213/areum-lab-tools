use serde::{Deserialize, Serialize};

use crate::common::api::{Attachment, Message};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadData {
    pub thread_id: String,
    pub name: String,
}

/// `daemon start` result. `foreground` distinguishes a background launch
/// (reports the detached child's pid) from a foreground run (reports this
/// process's pid, emitted on graceful shutdown).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStartData {
    pub pid: u32,
    pub foreground: bool,
}

/// `daemon stop` result — the pid that was signalled and confirmed exited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStopData {
    pub pid: u32,
    pub stopped: bool,
}

/// `daemon status` result. `pid` is present only when a live daemon is
/// recorded; `pending` is the outstanding-ask backlog either way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatusData {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub pending: usize,
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
    Thread(ThreadData),
    DaemonStart(DaemonStartData),
    DaemonStop(DaemonStopData),
    DaemonStatus(DaemonStatusData),
}

impl Payload {
    // Called from `crate::output::render`, the parent module.
    pub(super) fn to_human(&self) -> String {
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
            Payload::Thread(d) => format!("created thread {} \"{}\"", d.thread_id, d.name),
            Payload::DaemonStart(d) if d.foreground => {
                format!("daemon exited (pid {})", d.pid)
            }
            Payload::DaemonStart(d) => {
                format!("daemon started in background (pid {})", d.pid)
            }
            Payload::DaemonStop(d) => format!("daemon stopped (pid {})", d.pid),
            Payload::DaemonStatus(d) if d.running => format!(
                "daemon running (pid {}), {} pending ask(s)",
                d.pid.unwrap_or(0),
                d.pending
            ),
            Payload::DaemonStatus(d) => {
                format!("daemon not running, {} pending ask(s)", d.pending)
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::api::{Attachment, Author, Message};
    use crate::output::{Sink, render};

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
    fn human_thread_success_renders_readable_text() {
        let payload = Payload::Thread(ThreadData {
            thread_id: "111".into(),
            name: "discussion".into(),
        });
        let (sink, text, code) = render("thread", &Ok(payload), false);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(text, "created thread 111 \"discussion\"");
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
}
