//! Loopback tests: a real MCP server on 127.0.0.1 talks to a fake Discord REST
//! server, and a plain HTTP client speaks MCP Streamable HTTP to it.

use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use serde_json::{Value, json};

use super::*;

const TOKEN: &str = "good.secret-token";
const WRONG_TOKEN: &str = "wrong.secret-token";

/// Records the Authorization header of every Discord call and answers like
/// Discord would for the one valid token.
#[derive(Clone, Default)]
struct FakeDiscord {
    seen_auth: Arc<Mutex<Vec<String>>>,
}

impl FakeDiscord {
    /// `Some(401 response)` unless the request carries the valid token.
    fn deny(&self, headers: &HeaderMap) -> Option<Response> {
        let auth = headers["authorization"].to_str().unwrap().to_owned();
        self.seen_auth.lock().unwrap().push(auth.clone());
        (auth != format!("Bot {TOKEN}")).then(|| {
            json_response(
                StatusCode::UNAUTHORIZED,
                json!({"message": "401: Unauthorized", "code": 0}),
            )
        })
    }
}

async fn messages(
    State(fake): State<FakeDiscord>,
    Path(channel): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(denied) = fake.deny(&headers) {
        return denied;
    }
    json_response(
        StatusCode::OK,
        json!([{
            "id": "42",
            "channel_id": channel,
            "author": {"id": "u1", "username": "alice"},
            "content": "hello",
            "timestamp": "2024-01-01T00:00:00Z",
            "attachments": []
        }]),
    )
}

fn json_response(status: StatusCode, body: Value) -> Response {
    (
        status,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

async fn reaction(State(fake): State<FakeDiscord>, headers: HeaderMap) -> Response {
    fake.deny(&headers)
        .unwrap_or_else(|| StatusCode::NO_CONTENT.into_response())
}

async fn spawn(app: axum::Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// Starts the fake Discord and the MCP server; returns the MCP URL.
async fn start() -> (String, FakeDiscord) {
    let fake = FakeDiscord::default();
    let discord = spawn(
        axum::Router::new()
            .route("/channels/{channel}/messages", get(messages))
            .route(
                "/channels/{channel}/messages/{message}/reactions/{emoji}/@me",
                put(reaction).delete(reaction),
            )
            .with_state(fake.clone()),
    )
    .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        serve(listener, &discord, std::future::pending())
            .await
            .unwrap()
    });
    (format!("http://{addr}{MCP_PATH}"), fake)
}

/// Posts one JSON-RPC message; returns the HTTP status and the body text.
async fn post(url: &str, auth: Option<&str>, body: Value) -> (u16, String) {
    let mut request = reqwest::Client::new()
        .post(url)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .header("x-discord-channels", "dev=111")
        .json(&body);
    if let Some(auth) = auth {
        request = request.header("authorization", auth);
    }
    let response = request.send().await.unwrap();
    (response.status().as_u16(), response.text().await.unwrap())
}

async fn rpc(url: &str, auth: Option<&str>, id: u32, method: &str, params: Value) -> Value {
    let (status, body) = post(
        url,
        auth,
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
    )
    .await;
    assert_eq!(status, 200, "{method}: {body}");
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("{method}: {e}: {body}"))
}

fn initialize_params() -> Value {
    json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": {"name": "test", "version": "0"}
    })
}

#[tokio::test]
async fn initialize_list_and_call_over_loopback_http() {
    let (url, fake) = start().await;
    let auth = format!("Bot {TOKEN}");

    let init = rpc(&url, Some(&auth), 1, "initialize", initialize_params()).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "discord", "{init}");
    assert!(
        init["result"]["capabilities"]["tools"].is_object(),
        "{init}"
    );

    let (status, _) = post(
        &url,
        Some(&auth),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    assert_eq!(status, 202);

    let list = rpc(&url, Some(&auth), 2, "tools/list", json!({})).await;
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["send", "read", "react", "thread_create"]);

    let read = rpc(
        &url,
        Some(&auth),
        3,
        "tools/call",
        json!({"name": "read", "arguments": {"channel": "dev", "limit": 5}}),
    )
    .await;
    let result = &read["result"];
    assert_eq!(result["isError"], false, "{read}");
    assert_eq!(result["structuredContent"]["channel_id"], "111");
    assert_eq!(result["structuredContent"]["count"], 1);
    assert_eq!(result["structuredContent"]["cursor"], "42");

    let react = rpc(
        &url,
        Some(&auth),
        4,
        "tools/call",
        json!({"name": "react", "arguments": {"channel": "111", "message_id": "42", "emoji": "✅"}}),
    )
    .await;
    assert_eq!(
        react["result"]["structuredContent"],
        json!({"channel_id": "111", "message_id": "42", "emoji": "✅", "removed": false})
    );

    let seen = fake.seen_auth.lock().unwrap().clone();
    assert_eq!(seen, vec![auth.clone(), auth]);
}

#[tokio::test]
async fn missing_or_malformed_authorization_is_an_mcp_error_without_the_token() {
    let (url, fake) = start().await;
    let bad_auth = format!("Bearer {TOKEN}");
    for auth in [None, Some(bad_auth.as_str())] {
        for (method, params) in [
            ("initialize", initialize_params()),
            (
                "tools/call",
                json!({"name": "read", "arguments": {"channel": "dev"}}),
            ),
        ] {
            let response = rpc(&url, auth, 1, method, params).await;
            assert!(
                response["error"]["message"].is_string(),
                "{method}: {response}"
            );
            assert!(!response.to_string().contains("secret-token"), "{response}");
        }
    }
    assert!(fake.seen_auth.lock().unwrap().is_empty());
}

#[tokio::test]
async fn discord_rejecting_the_token_is_a_tool_error_without_the_token() {
    let (url, _fake) = start().await;
    let response = rpc(
        &url,
        Some(&format!("Bot {WRONG_TOKEN}")),
        1,
        "tools/call",
        json!({"name": "read", "arguments": {"channel": "dev"}}),
    )
    .await;
    let result = &response["result"];
    assert_eq!(result["isError"], true, "{response}");
    assert_eq!(result["structuredContent"]["kind"], "auth", "{response}");
    assert!(!response.to_string().contains("secret-token"), "{response}");
}

#[test]
fn call_log_line_reports_tool_and_outcome() {
    let ok = Ok(Payload::McpServe(crate::output::McpServeData {
        url: "u".into(),
    }));
    assert_eq!(call_log_line("read", &ok), "mcp: read ok");
    let err = Err(AppError::new(
        crate::common::error::ErrorKind::Auth,
        "denied",
    ));
    assert_eq!(
        call_log_line("send", &err),
        "mcp: send failed: error [auth]: denied"
    );
}
