//! Output contract shared by every `tools/*` CLI: the `--json` envelope, the
//! stdout/stderr routing with its exit code, the error kinds whose names and
//! exit codes are common to all tools, and canonical UTC timestamps.
//!
//! Each tool keeps its own `Payload`, `ErrorKind` and `AppError`; this crate
//! only fixes the shape they are rendered into.

mod envelope;
mod kind;
mod render;
pub mod time;

pub use envelope::{error_json, success_json};
pub use kind::CommonKind;
pub use render::{ExitStatus, HumanText, Sink, render};
