//! One file per subcommand. Adding a subcommand is adding a file plus a line
//! here — no existing command file changes (OCP).
//!
//! Every handler is a scaffold in this stage and answers
//! `internal: "<name> is not implemented yet"`. That is deliberate: a
//! synthesized success would report board changes that never happened, and
//! `todo!()` would panic past the envelope instead of through it. Failing
//! loudly through the normal error path keeps `--json` verifiable end to end
//! (see `rust-coding.md` principle 1).

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
