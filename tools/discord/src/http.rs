use crate::api::{DiscordApi, Message, SentMessage};
use crate::output::AppError;

const API_BASE: &str = "https://discord.com/api/v10";

/// Live Discord REST client. Bodies land in T3; the request scaffolding here
/// wires the token and base URL that the implementation will build on.
pub struct HttpDiscordApi {
    client: reqwest::Client,
    token: String,
}

impl HttpDiscordApi {
    pub fn new(token: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            token,
        }
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.header("Authorization", format!("Bot {}", self.token))
    }
}

impl DiscordApi for HttpDiscordApi {
    async fn send_message(&self, channel_id: &str, content: &str) -> Result<SentMessage, AppError> {
        let url = format!("{API_BASE}/channels/{channel_id}/messages");
        let _request = self
            .authorized(self.client.post(url))
            .json(&serde_json::json!({ "content": content }));
        todo!("T3: execute request, map status to AppError kinds, deserialize SentMessage")
    }

    async fn get_messages(
        &self,
        channel_id: &str,
        after: Option<&str>,
        limit: u8,
    ) -> Result<Vec<Message>, AppError> {
        let url = format!("{API_BASE}/channels/{channel_id}/messages");
        let mut request = self
            .authorized(self.client.get(url))
            .query(&[("limit", limit.to_string())]);
        if let Some(after) = after {
            request = request.query(&[("after", after)]);
        }
        let _request = request;
        todo!("T3: execute request, map status to AppError kinds, deserialize Vec<Message>")
    }
}
