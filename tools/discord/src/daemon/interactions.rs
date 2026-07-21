//! Pure interaction-handling logic: given a gateway `INTERACTION_CREATE`
//! payload (as JSON), the shared [`AskStore`], and a [`DiscordApi`], it resolves
//! the answer in the store and emits the right interaction-response callback.
//! No websocket dependency lives here, so every branch is unit-testable with a
//! JSON fixture, a mock API, and an in-memory store (see `daemon::gateway` for
//! the thin wiring that feeds it real events).
//!
//! The one ordering rule that matters: Discord invalidates the interaction
//! token 3 seconds after dispatch, so the local `try_answer` (fast, in-process
//! SQLite) runs *before* the callback — never a slow step between them.

use crate::common::api::DiscordApi;
use crate::common::error::{AppError, ErrorKind};
use crate::common::store::AskStore;

// Interaction-response `type` values (docs.discord.com interactions reference).
const RESPONSE_CHANNEL_MESSAGE: u8 = 4;
const RESPONSE_UPDATE_MESSAGE: u8 = 7;
const RESPONSE_MODAL: u8 = 9;
/// `flags: 64` marks a response ephemeral (visible only to the clicker).
const FLAG_EPHEMERAL: u64 = 64;

// Incoming interaction `type` values.
const TYPE_MESSAGE_COMPONENT: u64 = 3;
const TYPE_MODAL_SUBMIT: u64 = 5;

// Message component `type` values.
const COMPONENT_TEXT_INPUT: u64 = 4;
const COMPONENT_LABEL: u64 = 18;
/// Text input `style: 2` = paragraph (multi-line).
const TEXT_INPUT_STYLE_PARAGRAPH: u64 = 2;
/// Discord caps a modal title at 45 characters.
const MODAL_TITLE_MAX_CHARS: usize = 45;

/// Everything a handler needs from the raw interaction, gathered once so the
/// per-branch handlers stay under the argument ceiling.
struct Ctx<'a> {
    payload: &'a serde_json::Value,
    interaction_id: &'a str,
    token: &'a str,
    now: &'a str,
}

/// A parsed `ask:` custom_id. Anything outside this namespace is not ours.
#[derive(Debug, PartialEq)]
enum CustomId {
    /// `ask:<ask_id>:opt:<index>` — a choice button.
    Choice { ask_id: String, index: usize },
    /// `ask:<ask_id>:text` — the free-text button (opens a modal) and the
    /// modal's own custom_id (arrives again on submit).
    Text { ask_id: String },
}

/// Parses the `ask:` custom_id convention. Returns `None` for any id that is
/// not ours — the daemon shares the gateway with every other interaction, so a
/// non-matching id is ignored rather than treated as an error.
fn parse_custom_id(custom_id: &str) -> Option<CustomId> {
    let rest = custom_id.strip_prefix("ask:")?;
    // ask_id is a Discord snowflake (digits) and never contains ':'.
    let (ask_id, tail) = rest.split_once(':')?;
    if ask_id.is_empty() {
        return None;
    }
    if tail == "text" {
        return Some(CustomId::Text {
            ask_id: ask_id.to_owned(),
        });
    }
    let index = tail.strip_prefix("opt:")?.parse::<usize>().ok()?;
    Some(CustomId::Choice {
        ask_id: ask_id.to_owned(),
        index,
    })
}

/// Entry point: resolve one `INTERACTION_CREATE` payload. Non-`ask:`
/// interactions (and types we never emit) return `Ok(())` without responding.
pub(crate) async fn handle_interaction(
    api: &impl DiscordApi,
    store: &AskStore,
    payload: &serde_json::Value,
    now: &str,
) -> Result<(), AppError> {
    let kind = payload.get("type").and_then(serde_json::Value::as_u64);
    let custom_id = payload
        .get("data")
        .and_then(|d| d.get("custom_id"))
        .and_then(serde_json::Value::as_str);

    let (Some(kind), Some(custom_id)) = (kind, custom_id) else {
        return Ok(());
    };
    let Some(parsed) = parse_custom_id(custom_id) else {
        return Ok(());
    };

    let interaction_id = require_str(payload, "id")?;
    let token = require_str(payload, "token")?;
    let ctx = Ctx {
        payload,
        interaction_id: &interaction_id,
        token: &token,
        now,
    };

    match (kind, parsed) {
        (TYPE_MESSAGE_COMPONENT, CustomId::Choice { ask_id, index }) => {
            handle_choice(api, store, &ctx, &ask_id, index).await
        }
        (TYPE_MESSAGE_COMPONENT, CustomId::Text { ask_id }) => {
            handle_open_modal(api, store, &ctx, &ask_id).await
        }
        (TYPE_MODAL_SUBMIT, CustomId::Text { ask_id }) => {
            handle_modal_submit(api, store, &ctx, &ask_id).await
        }
        // e.g. a modal-submit carrying an `opt:` id, or a component type we
        // never emit — not a shape we produced, so ignore it.
        _ => Ok(()),
    }
}

