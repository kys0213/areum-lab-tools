use std::cell::RefCell;
use std::collections::VecDeque;

use super::*;
use crate::commands::testutil::MockDiscordApi;

const BOT: &str = "999";
const GUILD: &str = "g1";
const ISSUE: &str = "issue1";
const HOOK: &str = "/bin/hook";

/// Records each hook launch and replays scripted outcomes (default: started).
struct FakeRunner {
    results: RefCell<VecDeque<Result<(), AppError>>>,
    calls: RefCell<Vec<(Vec<String>, String)>>,
}

impl FakeRunner {
    fn new() -> Self {
        Self {
            results: RefCell::new(VecDeque::new()),
            calls: RefCell::new(Vec::new()),
        }
    }

    fn failing_once() -> Self {
        let runner = Self::new();
        runner
            .results
            .borrow_mut()
            .push_back(Err(AppError::new(ErrorKind::Internal, "spawn failed")));
        runner
    }
}

impl HookRunner for &FakeRunner {
    fn run(&self, argv: &[String], stdin_line: String) -> Result<(), AppError> {
        self.calls.borrow_mut().push((argv.to_vec(), stdin_line));
        self.results.borrow_mut().pop_front().unwrap_or(Ok(()))
    }
}

fn settings() -> MessageTriggerSettings {
    let config = parse(&format!(
        r#"{{"issue_channels":["{ISSUE}"],"on_message":["{HOOK}"]}}"#
    ));
    MessageTriggerSettings::from_config(&config)
        .unwrap()
        .expect("on_message is configured")
}

fn parse(json: &str) -> Config {
    crate::common::config::parse_config(json).unwrap()
}

fn trigger(runner: &FakeRunner) -> MessageTrigger<&FakeRunner> {
    let mut trigger = MessageTrigger::new(settings(), runner);
    trigger.set_bot_user_id(BOT.to_owned());
    trigger
}

fn message(channel_id: &str, mentions: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "id": "m1",
        "channel_id": channel_id,
        "guild_id": GUILD,
        "type": 0,
        "author": { "id": "u1", "username": "alice", "bot": false },
        "content": "<@999> please look",
        "timestamp": "2026-10-04T01:02:03.000000+00:00",
        "mentions": mentions.iter().map(|id| serde_json::json!({ "id": id, "username": "x" })).collect::<Vec<_>>(),
        "mention_roles": [],
        "mention_everyone": false
    })
}

fn text_channel(id: &str) -> Result<ChannelInfo, AppError> {
    Ok(ChannelInfo {
        id: id.to_owned(),
        kind: 0,
        parent_id: None,
    })
}

fn thread(id: &str, parent: &str) -> Result<ChannelInfo, AppError> {
    Ok(ChannelInfo {
        id: id.to_owned(),
        kind: 11,
        parent_id: Some(parent.to_owned()),
    })
}

fn api_with_channels(responses: Vec<Result<ChannelInfo, AppError>>) -> MockDiscordApi {
    let api = MockDiscordApi::new();
    *api.channel_responses.borrow_mut() = responses.into_iter().collect();
    api
}

fn sent_event(runner: &FakeRunner, index: usize) -> serde_json::Value {
    let calls = runner.calls.borrow();
    serde_json::from_str(calls[index].1.trim_end()).unwrap()
}

// ---- settings ----

