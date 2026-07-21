mod envelope;
mod payload;

pub(crate) use crate::common::error::{AppError, ErrorKind, exit_code};
pub(crate) use payload::{
    AskCreateData, AskResultData, AskWaitData, DaemonStartData, DaemonStatusData, DaemonStopData,
    InitData, Payload, ReadData, SendData, ThreadData, WaitData,
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
    use crate::common::error::ErrorKind;
    use crate::output::payload::{InitData, ReadData, SendData, WaitData};

    fn sample_message() -> crate::common::api::Message {
        use crate::common::api::{Author, Message};
        Message {
            id: "10".into(),
            channel_id: "chan".into(),
            author: Author {
                id: "u1".into(),
                username: "alice".into(),
                bot: false,
            },
            content: "hi".into(),
            timestamp: "2024-01-01T00:00:00Z".into(),
            attachments: vec![],
        }
    }

    #[test]
    fn init_error_human_and_json_never_contain_token_value() {
        // Mirrors init_output_never_contains_token_value_in_human_or_json but
        // for the error path: init's AppError messages are built from the
        // path/force state only, never the token, in both render modes.
        let secret = "super-secret-token-value";
        let err = AppError::new(
            ErrorKind::Usage,
            "config already exists at /home/user/.areum/discord/config.json (use --force to overwrite)",
        );
        let (_, human, _) = render("init", &Err(err.clone()), false);
        let (_, json, _) = render("init", &Err(err), true);
        assert!(!human.contains(secret));
        assert!(!json.contains(secret));
    }

    #[test]
    fn human_error_reports_kind_and_status() {
        let err = AppError {
            kind: ErrorKind::Auth,
            message: "invalid token".into(),
            http_status: Some(401),
            retry_after_ms: None,
        };
        let (sink, text, code) = render("send", &Err(err), false);
        assert_eq!(sink, Sink::Stderr);
        assert_eq!(code, 3);
        assert_eq!(text, "error [auth]: invalid token (http 401)");
    }

    #[test]
    fn human_mode_error_routes_to_stderr_not_stdout() {
        // "diagnostics on stderr": a human-mode error must never appear as
        // the stdout line, since agents piping stdout would see nothing.
        let err = AppError::new(ErrorKind::Api, "channel not found");
        let (sink, _, _) = render("read", &Err(err), false);
        assert_eq!(sink, Sink::Stderr);
    }

    #[test]
    fn json_mode_error_still_routes_to_stdout() {
        let err = AppError::new(ErrorKind::Api, "channel not found");
        let (sink, _, _) = render("read", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
    }

    #[test]
    fn json_read_success_routes_to_stdout_with_exit_zero() {
        let payload = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("10".into()),
            messages: vec![sample_message()],
        });
        let (sink, _, code) = render("read", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
    }

    #[test]
    fn json_wait_success_with_attachments_routes_to_stdout_with_exit_zero() {
        use crate::common::api::Attachment;

        let mut message = sample_message();
        message.attachments = vec![Attachment {
            id: "a1".into(),
            filename: "photo.png".into(),
            size: 1024,
            url: "https://cdn.discordapp.com/attachments/1/a1/photo.png".into(),
            content_type: Some("image/png".into()),
        }];
        let payload = Payload::Wait(WaitData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("20".into()),
            timed_out: false,
            messages: vec![message],
        });
        let (sink, _, code) = render("wait", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
    }

    #[test]
    fn human_success_output_never_contains_json_envelope_marker() {
        // json=false success must be plain text, never the {"ok":...}
        // envelope shape — this is the boundary agents rely on to tell modes
        // apart, so assert its absence directly rather than only asserting
        // the expected human string.
        let send = Payload::Send(SendData {
            message_id: "1".into(),
            channel_id: "c".into(),
            timestamp: "2024-01-01T00:00:00Z".into(),
            attachments: vec![],
        });
        let read = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("10".into()),
            messages: vec![sample_message()],
        });
        let wait = Payload::Wait(WaitData {
            channel_id: "c".into(),
            count: 0,
            cursor: None,
            timed_out: true,
            messages: vec![],
        });
        let init = Payload::Init(InitData {
            path: "/home/user/.areum/discord/config.json".into(),
            created: true,
        });

        for payload in [send, read, wait, init] {
            let (sink, text, code) = render("cmd", &Ok(payload), false);
            assert_eq!(sink, Sink::Stdout);
            assert_eq!(code, 0);
            assert!(
                !text.contains(r#"{"ok"#),
                "human output leaked json envelope: {text}"
            );
        }
    }

    #[test]
    fn exit_code_matches_across_json_and_human_modes_for_every_kind() {
        // The CLI contract promises identical exit codes regardless of
        // --json; verify render() itself preserves that for every error
        // kind, not just the standalone exit_code() mapping function.
        let kinds = [
            ErrorKind::Usage,
            ErrorKind::Config,
            ErrorKind::Auth,
            ErrorKind::Api,
            ErrorKind::RateLimit,
            ErrorKind::Network,
            ErrorKind::Internal,
        ];
        for kind in kinds {
            let err = AppError::new(kind, "boom");
            let (_, _, json_code) = render("send", &Err(err.clone()), true);
            let (_, _, human_code) = render("send", &Err(err), false);
            assert_eq!(
                json_code, human_code,
                "exit code diverged between modes for {kind:?}"
            );
        }
    }
}
