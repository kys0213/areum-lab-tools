//! One file per subcommand. Adding a subcommand is adding a file plus a line
//! here — no existing command file changes (OCP).
//!
//! Every handler resolves its own store from the board path `main.rs` passes
//! in, and answers either a [`crate::output::Payload`] variant or an
//! [`crate::output::AppError`] whose kind decides the exit code. Nothing here
//! prints: rendering is the output layer's job.

mod add;
mod assign;
mod done;
mod init;
mod list;
mod move_item;
mod next;
mod priority;
mod project;
mod release;
mod show;

#[cfg(test)]
pub(crate) mod testutil;

pub(crate) use add::run_add;
pub(crate) use assign::run_assign;
pub(crate) use done::run_done;
pub(crate) use init::run_init;
pub(crate) use list::run_list;
pub(crate) use move_item::run_move;
pub(crate) use next::run_next;
pub(crate) use priority::run_priority;
pub(crate) use project::{run_project_add, run_project_list, run_project_rm};
pub(crate) use release::run_release;
pub(crate) use show::run_show;
