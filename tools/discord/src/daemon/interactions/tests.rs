use super::*;
use crate::commands::testutil::MockDiscordApi;
use crate::common::store::{AskStore, NewAsk};

const NOW: &str = "2024-01-01T00:00:10Z";

fn store_with_pending(ask_id: &str, options: &[&str], timeout_at: &str) -> AskStore {
    let store = AskStore::open_in_memory().unwrap();
    store
        .insert_ask(NewAsk {
            ask_id: ask_id.to_owned(),
            channel_id: "chan1".to_owned(),
            question: "Proceed?".to_owned(),
            options: options.iter().map(|s| (*s).to_owned()).collect(),
            allow_text: true,
            created_at: "2024-01-01T00:00:00Z".to_owned(),
            timeout_at: timeout_at.to_owned(),
        })
        .unwrap();
    store
}

fn choice_payload(ask_id: &str, index: usize) -> serde_json::Value {
    serde_json::json!({
        "id": "int1",
        "token": "tok1",
        "type": 3,
        "data": { "custom_id": format!("ask:{ask_id}:opt:{index}"), "component_type": 2 },
        "member": { "user": { "id": "user1" } }
    })
}

fn text_button_payload(ask_id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "int2",
        "token": "tok2",
        "type": 3,
        "data": { "custom_id": format!("ask:{ask_id}:text") },
        "member": { "user": { "id": "user1" } }
    })
}

fn modal_submit_payload(ask_id: &str, value: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "int3",
        "token": "tok3",
        "type": 5,
        "data": {
            "custom_id": format!("ask:{ask_id}:text"),
            "components": [{
                "type": 18,
                "component": { "type": 4, "custom_id": "answer", "value": value }
            }]
        },
        "member": { "user": { "id": "user1" } }
    })
}

// --- (a) choice button win ---------------------------------------------------

#[tokio::test]
async fn choice_win_adopts_answer_and_updates_message() {
    let store = store_with_pending("msg1", &["Yes", "No"], "2024-01-01T01:00:00Z");
    let api = MockDiscordApi::new();

    handle_interaction(&api, &store, &choice_payload("msg1", 0), NOW)
        .await
        .unwrap();

    let record = store.get_ask("msg1").unwrap().unwrap();
    assert_eq!(record.status, crate::common::store::AskStatus::Answered);
    assert_eq!(record.kind.as_deref(), Some("choice"));
    assert_eq!(record.value.as_deref(), Some("Yes"));
    assert_eq!(record.answered_by.as_deref(), Some("user1"));

    let calls = api.interaction_calls.borrow();
    assert_eq!(calls.len(), 1);
    let (id, token, payload) = &calls[0];
    assert_eq!(id, "int1");
    assert_eq!(token, "tok1");
    assert_eq!(payload["type"], 7);
    assert_eq!(payload["data"]["components"], serde_json::json!([]));
    assert!(payload["data"]["content"].as_str().unwrap().contains("Yes"));
}

// --- (b) choice button loses (already resolved) ------------------------------

#[tokio::test]
async fn choice_loss_sends_ephemeral() {
    let store = store_with_pending("msg1", &["Yes", "No"], "2024-01-01T01:00:00Z");
    // A prior response already adopted the answer.
    assert!(
        store
            .try_answer("msg1", "choice", "No", "someone", NOW)
            .unwrap()
    );
    let api = MockDiscordApi::new();

    handle_interaction(&api, &store, &choice_payload("msg1", 0), NOW)
        .await
        .unwrap();

    // The winner's answer is untouched.
    assert_eq!(
        store.get_ask("msg1").unwrap().unwrap().value.as_deref(),
        Some("No")
    );
    let calls = api.interaction_calls.borrow();
    let payload = &calls[0].2;
    assert_eq!(payload["type"], 4);
    assert_eq!(payload["data"]["flags"], 64);
}

// --- (c) open modal ----------------------------------------------------------