/// Expires every due ask and disables its message's buttons with the bot token.
/// The interaction token is unusable here (a timed-out ask was never clicked,
/// so no token exists / it has expired), so this edits the message via REST.
/// `ask_id` is the Discord message id (data model), so it doubles as the
/// message to edit.
pub(crate) async fn expire_and_disable(
    api: &impl DiscordApi,
    store: &AskStore,
    now: &str,
) -> Result<usize, AppError> {
    let expired = store.expire_due(now)?;
    let disabled_components = serde_json::json!([]);
    for record in &expired {
        api.edit_message_components(&record.channel_id, &record.ask_id, &disabled_components)
            .await?;
    }
    Ok(expired.len())
}

async fn handle_choice(
    api: &impl DiscordApi,
    store: &AskStore,
    ctx: &Ctx<'_>,
    ask_id: &str,
    index: usize,
) -> Result<(), AppError> {
    let Some(record) = store.get_ask(ask_id)? else {
        return respond(api, ctx, &ephemeral("존재하지 않는 질문입니다.")).await;
    };
    let Some(label) = record.options.get(index) else {
        return respond(api, ctx, &ephemeral("알 수 없는 선택지입니다.")).await;
    };
    let user_id = interaction_user_id(ctx.payload)?;
    // 3-second rule: the local winner check happens before the callback.
    let won = store.try_answer(ask_id, "choice", label, &user_id, ctx.now)?;
    let response = if won {
        update_message_resolved(&format!("선택됨: {label}"))
    } else {
        ephemeral("이미 마감된 질문입니다.")
    };
    respond(api, ctx, &response).await
}

async fn handle_open_modal(
    api: &impl DiscordApi,
    store: &AskStore,
    ctx: &Ctx<'_>,
    ask_id: &str,
) -> Result<(), AppError> {
    let Some(record) = store.get_ask(ask_id)? else {
        return respond(api, ctx, &ephemeral("존재하지 않는 질문입니다.")).await;
    };
    // A real client can only produce this click when `ask create` attached the
    // text button (spec §5: only emitted when `allow_text`), but the daemon
    // must not trust the client's component set as the source of truth — the
    // ask's own record is. A forged/stale `:text` custom_id against an ask
    // that disallows it is rejected here, the same as any other not-adopted
    // outcome.
    if !record.allow_text {
        return respond(
            api,
            ctx,
            &ephemeral("텍스트 응답이 허용되지 않는 질문입니다."),
        )
        .await;
    }
    // Opening the modal adopts no answer; the MODAL_SUBMIT `try_answer` is the
    // authoritative concurrency point, so a race that resolves the ask between
    // open and submit is handled there (the submit loses and gets ephemeral).
    respond(api, ctx, &modal_open(ask_id, &record.question)).await
}

async fn handle_modal_submit(
    api: &impl DiscordApi,
    store: &AskStore,
    ctx: &Ctx<'_>,
    ask_id: &str,
) -> Result<(), AppError> {
    let Some(record) = store.get_ask(ask_id)? else {
        return respond(api, ctx, &ephemeral("존재하지 않는 질문입니다.")).await;
    };
    // Same forged-path rejection as `handle_open_modal` — a submit against an
    // ask that disallows free text must not adopt an answer regardless of how
    // the client got to a MODAL_SUBMIT.
    if !record.allow_text {
        return respond(
            api,
            ctx,
            &ephemeral("텍스트 응답이 허용되지 않는 질문입니다."),
        )
        .await;
    }
    let value = extract_modal_text(ctx.payload.get("data")).ok_or_else(|| {
        AppError::new(
            ErrorKind::Internal,
            "modal submit payload missing text input value",
        )
    })?;
    let user_id = interaction_user_id(ctx.payload)?;
    let won = store.try_answer(ask_id, "text", &value, &user_id, ctx.now)?;
    let response = if won {
        // A modal opened from a message component can UPDATE_MESSAGE the
        // originating (ask) message, disabling its buttons in the same
        // callback — no separate REST edit needed. Verify in E2E (§9).
        update_message_resolved(&format!("답변: {value}"))
    } else {
        ephemeral("이미 마감된 질문입니다.")
    };
    respond(api, ctx, &response).await
}

