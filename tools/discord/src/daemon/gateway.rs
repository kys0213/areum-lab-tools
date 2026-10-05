//! Thin twilight-gateway wiring: keeps a `Shard` connected, feeds each
//! `INTERACTION_CREATE` to the pure [`interactions`] logic and — when
//! `on_message` is configured — each `MESSAGE_CREATE` to the
//! [`message_trigger`], plus runs the periodic expire/cleanup passes. twilight
//! owns Resume/reconnect/heartbeat and their backoff internally — our only
//! responsibility is to stop the daemon on a fatal close code (the shard's
//! stream ends only then) rather than looping into Discord's 1000-per-24h
//! Identify limit.
//!
//! This layer is deliberately kept minimal because it cannot be unit-tested
//! without a live gateway; the behaviour that carries logic lives in
//! [`interactions`] and [`message_trigger`] (pure, fully tested).
//! Real-connection coverage is the follow-up E2E step (spec §8).

use std::time::Duration;

use tokio::signal::unix::{SignalKind, signal};
use twilight_gateway::{
    Event, EventTypeFlags, Intents, Shard, ShardId, ShardState, StreamExt as _,
};
use twilight_model::gateway::CloseCode;
use twilight_model::gateway::payload::incoming::{InteractionCreate, MessageCreate};

use crate::common::api::DiscordApi;
use crate::common::error::{AppError, ErrorKind};
use crate::common::store::AskStore;
use crate::common::time::now_rfc3339;

use super::hook_runner::{HookRunner, ProcessHookRunner};
use super::interactions::{ExpireRetryQueue, expire_and_disable, handle_interaction};
use super::message_trigger::{MessageTrigger, MessageTriggerSettings};

/// How often to scan for asks whose `timeout_at` has passed. A few seconds is
/// timely enough for a human-in-the-loop deadline without busy-polling.
const EXPIRE_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Retention cleanup cadence. `tokio::time::interval` fires once immediately, so
/// this also gives the startup cleanup the spec calls for.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Days of resolved-ask history to keep (spec §3 retention default).
const RETENTION_DAYS: u32 = 30;

/// Events the loop parses. `MESSAGE_CREATE` only ever arrives when the
/// message intents are requested, so listing it unconditionally is harmless.
const WANTED_EVENTS: EventTypeFlags = EventTypeFlags::INTERACTION_CREATE
    .union(EventTypeFlags::READY)
    .union(EventTypeFlags::MESSAGE_CREATE);

/// Runs the daemon event loop until SIGTERM (graceful `Ok`) or a fatal gateway
/// close (`Err`, no retry). Holds the `!Sync` [`AskStore`] across `.await`
/// points on a single task — never `tokio::spawn`ed — so the connection's
/// non-`Sync`-ness is not a problem.
pub(crate) async fn run(
    api: &impl DiscordApi,
    store: &AskStore,
    token: String,
    message_trigger: Option<MessageTriggerSettings>,
) -> Result<(), AppError> {
    let intents = gateway_intents(message_trigger.is_some());
    let mut shard = Shard::new(ShardId::ONE, token, intents);
    let mut message_trigger =
        message_trigger.map(|settings| MessageTrigger::new(settings, ProcessHookRunner));
    // The code of the most recent gateway close frame; twilight emits it just
    // before ending the stream, so it explains a fatal close.
    let mut last_close_code: Option<u16> = None;
    let mut sigterm = signal(SignalKind::terminate()).map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("failed to install SIGTERM handler: {e}"),
        )
    })?;
    let mut expire_tick = tokio::time::interval(EXPIRE_POLL_INTERVAL);
    let mut cleanup_tick = tokio::time::interval(CLEANUP_INTERVAL);
    // Lives across ticks (not re-created per tick) so a transient edit
    // failure queued on one tick is retried on the next.
    let mut expire_retry_queue = ExpireRetryQueue::new();

    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                // Graceful stop: the select! arms are cancel-safe, so an
                // in-flight callback is either already committed to the store
                // or simply retried by Discord — nothing to unwind here.
                return Ok(());
            }
            _ = expire_tick.tick() => {
                if let Err(err) = expire_and_disable(api, store, &now_rfc3339(), &mut expire_retry_queue).await {
                    eprintln!("daemon: expire pass failed: {}", err.to_human());
                }
            }
            _ = cleanup_tick.tick() => {
                if let Err(err) = store.cleanup(RETENTION_DAYS, &now_rfc3339()) {
                    eprintln!("daemon: retention cleanup failed: {}", err.to_human());
                }
            }
            item = shard.next_event(WANTED_EVENTS) => {
                match item {
                    Some(Ok(Event::InteractionCreate(interaction))) => {
                        // One malformed/failed interaction must not take down the
                        // daemon — log and keep serving the rest.
                        if let Err(err) = dispatch(api, store, &interaction).await {
                            eprintln!("daemon: interaction handling failed: {}", err.to_human());
                        }
                    }
                    Some(Ok(Event::Ready(ready))) => {
                        if let Some(trigger) = message_trigger.as_mut() {
                            trigger.set_bot_user_id(ready.user.id.to_string());
                        }
                    }
                    Some(Ok(Event::MessageCreate(message))) => {
                        if let Some(trigger) = message_trigger.as_mut() {
                            on_message_create(api, trigger, &message).await;
                        }
                    }
                    Some(Ok(Event::GatewayClose(frame))) => {
                        last_close_code = frame.map(|f| f.code);
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        eprintln!("daemon: gateway receive error: {err}");
                    }
                    None => return Err(fatal_close_error(shard.state(), last_close_code)),
                }
            }
        }
    }
}

