#[cfg(test)]
mod envelope;
mod payload;

pub(crate) use crate::common::error::{AppError, ErrorKind};
// Command tests assert exit codes through this surface; production code
// reaches exit codes only via `cli_contract::render`.
#[cfg(test)]
pub(crate) use crate::common::error::exit_code;
// The payload contract is this module's surface: command handlers import the
// data types from here rather than reaching into `payload` directly.
pub(crate) use payload::{
    AddData, AssignData, ClaimedItemData, DoneData, InitData, ItemData, ItemSummaryData, LabelData,
    ListData, MoveData, NextData, Payload, PriorityData, ProjectData, ProjectListData,
    ProjectRmData, ReleaseData,
};

pub use cli_contract::Sink;

/// Renders a command result (see [`cli_contract::render`]), pinned to
/// kanban's payload and error types.
pub fn render(
    command: &str,
    result: &Result<Payload, AppError>,
    json: bool,
) -> (Sink, String, i32) {
    cli_contract::render(command, result, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_mode_error_routes_to_stderr_not_stdout() {
        // "diagnostics on stderr": a human-mode error must never appear as
        // the stdout line, since agents piping stdout would see nothing.
        let err = AppError::new(ErrorKind::NotFound, "no item itm-000017");
        let (sink, text, code) = render("show", &Err(err), false);
        assert_eq!(sink, Sink::Stderr);
        assert_eq!(code, 7);
        assert_eq!(text, "error [not_found]: no item itm-000017");
    }

    #[test]
    fn json_mode_error_still_routes_to_stdout() {
        let err = AppError::new(ErrorKind::Conflict, "state transition rejected");
        let (sink, _, _) = render("done", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
    }

    #[test]
    fn exit_code_matches_across_json_and_human_modes_for_every_kind() {
        // The CLI contract promises identical exit codes regardless of
        // --json; verify render() itself preserves that for every error kind,
        // not just the standalone exit_code() mapping.
        let kinds = [
            ErrorKind::Usage,
            ErrorKind::Config,
            ErrorKind::Api,
            ErrorKind::RateLimit,
            ErrorKind::Network,
            ErrorKind::NotFound,
            ErrorKind::Conflict,
            ErrorKind::Internal,
        ];
        for kind in kinds {
            let err = AppError::new(kind, "boom");
            let (_, _, json_code) = render("next", &Err(err.clone()), true);
            let (_, _, human_code) = render("next", &Err(err), false);
            assert_eq!(
                json_code, human_code,
                "exit code diverged between modes for {kind:?}"
            );
        }
    }
}