async fn respond(
    api: &impl DiscordApi,
    ctx: &Ctx<'_>,
    response: &serde_json::Value,
) -> Result<(), AppError> {
    api.create_interaction_response(ctx.interaction_id, ctx.token, response)
        .await
}

/// Ephemeral notice (type 4 + `flags: 64`) — used for every "not adopted"
/// outcome: already-closed asks, unknown ask ids, out-of-range options.
fn ephemeral(text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": RESPONSE_CHANNEL_MESSAGE,
        "data": { "content": text, "flags": FLAG_EPHEMERAL }
    })
}

/// UPDATE_MESSAGE (type 7) that records the outcome and drops the buttons.
/// Empty `components` removes them rather than re-emitting each disabled: the
/// handler has no access to the original button definitions, and removal is
/// strictly stronger than disabling for "no longer answerable".
fn update_message_resolved(text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": RESPONSE_UPDATE_MESSAGE,
        "data": { "content": text, "components": [] }
    })
}

/// MODAL (type 9) with a single Text Input (type 4, paragraph) wrapped in a
/// Label (type 18) — the current documented standard; the old Action Row
/// wrapping is deprecated. Requires E2E verification (§9). The modal reuses the
/// `ask:<id>:text` custom_id so the submit maps back to the ask.
fn modal_open(ask_id: &str, question: &str) -> serde_json::Value {
    let title: String = question.chars().take(MODAL_TITLE_MAX_CHARS).collect();
    serde_json::json!({
        "type": RESPONSE_MODAL,
        "data": {
            "custom_id": format!("ask:{ask_id}:text"),
            "title": title,
            "components": [{
                "type": COMPONENT_LABEL,
                "label": "답변",
                "component": {
                    "type": COMPONENT_TEXT_INPUT,
                    "custom_id": "answer",
                    "style": TEXT_INPUT_STYLE_PARAGRAPH,
                    "required": true
                }
            }]
        }
    })
}

/// Finds the first submitted text input value anywhere in the modal-submit
/// `data`. The Label-vs-Action-Row nesting of the returned components is the
/// exact shape flagged for E2E verification (§9), so this searches the tree for
/// a `type: 4` node with a `value` rather than assuming one fixed depth.
fn extract_modal_text(data: Option<&serde_json::Value>) -> Option<String> {
    fn find(node: &serde_json::Value) -> Option<String> {
        if let Some(obj) = node.as_object() {
            if obj.get("type").and_then(serde_json::Value::as_u64) == Some(COMPONENT_TEXT_INPUT)
                && let Some(value) = obj.get("value").and_then(serde_json::Value::as_str)
            {
                return Some(value.to_owned());
            }
            return obj.values().find_map(find);
        }
        if let Some(arr) = node.as_array() {
            return arr.iter().find_map(find);
        }
        None
    }
    data.and_then(find)
}

/// The responding user's Discord id: `member.user.id` in a guild, `user.id` in
/// a DM. Absent from a component/modal interaction is a contract violation, so
/// it fails fast rather than defaulting.
fn interaction_user_id(payload: &serde_json::Value) -> Result<String, AppError> {
    payload
        .get("member")
        .and_then(|m| m.get("user"))
        .and_then(|u| u.get("id"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            payload
                .get("user")
                .and_then(|u| u.get("id"))
                .and_then(serde_json::Value::as_str)
        })
        .map(str::to_owned)
        .ok_or_else(|| AppError::new(ErrorKind::Internal, "interaction payload missing user id"))
}

fn require_str(payload: &serde_json::Value, key: &str) -> Result<String, AppError> {
    payload
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            AppError::new(
                ErrorKind::Internal,
                format!("interaction payload missing {key}"),
            )
        })
}

#[cfg(test)]
mod tests;
