use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::api::{DiscordApi, Message, SentMessage};
use crate::output::{AppError, ErrorKind};

const API_BASE: &str = "https://discord.com/api/v10";
const USER_AGENT: &str = "DiscordBot (https://github.com/kys0213/areum-lab-tools, 0.0.0)";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RATE_LIMIT_RETRIES: u32 = 3;

/// Live Discord REST client.
pub struct HttpDiscordApi {
    client: reqwest::Client,
    token: String,
    /// Fixed to Discord's REST base in production; kept as a field (not the
    /// `API_BASE` const directly) rather than baked into every call site.
    base_url: String,
}

impl HttpDiscordApi {
    pub fn new(token: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .expect("static timeout + user-agent config is always a valid client");
        Self {
            client,
            token,
            base_url: API_BASE.to_string(),
        }
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.header("Authorization", format!("Bot {}", self.token))
    }

    /// Sends `request`, retrying on 429 up to [`MAX_RATE_LIMIT_RETRIES`] times,
    /// and deserializes a 2xx body into `T`. Every other outcome maps to a
    /// classified [`AppError`] — no fallback/default substitution on mismatch.
    async fn execute<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, AppError> {
        let mut retries = 0;
        loop {
            // Cloned per attempt: the same builder is resent on 429 retries.
            let attempt = request.try_clone().expect(
                "requests built here carry a small JSON/query body and are always cloneable",
            );
            let response = attempt.send().await.map_err(network_error)?;
            let status = response.status();

            if status.is_success() {
                let bytes = response.bytes().await.map_err(network_error)?;
                return serde_json::from_slice(&bytes)
                    .map_err(|err| deserialize_error(&bytes, &err));
            }

            if status.as_u16() == 429 {
                let header = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let body_bytes = response.bytes().await.unwrap_or_default();
                let body_json: Option<serde_json::Value> = serde_json::from_slice(&body_bytes).ok();
                let retry_after_ms = parse_retry_after(header.as_deref(), body_json.as_ref());

                match retry_after_ms {
                    Some(ms) if retries < MAX_RATE_LIMIT_RETRIES => {
                        retries += 1;
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                        continue;
                    }
                    _ => return Err(rate_limit_error(retry_after_ms)),
                }
            }

            let body = response.text().await.unwrap_or_default();
            return Err(status_error(status.as_u16(), &body));
        }
    }
}

impl DiscordApi for HttpDiscordApi {
    async fn send_message(&self, channel_id: &str, content: &str) -> Result<SentMessage, AppError> {
        let url = format!("{}/channels/{channel_id}/messages", self.base_url);
        let request = self
            .authorized(self.client.post(url))
            .json(&serde_json::json!({ "content": content }));
        self.execute(request).await
    }

    async fn get_messages(
        &self,
        channel_id: &str,
        after: Option<&str>,
        limit: u8,
    ) -> Result<Vec<Message>, AppError> {
        let url = get_messages_url(&self.base_url, channel_id, after, limit);
        let request = self.authorized(self.client.get(url));
        self.execute(request).await
    }
}

/// Pure URL+query assembly so the `after` Some/None branching is testable
/// without a network round trip. Ordering of returned messages is whatever
/// Discord sends back — sorting is the caller's responsibility.
fn get_messages_url(base_url: &str, channel_id: &str, after: Option<&str>, limit: u8) -> String {
    let mut url = format!("{base_url}/channels/{channel_id}/messages?limit={limit}");
    if let Some(after) = after {
        url.push_str("&after=");
        url.push_str(after);
    }
    url
}

/// Maps an HTTP status (already known not to be 2xx/429) to an [`ErrorKind`].
fn classify_status(status: u16) -> ErrorKind {
    match status {
        401 | 403 => ErrorKind::Auth,
        _ => ErrorKind::Api,
    }
}

fn status_error(status: u16, body: &str) -> AppError {
    let snippet: String = body.chars().take(200).collect();
    AppError {
        kind: classify_status(status),
        message: format!("Discord API returned {status}: {snippet}"),
        http_status: Some(status),
        retry_after_ms: None,
    }
}

fn rate_limit_error(retry_after_ms: Option<u64>) -> AppError {
    AppError {
        kind: ErrorKind::RateLimit,
        message: "Discord rate limit retries exhausted".to_string(),
        http_status: Some(429),
        retry_after_ms,
    }
}

fn network_error(err: reqwest::Error) -> AppError {
    AppError::new(
        ErrorKind::Network,
        format!("request to Discord failed: {err}"),
    )
}

fn deserialize_error(body: &[u8], err: &serde_json::Error) -> AppError {
    let snippet = String::from_utf8_lossy(body);
    AppError::new(
        ErrorKind::Api,
        format!("failed to deserialize Discord response: {err}; body: {snippet}"),
    )
}

