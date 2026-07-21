mod gateway;
mod interactions;

pub(crate) use gateway::run;
// Re-exported for the CLI<->daemon contract round-trip tests in
// `commands::ask::tests`, which call the daemon's real interaction handler
// against `ask create`'s real output rather than a hand-built fixture.
// `gateway.rs` (the only production caller) reaches these via
// `super::interactions` directly, so this path is test-only.
#[cfg(test)]
pub(crate) use interactions::{expire_and_disable, handle_interaction};
