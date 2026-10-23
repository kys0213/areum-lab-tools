//! The resident daemon (docs/doppel-protocol.md §6): one router served on the
//! Unix socket (plugins, CLI) and on a loopback TCP port (web UI).

pub(crate) mod api;
pub(crate) mod auto;
mod board;
pub(crate) mod brief;
mod guard;
mod hub;
pub(crate) mod launch;
pub(crate) mod listen;
pub(crate) mod model_call;
pub(crate) mod proposal;
mod routes;
mod runs;
pub(crate) mod state;
mod ui;

#[cfg(test)]
mod tests;
