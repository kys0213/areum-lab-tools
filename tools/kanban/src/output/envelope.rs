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
    use crate::common::error::ErrorKind;
    use crate::output::payload::{
        AddData, AssignData, ClaimedItemData, DoneData, InitData, ItemSummaryData, LabelData,
        ListData, NextData, ReleaseData,
    };
    use crate::output::{Sink, render};

    #[test]
    fn init_success_matches_contract() {
        let payload = Payload::Init(InitData {
            path: "/home/user/.areum/kanban/kanban.db".into(),
            created: true,
        });
        let (sink, json, code) = render("init", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"init","data":{"path":"/home/user/.areum/kanban/kanban.db","created":true}}"#
        );
    }

    #[test]
    fn add_success_matches_contract() {
        let payload = Payload::Add(AddData {
            id: "itm-000017".into(),
            source: "discord".into(),
            external_id: "msg-1".into(),
            title: "cache keeps drifting".into(),
            state: "inbox".into(),
            created_at: "2024-01-01T00:00:00Z".into(),
        });
        let (sink, json, code) = render("add", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"add","data":{"id":"itm-000017","source":"discord","external_id":"msg-1","title":"cache keeps drifting","state":"inbox","created_at":"2024-01-01T00:00:00Z"}}"#
        );
    }

    #[test]
    fn next_on_empty_backlog_is_exactly_id_null() {
        // docs §5 pins the empty answer to `{"id": null}` — agents branch on
        // that null, so it must not become `{}` or an error envelope.
        let (sink, json, code) = render("next", &Ok(Payload::Next(NextData::empty())), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(json, r#"{"ok":true,"command":"next","data":{"id":null}}"#);
    }

    #[test]
    fn next_claim_serializes_as_the_bare_item() {
        let payload = Payload::Next(NextData::Claimed(ClaimedItemData {
            id: "itm-000017".into(),
            title: "fix the build".into(),
            body: "steps".into(),
            project: "belt".into(),
            priority: "P1".into(),
            state: "running".into(),
            session_id: "sess-abc".into(),
            agent: "claude".into(),
            claimed_at: "2024-01-02T00:00:00Z".into(),
        }));
        let (_, json, _) = render("next", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"next","data":{"id":"itm-000017","title":"fix the build","body":"steps","project":"belt","priority":"P1","state":"running","session_id":"sess-abc","agent":"claude","claimed_at":"2024-01-02T00:00:00Z"}}"#
        );
    }

    #[test]
    fn done_preserves_the_session_that_did_the_work() {
        // docs §4: `done` must not clear session_id/agent — who did it has to
        // survive completion.
        let payload = Payload::Done(DoneData {
            id: "itm-000017".into(),
            state: "done".into(),
            project: "belt".into(),
            session_id: Some("sess-abc".into()),
            agent: Some("claude".into()),
            updated_at: "2024-01-03T00:00:00Z".into(),
        });
        let (_, json, _) = render("done", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"done","data":{"id":"itm-000017","state":"done","project":"belt","session_id":"sess-abc","agent":"claude","updated_at":"2024-01-03T00:00:00Z"}}"#
        );
    }

    #[test]
    fn done_by_a_human_reports_null_session_rather_than_omitting_it() {
        let payload = Payload::Done(DoneData {
            id: "itm-000017".into(),
            state: "done".into(),
            project: "belt".into(),
            session_id: None,
            agent: None,
            updated_at: "2024-01-03T00:00:00Z".into(),
        });
        let (_, json, _) = render("done", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"done","data":{"id":"itm-000017","state":"done","project":"belt","session_id":null,"agent":null,"updated_at":"2024-01-03T00:00:00Z"}}"#
        );
    }

    #[test]
    fn release_reports_the_claim_it_cleared() {
        // docs §4: release empties session_id/agent/claimed_at, so these
        // fields describe what was dropped — the row itself is unowned again.
        let payload = Payload::Release(ReleaseData {
            id: "itm-000017".into(),
            state: "backlog".into(),
            project: "belt".into(),
            reason: "build failed".into(),
            released_session_id: Some("sess-abc".into()),
            released_agent: Some("claude".into()),
            updated_at: "2024-01-03T00:00:00Z".into(),
        });
        let (_, json, _) = render("release", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"release","data":{"id":"itm-000017","state":"backlog","project":"belt","reason":"build failed","released_session_id":"sess-abc","released_agent":"claude","updated_at":"2024-01-03T00:00:00Z"}}"#
        );
    }

    #[test]
    fn assign_success_matches_contract() {
        let payload = Payload::Assign(AssignData {
            id: "itm-000021".into(),
            state: "backlog".into(),
            previous_state: "unmatched".into(),
            project: "belt".into(),
            priority: "P1".into(),
            updated_at: "2024-01-03T00:00:00Z".into(),
        });
        let (_, json, _) = render("assign", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"assign","data":{"id":"itm-000021","state":"backlog","previous_state":"unmatched","project":"belt","priority":"P1","updated_at":"2024-01-03T00:00:00Z"}}"#
        );
    }

    #[test]
    fn list_serializes_unassigned_items_with_null_project_and_claim() {
        let payload = Payload::List(ListData {
            count: 1,
            items: vec![ItemSummaryData {
                id: "itm-000021".into(),
                source: "cli".into(),
                external_id: "local-3".into(),
                title: "cache keeps drifting".into(),
                state: "unmatched".into(),
                project: None,
                priority: "P2".into(),
                session_id: None,
                agent: None,
                claimed_at: None,
                created_at: "2024-01-01T00:00:00Z".into(),
                updated_at: "2024-01-01T00:00:00Z".into(),
                labels: vec![LabelData {
                    key: "kind".into(),
                    value: "bug".into(),
                    confidence: Some(0.5),
                }],
            }],
        });
        let (_, json, _) = render("list", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"list","data":{"count":1,"items":[{"id":"itm-000021","source":"cli","external_id":"local-3","title":"cache keeps drifting","state":"unmatched","project":null,"priority":"P2","session_id":null,"agent":null,"claimed_at":null,"created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z","labels":[{"key":"kind","value":"bug","confidence":0.5}]}]}}"#
        );
    }

    #[test]
    fn error_json_matches_contract() {
        let err = AppError::new(ErrorKind::NotFound, "no item itm-000017");
        let (sink, json, code) = render("show", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 7);
        assert_eq!(
            json,
            r#"{"ok":false,"command":"show","error":{"kind":"not_found","message":"no item itm-000017"}}"#
        );
    }

    #[test]
    fn conflict_error_json_matches_contract() {
        // A repeat of the same (source, external_id) is rejected, not merged.
        let err = AppError::new(
            ErrorKind::Conflict,
            "item already exists for source=discord external_id=msg-1",
        );
        let (sink, json, code) = render("add", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 8);
        assert_eq!(
            json,
            r#"{"ok":false,"command":"add","error":{"kind":"conflict","message":"item already exists for source=discord external_id=msg-1"}}"#
        );
    }

    #[test]
    fn error_includes_http_status_and_retry_when_present() {
        let err = AppError {
            kind: ErrorKind::RateLimit,
            message: "rate limited".into(),
            http_status: Some(429),
            retry_after_ms: Some(1200),
        };
        let (sink, json, code) = render("list", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 5);
        assert_eq!(
            json,
            r#"{"ok":false,"command":"list","error":{"kind":"rate_limit","message":"rate limited","http_status":429,"retry_after_ms":1200}}"#
        );
    }

    #[test]
    fn unimplemented_stub_error_renders_through_the_normal_envelope_path() {
        // Stage-1 stubs fail loudly with `internal` rather than panicking, so
        // the --json contract is verifiable end to end before the command
        // bodies exist.
        let err = AppError::new(ErrorKind::Internal, "show is not implemented yet");
        let (sink, json, code) = render("show", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 1);
        assert_eq!(
            json,
            r#"{"ok":false,"command":"show","error":{"kind":"internal","message":"show is not implemented yet"}}"#
        );
    }
}
