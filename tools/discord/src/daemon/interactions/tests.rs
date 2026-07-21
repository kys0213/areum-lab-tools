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

fn store_with_pending_no_text(ask_id: &str, options: &[&str], timeout_at: &str) -> AskStore {
    let store = AskStore::open_in_memory().unwrap();
    store
        .insert_ask(NewAsk {
            ask_id: ask_id.to_owned(),
            channel_id: "chan1".to_owned(),
            question: "Proceed?".to_owned(),
            options: options.iter().map(|s| (*s).to_owned()).collect(),
            allow_text: false,
            created_at: "2024-01-01T00:00:00Z".to_owned(),
            timeout_at: timeout_at.to_owned(),
        })
        .unwrap();
    store
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

/// P2-2: documents the intended (not accidental) behavior when the callback
/// itself fails after the answer was already adopted. `try_answer` commits
/// before the callback (module doc's 3-second rule), so a callback failure
/// (expired token here) must not undo that adoption — it only propagates for
/// the gateway loop to log, per `respond`'s doc comment.
#[tokio::test]
async fn choice_win_persists_the_answer_even_when_the_callback_fails() {
    let store = store_with_pending("msg1", &["Yes", "No"], "2024-01-01T01:00:00Z");
    let api = MockDiscordApi::new();
    api.interaction_responses
        .borrow_mut()
        .push_back(Err(crate::common::error::AppError::new(
            crate::common::error::ErrorKind::Api,
            "interaction token expired",
        )));

    let err = handle_interaction(&api, &store, &choice_payload("msg1", 0), NOW)
        .await
        .expect_err("a failed callback must propagate for the gateway to log");
    assert_eq!(err.kind, crate::common::error::ErrorKind::Api);

    // The answer was adopted before the callback ran, so it stands despite
    // the callback's own failure — the user's Discord client may show no
    // confirmation, but `ask result`/`ask wait` read the correct outcome.
    let record = store.get_ask("msg1").unwrap().unwrap();
    assert_eq!(record.status, crate::common::store::AskStatus::Answered);
    assert_eq!(record.value.as_deref(), Some("Yes"));
    assert_eq!(record.answered_by.as_deref(), Some("user1"));
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

/// P2-4: Discord caps a modal title at 45 characters; a longer question must
/// be truncated to fit rather than sent verbatim and rejected by Discord.
#[tokio::test]
async fn text_button_opens_modal_with_title_truncated_to_45_chars() {
    let long_question = "a".repeat(60);
    let store = AskStore::open_in_memory().unwrap();
    store
        .insert_ask(NewAsk {
            ask_id: "msg1".to_owned(),
            channel_id: "chan1".to_owned(),
            question: long_question.clone(),
            options: vec!["Yes".to_owned()],
            allow_text: true,
            created_at: "2024-01-01T00:00:00Z".to_owned(),
            timeout_at: "2024-01-01T01:00:00Z".to_owned(),
        })
        .unwrap();
    let api = MockDiscordApi::new();

    handle_interaction(&api, &store, &text_button_payload("msg1"), NOW)
        .await
        .unwrap();

    let calls = api.interaction_calls.borrow();
    let title = calls[0].2["data"]["title"].as_str().unwrap();
    assert_eq!(title.chars().count(), 45);
    assert_eq!(title, &long_question[..45]);
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

// --- allow_text=false rejects a forged text interaction ----------------------

/// `ask create` never attaches a text button when `allow_text` is false, so a
/// real Discord client can't produce this click — but the daemon must not
/// trust the client's component set. A payload carrying `ask:<id>:text` for
/// such an ask is a path the ask's own record disallows, so it must be
/// rejected (ephemeral, no modal opened, ask stays pending) rather than
/// honored just because the custom_id parses.
#[tokio::test]
async fn text_button_click_rejected_when_allow_text_is_false() {
    let store = store_with_pending_no_text("msg1", &["Yes"], "2024-01-01T01:00:00Z");
    let api = MockDiscordApi::new();

    handle_interaction(&api, &store, &text_button_payload("msg1"), NOW)
        .await
        .unwrap();

    let calls = api.interaction_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].2["type"], 4, "must not open a modal (type 9)");
    assert_eq!(calls[0].2["data"]["flags"], 64);
    assert_eq!(
        store.get_ask("msg1").unwrap().unwrap().status,
        crate::common::store::AskStatus::Pending
    );
}

/// Same forged-path rejection, but for the `MODAL_SUBMIT` itself — a client
/// that opened a modal some other way (or replayed a stale one) must not be
/// able to adopt a text answer against an ask that disallows it.
#[tokio::test]
async fn modal_submit_rejected_when_allow_text_is_false() {
    let store = store_with_pending_no_text("msg1", &["Yes"], "2024-01-01T01:00:00Z");
    let api = MockDiscordApi::new();

    handle_interaction(&api, &store, &modal_submit_payload("msg1", "sneaky"), NOW)
        .await
        .unwrap();

    let calls = api.interaction_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].2["type"], 4);
    assert_eq!(calls[0].2["data"]["flags"], 64);
    let record = store.get_ask("msg1").unwrap().unwrap();
    assert_eq!(record.status, crate::common::store::AskStatus::Pending);
    assert_eq!(record.value, None);
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