/// Serializes twilight's typed interaction back to the raw JSON shape the pure
/// logic consumes, then hands it off. The round-trip is faithful because
/// twilight's `Interaction` serializes with Discord's field names (`type`,
/// `data.custom_id`, `member.user.id`).
async fn dispatch(
    api: &impl DiscordApi,
    store: &AskStore,
    interaction: &InteractionCreate,
) -> Result<(), AppError> {
    let payload = serde_json::to_value(&interaction.0).map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("failed to serialize gateway interaction: {e}"),
        )
    })?;
    handle_interaction(api, store, &payload, &now_rfc3339()).await
}

/// Message intents are requested only when `on_message` is configured.
/// `MESSAGE_CONTENT` is privileged: if it is not enabled in the Developer
/// Portal the gateway closes with 4014, so a daemon that does not use message
/// detection must not ask for it.
fn gateway_intents(message_detection: bool) -> Intents {
    if message_detection {
        Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT
    } else {
        Intents::empty()
    }
}

/// Runs one `MESSAGE_CREATE` through the trigger. Failures are logged and the
/// loop keeps serving — a bad message or hook must not stop the daemon.
async fn on_message_create<R: HookRunner>(
    api: &impl DiscordApi,
    trigger: &mut MessageTrigger<R>,
    message: &MessageCreate,
) {
    let result = match serde_json::to_value(&message.0) {
        Ok(payload) => trigger.handle(api, &payload).await,
        Err(e) => Err(AppError::new(
            ErrorKind::Internal,
            format!("failed to serialize gateway message: {e}"),
        )),
    };
    match result {
        Ok(Some(kind)) => eprintln!(
            "daemon: message {} in {} -> on_message ({kind:?})",
            message.id, message.channel_id
        ),
        Ok(None) => {}
        Err(err) => eprintln!("daemon: message handling failed: {}", err.to_human()),
    }
}

/// Maps the terminal shard state to an error. twilight ends the stream only on
/// a non-reconnectable close code (4004 auth, 4013/4014 intents), so this is a
/// failure the daemon must not retry. An intents close names the Portal fix,
/// since only message detection requests a privileged intent.
fn fatal_close_error(state: ShardState, close_code: Option<u16>) -> AppError {
    let close = close_code.map(CloseCode::try_from);
    match (state, close) {
        (
            ShardState::FatallyClosed,
            Some(Ok(code @ (CloseCode::DisallowedIntents | CloseCode::InvalidIntents))),
        ) => AppError::new(
            ErrorKind::Config,
            format!(
                "gateway closed with {} ({code}): message detection (`on_message` in config) \
                 needs the privileged Message Content intent. Enable \"Message Content Intent\" \
                 under Bot > Privileged Gateway Intents in the Discord Developer Portal, or remove \
                 `on_message` from config; daemon will not retry",
                code as u16
            ),
        ),
        (ShardState::FatallyClosed, _) => AppError::new(
            ErrorKind::Auth,
            "gateway fatally closed (authentication failed or invalid intents); daemon will not retry",
        ),
        (other, _) => AppError::new(
            ErrorKind::Network,
            format!("gateway stream ended unexpectedly (state: {other:?})"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fatal_close_maps_to_non_retryable_auth_error() {
        let err = fatal_close_error(ShardState::FatallyClosed, Some(4004));
        assert_eq!(err.kind, ErrorKind::Auth);
        assert!(err.message.contains("will not retry"));
    }

    #[test]
    fn disallowed_intents_close_names_the_portal_fix() {
        let err = fatal_close_error(ShardState::FatallyClosed, Some(4014));
        assert_eq!(err.kind, ErrorKind::Config);
        assert!(err.message.contains("4014"));
        assert!(err.message.contains("Message Content Intent"));
        assert!(err.message.contains("Developer Portal"));
        assert!(err.message.contains("will not retry"));
    }

    #[test]
    fn invalid_intents_close_also_names_the_portal_fix() {
        let err = fatal_close_error(ShardState::FatallyClosed, Some(4013));
        assert_eq!(err.kind, ErrorKind::Config);
        assert!(err.message.contains("Message Content Intent"));
    }

    #[test]
    fn unexpected_stream_end_maps_to_network_error() {
        let err = fatal_close_error(ShardState::Active, None);
        assert_eq!(err.kind, ErrorKind::Network);
    }

    #[test]
    fn no_message_intents_without_on_message() {
        assert_eq!(gateway_intents(false), Intents::empty());
    }

    #[test]
    fn message_intents_requested_with_on_message() {
        assert_eq!(
            gateway_intents(true),
            Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT
        );
    }
}
