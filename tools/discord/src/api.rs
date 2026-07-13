use serde::{Deserialize, Serialize};

use crate::output::AppError;

/// Abstraction over the Discord REST calls the CLI needs.
///
/// Non-pub + static dispatch (`&impl DiscordApi`) on purpose: keeps the
/// `async_fn_in_trait` lint quiet and lets `command` stay testable with a mock.
pub(crate) trait DiscordApi {
    async fn send_message(&self, req: &SendRequest) -> Result<SentMessage, AppError>;

    async fn get_messages(
        &self,
        channel_id: &str,
        after: Option<&str>,
        limit: u8,
    ) -> Result<Vec<Message>, AppError>;
}

/// Request for `POST /channels/{channel_id}/messages`. Plain struct (not a
/// builder) on purpose — follow-up work (file attachments) extends this by
/// adding a field, not by changing the shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SendRequest {
    pub channel_id: String,
    pub content: String,
    /// Message id to reply to in the same channel; `None` sends a plain message.
    pub reply_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    pub id: String,
    pub username: String,
    /// Absent on non-bot authors in the Discord payload; absent means false.
    #[serde(default)]
    pub bot: bool,
}

/// A file attached to a message. `id`/`filename`/`size`/`url` are required —
/// a Discord payload missing one of these is an API contract violation, not
/// something to paper over with a default (fail-fast, see `rust-coding.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Attachment {
    pub id: String,
    pub filename: String,
    pub size: u64,
    pub url: String,
    /// Discord may omit this; absent means unknown, not empty string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub author: Author,
    pub content: String,
    pub timestamp: String,
    /// Absent in a payload means no attachments, not an unknown field.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

/// Result of a successful send; a Discord message object narrowed to what
/// the `send` envelope reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentMessage {
    pub id: String,
    pub channel_id: String,
    pub timestamp: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_deserializes_with_empty_content() {
        let json = r#"{
            "id": "10",
            "channel_id": "chan",
            "author": {"id": "u1", "username": "alice", "bot": false},
            "content": "",
            "timestamp": "2024-01-01T00:00:00Z"
        }"#;
        let message: Message = serde_json::from_str(json).unwrap();
        assert_eq!(message.content, "");
    }

    #[test]
    fn author_bot_defaults_to_false_when_field_is_absent() {
        let json = r#"{"id": "u1", "username": "alice"}"#;
        let author: Author = serde_json::from_str(json).unwrap();
        assert!(!author.bot);
    }

    #[test]
    fn message_with_absent_author_bot_field_deserializes() {
        let json = r#"{
            "id": "10",
            "channel_id": "chan",
            "author": {"id": "u1", "username": "alice"},
            "content": "hi",
            "timestamp": "2024-01-01T00:00:00Z"
        }"#;
        let message: Message = serde_json::from_str(json).unwrap();
        assert!(!message.author.bot);
    }

    #[test]
    fn attachment_deserializes_with_content_type() {
        let json = r#"{
            "id": "a1",
            "filename": "photo.png",
            "size": 1024,
            "url": "https://cdn.discordapp.com/attachments/1/a1/photo.png",
            "content_type": "image/png"
        }"#;
        let attachment: Attachment = serde_json::from_str(json).unwrap();
        assert_eq!(attachment.content_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn attachment_deserializes_without_content_type() {
        let json = r#"{
            "id": "a1",
            "filename": "photo.png",
            "size": 1024,
            "url": "https://cdn.discordapp.com/attachments/1/a1/photo.png"
        }"#;
        let attachment: Attachment = serde_json::from_str(json).unwrap();
        assert_eq!(attachment.content_type, None);
    }

    #[test]
    fn attachment_deserialize_fails_when_size_is_missing() {
        let json = r#"{
            "id": "a1",
            "filename": "photo.png",
            "url": "https://cdn.discordapp.com/attachments/1/a1/photo.png"
        }"#;
        let result: Result<Attachment, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn attachment_deserialize_fails_when_url_is_missing() {
        let json = r#"{
            "id": "a1",
            "filename": "photo.png",
            "size": 1024
        }"#;
        let result: Result<Attachment, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn attachment_serialize_omits_content_type_key_when_none() {
        let attachment = Attachment {
            id: "a1".into(),
            filename: "photo.png".into(),
            size: 1024,
            url: "https://cdn.discordapp.com/attachments/1/a1/photo.png".into(),
            content_type: None,
        };
        let json = serde_json::to_string(&attachment).unwrap();
        assert!(!json.contains("content_type"));
    }

    #[test]
    fn message_deserializes_with_empty_attachments_when_key_is_absent() {
        let json = r#"{
            "id": "10",
            "channel_id": "chan",
            "author": {"id": "u1", "username": "alice", "bot": false},
            "content": "hi",
            "timestamp": "2024-01-01T00:00:00Z"
        }"#;
        let message: Message = serde_json::from_str(json).unwrap();
        assert!(message.attachments.is_empty());
    }

    #[test]
    fn message_deserializes_with_populated_attachments() {
        let json = r#"{
            "id": "10",
            "channel_id": "chan",
            "author": {"id": "u1", "username": "alice", "bot": false},
            "content": "hi",
            "timestamp": "2024-01-01T00:00:00Z",
            "attachments": [{
                "id": "a1",
                "filename": "photo.png",
                "size": 1024,
                "url": "https://cdn.discordapp.com/attachments/1/a1/photo.png"
            }]
        }"#;
        let message: Message = serde_json::from_str(json).unwrap();
        assert_eq!(message.attachments.len(), 1);
        assert_eq!(message.attachments[0].filename, "photo.png");
    }
}