#[test]
fn settings_are_absent_when_on_message_is_not_configured() {
    let config = parse(r#"{"token":"t","issue_channels":["123"]}"#);
    assert_eq!(MessageTriggerSettings::from_config(&config).unwrap(), None);
}

#[test]
fn settings_resolve_issue_channel_aliases_through_channels() {
    let config = parse(
        r#"{"channels":{"issues":"555"},"issue_channels":["issues","777"],"on_message":["/bin/hook"]}"#,
    );
    let settings = MessageTriggerSettings::from_config(&config)
        .unwrap()
        .unwrap();
    assert_eq!(
        settings.issue_channels,
        HashSet::from(["555".to_owned(), "777".to_owned()])
    );
}

#[test]
fn settings_reject_an_empty_on_message_array() {
    let config = parse(r#"{"on_message":[]}"#);
    let err = MessageTriggerSettings::from_config(&config).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Config);
}

// ---- filter ----

#[tokio::test]
async fn bot_authored_message_is_ignored_even_with_a_mention() {
    let runner = FakeRunner::new();
    let api = MockDiscordApi::new();
    let mut payload = message("c1", &[BOT]);
    payload["author"]["bot"] = serde_json::json!(true);

    let outcome = trigger(&runner).handle(&api, &payload).await.unwrap();

    assert_eq!(outcome, None);
    assert!(runner.calls.borrow().is_empty());
    assert!(api.channel_calls.borrow().is_empty());
}

#[tokio::test]
async fn own_message_is_ignored() {
    let runner = FakeRunner::new();
    let api = MockDiscordApi::new();
    let mut payload = message(ISSUE, &[BOT]);
    payload["author"]["id"] = serde_json::json!(BOT);

    let outcome = trigger(&runner).handle(&api, &payload).await.unwrap();

    assert_eq!(outcome, None);
    assert!(runner.calls.borrow().is_empty());
}

#[tokio::test]
async fn thread_message_with_bot_mention_publishes_mention() {
    let runner = FakeRunner::new();
    let api = api_with_channels(vec![thread("t1", "c1")]);

    let outcome = trigger(&runner)
        .handle(&api, &message("t1", &[BOT]))
        .await
        .unwrap();

    assert_eq!(outcome, Some(Trigger::Mention));
    let event = sent_event(&runner, 0);
    assert_eq!(event["trigger"], "mention");
    assert_eq!(event["channel_id"], "t1");
    assert_eq!(event["parent_channel_id"], "c1");
    assert_eq!(event["is_thread"], true);
}

#[tokio::test]
async fn thread_message_without_mention_is_ignored() {
    let runner = FakeRunner::new();
    let api = api_with_channels(vec![thread("t1", "c1")]);

    let outcome = trigger(&runner)
        .handle(&api, &message("t1", &[]))
        .await
        .unwrap();

    assert_eq!(outcome, None);
    assert!(runner.calls.borrow().is_empty());
}

#[tokio::test]
async fn issue_channel_top_level_post_publishes_without_mention() {
    let runner = FakeRunner::new();
    let api = api_with_channels(vec![text_channel(ISSUE)]);

    let outcome = trigger(&runner)
        .handle(&api, &message(ISSUE, &[]))
        .await
        .unwrap();

    assert_eq!(outcome, Some(Trigger::IssueChannel));
    let event = sent_event(&runner, 0);
    assert_eq!(event["trigger"], "issue_channel");
    assert_eq!(event["is_thread"], false);
    assert_eq!(event["parent_channel_id"], serde_json::Value::Null);
}

#[tokio::test]
async fn issue_channel_post_with_mention_publishes_once_as_issue_channel() {
    let runner = FakeRunner::new();
    let api = api_with_channels(vec![text_channel(ISSUE)]);

    let outcome = trigger(&runner)
        .handle(&api, &message(ISSUE, &[BOT]))
        .await
        .unwrap();

    assert_eq!(outcome, Some(Trigger::IssueChannel));
    assert_eq!(runner.calls.borrow().len(), 1);
}

#[tokio::test]
async fn thread_under_issue_channel_follows_thread_rules() {
    let runner = FakeRunner::new();
    let api = api_with_channels(vec![thread("t2", ISSUE)]);
    let mut trigger = trigger(&runner);

    let unmentioned = trigger.handle(&api, &message("t2", &[])).await.unwrap();
    let mentioned = trigger.handle(&api, &message("t2", &[BOT])).await.unwrap();

    assert_eq!(unmentioned, None);
    assert_eq!(mentioned, Some(Trigger::Mention));
    assert_eq!(runner.calls.borrow().len(), 1);
    assert_eq!(sent_event(&runner, 0)["parent_channel_id"], ISSUE);
}

#[tokio::test]
async fn other_channel_with_mention_publishes_mention() {
    let runner = FakeRunner::new();
    let api = api_with_channels(vec![text_channel("c1")]);

    let outcome = trigger(&runner)
        .handle(&api, &message("c1", &[BOT]))
        .await
        .unwrap();

    assert_eq!(outcome, Some(Trigger::Mention));
    assert_eq!(sent_event(&runner, 0)["is_thread"], false);
}

#[tokio::test]
async fn other_channel_without_mention_is_ignored() {
    let runner = FakeRunner::new();
    let api = MockDiscordApi::new();

    let outcome = trigger(&runner)
        .handle(&api, &message("c1", &["someone-else"]))
        .await
        .unwrap();

    assert_eq!(outcome, None);
    assert!(runner.calls.borrow().is_empty());
}

#[tokio::test]
async fn role_and_everyone_mentions_do_not_count_as_bot_mentions() {
    let runner = FakeRunner::new();
    let api = MockDiscordApi::new();
    let mut payload = message("c1", &[]);
    payload["mention_roles"] = serde_json::json!([BOT]);
    payload["mention_everyone"] = serde_json::json!(true);

    let outcome = trigger(&runner).handle(&api, &payload).await.unwrap();

    assert_eq!(outcome, None);
    assert!(runner.calls.borrow().is_empty());
}

#[tokio::test]
async fn system_message_in_issue_channel_is_ignored() {
    let runner = FakeRunner::new();
    let api = MockDiscordApi::new();
    let mut payload = message(ISSUE, &[]);
    payload["type"] = serde_json::json!(18); // THREAD_CREATED

    let outcome = trigger(&runner).handle(&api, &payload).await.unwrap();

    assert_eq!(outcome, None);
}

// ---- payload ----

#[tokio::test]
async fn stdin_payload_is_one_json_line_with_the_contract_fields() {
    let runner = FakeRunner::new();
    let api = api_with_channels(vec![text_channel("c1")]);

    trigger(&runner)
        .handle(&api, &message("c1", &[BOT]))
        .await
        .unwrap();

    let calls = runner.calls.borrow();
    let (argv, line) = &calls[0];
    assert_eq!(argv, &vec![HOOK.to_owned()]);
    assert!(line.ends_with('\n'));
    assert_eq!(line.matches('\n').count(), 1, "exactly one line: {line:?}");
    let event: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(
        event,
        serde_json::json!({
            "trigger": "mention",
            "guild_id": GUILD,
            "channel_id": "c1",
            "parent_channel_id": null,
            "is_thread": false,
            "message_id": "m1",
            "content": "<@999> please look",
            "author": { "id": "u1", "username": "alice", "bot": false },
            "timestamp": "2026-10-04T01:02:03.000000+00:00",
            "bot_user_id": BOT,
            "message_reference": null
        })
    );
}

#[tokio::test]
async fn stdin_payload_carries_the_replied_message_id() {
    let runner = FakeRunner::new();
    let api = api_with_channels(vec![text_channel("c1")]);
    let mut payload = message("c1", &[BOT]);
    payload["type"] = serde_json::json!(19);
    payload["message_reference"] = serde_json::json!({ "message_id": "m0", "channel_id": "c1" });

    trigger(&runner).handle(&api, &payload).await.unwrap();

    assert_eq!(sent_event(&runner, 0)["message_reference"], "m0");
}

// ---- failures ----

#[tokio::test]
async fn hook_start_failure_is_reported_and_the_next_message_still_publishes() {
    let runner = FakeRunner::failing_once();
    let api = api_with_channels(vec![text_channel("c1")]);
    let mut trigger = trigger(&runner);

    let first = trigger.handle(&api, &message("c1", &[BOT])).await;
    let second = trigger.handle(&api, &message("c1", &[BOT])).await;

    assert!(first.is_err());
    assert_eq!(second.unwrap(), Some(Trigger::Mention));
    assert_eq!(runner.calls.borrow().len(), 2);
}

#[tokio::test]
async fn channel_lookup_failure_does_not_publish_and_is_not_cached() {
    let runner = FakeRunner::new();
    let api = api_with_channels(vec![
        Err(AppError::new(ErrorKind::Network, "boom")),
        text_channel("c1"),
    ]);
    let mut trigger = trigger(&runner);

    let first = trigger.handle(&api, &message("c1", &[BOT])).await;
    let second = trigger.handle(&api, &message("c1", &[BOT])).await;

    assert!(first.is_err());
    assert_eq!(second.unwrap(), Some(Trigger::Mention));
    assert_eq!(runner.calls.borrow().len(), 1);
    assert_eq!(api.channel_calls.borrow().len(), 2);
}

#[tokio::test]
async fn channel_placement_is_cached_after_a_successful_lookup() {
    let runner = FakeRunner::new();
    let api = api_with_channels(vec![thread("t1", "c1")]);
    let mut trigger = trigger(&runner);

    trigger.handle(&api, &message("t1", &[BOT])).await.unwrap();
    trigger.handle(&api, &message("t1", &[BOT])).await.unwrap();

    assert_eq!(api.channel_calls.borrow().as_slice(), ["t1"]);
    assert_eq!(runner.calls.borrow().len(), 2);
}

#[tokio::test]
async fn thread_without_parent_id_is_an_error_not_a_guess() {
    let runner = FakeRunner::new();
    let api = api_with_channels(vec![Ok(ChannelInfo {
        id: "t1".to_owned(),
        kind: 11,
        parent_id: None,
    })]);

    let result = trigger(&runner).handle(&api, &message("t1", &[BOT])).await;

    assert!(result.is_err());
    assert!(runner.calls.borrow().is_empty());
}

#[tokio::test]
async fn message_before_ready_is_an_error() {
    let runner = FakeRunner::new();
    let api = MockDiscordApi::new();
    let mut trigger = MessageTrigger::new(settings(), &runner);

    let result = trigger.handle(&api, &message("c1", &[BOT])).await;

    assert!(result.is_err());
    assert!(runner.calls.borrow().is_empty());
}

/// The gateway hands `handle` twilight's typed message serialized back to
/// JSON; this pins that the round trip keeps the wire names the trigger reads
/// (`type`, `message_reference`, string ids, timestamp).
#[tokio::test]
async fn handles_a_twilight_message_create_round_trip() {
    let raw = serde_json::json!({
        "id": "1001",
        "channel_id": "2002",
        "guild_id": "3003",
        "type": 19,
        "author": { "id": "4004", "username": "alice", "discriminator": "0", "avatar": null },
        "content": "<@999> hi",
        "timestamp": "2026-10-04T01:02:03.000000+00:00",
        "edited_timestamp": null,
        "tts": false,
        "mention_everyone": false,
        "mentions": [{ "id": BOT, "username": "bot", "discriminator": "0", "avatar": null, "bot": true, "public_flags": 0 }],
        "mention_roles": [],
        "attachments": [],
        "embeds": [],
        "pinned": false,
        "message_reference": { "type": 0, "message_id": "1000", "channel_id": "2002", "guild_id": "3003" }
    });
    let typed: twilight_model::gateway::payload::incoming::MessageCreate =
        serde_json::from_value(raw).expect("realistic MESSAGE_CREATE must deserialize");
    let payload = serde_json::to_value(&typed.0).unwrap();
    let runner = FakeRunner::new();
    let api = api_with_channels(vec![text_channel("2002")]);

    let outcome = trigger(&runner).handle(&api, &payload).await.unwrap();

    assert_eq!(outcome, Some(Trigger::Mention));
    let event = sent_event(&runner, 0);
    assert_eq!(event["message_id"], "1001");
    assert_eq!(event["guild_id"], "3003");
    assert_eq!(event["author"]["id"], "4004");
    assert_eq!(event["message_reference"], "1000");
    assert!(
        event["timestamp"]
            .as_str()
            .unwrap()
            .starts_with("2026-10-04T01:02:03")
    );
}
