//! `ask create/result/wait` — HITL question lifecycle from the CLI side. See
//! `docs/discord-daemon-hitl.md` §4/§5 for the full contract; the daemon side
//! (`daemon/interactions.rs`) shares the same `custom_id` convention and the
//! same `AskStore`.

use std::path::Path;
use std::time::Duration;

use crate::common::api::{DiscordApi, SendRequest};
use crate::common::store::{AskRecord, AskStatus, AskStore, NewAsk};
use crate::common::time::{now_rfc3339, rfc3339_after_secs};
use crate::output::{AppError, AskCreateData, AskResultData, AskWaitData, ErrorKind, Payload};

use super::wait::Sleeper;

/// Action rows hold at most 5 buttons (spec §7); one slot is reserved for the
/// optional free-text button, so choices are capped at 4.
const MIN_OPTIONS: usize = 1;
const MAX_OPTIONS: usize = 4;

const COMPONENT_TYPE_ACTION_ROW: u8 = 1;
const COMPONENT_TYPE_BUTTON: u8 = 2;
const BUTTON_STYLE_PRIMARY: u8 = 1;
const BUTTON_STYLE_SECONDARY: u8 = 2;
const TEXT_BUTTON_LABEL: &str = "✏️ 직접 입력";

/// Plain request struct (not a builder) — `ask create`'s validated inputs,
/// gathered so `run_ask_create` stays under the argument ceiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AskCreateRequest {
    pub(crate) channel_id: String,
    pub(crate) question: String,
    pub(crate) options: Vec<String>,
    pub(crate) allow_text: bool,
    pub(crate) timeout_secs: u64,
}

/// Sends the question message, attaches its buttons, and persists the
/// `pending` ask — in that order, so a DB row never outlives a message that
/// doesn't exist and buttons are never attached to an ask nobody can look up.
///
/// `is_daemon_running` is a closure (not a `pid_path`) so tests can simulate
/// "daemon down" without a real pidfile/process.
pub(crate) async fn run_ask_create(
    api: &impl DiscordApi,
    db_path: &Path,
    is_daemon_running: impl FnOnce() -> Result<bool, AppError>,
    req: &AskCreateRequest,
) -> Result<Payload, AppError> {
    if !is_daemon_running()? {
        return Err(AppError::new(
            ErrorKind::Usage,
            "daemon not running — run 'discord daemon start'",
        ));
    }
    validate_ask_create(req)?;

    let sent = api
        .send_message(&SendRequest {
            channel_id: req.channel_id.clone(),
            content: req.question.clone(),
            reply_to: None,
            files: Vec::new(),
        })
        .await?;

    let components = build_ask_components(&sent.id, &req.options, req.allow_text);
    if let Err(err) = api
        .edit_message_components(&sent.channel_id, &sent.id, &components)
        .await
    {
        mark_orphaned_ask_message(api, &sent.channel_id, &sent.id).await;
        return Err(err);
    }

    insert_pending_ask(db_path, &sent.id, &sent.channel_id, req)?;

    Ok(Payload::AskCreate(AskCreateData {
        ask_id: sent.id,
        status: "pending".to_owned(),
        channel_id: sent.channel_id,
    }))
}

/// Leaves a visible marker on the sent message when button attachment fails,
/// so it never becomes a silent orphan (spec: a question that can't be
/// answered must not look answerable). Best-effort: its own failure must not
/// shadow the original error being returned to the caller.
async fn mark_orphaned_ask_message(api: &impl DiscordApi, channel_id: &str, message_id: &str) {
    let _ = api
        .send_message(&SendRequest {
            channel_id: channel_id.to_owned(),
            content: "⚠️ 질문 등록 실패 — 버튼을 연결하지 못했습니다.".to_owned(),
            reply_to: Some(message_id.to_owned()),
            files: Vec::new(),
        })
        .await;
}

/// Opens a connection, inserts the one row, and drops it immediately —
/// minimizes overlap with the daemon's 3-second interaction-response path
/// rather than holding the connection for the whole command.
fn insert_pending_ask(
    db_path: &Path,
    ask_id: &str,
    channel_id: &str,
    req: &AskCreateRequest,
) -> Result<(), AppError> {
    let store = AskStore::open(db_path)?;
    store.insert_ask(NewAsk {
        ask_id: ask_id.to_owned(),
        channel_id: channel_id.to_owned(),
        question: req.question.clone(),
        options: req.options.clone(),
        allow_text: req.allow_text,
        created_at: now_rfc3339(),
        timeout_at: rfc3339_after_secs(req.timeout_secs),
    })
}

fn validate_ask_create(req: &AskCreateRequest) -> Result<(), AppError> {
    if req.question.trim().is_empty() {
        return Err(AppError::new(
            ErrorKind::Usage,
            "question must not be empty",
        ));
    }
    if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&req.options.len()) {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!(
                "--option must be given {MIN_OPTIONS}..={MAX_OPTIONS} times (got {})",
                req.options.len()
            ),
        ));
    }
    if req.options.iter().any(|opt| opt.trim().is_empty()) {
        return Err(AppError::new(
            ErrorKind::Usage,
            "--option labels must not be empty",
        ));
    }
    if req.timeout_secs == 0 {
        return Err(AppError::new(
            ErrorKind::Usage,
            "--timeout must be greater than 0",
        ));
    }
    Ok(())
}

