//! Thin twilight-gateway wiring: keeps a `Shard` connected and feeds each
//! `INTERACTION_CREATE` to the pure [`interactions`] logic, plus runs the
//! periodic expire/cleanup passes. twilight owns Resume/reconnect/heartbeat and
//! their backoff internally — our only responsibility is to stop the daemon on
//! a fatal close code (the shard's stream ends only then) rather than looping
//! into Discord's 1000-per-24h Identify limit.
//!
//! This layer is deliberately kept minimal because it cannot be unit-tested
//! without a live gateway; the behaviour that carries logic lives in
//! [`interactions`] (pure, fully tested). Real-connection coverage is the
//! follow-up E2E step (spec §8).

use std::time::Duration;

use tokio::signal::unix::{SignalKind, signal};
use twilight_gateway::{
    Event, EventTypeFlags, Intents, Shard, ShardId, ShardState, StreamExt as _,
};
use twilight_model::gateway::payload::incoming::InteractionCreate;

use crate::common::api::DiscordApi;
use crate::common::error::{AppError, ErrorKind};
use crate::common::store::AskStore;
use crate::common::time::now_rfc3339;

use super::interactions::{expire_and_disable, handle_interaction};

/// How often to scan for asks whose `timeout_at` has passed. A few seconds is
/// timely enough for a human-in-the-loop deadline without busy-polling.
const EXPIRE_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Retention cleanup cadence. `tokio::time::interval` fires once immediately, so
/// this also gives the startup cleanup the spec calls for.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Days of resolved-ask history to keep (spec §3 retention default).
const RETENTION_DAYS: u32 = 30;

/// Runs the daemon event loop until SIGTERM (graceful `Ok`) or a fatal gateway
/// close (`Err`, no retry). Holds the `!Sync` [`AskStore`] across `.await`
/// points on a single task — never `tokio::spawn`ed — so the connection's
/// non-`Sync`-ness is not a problem.
pub(crate) async fn run(
    api: &impl DiscordApi,
    store: &AskStore,
    token: String,
) -> Result<(), AppError> {
    let mut shard = Shard::new(ShardId::ONE, token, Intents::empty());
    let mut sigterm = signal(SignalKind::terminate()).map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("failed to install SIGTERM handler: {e}"),
        )
    })?;
    let mut expire_tick = tokio::time::interval(EXPIRE_POLL_INTERVAL);
    let mut cleanup_tick = tokio::time::interval(CLEANUP_INTERVAL);

    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                // Graceful stop: the select! arms are cancel-safe, so an
                // in-flight callback is either already committed to the store
                // or simply retried by Discord — nothing to unwind here.
                return Ok(());
            }
            _ = expire_tick.tick() => {
                if let Err(err) = expire_and_disable(api, store, &now_rfc3339()).await {
                    eprintln!("daemon: expire pass failed: {}", err.to_human());
                }
            }
            _ = cleanup_tick.tick() => {
                if let Err(err) = store.cleanup(RETENTION_DAYS, &now_rfc3339()) {
                    eprintln!("daemon: retention cleanup failed: {}", err.to_human());
                }
            }
            item = shard.next_event(EventTypeFlags::INTERACTION_CREATE) => {
                match item {
                    Some(Ok(Event::InteractionCreate(interaction))) => {
                        // One malformed/failed interaction must not take down the
                        // daemon — log and keep serving the rest.
                        if let Err(err) = dispatch(api, store, &interaction).await {
                            eprintln!("daemon: interaction handling failed: {}", err.to_human());
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        eprintln!("daemon: gateway receive error: {err}");
                    }
                    None => return Err(fatal_close_error(shard.state())),
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

/// Maps the terminal shard state to an error. twilight ends the stream only on
/// a non-reconnectable close code (4004 auth, 4013/4014 intents), so this is an
/// auth-class failure the daemon must not retry.
fn fatal_close_error(state: ShardState) -> AppError {
    match state {
        ShardState::FatallyClosed => AppError::new(
            ErrorKind::Auth,
            "gateway fatally closed (authentication failed or invalid intents); daemon will not retry",
        ),
        other => AppError::new(
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
        let err = fatal_close_error(ShardState::FatallyClosed);
        assert_eq!(err.kind, ErrorKind::Auth);
        assert!(err.message.contains("will not retry"));
    }

    #[test]
    fn unexpected_stream_end_maps_to_network_error() {
        let err = fatal_close_error(ShardState::Active);
        assert_eq!(err.kind, ErrorKind::Network);
    }
}