/// Parses the 429 retry delay: `Retry-After` header (seconds, decimal) takes
/// priority over the JSON body's `retry_after` (seconds, float). `None` when
/// neither is parseable — the caller does not retry in that case.
fn parse_retry_after(header: Option<&str>, body: Option<&serde_json::Value>) -> Option<u64> {
    if let Some(seconds) = header.and_then(|h| h.parse::<f64>().ok()) {
        return Some((seconds * 1000.0).round() as u64);
    }
    if let Some(seconds) = body
        .and_then(|v| v.get("retry_after"))
        .and_then(|v| v.as_f64())
    {
        return Some((seconds * 1000.0).round() as u64);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_status_maps_auth_statuses() {
        assert_eq!(classify_status(401), ErrorKind::Auth);
        assert_eq!(classify_status(403), ErrorKind::Auth);
    }

    #[test]
    fn classify_status_maps_other_statuses_to_api() {
        assert_eq!(classify_status(400), ErrorKind::Api);
        assert_eq!(classify_status(404), ErrorKind::Api);
        assert_eq!(classify_status(500), ErrorKind::Api);
    }

    #[test]
    fn status_error_preserves_http_status_for_api_kind() {
        let err = status_error(404, "not found");
        assert_eq!(err.kind, ErrorKind::Api);
        assert_eq!(err.http_status, Some(404));
    }

    #[test]
    fn status_error_preserves_http_status_for_auth_kind() {
        let err = status_error(401, "unauthorized");
        assert_eq!(err.kind, ErrorKind::Auth);
        assert_eq!(err.http_status, Some(401));
    }

    #[test]
    fn parse_retry_after_reads_header_seconds() {
        assert_eq!(parse_retry_after(Some("1.5"), None), Some(1500));
    }

    #[test]
    fn parse_retry_after_reads_header_integer_seconds() {
        assert_eq!(parse_retry_after(Some("2"), None), Some(2000));
    }

    #[test]
    fn parse_retry_after_reads_body_seconds() {
        let body = serde_json::json!({ "retry_after": 0.25 });
        assert_eq!(parse_retry_after(None, Some(&body)), Some(250));
    }

    #[test]
    fn parse_retry_after_prefers_header_over_body() {
        let body = serde_json::json!({ "retry_after": 9.0 });
        assert_eq!(parse_retry_after(Some("1.5"), Some(&body)), Some(1500));
    }

    #[test]
    fn parse_retry_after_none_when_neither_parses() {
        let body = serde_json::json!({ "unrelated": "field" });
        assert_eq!(parse_retry_after(None, Some(&body)), None);
        assert_eq!(parse_retry_after(Some("not-a-number"), None), None);
        assert_eq!(parse_retry_after(None, None), None);
    }

    #[test]
    fn get_messages_url_includes_after_when_present() {
        let url = get_messages_url(API_BASE, "123", Some("456"), 50);
        assert_eq!(
            url,
            "https://discord.com/api/v10/channels/123/messages?limit=50&after=456"
        );
    }

    #[test]
    fn get_messages_url_omits_after_when_absent() {
        let url = get_messages_url(API_BASE, "123", None, 50);
        assert_eq!(
            url,
            "https://discord.com/api/v10/channels/123/messages?limit=50"
        );
    }

    #[test]
    fn sent_message_deserializes_from_full_discord_message_payload() {
        let json = r#"{
            "id": "999",
            "channel_id": "chan1",
            "author": {"id": "u1", "username": "bot", "bot": true},
            "content": "hello",
            "timestamp": "2024-01-01T00:00:00Z",
            "type": 0,
            "embeds": [],
            "attachments": []
        }"#;
        let sent: SentMessage = serde_json::from_str(json).unwrap();
        assert_eq!(sent.id, "999");
        assert_eq!(sent.channel_id, "chan1");
        assert_eq!(sent.timestamp, "2024-01-01T00:00:00Z");
    }

    #[test]
    fn deserialize_error_maps_json_parse_failure_to_api_kind_without_fallback() {
        // A malformed body must surface as an Api error, not silently fall back
        // to a default-constructed Message/SentMessage.
        let body = b"{ not valid json";
        let parse_err = serde_json::from_slice::<Message>(body).unwrap_err();
        let err = deserialize_error(body, &parse_err);
        assert_eq!(err.kind, ErrorKind::Api);
        assert!(
            err.message
                .contains("failed to deserialize Discord response")
        );
        assert!(err.message.contains("not valid json"));
    }

    #[test]
    fn messages_deserialize_with_empty_content_and_unknown_fields() {
        let json = r#"[
            {
                "id": "1",
                "channel_id": "chan1",
                "author": {"id": "u1", "username": "alice", "bot": false},
                "content": "",
                "timestamp": "2024-01-01T00:00:00Z",
                "attachments": [{"id": "a1", "filename": "x.png"}],
                "embeds": []
            }
        ]"#;
        let messages: Vec<Message> = serde_json::from_str(json).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "");
    }
}