/// `create` doesn't reject duplicate `--option` labels, and resolution is
/// index-based (`ask:<id>:opt:<index>`), so two options sharing a label must
/// still resolve unambiguously by index rather than by (possibly duplicate)
/// text.
#[tokio::test]
async fn duplicate_option_labels_resolve_by_index_not_by_text() {
    let store = store_with_pending("msg1", &["yes", "yes"], "2024-01-01T01:00:00Z");
    let api = MockDiscordApi::new();

    handle_interaction(&api, &store, &choice_payload("msg1", 1), NOW)
        .await
        .unwrap();

    let record = store.get_ask("msg1").unwrap().unwrap();
    assert_eq!(record.status, crate::common::store::AskStatus::Answered);
    assert_eq!(record.value.as_deref(), Some("yes"));
    assert_eq!(record.answered_by.as_deref(), Some("user1"));

    let calls = api.interaction_calls.borrow();
    assert_eq!(calls[0].2["type"], 7);
}

// --- modal submit payload variants -------------------------------------------

/// A submit whose text input carries no `value` at all (e.g. an optional
/// field left empty in a client that omits rather than sends `""`) must fail
/// fast rather than silently adopting an empty/absent answer — no response is
/// sent and the ask stays pending for a legitimate retry.
#[tokio::test]
async fn modal_submit_missing_value_fails_fast() {
    let store = store_with_pending("msg1", &["Yes"], "2024-01-01T01:00:00Z");
    let api = MockDiscordApi::new();
    let payload = serde_json::json!({
        "id": "int3",
        "token": "tok3",
        "type": 5,
        "data": {
            "custom_id": "ask:msg1:text",
            "components": [{
                "type": 18,
                "component": { "type": 4, "custom_id": "answer" }
            }]
        },
        "member": { "user": { "id": "user1" } }
    });

    let err = handle_interaction(&api, &store, &payload, NOW)
        .await
        .expect_err("a modal submit with no text value must fail fast");
    assert_eq!(err.kind, crate::common::error::ErrorKind::Internal);
    assert!(api.interaction_calls.borrow().is_empty());
    assert_eq!(
        store.get_ask("msg1").unwrap().unwrap().status,
        crate::common::store::AskStatus::Pending
    );
}

/// A `MODAL_SUBMIT` carrying a choice-shaped (`opt:`) custom_id is not a
/// shape this daemon ever produces (modals only ever reuse the `:text`
/// custom_id) — it must be ignored like any other foreign id, not treated as
/// an error or a choice answer.
#[tokio::test]
async fn modal_submit_with_choice_shaped_custom_id_is_ignored() {
    let store = store_with_pending("msg1", &["Yes"], "2024-01-01T01:00:00Z");
    let api = MockDiscordApi::new();
    let payload = serde_json::json!({
        "id": "int3",
        "token": "tok3",
        "type": 5,
        "data": { "custom_id": "ask:msg1:opt:0" },
        "member": { "user": { "id": "user1" } }
    });

    handle_interaction(&api, &store, &payload, NOW)
        .await
        .unwrap();

    assert!(api.interaction_calls.borrow().is_empty());
    assert_eq!(
        store.get_ask("msg1").unwrap().unwrap().status,
        crate::common::store::AskStatus::Pending
    );
}

// --- interaction payload gaps -------------------------------------------------

