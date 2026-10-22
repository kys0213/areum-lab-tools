use serde::Serialize;

use crate::envelope::{error_json, success_json};

/// Plain-text form of a result, printed when `--json` is not set.
pub trait HumanText {
    fn to_human(&self) -> String;
}

/// Process exit code of a failure. The same in human and JSON mode.
pub trait ExitStatus {
    fn exit_code(&self) -> i32;
}

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
pub fn render<P, E>(command: &str, result: &Result<P, E>, json: bool) -> (Sink, String, i32)
where
    P: Serialize + HumanText,
    E: Serialize + HumanText + ExitStatus,
{
    match result {
        Ok(payload) => {
            let text = if json {
                success_json(command, payload)
            } else {
                payload.to_human()
            };
            (Sink::Stdout, text, 0)
        }
        Err(err) => {
            let text = if json {
                error_json(command, err)
            } else {
                err.to_human()
            };
            let sink = if json { Sink::Stdout } else { Sink::Stderr };
            (sink, text, err.exit_code())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Done {
        id: &'static str,
    }

    impl HumanText for Done {
        fn to_human(&self) -> String {
            format!("done {}", self.id)
        }
    }

    #[derive(Serialize)]
    struct Failed {
        kind: &'static str,
        message: &'static str,
    }

    impl HumanText for Failed {
        fn to_human(&self) -> String {
            format!("error [{}]: {}", self.kind, self.message)
        }
    }

    impl ExitStatus for Failed {
        fn exit_code(&self) -> i32 {
            7
        }
    }

    fn ok() -> Result<Done, Failed> {
        Ok(Done { id: "x-1" })
    }

    fn err() -> Result<Done, Failed> {
        Err(Failed {
            kind: "not_found",
            message: "no x-1",
        })
    }

    #[test]
    fn human_success_is_plain_text_on_stdout_with_exit_zero() {
        assert_eq!(
            render("done", &ok(), false),
            (Sink::Stdout, "done x-1".to_owned(), 0)
        );
    }

    #[test]
    fn json_success_is_the_envelope_on_stdout_with_exit_zero() {
        assert_eq!(
            render("done", &ok(), true),
            (
                Sink::Stdout,
                r#"{"ok":true,"command":"done","data":{"id":"x-1"}}"#.to_owned(),
                0
            )
        );
    }

    #[test]
    fn human_error_goes_to_stderr_with_its_exit_code() {
        assert_eq!(
            render("done", &err(), false),
            (Sink::Stderr, "error [not_found]: no x-1".to_owned(), 7)
        );
    }

    #[test]
    fn json_error_stays_on_stdout_with_the_same_exit_code() {
        assert_eq!(
            render("done", &err(), true),
            (
                Sink::Stdout,
                r#"{"ok":false,"command":"done","error":{"kind":"not_found","message":"no x-1"}}"#
                    .to_owned(),
                7
            )
        );
    }
}
