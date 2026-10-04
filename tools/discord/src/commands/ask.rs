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
/// Discord's action-row button label cap (spec §7) — enforced here so a
/// too-long label fails fast in validation rather than surfacing as a 400
/// from Discord after the message is already sent.
const MAX_OPTION_LABEL_CHARS: usize = 80;
/// String select menu limits (docs.discord.com components reference): at most
/// 25 options, each label at most 100 characters. A select occupies a whole
/// action row, so the optional text button moves to a second row and does not
/// compete for a slot.
const MAX_SELECT_OPTIONS: usize = 25;
const MAX_SELECT_LABEL_CHARS: usize = 100;
/// Upper bound on `--timeout`: 30 days. Not just a sanity limit — a
/// `timeout_at` past year 9999 fails `normalize_utc`'s SQLite `datetime()`
/// parse (returns NULL), which `insert_pending_ask` surfaces as an opaque
/// Internal error; this rejects that input as a usage error instead, before
/// any message is sent.
const MAX_TIMEOUT_SECS: u64 = 30 * 24 * 60 * 60;

const COMPONENT_TYPE_ACTION_ROW: u8 = 1;
const COMPONENT_TYPE_BUTTON: u8 = 2;
const COMPONENT_TYPE_STRING_SELECT: u8 = 3;
const BUTTON_STYLE_PRIMARY: u8 = 1;
const BUTTON_STYLE_SECONDARY: u8 = 2;
const TEXT_BUTTON_LABEL: &str = "✏️ 직접 입력";
/// `kind` the daemon records for a select-menu answer (spec §3).
const KIND_MULTI_CHOICE: &str = "multi_choice";

/// Plain request struct (not a builder) — `ask create`'s validated inputs,
/// gathered so `run_ask_create` stays under the argument ceiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AskCreateRequest {
    pub(crate) channel_id: String,
    pub(crate) question: String,
    pub(crate) options: Vec<String>,
    pub(crate) allow_text: bool,
    /// Discord user ids allowed to answer; empty means anyone.
    pub(crate) allowed_users: Vec<String>,
    /// Show the options as a multi-choice select menu instead of buttons.
    pub(crate) multi_select: bool,
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

    let components = build_ask_components(&sent.id, req);
    if let Err(err) = api
        .edit_message_components(&sent.channel_id, &sent.id, &components)
        .await
    {
        mark_orphaned_ask_message(api, &sent.channel_id, &sent.id).await;
        return Err(err);
    }

    if let Err(err) = insert_pending_ask(db_path, &sent.id, &sent.channel_id, req) {
        // The message's buttons are live (the PATCH above succeeded) but no
        // row exists to resolve a click against — strip them so the message
        // doesn't look answerable, mirroring the PATCH-failure branch above
        // rather than leaving an asymmetric silent orphan.
        let no_components = serde_json::json!([]);
        let _ = api
            .edit_message_components(&sent.channel_id, &sent.id, &no_components)
            .await;
        mark_orphaned_ask_message(api, &sent.channel_id, &sent.id).await;
        return Err(err);
    }

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
        allowed_users: req.allowed_users.clone(),
        multi_select: req.multi_select,
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
    let (max_options, max_label_chars) = if req.multi_select {
        (MAX_SELECT_OPTIONS, MAX_SELECT_LABEL_CHARS)
    } else {
        (MAX_OPTIONS, MAX_OPTION_LABEL_CHARS)
    };
    if !(MIN_OPTIONS..=max_options).contains(&req.options.len()) {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!(
                "--option must be given {MIN_OPTIONS}..={max_options} times (got {})",
                req.options.len()
            ),
        ));
    }
    if let Some(bad) = req
        .allowed_users
        .iter()
        .find(|id| id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()))
    {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!("--allowed-user must be a numeric Discord user id (got {bad:?})"),
        ));
    }
    if req.options.iter().any(|opt| opt.trim().is_empty()) {
        return Err(AppError::new(
            ErrorKind::Usage,
            "--option labels must not be empty",
        ));
    }
    if let Some(too_long) = req
        .options
        .iter()
        .find(|opt| opt.chars().count() > max_label_chars)
    {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!(
                "--option label must be at most {max_label_chars} characters (got {}: {too_long:?})",
                too_long.chars().count()
            ),
        ));
    }
    if req.timeout_secs == 0 {
        return Err(AppError::new(
            ErrorKind::Usage,
            "--timeout must be greater than 0",
        ));
    }
    if req.timeout_secs > MAX_TIMEOUT_SECS {
        return Err(AppError::new(
            ErrorKind::Usage,
            format!("--timeout must be at most {MAX_TIMEOUT_SECS} seconds (30 days)"),
        ));
    }
    Ok(())
}