/// Neither `member.user.id` nor `user.id` is present — a contract violation
/// per `interaction_user_id`'s doc comment. Must fail fast before any store
/// write or response, not silently attribute the answer to an empty id.
#[tokio::test]
async fn choice_click_without_member_or_user_fails_fast() {
    let store = store_with_pending("msg1", &["Yes"], "2024-01-01T01:00:00Z");
    let api = MockDiscordApi::new();
    let payload = serde_json::json!({
        "id": "int1",
        "token": "tok1",
        "type": 3,
        "data": { "custom_id": "ask:msg1:opt:0" }
    });

    let err = handle_interaction(&api, &store, &payload, NOW)
        .await
        .expect_err("a payload missing both member and user must fail fast");
    assert_eq!(err.kind, crate::common::error::ErrorKind::Internal);
    assert!(err.message.contains("user id"));
    assert!(api.interaction_calls.borrow().is_empty());
    assert_eq!(
        store.get_ask("msg1").unwrap().unwrap().status,
        crate::common::store::AskStatus::Pending
    );
}

/// A DM interaction carries `user.id` directly (no `member` wrapper) —
/// `interaction_user_id`'s fallback path, previously exercised only by its
/// guild-shaped (`member.user.id`) branch.
#[tokio::test]
async fn choice_click_in_dm_uses_top_level_user_id() {
    let store = store_with_pending("msg1", &["Yes", "No"], "2024-01-01T01:00:00Z");
    let api = MockDiscordApi::new();
    let payload = serde_json::json!({
        "id": "int1",
        "token": "tok1",
        "type": 3,
        "data": { "custom_id": "ask:msg1:opt:0" },
        "user": { "id": "dm-user" }
    });

    handle_interaction(&api, &store, &payload, NOW)
        .await
        .unwrap();

    let record = store.get_ask("msg1").unwrap().unwrap();
    assert_eq!(record.answered_by.as_deref(), Some("dm-user"));
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
    let mut retry_queue = ExpireRetryQueue::new();

    let count = expire_and_disable(&api, &store, NOW, &mut retry_queue)
        .await
        .unwrap();

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
    let mut retry_queue = ExpireRetryQueue::new();

    let count = expire_and_disable(&api, &store, NOW, &mut retry_queue)
        .await
        .unwrap();

    assert_eq!(count, 0);
    assert!(api.edit_components_calls.borrow().is_empty());
}

/// P1 regression: a single failing edit in the middle of a batch must not
/// stop the rest of the batch from being disabled, and each outcome must be
/// classified correctly — a permanent 404 is dropped, a transient failure
/// (429 here) is queued and retried on the next tick until it succeeds.
#[tokio::test]
async fn expire_and_disable_isolates_failures_and_retries_transient_ones_next_tick() {
    let store = AskStore::open_in_memory().unwrap();
    for id in ["a1", "a2", "a3", "a4"] {
        store
            .insert_ask(NewAsk {
                ask_id: id.to_owned(),
                channel_id: "chan1".to_owned(),
                question: "q".to_owned(),
                options: vec!["Yes".to_owned()],
                allow_text: false,
                created_at: "2024-01-01T00:00:00Z".to_owned(),
                timeout_at: "2024-01-01T00:00:05Z".to_owned(),
            })
            .unwrap();
    }
    let api = MockDiscordApi::new();
    api.edit_components_responses.borrow_mut().extend([
        Ok(()),
        Err(AppError {
            kind: ErrorKind::Api,
            message: "message deleted".to_owned(),
            http_status: Some(404),
            retry_after_ms: None,
        }),
        Err(AppError {
            kind: ErrorKind::RateLimit,
            message: "rate limited".to_owned(),
            http_status: Some(429),
            retry_after_ms: Some(500),
        }),
        Ok(()),
    ]);
    let mut retry_queue = ExpireRetryQueue::new();

    // --- tick 1: all four transition; four edits attempted this tick -------
    let count = expire_and_disable(&api, &store, NOW, &mut retry_queue)
        .await
        .unwrap();
    assert_eq!(count, 4, "all four overdue asks must transition this tick");
    for id in ["a1", "a2", "a3", "a4"] {
        assert_eq!(
            store.get_ask(id).unwrap().unwrap().status,
            crate::common::store::AskStatus::TimedOut,
            "{id} must transition regardless of its edit outcome"
        );
    }
    let calls_after_tick1 = api.edit_components_calls.borrow().len();
    assert_eq!(
        calls_after_tick1, 4,
        "one edit attempt per due ask this tick"
    );
    // Calls are recorded in the same order responses were popped (single
    // sequential loop, no concurrency), so index 1 is the 404 call and index
    // 2 is the 429 call regardless of which ask_id landed there.
    let ask_that_got_404 = api.edit_components_calls.borrow()[1].1.clone();
    let ask_that_got_429 = api.edit_components_calls.borrow()[2].1.clone();

    // --- tick 2: nothing newly due; only the queued 429 retry fires --------
    let count = expire_and_disable(&api, &store, NOW, &mut retry_queue)
        .await
        .unwrap();
    assert_eq!(count, 0, "no ask newly transitions on this tick");
    let calls_after_tick2_len = api.edit_components_calls.borrow().len();
    let last_call_ask_id = api.edit_components_calls.borrow().last().unwrap().1.clone();
    assert_eq!(
        calls_after_tick2_len,
        calls_after_tick1 + 1,
        "exactly one retried edit (the 429 case) — the 404 case must not be retried"
    );
    assert_eq!(
        last_call_ask_id, ask_that_got_429,
        "the retried edit must be for the ask that got the transient (429) failure"
    );
    assert_ne!(
        last_call_ask_id, ask_that_got_404,
        "the permanently-failed (404) ask must never be retried"
    );

    // --- tick 3: the successful retry must not be retried again -------------
    let count = expire_and_disable(&api, &store, NOW, &mut retry_queue)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        api.edit_components_calls.borrow().len(),
        calls_after_tick1 + 1,
        "the retry queue must be empty after the 429 case's successful retry"
    );
}

