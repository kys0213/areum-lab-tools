mod envelope;
mod payload;

pub(crate) use crate::common::error::{AppError, ErrorKind, exit_code};
// The payload contract is the module's public surface, but the command
// handlers that construct it are still scaffolds — nothing imports these yet.
#[allow(unused_imports)]
pub(crate) use payload::{
    AddData, AssignData, ClaimedItemData, DoneData, InitData, ItemData, ItemSummaryData, LabelData,
    ListData, MoveData, NextData, Payload, PriorityData, ProjectData, ProjectListData,
    ProjectRmData, ReleaseData,
};

/// Which stream a rendered line belongs on. Human-mode errors are
/// diagnostics and must not pollute stdout; every other case is a result
/// payload and belongs on stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sink {
    Stdout,
    Stderr,
}

/// Renders a command result to the stream it belongs on, the line to print,
/// and the process exit code. Human text by default; the JSON envelope when
/// `json` is set. In human mode, errors go to stderr and stdout stays empty
/// ("diagnostics on stderr"); the JSON envelope always goes to stdout.
pub fn render(
    command: &str,
    result: &Result<Payload, AppError>,
    json: bool,
) -> (Sink, String, i32) {
    match result {
        Ok(payload) => {
            let text = if json {
                envelope::success_json(command, payload)
            } else {
                payload.to_human()
            };
            (Sink::Stdout, text, 0)
        }
        Err(err) => {
            let text = if json {
                envelope::error_json(command, err)
            } else {
                err.to_human()
            };
            let sink = if json { Sink::Stdout } else { Sink::Stderr };
            (sink, text, exit_code(err))
        }
    }
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
