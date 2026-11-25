//! One file per subcommand group. Adding a subcommand is adding a file (or a
//! function in its group's file) plus a line here — no other command file
//! changes (OCP).
//!
//! Every handler opens its own store from the database path `main.rs` passes
//! in, and answers either a [`crate::output::Payload`] variant or an
//! [`crate::output::AppError`] whose kind decides the exit code. Nothing here
//! prints: rendering is the output layer's job.

mod ask;
mod attach;
mod card;
mod category;
mod convert;
mod daemon;
mod decision;
mod golden;
mod ingest;
mod init;
mod project;
mod remote;
mod review;
mod runtime;
mod search;
mod session;
mod stats;

#[cfg(test)]
pub(crate) mod testutil;

pub(crate) use ask::{run_ask_answer, run_ask_list, run_ask_wait};
pub(crate) use attach::run_attach;
pub(crate) use card::{
    CardInput, run_card_add, run_card_dispatch, run_card_done, run_card_list, run_card_propose,
    run_card_release, run_card_show, run_card_triage,
};
pub(crate) use category::{run_category_add, run_category_list, run_category_retire};
pub(crate) use daemon::{run_daemon_run, run_daemon_start, run_daemon_status, run_daemon_stop};
pub(crate) use decision::{run_decision_categorize, run_decision_list, run_decision_show};
pub(crate) use golden::{
    run_golden_confirm, run_golden_list, run_golden_propose, run_golden_retire,
};
pub(crate) use ingest::{run_ingest_plans, run_ingest_sessions};
pub(crate) use init::run_init;
pub(crate) use project::{run_project_add, run_project_list};
pub(crate) use review::run_review;
pub(crate) use search::run_search;
pub(crate) use session::run_session_list;
pub(crate) use stats::run_stats;
