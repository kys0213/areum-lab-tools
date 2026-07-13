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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub author: Author,
    pub content: String,
    pub timestamp: String,
}

/// Result of a successful send; a Discord message object narrowed to what
/// the `send` envelope reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentMessage {
    pub id: String,
    pub channel_id: String,
    pub timestamp: String,
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
}