#[tokio::test]
async fn text_button_opens_modal_with_label_wrapped_text_input() {
    let store = store_with_pending("msg1", &["Yes"], "2024-01-01T01:00:00Z");
    let api = MockDiscordApi::new();

    handle_interaction(&api, &store, &text_button_payload("msg1"), NOW)
        .await
        .unwrap();

    let calls = api.interaction_calls.borrow();
    let payload = &calls[0].2;
    assert_eq!(payload["type"], 9);
    assert_eq!(payload["data"]["custom_id"], "ask:msg1:text");
    let label = &payload["data"]["components"][0];
    assert_eq!(label["type"], 18, "text input must be wrapped in a Label");
    assert_eq!(label["component"]["type"], 4);
    assert_eq!(label["component"]["style"], 2);
    assert_eq!(label["component"]["custom_id"], "answer");
    // No answer is adopted merely by opening the modal.
    assert_eq!(
        store.get_ask("msg1").unwrap().unwrap().status,
        crate::common::store::AskStatus::Pending
    );
}

// --- (d) modal submit win / loss ---------------------------------------------

#[tokio::test]
async fn modal_submit_win_adopts_text_answer() {
    let store = store_with_pending("msg1", &["Yes"], "2024-01-01T01:00:00Z");
    let api = MockDiscordApi::new();

    handle_interaction(&api, &store, &modal_submit_payload("msg1", "ship it"), NOW)
        .await
        .unwrap();

    let record = store.get_ask("msg1").unwrap().unwrap();
    assert_eq!(record.kind.as_deref(), Some("text"));
    assert_eq!(record.value.as_deref(), Some("ship it"));

    let calls = api.interaction_calls.borrow();
    let payload = &calls[0].2;
    assert_eq!(payload["type"], 7);
    assert_eq!(payload["data"]["components"], serde_json::json!([]));
}

#[tokio::test]
async fn modal_submit_loss_sends_ephemeral() {
    let store = store_with_pending("msg1", &["Yes"], "2024-01-01T01:00:00Z");
    assert!(
        store
            .try_answer("msg1", "choice", "Yes", "someone", NOW)
            .unwrap()
    );
    let api = MockDiscordApi::new();

    handle_interaction(&api, &store, &modal_submit_payload("msg1", "late"), NOW)
        .await
        .unwrap();

    let calls = api.interaction_calls.borrow();
    let payload = &calls[0].2;
    assert_eq!(payload["type"], 4);
    assert_eq!(payload["data"]["flags"], 64);
}

// --- (e) foreign custom_id ignored -------------------------------------------

#[tokio::test]
async fn foreign_custom_id_is_ignored_without_response() {
    let store = AskStore::open_in_memory().unwrap();
    let api = MockDiscordApi::new();
    let payload = serde_json::json!({
        "id": "x", "token": "y", "type": 3,
        "data": { "custom_id": "othertool:widget:1" },
        "member": { "user": { "id": "user1" } }
    });

    handle_interaction(&api, &store, &payload, NOW)
        .await
        .unwrap();

    assert!(api.interaction_calls.borrow().is_empty());
}

#[tokio::test]
async fn missing_custom_id_is_ignored() {
    let store = AskStore::open_in_memory().unwrap();
    let api = MockDiscordApi::new();
    let payload = serde_json::json!({ "id": "x", "token": "y", "type": 2, "data": {} });

    handle_interaction(&api, &store, &payload, NOW)
        .await
        .unwrap();

    assert!(api.interaction_calls.borrow().is_empty());
}

// --- (f) unknown ask id → ephemeral ------------------------------------------

#[tokio::test]
async fn unknown_ask_id_sends_ephemeral() {
    let store = AskStore::open_in_memory().unwrap();
    let api = MockDiscordApi::new();

    handle_interaction(&api, &store, &choice_payload("ghost", 0), NOW)
        .await
        .unwrap();

    let calls = api.interaction_calls.borrow();
    let payload = &calls[0].2;
    assert_eq!(payload["type"], 4);
    assert_eq!(payload["data"]["flags"], 64);
}