/// Builds the question's components with the `custom_id` convention
/// `daemon/interactions.rs` parses: one row of choice buttons
/// (`ask:<id>:opt:<n>`), or — for multi-select — one string select row
/// (`ask:<id>:sel`, option values are indices). The free-text button
/// (`ask:<id>:text`) trails the buttons, or gets its own row after the select.
fn build_ask_components(ask_id: &str, req: &AskCreateRequest) -> serde_json::Value {
    let text_button = serde_json::json!({
        "type": COMPONENT_TYPE_BUTTON,
        "style": BUTTON_STYLE_SECONDARY,
        "label": TEXT_BUTTON_LABEL,
        "custom_id": format!("ask:{ask_id}:text"),
    });
    if req.multi_select {
        let select_options: Vec<serde_json::Value> = req
            .options
            .iter()
            .enumerate()
            .map(|(index, label)| serde_json::json!({ "label": label, "value": index.to_string() }))
            .collect();
        let mut rows = vec![serde_json::json!({
            "type": COMPONENT_TYPE_ACTION_ROW,
            "components": [{
                "type": COMPONENT_TYPE_STRING_SELECT,
                "custom_id": format!("ask:{ask_id}:sel"),
                "options": select_options,
                "min_values": 1,
                "max_values": req.options.len(),
            }],
        })];
        if req.allow_text {
            rows.push(
                serde_json::json!({ "type": COMPONENT_TYPE_ACTION_ROW, "components": [text_button] }),
            );
        }
        return serde_json::Value::Array(rows);
    }
    let mut buttons: Vec<serde_json::Value> = req
        .options
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
    if req.allow_text {
        buttons.push(text_button);
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

/// `multi_choice` answers are stored as a JSON array string; decode it so the
/// output carries a real array. Every other kind stays a plain string, so the
/// single-choice and text output shapes are unchanged.
fn answer_value(kind: &str, stored: &str) -> serde_json::Value {
    if kind == KIND_MULTI_CHOICE {
        serde_json::from_str(stored)
            .expect("daemon stores multi_choice values as a JSON array of labels")
    } else {
        serde_json::Value::String(stored.to_owned())
    }
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
        AskStatus::Answered => {
            let kind = record
                .kind
                .as_ref()
                .expect("try_answer always sets kind alongside status = 'answered'");
            AskResultData::Answered {
                ask_id: record.ask_id.clone(),
                kind: kind.clone(),
                value: answer_value(
                    kind,
                    record
                        .value
                        .as_deref()
                        .expect("try_answer always sets value alongside status = 'answered'"),
                ),
                answered_by: record
                    .answered_by
                    .clone()
                    .expect("try_answer always sets answered_by alongside status = 'answered'"),
                answered_at: record
                    .answered_at
                    .clone()
                    .expect("try_answer always sets answered_at alongside status = 'answered'"),
            }
        }
        AskStatus::TimedOut => AskResultData::TimedOut {
            ask_id: record.ask_id.clone(),
        },
    }
}

#[cfg(test)]
mod tests;
