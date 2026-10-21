//! The resident daemon (docs/doppel-protocol.md §6): one router served on the
//! Unix socket (plugins, CLI) and on a loopback TCP port (web UI).

pub(crate) mod api;
pub(crate) mod brief;
mod guard;
mod hub;
pub(crate) mod listen;
mod routes;
pub(crate) mod state;
mod ui;

#[cfg(test)]
mod tests;