/// Builds the single action row of choice buttons (`ask:<id>:opt:<n>`), plus
/// a trailing free-text button (`ask:<id>:text`) when allowed — the same
/// `custom_id` convention `daemon/interactions.rs` parses.
fn build_ask_components(ask_id: &str, options: &[String], allow_text: bool) -> serde_json::Value {
    let mut buttons: Vec<serde_json::Value> = options
        .iter()
        .enumerate()
        .map(|(index, label)| {
            serde_json::json!({
                "type": COMPONENT_TYPE_BUTTON,
                "style": BUTTON_STYLE_PRIMARY,
                "label": label,
                "custom_id": format!("ask:{ask_id}:opt:{index}"),
            })
        })
        .collect();
    if allow_text {
        buttons.push(serde_json::json!({
            "type": COMPONENT_TYPE_BUTTON,
            "style": BUTTON_STYLE_SECONDARY,
            "label": TEXT_BUTTON_LABEL,
            "custom_id": format!("ask:{ask_id}:text"),
        }));
    }
    serde_json::json!([{ "type": COMPONENT_TYPE_ACTION_ROW, "components": buttons }])
}

/// Single SELECT, no polling — reports whatever state the ask is in right now.
pub(crate) fn run_ask_result(db_path: &Path, ask_id: &str) -> Result<Payload, AppError> {
    let store = AskStore::open(db_path)?;
    let record = get_ask_or_usage_error(&store, ask_id)?;
    Ok(Payload::AskResult(ask_result_from_record(&record)))
}

/// Polls `ask_id` every `interval_secs` until it leaves `pending` or the poll
/// budget (`ceil(poll_timeout_secs / interval_secs)` polls) is exhausted.
/// Reaching the poll budget is a normal outcome (exit 0, `timed_out: true`),
/// distinct from the ask's own `status: "timed_out"` — see [`AskWaitData`].
pub(crate) async fn run_ask_wait(
    db_path: &Path,
    sleeper: &impl Sleeper,
    ask_id: &str,
    poll_timeout_secs: u64,
    interval_secs: u64,
) -> Result<Payload, AppError> {
    if interval_secs == 0 {
        return Err(AppError::new(
            ErrorKind::Usage,
            "interval must be greater than 0",
        ));
    }
    let max_polls = poll_timeout_secs.div_ceil(interval_secs).max(1);

    for poll in 0..max_polls {
        let record = fetch_expiring_overdue(db_path, ask_id)?;
        if record.status != AskStatus::Pending {
            return Ok(ask_wait_payload(&record, false));
        }
        let is_last_poll = poll + 1 == max_polls;
        if !is_last_poll {
            sleeper.sleep(Duration::from_secs(interval_secs)).await;
        }
    }

    let record = fetch_expiring_overdue(db_path, ask_id)?;
    let poll_timed_out = record.status == AskStatus::Pending;
    Ok(ask_wait_payload(&record, poll_timed_out))
}

fn ask_wait_payload(record: &AskRecord, timed_out: bool) -> Payload {
    Payload::AskWait(AskWaitData {
        result: ask_result_from_record(record),
        timed_out,
    })
}

/// Fetches the ask, falling back to a local `try_timeout` when it is still
/// `pending` but its own deadline has already passed — a daemon that is down
/// or slow to run its own expiry sweep must not strand `wait` polling forever
/// (spec §4 step 3). Opens and closes its own connection per call, same as
/// every other single-statement store access in this module.
fn fetch_expiring_overdue(db_path: &Path, ask_id: &str) -> Result<AskRecord, AppError> {
    let store = AskStore::open(db_path)?;
    let record = get_ask_or_usage_error(&store, ask_id)?;
    if record.status != AskStatus::Pending || record.timeout_at.as_str() > now_rfc3339().as_str() {
        return Ok(record);
    }
    store.try_timeout(ask_id)?;
    get_ask_or_usage_error(&store, ask_id)
}

fn get_ask_or_usage_error(store: &AskStore, ask_id: &str) -> Result<AskRecord, AppError> {
    store
        .get_ask(ask_id)?
        .ok_or_else(|| AppError::new(ErrorKind::Usage, format!("no such ask: {ask_id}")))
}

/// `kind`/`value`/`answered_by`/`answered_at` are guaranteed present together
/// once `status = 'answered'` — `try_answer` is the only writer of that
/// status and always sets all four in the same statement — so unwrapping them
/// here documents an invariant rather than papering over a real absence.
fn ask_result_from_record(record: &AskRecord) -> AskResultData {
    match record.status {
        AskStatus::Pending => AskResultData::Pending {
            ask_id: record.ask_id.clone(),
        },
        AskStatus::Answered => AskResultData::Answered {
            ask_id: record.ask_id.clone(),
            kind: record
                .kind
                .clone()
                .expect("try_answer always sets kind alongside status = 'answered'"),
            value: record
                .value
                .clone()
                .expect("try_answer always sets value alongside status = 'answered'"),
            answered_by: record
                .answered_by
                .clone()
                .expect("try_answer always sets answered_by alongside status = 'answered'"),
            answered_at: record
                .answered_at
                .clone()
                .expect("try_answer always sets answered_at alongside status = 'answered'"),
        },
        AskStatus::TimedOut => AskResultData::TimedOut {
            ask_id: record.ask_id.clone(),
        },
    }
}

#[cfg(test)]
mod tests;
