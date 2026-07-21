use std::path::PathBuf;

use super::*;
use crate::commands::testutil::{FakeSleeper, MockDiscordApi};
use crate::common::api::SentMessage;

fn unique_db_path(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "areum-discord-ask-test-{}-{label}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("discord.db")
}

fn sample_request() -> AskCreateRequest {
    AskCreateRequest {
        channel_id: "chan-1".to_owned(),
        question: "proceed?".to_owned(),
        options: vec!["yes".to_owned(), "no".to_owned()],
        allow_text: true,
        timeout_secs: 60,
    }
}

fn sent_message(id: &str, channel_id: &str) -> SentMessage {
    SentMessage {
        id: id.to_owned(),
        channel_id: channel_id.to_owned(),
        timestamp: "2024-01-01T00:00:00Z".to_owned(),
        attachments: vec![],
    }
}

fn daemon_up() -> Result<bool, AppError> {
    Ok(true)
}

// --- ask create ---------------------------------------------------------

#[tokio::test]
async fn create_sends_then_patches_components_then_inserts_pending_ask() {
    let db_path = unique_db_path("create-happy");
    let api = MockDiscordApi::new();
    api.send_responses
        .borrow_mut()
        .push_back(Ok(sent_message("555", "chan-1")));

    let payload = run_ask_create(&api, &db_path, daemon_up, &sample_request())
        .await
        .unwrap();

    match payload {
        Payload::AskCreate(d) => {
            assert_eq!(d.ask_id, "555");
            assert_eq!(d.status, "pending");
            assert_eq!(d.channel_id, "chan-1");
        }
        other => panic!("expected AskCreate, got {other:?}"),
    }

    // Only the question send, no orphan-marker follow-up.
    assert_eq!(api.send_calls.borrow().len(), 1);

    let edits = api.edit_components_calls.borrow();
    assert_eq!(edits.len(), 1);
    let (channel_id, message_id, components) = &edits[0];
    assert_eq!(channel_id, "chan-1");
    assert_eq!(message_id, "555");
    assert_eq!(
        *components,
        serde_json::json!([{
            "type": 1,
            "components": [
                { "type": 2, "style": 1, "label": "yes", "custom_id": "ask:555:opt:0" },
                { "type": 2, "style": 1, "label": "no", "custom_id": "ask:555:opt:1" },
                { "type": 2, "style": 2, "label": "✏️ 직접 입력", "custom_id": "ask:555:text" },
            ]
        }])
    );

    let store = AskStore::open(&db_path).unwrap();
    let record = store.get_ask("555").unwrap().expect("ask must be inserted");
    assert_eq!(record.status, AskStatus::Pending);
    assert_eq!(record.channel_id, "chan-1");
    assert_eq!(record.question, "proceed?");
    assert_eq!(record.options, vec!["yes".to_owned(), "no".to_owned()]);
    assert!(record.allow_text);

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

#[tokio::test]
async fn create_omits_text_button_when_allow_text_is_false() {
    let db_path = unique_db_path("create-no-text");
    let api = MockDiscordApi::new();
    api.send_responses
        .borrow_mut()
        .push_back(Ok(sent_message("556", "chan-1")));
    let mut req = sample_request();
    req.allow_text = false;

    run_ask_create(&api, &db_path, daemon_up, &req)
        .await
        .unwrap();

    let edits = api.edit_components_calls.borrow();
    let (_, _, components) = &edits[0];
    let buttons = components[0]["components"].as_array().unwrap();
    assert_eq!(buttons.len(), 2, "no trailing text button expected");

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

#[tokio::test]
async fn create_errors_when_daemon_is_not_running() {
    let db_path = unique_db_path("create-daemon-down");
    let api = MockDiscordApi::new();

    let err = run_ask_create(&api, &db_path, || Ok(false), &sample_request())
        .await
        .unwrap_err();

    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(err.message.contains("daemon not running"));
    assert_eq!(api.send_calls.borrow().len(), 0);

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

#[tokio::test]
async fn create_marks_message_and_propagates_error_when_components_patch_fails() {
    let db_path = unique_db_path("create-patch-fails");
    let api = MockDiscordApi::new();
    api.send_responses
        .borrow_mut()
        .push_back(Ok(sent_message("557", "chan-1")));
    // Second queued response: the best-effort orphan-marker reply.
    api.send_responses
        .borrow_mut()
        .push_back(Ok(sent_message("558", "chan-1")));
    api.edit_components_responses
        .borrow_mut()
        .push_back(Err(AppError::new(ErrorKind::Api, "components rejected")));

    let err = run_ask_create(&api, &db_path, daemon_up, &sample_request())
        .await
        .unwrap_err();

    assert_eq!(err.kind, ErrorKind::Api);
    assert_eq!(err.message, "components rejected");

    // Original send + a best-effort orphan marker reply.
    let sends = api.send_calls.borrow();
    assert_eq!(sends.len(), 2);
    assert_eq!(sends[1].reply_to.as_deref(), Some("557"));
    assert!(sends[1].content.contains("질문 등록 실패"));

    let store = AskStore::open(&db_path).unwrap();
    assert!(
        store.get_ask("557").unwrap().is_none(),
        "a failed components PATCH must not leave a DB row behind"
    );

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

#[test]
fn create_rejects_zero_options() {
    let mut req = sample_request();
    req.options = vec![];
    let err = validate_ask_create(&req).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn create_rejects_more_than_four_options() {
    let mut req = sample_request();
    req.options = vec!["a", "b", "c", "d", "e"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let err = validate_ask_create(&req).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn create_rejects_blank_option_label() {
    let mut req = sample_request();
    req.options = vec!["yes".to_owned(), "  ".to_owned()];
    let err = validate_ask_create(&req).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn create_rejects_blank_question() {
    let mut req = sample_request();
    req.question = "   ".to_owned();
    let err = validate_ask_create(&req).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn create_rejects_zero_timeout() {
    let mut req = sample_request();
    req.timeout_secs = 0;
    let err = validate_ask_create(&req).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn create_accepts_four_options_with_allow_text() {
    // 4 options + the text button = exactly the 5-button action-row ceiling.
    let mut req = sample_request();
    req.options = vec!["a", "b", "c", "d"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    req.allow_text = true;
    assert!(validate_ask_create(&req).is_ok());
}

// --- ask result ----------------------------------------------------------

#[test]
fn result_reports_pending() {
    let db_path = unique_db_path("result-pending");
    let store = AskStore::open(&db_path).unwrap();
    store
        .insert_ask(sample_ask("ask-1", "2999-01-01T00:00:00Z"))
        .unwrap();

    let payload = run_ask_result(&db_path, "ask-1").unwrap();
    assert_eq!(
        payload,
        Payload::AskResult(AskResultData::Pending {
            ask_id: "ask-1".into()
        })
    );

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

#[test]
fn result_reports_answered_with_all_fields() {
    let db_path = unique_db_path("result-answered");
    let store = AskStore::open(&db_path).unwrap();
    store
        .insert_ask(sample_ask("ask-2", "2999-01-01T00:00:00Z"))
        .unwrap();
    store
        .try_answer("ask-2", "choice", "yes", "u1", "2024-01-01T00:00:10Z")
        .unwrap();

    let payload = run_ask_result(&db_path, "ask-2").unwrap();
    assert_eq!(
        payload,
        Payload::AskResult(AskResultData::Answered {
            ask_id: "ask-2".into(),
            kind: "choice".into(),
            value: "yes".into(),
            answered_by: "u1".into(),
            answered_at: "2024-01-01T00:00:10Z".into(),
        })
    );

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

#[test]
fn result_reports_timed_out() {
    let db_path = unique_db_path("result-timed-out");
    let store = AskStore::open(&db_path).unwrap();
    store
        .insert_ask(sample_ask("ask-3", "2000-01-01T00:00:00Z"))
        .unwrap();
    assert!(store.try_timeout("ask-3").unwrap());

    let payload = run_ask_result(&db_path, "ask-3").unwrap();
    assert_eq!(
        payload,
        Payload::AskResult(AskResultData::TimedOut {
            ask_id: "ask-3".into()
        })
    );

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

#[test]
fn result_errors_for_unknown_ask_id() {
    let db_path = unique_db_path("result-missing");
    AskStore::open(&db_path).unwrap(); // create the db file, no rows

    let err = run_ask_result(&db_path, "does-not-exist").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

fn sample_ask(ask_id: &str, timeout_at: &str) -> NewAsk {
    NewAsk {
        ask_id: ask_id.to_owned(),
        channel_id: "chan-1".to_owned(),
        question: "proceed?".to_owned(),
        options: vec!["yes".to_owned(), "no".to_owned()],
        allow_text: true,
        created_at: "2024-01-01T00:00:00Z".to_owned(),
        timeout_at: timeout_at.to_owned(),
    }
}

// --- ask wait --------------------------------------------------------------

#[tokio::test]
async fn wait_returns_immediately_when_already_answered() {
    let db_path = unique_db_path("wait-already-answered");
    let store = AskStore::open(&db_path).unwrap();
    store
        .insert_ask(sample_ask("ask-4", "2999-01-01T00:00:00Z"))
        .unwrap();
    store
        .try_answer("ask-4", "text", "sure", "u2", "2024-01-01T00:00:10Z")
        .unwrap();
    let sleeper = FakeSleeper::new();

    let payload = run_ask_wait(&db_path, &sleeper, "ask-4", 30, 5)
        .await
        .unwrap();

    match payload {
        Payload::AskWait(d) => {
            assert!(!d.timed_out);
            assert_eq!(
                d.result,
                AskResultData::Answered {
                    ask_id: "ask-4".into(),
                    kind: "text".into(),
                    value: "sure".into(),
                    answered_by: "u2".into(),
                    answered_at: "2024-01-01T00:00:10Z".into(),
                }
            );
        }
        other => panic!("expected AskWait, got {other:?}"),
    }
    assert_eq!(sleeper.calls.borrow().len(), 0);

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

/// A [`Sleeper`] that answers the ask as a side effect of its first sleep —
/// stands in for "someone clicked a button during this poll interval"
/// without depending on real wall-clock timing.
struct AnswerDuringFirstSleep<'a> {
    db_path: &'a Path,
    ask_id: &'a str,
}

impl Sleeper for AnswerDuringFirstSleep<'_> {
    async fn sleep(&self, _dur: Duration) {
        let store = AskStore::open(self.db_path).unwrap();
        store
            .try_answer(self.ask_id, "choice", "yes", "u1", "2024-01-01T00:00:10Z")
            .unwrap();
    }
}

#[tokio::test]
async fn wait_polls_and_returns_as_soon_as_answered() {
    let db_path = unique_db_path("wait-polls-until-answered");
    let store = AskStore::open(&db_path).unwrap();
    store
        .insert_ask(sample_ask("ask-5", "2999-01-01T00:00:00Z"))
        .unwrap();
    let sleeper = AnswerDuringFirstSleep {
        db_path: &db_path,
        ask_id: "ask-5",
    };

    // timeout 15 / interval 5 -> 3 polls budgeted; the ask is answered during
    // the sleep after poll 1, so poll 2 must see it and return without a
    // third poll or second sleep.
    let payload = run_ask_wait(&db_path, &sleeper, "ask-5", 15, 5)
        .await
        .unwrap();

    match payload {
        Payload::AskWait(d) => {
            assert!(!d.timed_out);
            assert!(matches!(d.result, AskResultData::Answered { .. }));
        }
        other => panic!("expected AskWait, got {other:?}"),
    }

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

#[tokio::test]
async fn wait_exhausts_poll_budget_and_reports_poll_timed_out() {
    let db_path = unique_db_path("wait-poll-timeout");
    let store = AskStore::open(&db_path).unwrap();
    // Deadline far in the future: the ask itself must not expire during this
    // test, isolating the poll-budget-exhaustion path.
    store
        .insert_ask(sample_ask("ask-6", "2999-01-01T00:00:00Z"))
        .unwrap();
    let sleeper = FakeSleeper::new();

    let payload = run_ask_wait(&db_path, &sleeper, "ask-6", 15, 5)
        .await
        .unwrap();

    match payload {
        Payload::AskWait(d) => {
            assert!(d.timed_out, "poll budget exhaustion must set timed_out");
            assert_eq!(
                d.result,
                AskResultData::Pending {
                    ask_id: "ask-6".into()
                }
            );
        }
        other => panic!("expected AskWait, got {other:?}"),
    }
    // ceil(15/5) = 3 polls, 2 sleeps between them.
    assert_eq!(sleeper.calls.borrow().len(), 2);

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

#[tokio::test]
async fn wait_falls_back_to_local_try_timeout_when_ask_deadline_already_passed() {
    let db_path = unique_db_path("wait-expiry-fallback");
    let store = AskStore::open(&db_path).unwrap();
    // Deadline already in the past — simulates a daemon that is down/slow to
    // run its own expiry sweep.
    store
        .insert_ask(sample_ask("ask-7", "2000-01-01T00:00:00Z"))
        .unwrap();
    let sleeper = FakeSleeper::new();

    let payload = run_ask_wait(&db_path, &sleeper, "ask-7", 15, 5)
        .await
        .unwrap();

    match payload {
        Payload::AskWait(d) => {
            // Resolved via the ask's own overdue deadline, not the poll budget.
            assert!(!d.timed_out);
            assert_eq!(
                d.result,
                AskResultData::TimedOut {
                    ask_id: "ask-7".into()
                }
            );
        }
        other => panic!("expected AskWait, got {other:?}"),
    }
    assert_eq!(
        sleeper.calls.borrow().len(),
        0,
        "expiry fallback must resolve on the first poll"
    );

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

#[tokio::test]
async fn wait_rejects_interval_zero() {
    let db_path = unique_db_path("wait-interval-zero");
    let sleeper = FakeSleeper::new();

    let err = run_ask_wait(&db_path, &sleeper, "ask-8", 30, 0)
        .await
        .unwrap_err();

    assert_eq!(err.kind, ErrorKind::Usage);
}

#[tokio::test]
async fn wait_errors_for_unknown_ask_id() {
    let db_path = unique_db_path("wait-missing");
    AskStore::open(&db_path).unwrap();
    let sleeper = FakeSleeper::new();

    let err = run_ask_wait(&db_path, &sleeper, "does-not-exist", 10, 5)
        .await
        .unwrap_err();

    assert_eq!(err.kind, ErrorKind::Usage);

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

// --- CLI <-> daemon contract round trip -------------------------------------
//
// Every test above (and every test in `daemon::interactions::tests`) exercises
// one side of the HITL contract against a fixture it wrote itself: `ask
// create`'s tests assert the `custom_id`/DB shape it produces, the daemon's
// tests hand-build an INTERACTION_CREATE payload with a matching id. Neither
// catches the two sides drifting apart from each other. These tests run both
// real functions back to back against the same on-disk store, so the
// `custom_id` the daemon parses is the exact one `ask create` emitted.

#[tokio::test]
async fn create_to_choice_click_to_result_round_trips_through_the_real_contract() {
    let db_path = unique_db_path("e2e-choice");
    let api = MockDiscordApi::new();
    api.send_responses
        .borrow_mut()
        .push_back(Ok(sent_message("900", "chan-1")));

    let create_payload = run_ask_create(&api, &db_path, daemon_up, &sample_request())
        .await
        .unwrap();
    let ask_id = match create_payload {
        Payload::AskCreate(d) => d.ask_id,
        other => panic!("expected AskCreate, got {other:?}"),
    };

    // Pulled from the actual PATCH `ask create` sent — not a hand-rolled
    // fixture string — so a drift in the custom_id convention on either side
    // fails this test instead of silently passing.
    let custom_id = {
        let edits = api.edit_components_calls.borrow();
        edits[0].2[0]["components"][1]["custom_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(custom_id, format!("ask:{ask_id}:opt:1"));

    let click_payload = serde_json::json!({
        "id": "int1",
        "token": "tok1",
        "type": 3,
        "data": { "custom_id": custom_id, "component_type": 2 },
        "member": { "user": { "id": "clicker-1" } }
    });
    let store = AskStore::open(&db_path).unwrap();
    crate::daemon::handle_interaction(&api, &store, &click_payload, "2024-01-01T00:00:05Z")
        .await
        .unwrap();
    drop(store);

    let result = run_ask_result(&db_path, &ask_id).unwrap();
    assert_eq!(
        result,
        Payload::AskResult(AskResultData::Answered {
            ask_id: ask_id.clone(),
            kind: "choice".into(),
            value: "no".into(),
            answered_by: "clicker-1".into(),
            answered_at: "2024-01-01T00:00:05Z".into(),
        })
    );

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

#[tokio::test]
async fn create_to_modal_submit_to_result_round_trips_through_the_real_contract() {
    let db_path = unique_db_path("e2e-text");
    let api = MockDiscordApi::new();
    api.send_responses
        .borrow_mut()
        .push_back(Ok(sent_message("901", "chan-1")));

    let create_payload = run_ask_create(&api, &db_path, daemon_up, &sample_request())
        .await
        .unwrap();
    let ask_id = match create_payload {
        Payload::AskCreate(d) => d.ask_id,
        other => panic!("expected AskCreate, got {other:?}"),
    };

    let modal_submit_payload = serde_json::json!({
        "id": "int2",
        "token": "tok2",
        "type": 5,
        "data": {
            "custom_id": format!("ask:{ask_id}:text"),
            "components": [{
                "type": 18,
                "component": { "type": 4, "custom_id": "answer", "value": "ship it" }
            }]
        },
        "member": { "user": { "id": "clicker-2" } }
    });
    let store = AskStore::open(&db_path).unwrap();
    crate::daemon::handle_interaction(&api, &store, &modal_submit_payload, "2024-01-01T00:00:05Z")
        .await
        .unwrap();
    drop(store);

    let result = run_ask_result(&db_path, &ask_id).unwrap();
    assert_eq!(
        result,
        Payload::AskResult(AskResultData::Answered {
            ask_id: ask_id.clone(),
            kind: "text".into(),
            value: "ship it".into(),
            answered_by: "clicker-2".into(),
            answered_at: "2024-01-01T00:00:05Z".into(),
        })
    );

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}

// --- wait's local expiry fallback vs. the daemon's periodic sweep -----------

/// Real 2-connection race between `ask wait`'s local `try_timeout` fallback
/// (spec §4 step 5's "daemon down/slow" escape hatch) and the daemon's own
/// periodic `expire_and_disable` sweep, both acting on the same overdue ask
/// from independent connections/threads (mirrors
/// `common::store::tests::two_connections_racing_click_and_expire_exactly_one_wins`,
/// but for the two paths that both resolve to the *same* terminal status).
/// Unlike a click-vs-expire race there is no "wrong winner" to assert against
/// — both write `timed_out` — so the invariant under test is that the ask
/// converges to `timed_out` and `ask wait` reports it consistently however the
/// two interleave, with neither side erroring on the other's write.
#[tokio::test]
async fn wait_local_expiry_and_daemon_sweep_race_without_diverging() {
    let db_path = unique_db_path("wait-vs-daemon-expire-race");
    let setup = AskStore::open(&db_path).unwrap();
    setup
        .insert_ask(sample_ask("race-ask", "2024-01-01T00:00:00Z")) // already overdue
        .unwrap();
    drop(setup);

    let wait_db_path = db_path.clone();
    let wait_handle = std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            run_ask_wait(&wait_db_path, &FakeSleeper::new(), "race-ask", 5, 5).await
        })
    });

    let expire_db_path = db_path.clone();
    let expire_handle = std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let store = AskStore::open(&expire_db_path).unwrap();
            let api = MockDiscordApi::new();
            let mut retry_queue = crate::daemon::ExpireRetryQueue::new();
            crate::daemon::expire_and_disable(
                &api,
                &store,
                "2024-01-01T00:00:10Z",
                &mut retry_queue,
            )
            .await
        })
    });

    let wait_result = wait_handle.join().unwrap().unwrap();
    expire_handle.join().unwrap().unwrap();

    match wait_result {
        Payload::AskWait(d) => {
            assert!(
                !d.timed_out,
                "resolved via the ask's own overdue deadline, not the poll budget"
            );
            assert_eq!(
                d.result,
                AskResultData::TimedOut {
                    ask_id: "race-ask".into()
                }
            );
        }
        other => panic!("expected AskWait, got {other:?}"),
    }

    let verify = AskStore::open(&db_path).unwrap();
    assert_eq!(
        verify.get_ask("race-ask").unwrap().unwrap().status,
        AskStatus::TimedOut
    );

    std::fs::remove_dir_all(db_path.parent().unwrap()).ok();
}