#[tokio::test]
async fn out_of_range_option_sends_ephemeral() {
    let store = store_with_pending("msg1", &["Yes"], "2024-01-01T01:00:00Z");
    let api = MockDiscordApi::new();

    handle_interaction(&api, &store, &choice_payload("msg1", 9), NOW)
        .await
        .unwrap();

    let calls = api.interaction_calls.borrow();
    assert_eq!(calls[0].2["type"], 4);
    assert_eq!(
        store.get_ask("msg1").unwrap().unwrap().status,
        crate::common::store::AskStatus::Pending
    );
}

// --- (g) expire task disables buttons ----------------------------------------

#[tokio::test]
async fn expire_and_disable_transitions_due_asks_and_edits_messages() {
    let store = AskStore::open_in_memory().unwrap();
    store
        .insert_ask(NewAsk {
            ask_id: "msg1".to_owned(),
            channel_id: "chan1".to_owned(),
            question: "q".to_owned(),
            options: vec!["Yes".to_owned()],
            allow_text: false,
            created_at: "2024-01-01T00:00:00Z".to_owned(),
            timeout_at: "2024-01-01T00:00:05Z".to_owned(),
        })
        .unwrap();
    let api = MockDiscordApi::new();

    let count = expire_and_disable(&api, &store, NOW).await.unwrap();

    assert_eq!(count, 1);
    assert_eq!(
        store.get_ask("msg1").unwrap().unwrap().status,
        crate::common::store::AskStatus::TimedOut
    );
    let edits = api.edit_components_calls.borrow();
    assert_eq!(edits.len(), 1);
    let (channel_id, message_id, components) = &edits[0];
    assert_eq!(channel_id, "chan1");
    assert_eq!(message_id, "msg1");
    assert_eq!(components, &serde_json::json!([]));
}

#[tokio::test]
async fn expire_and_disable_noop_when_nothing_due() {
    let store = store_with_pending("msg1", &["Yes"], "2024-01-01T09:00:00Z");
    let api = MockDiscordApi::new();

    let count = expire_and_disable(&api, &store, NOW).await.unwrap();

    assert_eq!(count, 0);
    assert!(api.edit_components_calls.borrow().is_empty());
}

// --- pure parsing units ------------------------------------------------------

#[test]
fn parse_custom_id_reads_choice_and_text() {
    assert_eq!(
        parse_custom_id("ask:123:opt:2"),
        Some(CustomId::Choice {
            ask_id: "123".to_owned(),
            index: 2
        })
    );
    assert_eq!(
        parse_custom_id("ask:123:text"),
        Some(CustomId::Text {
            ask_id: "123".to_owned()
        })
    );
}

#[test]
fn parse_custom_id_rejects_foreign_and_malformed() {
    assert_eq!(parse_custom_id("othertool:1"), None);
    assert_eq!(parse_custom_id("ask:"), None);
    assert_eq!(parse_custom_id("ask::text"), None);
    assert_eq!(parse_custom_id("ask:123:opt:notanumber"), None);
}

#[test]
fn extract_modal_text_finds_value_regardless_of_nesting() {
    let label_wrapped = serde_json::json!({
        "components": [{ "type": 18, "component": { "type": 4, "value": "hi" } }]
    });
    assert_eq!(
        extract_modal_text(Some(&label_wrapped)),
        Some("hi".to_owned())
    );

    let action_row = serde_json::json!({
        "components": [{ "type": 1, "components": [{ "type": 4, "value": "yo" }] }]
    });
    assert_eq!(extract_modal_text(Some(&action_row)), Some("yo".to_owned()));

    let none = serde_json::json!({ "components": [] });
    assert_eq!(extract_modal_text(Some(&none)), None);
}