/// A transient failure that keeps failing must eventually be abandoned
/// (bounded retry budget) rather than retried on every tick forever.
#[tokio::test]
async fn expire_and_disable_gives_up_after_max_attempts() {
    let store = store_with_pending("msg1", &["Yes"], "2024-01-01T00:00:05Z");
    let api = MockDiscordApi::new();
    let always_fails = || {
        Err(AppError {
            kind: ErrorKind::Api,
            message: "server error".to_owned(),
            http_status: Some(500),
            retry_after_ms: None,
        })
    };
    api.edit_components_responses.borrow_mut().extend([
        always_fails(),
        always_fails(),
        always_fails(),
    ]);
    let mut retry_queue = ExpireRetryQueue::new();

    // Ticks 1-3: each attempt fails and is retried up to MAX_EDIT_ATTEMPTS.
    for _ in 0..3 {
        expire_and_disable(&api, &store, NOW, &mut retry_queue)
            .await
            .unwrap();
    }
    assert_eq!(api.edit_components_calls.borrow().len(), 3);

    // Tick 4: the budget is exhausted, so no further attempt is made even
    // though no response is queued (a queued response would be required if
    // the mock were actually invoked again).
    expire_and_disable(&api, &store, NOW, &mut retry_queue)
        .await
        .unwrap();
    assert_eq!(
        api.edit_components_calls.borrow().len(),
        3,
        "must stop retrying once MAX_EDIT_ATTEMPTS is reached"
    );
    assert_eq!(
        store.get_ask("msg1").unwrap().unwrap().status,
        crate::common::store::AskStatus::TimedOut,
        "the ask itself stays timed_out regardless of the abandoned edit"
    );
}

/// The click-vs-expire race, from the full `handle_interaction` side: once
/// `expire_and_disable` has already resolved an ask, a stray click arriving
/// after it must get the same "already closed" ephemeral as a click that
/// loses to another click — the record stays `timed_out`, not re-adopted.
#[tokio::test]
async fn choice_after_expire_receives_ephemeral_and_leaves_timed_out_record() {
    let store = store_with_pending("msg1", &["Yes"], "2024-01-01T00:00:05Z");
    let api = MockDiscordApi::new();
    let mut retry_queue = ExpireRetryQueue::new();

    let expired = expire_and_disable(&api, &store, NOW, &mut retry_queue)
        .await
        .unwrap();
    assert_eq!(expired, 1);
    api.edit_components_calls.borrow_mut().clear();

    handle_interaction(&api, &store, &choice_payload("msg1", 0), NOW)
        .await
        .unwrap();

    let calls = api.interaction_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].2["type"], 4);
    assert_eq!(calls[0].2["data"]["flags"], 64);

    let record = store.get_ask("msg1").unwrap().unwrap();
    assert_eq!(record.status, crate::common::store::AskStatus::TimedOut);
    assert_eq!(record.answered_by, None);
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
