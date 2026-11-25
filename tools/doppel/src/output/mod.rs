mod board;
mod envelope;
mod payload;

pub(crate) use crate::common::error::{AppError, exit_code};
pub(crate) use board::{CardListData, ProjectListData};
// The payload contract is this module's surface: command handlers import the
// data types from here rather than reaching into `payload` directly.
pub(crate) use payload::{
    AskListData, AttachData, CategorizeData, CategoryData, CategoryListData, CategoryStatsData,
    DaemonData, DecisionData, DecisionListData, DecisionSummaryData, GoldenData, GoldenListData,
    GoldenSummaryData, IngestData, InitData, OptionData, Payload, ProposalStatsData, ReviewData,
    SearchData, SearchHitData, SessionListData, StatsData,
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
/// `json` is set.
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
    use crate::common::error::ErrorKind;

    #[test]
    fn human_mode_error_routes_to_stderr_not_stdout() {
        let err = AppError::new(ErrorKind::NotFound, "no decision dec-000017");
        let (sink, text, code) = render("decision show", &Err(err), false);
        assert_eq!(sink, Sink::Stderr);
        assert_eq!(code, 7);
        assert_eq!(text, "error [not_found]: no decision dec-000017");
    }

    #[test]
    fn exit_code_matches_across_json_and_human_modes() {
        for kind in [
            ErrorKind::Usage,
            ErrorKind::Config,
            ErrorKind::Network,
            ErrorKind::NotFound,
            ErrorKind::Conflict,
            ErrorKind::Internal,
        ] {
            let err = AppError::new(kind, "x");
            let (_, _, human) = render("review", &Err(err.clone()), false);
            let (_, _, json) = render("review", &Err(err), true);
            assert_eq!(human, json, "{kind:?}");
        }
    }

    #[test]
    fn human_success_goes_to_stdout() {
        let payload = Payload::Init(InitData {
            path: "/x/doppel.db".into(),
            created: false,
            seeded_categories: vec![],
        });
        let (sink, text, code) = render("init", &Ok(payload), false);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(text, "doppel already initialized: /x/doppel.db");
    }
}
