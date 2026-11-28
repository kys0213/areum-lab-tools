use std::collections::HashMap;

use serde_json::json;

use super::*;
use crate::commands::testutil::{MockDiscordApi, ReactionCall, message};
use crate::common::api::{CreatedThread, SentMessage};

fn headers() -> RequestHeaders {
    RequestHeaders {
        token: "t".into(),
        channels: HashMap::from([("dev".to_owned(), "111".to_owned())]),
        allowed_channels: None,
    }
}

fn args(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => panic!("expected object, got {other}"),
    }
}

#[test]
fn catalog_lists_the_four_v1_tools_with_object_schemas() {
    let names: Vec<&str> = tool_specs().iter().map(|t| t.name).collect();
    assert_eq!(names, ["send", "read", "react", "thread_create"]);
    for spec in tool_specs() {
        assert_eq!(spec.input_schema["type"], "object", "{}", spec.name);
        assert!(
            spec.input_schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("channel"))
        );
    }
}

#[tokio::test]
async fn send_resolves_alias_and_returns_send_data() {
    let api = MockDiscordApi::new();
    api.send_responses.borrow_mut().push_back(Ok(SentMessage {
        id: "m1".into(),
        channel_id: "111".into(),
        timestamp: "2024-01-01T00:00:00Z".into(),
        attachments: vec![],
    }));
    let payload = call_tool(
        &api,
        &headers(),
        "send",
        args(json!({"channel": "dev", "text": "hi", "reply_to": "m0"})),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(&payload).unwrap(),
        json!({"message_id": "m1", "channel_id": "111", "timestamp": "2024-01-01T00:00:00Z", "attachments": []})
    );
    let calls = api.send_calls.borrow();
    assert_eq!(calls[0].channel_id, "111");
    assert_eq!(calls[0].content, "hi");
    assert_eq!(calls[0].reply_to.as_deref(), Some("m0"));
}

#[tokio::test]
async fn send_rejects_files_argument_without_calling_discord() {
    let api = MockDiscordApi::new();
    let err = call_tool(
        &api,
        &headers(),
        "send",
        args(json!({"channel": "dev", "text": "hi", "files": ["/etc/passwd"]})),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(api.send_calls.borrow().is_empty());
}

#[tokio::test]
async fn read_uses_default_limit_and_passes_after() {
    let api = MockDiscordApi::with_get_responses(vec![Ok(vec![message("5")])]);
    let payload = call_tool(
        &api,
        &headers(),
        "read",
        args(json!({"channel": "dev", "after": "4"})),
    )
    .await
    .unwrap();
    assert_eq!(
        *api.get_calls.borrow(),
        vec![("111".to_owned(), Some("4".to_owned()), 50)]
    );
    let data = serde_json::to_value(&payload).unwrap();
    assert_eq!(data["count"], 1);
    assert_eq!(data["cursor"], "5");
}

#[tokio::test]
async fn react_adds_then_removes() {
    let api = MockDiscordApi::new();
    for remove in [false, true] {
        let payload = call_tool(
            &api,
            &headers(),
            "react",
            args(json!({"channel": "dev", "message_id": "m1", "emoji": "✅", "remove": remove})),
        )
        .await
        .unwrap();
        assert_eq!(serde_json::to_value(&payload).unwrap()["removed"], remove);
    }
    let calls = api.reaction_calls.borrow();
    assert_eq!(
        *calls,
        vec![
            ReactionCall {
                remove: false,
                channel_id: "111".into(),
                message_id: "m1".into(),
                emoji: "✅".into()
            },
            ReactionCall {
                remove: true,
                channel_id: "111".into(),
                message_id: "m1".into(),
                emoji: "✅".into()
            },
        ]
    );
}

#[tokio::test]
async fn thread_create_returns_thread_data() {
    let api = MockDiscordApi::with_thread_responses(vec![Ok(CreatedThread {
        id: "t1".into(),
        name: "topic".into(),
    })]);
    let payload = call_tool(
        &api,
        &headers(),
        "thread_create",
        args(json!({"channel": "dev", "name": "topic", "from_message": "m1"})),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(&payload).unwrap(),
        json!({"thread_id": "t1", "name": "topic"})
    );
    assert_eq!(
        api.thread_calls.borrow()[0].from_message_id.as_deref(),
        Some("m1")
    );
}

#[tokio::test]
async fn disallowed_channel_is_denied_before_the_tool_runs() {
    let api = MockDiscordApi::new();
    api.channel_responses
        .borrow_mut()
        .push_back(Ok(crate::common::api::ChannelInfo {
            id: "222".into(),
            kind: 0,
            parent_id: None,
        }));
    let mut denied = headers();
    denied.allowed_channels = Some(["111".to_owned()].into());
    let err = call_tool(
        &api,
        &denied,
        "send",
        args(json!({"channel": "222", "text": "hi"})),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Auth);
    assert!(api.send_calls.borrow().is_empty());
}

#[tokio::test]
async fn unknown_tool_and_missing_argument_are_usage_errors() {
    let api = MockDiscordApi::new();
    let unknown = call_tool(&api, &headers(), "ask", args(json!({})))
        .await
        .unwrap_err();
    assert_eq!(unknown.kind, ErrorKind::Usage);
    let missing = call_tool(&api, &headers(), "read", args(json!({})))
        .await
        .unwrap_err();
    assert_eq!(missing.kind, ErrorKind::Usage);
}
