use serde::Serialize;

use crate::common::error::AppError;
use crate::output::payload::Payload;

#[derive(Serialize)]
struct SuccessEnvelope<'a> {
    ok: bool,
    command: &'a str,
    data: &'a Payload,
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    ok: bool,
    command: &'a str,
    error: &'a AppError,
}

// Called from `crate::output::render`, the parent module.
pub(super) fn success_json(command: &str, data: &Payload) -> String {
    let envelope = SuccessEnvelope {
        ok: true,
        command,
        data,
    };
    serde_json::to_string(&envelope)
        .expect("success envelope serialization is infallible for plain data")
}

pub(super) fn error_json(command: &str, error: &AppError) -> String {
    let envelope = ErrorEnvelope {
        ok: false,
        command,
        error,
    };
    serde_json::to_string(&envelope)
        .expect("error envelope serialization is infallible for plain data")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::api::{Attachment, Author, Message};
    use crate::common::error::ErrorKind;
    use crate::output::payload::{InitData, ReadData, SendData, ThreadData, WaitData};
    use crate::output::{Sink, render};

    fn sample_message() -> Message {
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

    fn sample_attachment() -> Attachment {
        Attachment {
            id: "a1".into(),
            filename: "photo.png".into(),
            size: 1024,
            url: "https://cdn.discordapp.com/attachments/1/a1/photo.png".into(),
            content_type: Some("image/png".into()),
        }
    }

    #[test]
    fn send_success_matches_contract() {
        let payload = Payload::Send(SendData {
            message_id: "123".into(),
            channel_id: "456".into(),
            timestamp: "2024-01-01T00:00:00Z".into(),
            attachments: vec![],
        });
        let (sink, json, code) = render("send", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"send","data":{"message_id":"123","channel_id":"456","timestamp":"2024-01-01T00:00:00Z","attachments":[]}}"#
        );
    }

    #[test]
    fn send_success_with_attachments_includes_full_fields() {
        let payload = Payload::Send(SendData {
            message_id: "123".into(),
            channel_id: "456".into(),
            timestamp: "2024-01-01T00:00:00Z".into(),
            attachments: vec![sample_attachment()],
        });
        let (sink, json, code) = render("send", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"send","data":{"message_id":"123","channel_id":"456","timestamp":"2024-01-01T00:00:00Z","attachments":[{"id":"a1","filename":"photo.png","size":1024,"url":"https://cdn.discordapp.com/attachments/1/a1/photo.png","content_type":"image/png"}]}}"#
        );
    }

    #[test]
    fn init_success_matches_contract() {
        let payload = Payload::Init(InitData {
            path: "/home/user/.areum/discord/config.json".into(),
            created: true,
        });
        let (sink, json, code) = render("init", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"init","data":{"path":"/home/user/.areum/discord/config.json","created":true}}"#
        );
    }

    #[test]
    fn init_overwrite_reports_created_false() {
        let payload = Payload::Init(InitData {
            path: "/home/user/.areum/discord/config.json".into(),
            created: false,
        });
        let (_, json, _) = render("init", &Ok(payload), true);
        assert!(json.contains(r#""created":false"#));
    }

    #[test]
    fn init_overwrite_success_matches_contract_exactly() {
        // init_overwrite_reports_created_false above only spot-checks the
        // `created` field; this pins the whole envelope for the --force
        // (created:false) branch the same way init_success_matches_contract
        // does for the fresh-create branch.
        let payload = Payload::Init(InitData {
            path: "/home/user/.areum/discord/config.json".into(),
            created: false,
        });
        let (sink, json, code) = render("init", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"init","data":{"path":"/home/user/.areum/discord/config.json","created":false}}"#
        );
    }

    #[test]
    fn init_error_json_matches_contract() {
        let err = AppError::new(
            ErrorKind::Usage,
            "config already exists at /home/user/.areum/discord/config.json (use --force to overwrite)",
        );
        let (sink, json, code) = render("init", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 2);
        assert_eq!(
            json,
            r#"{"ok":false,"command":"init","error":{"kind":"usage","message":"config already exists at /home/user/.areum/discord/config.json (use --force to overwrite)"}}"#
        );
    }

    #[test]
    fn read_success_has_no_timed_out_field() {
        let payload = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("10".into()),
            messages: vec![sample_message()],
        });
        let (_, json, _) = render("read", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"read","data":{"channel_id":"c","count":1,"cursor":"10","messages":[{"id":"10","channel_id":"chan","author":{"id":"u1","username":"alice","bot":false},"content":"hi","timestamp":"2024-01-01T00:00:00Z","attachments":[]}]}}"#
        );
    }

    #[test]
    fn read_success_with_attachment_bearing_message_matches_contract() {
        let mut message = sample_message();
        message.attachments = vec![Attachment {
            id: "a2".into(),
            filename: "notes.txt".into(),
            size: 42,
            url: "https://cdn.discordapp.com/attachments/1/a2/notes.txt".into(),
            content_type: None,
        }];
        let payload = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("10".into()),
            messages: vec![message],
        });
        let (_, json, _) = render("read", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"read","data":{"channel_id":"c","count":1,"cursor":"10","messages":[{"id":"10","channel_id":"chan","author":{"id":"u1","username":"alice","bot":false},"content":"hi","timestamp":"2024-01-01T00:00:00Z","attachments":[{"id":"a2","filename":"notes.txt","size":42,"url":"https://cdn.discordapp.com/attachments/1/a2/notes.txt"}]}]}}"#
        );
    }

    #[test]
    fn read_cursor_none_serializes_as_null() {
        let payload = Payload::Read(ReadData {
            channel_id: "c".into(),
            count: 0,
            cursor: None,
            messages: vec![],
        });
        let (_, json, _) = render("read", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"read","data":{"channel_id":"c","count":0,"cursor":null,"messages":[]}}"#
        );
    }

    #[test]
    fn wait_success_includes_timed_out() {
        let payload = Payload::Wait(WaitData {
            channel_id: "c".into(),
            count: 0,
            cursor: None,
            timed_out: true,
            messages: vec![],
        });
        let (sink, json, code) = render("wait", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"wait","data":{"channel_id":"c","count":0,"cursor":null,"timed_out":true,"messages":[]}}"#
        );
    }

    #[test]
    fn wait_success_with_attachment_bearing_message_matches_contract() {
        let mut message = sample_message();
        message.attachments = vec![sample_attachment()];
        let payload = Payload::Wait(WaitData {
            channel_id: "c".into(),
            count: 1,
            cursor: Some("20".into()),
            timed_out: false,
            messages: vec![message],
        });
        let (_, json, _) = render("wait", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"wait","data":{"channel_id":"c","count":1,"cursor":"20","timed_out":false,"messages":[{"id":"10","channel_id":"chan","author":{"id":"u1","username":"alice","bot":false},"content":"hi","timestamp":"2024-01-01T00:00:00Z","attachments":[{"id":"a1","filename":"photo.png","size":1024,"url":"https://cdn.discordapp.com/attachments/1/a1/photo.png","content_type":"image/png"}]}]}}"#
        );
    }

    #[test]
    fn ask_create_success_matches_contract() {
        use crate::output::payload::AskCreateData;

        let payload = Payload::AskCreate(AskCreateData {
            ask_id: "111".into(),
            status: "pending".into(),
            channel_id: "222".into(),
        });
        let (sink, json, code) = render("ask", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"ask","data":{"ask_id":"111","status":"pending","channel_id":"222"}}"#
        );
    }

    #[test]
    fn ask_result_pending_matches_contract() {
        use crate::output::payload::AskResultData;

        let payload = Payload::AskResult(AskResultData::Pending {
            ask_id: "111".into(),
        });
        let (_, json, _) = render("ask", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"ask","data":{"status":"pending","ask_id":"111"}}"#
        );
    }

    #[test]
    fn ask_result_answered_matches_contract() {
        use crate::output::payload::AskResultData;

        let payload = Payload::AskResult(AskResultData::Answered {
            ask_id: "111".into(),
            kind: "choice".into(),
            value: "yes".into(),
            answered_by: "u1".into(),
            answered_at: "2024-01-01T00:00:00Z".into(),
        });
        let (_, json, _) = render("ask", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"ask","data":{"status":"answered","ask_id":"111","kind":"choice","value":"yes","answered_by":"u1","answered_at":"2024-01-01T00:00:00Z"}}"#
        );
    }

    #[test]
    fn ask_result_timed_out_matches_contract() {
        use crate::output::payload::AskResultData;

        let payload = Payload::AskResult(AskResultData::TimedOut {
            ask_id: "111".into(),
        });
        let (_, json, _) = render("ask", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"ask","data":{"status":"timed_out","ask_id":"111"}}"#
        );
    }

    #[test]
    fn ask_wait_flattens_result_alongside_its_own_timed_out_field() {
        use crate::output::payload::{AskResultData, AskWaitData};

        // The wait poll's own `timed_out` must appear as a top-level sibling
        // of the flattened ask status fields, not nested or name-collided
        // with `status: "timed_out"`.
        let payload = Payload::AskWait(AskWaitData {
            result: AskResultData::Pending {
                ask_id: "111".into(),
            },
            timed_out: true,
        });
        let (_, json, _) = render("ask", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"ask","data":{"status":"pending","ask_id":"111","timed_out":true}}"#
        );
    }

    #[test]
    fn thread_success_matches_contract() {
        let payload = Payload::Thread(ThreadData {
            thread_id: "111".into(),
            name: "discussion".into(),
        });
        let (sink, json, code) = render("thread", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"thread","data":{"thread_id":"111","name":"discussion"}}"#
        );
    }

    #[test]
    fn thread_permission_denied_error_matches_contract() {
        // Acceptance criterion 3: insufficient permission on thread create
        // must surface as a `kind: "auth"` JSON envelope, not a generic
        // error — pins the full serialized shape, not just the ErrorKind.
        let err = AppError {
            kind: ErrorKind::Auth,
            message: "Discord API returned 403: Missing Access".into(),
            http_status: Some(403),
            retry_after_ms: None,
        };
        let (sink, json, code) = render("thread", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 3);
        assert_eq!(
            json,
            r#"{"ok":false,"command":"thread","error":{"kind":"auth","message":"Discord API returned 403: Missing Access","http_status":403}}"#
        );
    }

    #[test]
    fn thread_duplicate_creation_error_matches_contract() {
        // Acceptance criterion 3: re-requesting a thread on a message that
        // already has one (Discord's THREAD_ALREADY_CREATED) must surface as
        // a clear JSON envelope with the Discord message preserved verbatim.
        let err = AppError {
            kind: ErrorKind::Api,
            message: "Discord API returned 400: THREAD_ALREADY_CREATED".into(),
            http_status: Some(400),
            retry_after_ms: None,
        };
        let (sink, json, code) = render("thread", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 4);
        assert_eq!(
            json,
            r#"{"ok":false,"command":"thread","error":{"kind":"api","message":"Discord API returned 400: THREAD_ALREADY_CREATED","http_status":400}}"#
        );
    }

    #[test]
    fn error_omits_absent_optionals() {
        let err = AppError::new(ErrorKind::Config, "no bot token");
        let (sink, json, code) = render("send", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 3);
        assert_eq!(
            json,
            r#"{"ok":false,"command":"send","error":{"kind":"config","message":"no bot token"}}"#
        );
    }

    #[test]
    fn error_includes_http_status_and_retry() {
        let err = AppError {
            kind: ErrorKind::RateLimit,
            message: "rate limited".into(),
            http_status: Some(429),
            retry_after_ms: Some(1200),
        };
        let (sink, json, code) = render("read", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 5);
        assert_eq!(
            json,
            r#"{"ok":false,"command":"read","error":{"kind":"rate_limit","message":"rate limited","http_status":429,"retry_after_ms":1200}}"#
        );
    }
}
