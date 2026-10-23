//! Card runs (docs/doppel.md §6 "세션 실행"): launch the configured command in
//! the card's project, hand it the card as the first message over
//! stream-json, and move the card when the session is done.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::common::error::{AppError, ErrorKind};
use crate::common::store::CardRecord;
use crate::daemon::auto;
use crate::daemon::routes::AppState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchConfig {
    pub(crate) argv: Vec<String>,
    pub(crate) max_running: u32,
    /// This daemon's socket, so the launched session's plugin connects here.
    pub(crate) socket: PathBuf,
}

impl LaunchConfig {
    pub(crate) const DEFAULT_COMMAND: &'static str = "claude -p --input-format stream-json \
         --output-format stream-json --verbose --permission-mode auto --permission-prompt-tool stdio";

    /// Splits on whitespace — no shell, no quoting. `None` for a blank
    /// command.
    pub(crate) fn from_command(command: &str, max_running: u32, socket: PathBuf) -> Option<Self> {
        let argv: Vec<String> = command.split_whitespace().map(str::to_owned).collect();
        (!argv.is_empty()).then_some(LaunchConfig {
            argv,
            max_running,
            socket,
        })
    }
}

/// How a dispatch starts the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Mode {
    /// A new session that reads the card.
    Fresh,
    /// The card's last session, continued with `note` — or, without one,
    /// with the reason the card was sent back.
    Resume { note: Option<String> },
}

/// One run's launch: where, which session to continue, what to say first.
struct Plan {
    project_path: String,
    resume: Option<String>,
    message: String,
}

/// Claims the card and launches its session in the background. Answers the
/// card as claimed, before the session has started.
pub(super) fn start(app: &AppState, id: &str, mode: Mode) -> Result<CardRecord, AppError> {
    let config = app.launch.clone().ok_or_else(|| {
        AppError::new(
            ErrorKind::Config,
            "this daemon launches no sessions (empty --launch-command)",
        )
    })?;
    let (claimed, plan) = {
        let store = app.store();
        let card = store.get_card(id)?;
        let project = card
            .project
            .as_deref()
            .ok_or_else(|| usage(format!("{id} has no project; triage it first")))?;
        let project_path = store.get_project(project)?.path;
        if !Path::new(&project_path).is_dir() {
            return Err(usage(format!(
                "project {project} path {project_path} is not a directory"
            )));
        }
        let (resume, message) = match mode {
            Mode::Fresh => (None, first_message(&card)),
            Mode::Resume { note } => {
                let session = card
                    .session_id
                    .clone()
                    .ok_or_else(|| usage(format!("{id} has no earlier session to resume")))?;
                let message = resume_message(note.as_deref(), card.last_error.as_deref());
                (Some(session), message)
            }
        };
        let claimed = store.claim_card(id, config.max_running)?;
        (
            claimed,
            Plan {
                project_path,
                resume,
                message,
            },
        )
    };
    app.hub.cards_changed();
    tokio::spawn(run(app.clone(), config, claimed.clone(), plan));
    Ok(claimed)
}

fn usage(message: String) -> AppError {
    AppError::new(ErrorKind::Usage, message)
}

fn user_message(text: &str) -> String {
    json!({ "type": "user", "message": { "role": "user", "content": text } }).to_string()
}

/// The first message a new session reads.
pub(super) fn first_message(card: &CardRecord) -> String {
    let text = if card.body.trim().is_empty() {
        format!("다음 작업을 처리해 주세요.\n\n# {}", card.title)
    } else {
        format!(
            "다음 작업을 처리해 주세요.\n\n# {}\n\n{}",
            card.title, card.body
        )
    };
    user_message(&text)
}

/// The first message a resumed session reads.
pub(super) fn resume_message(note: Option<&str>, sent_back_because: Option<&str>) -> String {
    let text = match (note.filter(|n| !n.trim().is_empty()), sent_back_because) {
        (Some(note), _) => note.to_owned(),
        (None, Some(reason)) => format!("이어서 진행해 주세요.\n\n되돌린 이유: {reason}"),
        (None, None) => "이어서 진행해 주세요.".to_owned(),
    };
    user_message(&text)
}

/// What one stream-json output line means to the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StreamEvent {
    Started { session_id: String },
    Answered { text: String },
    Failed { reason: String },
    Other,
}

pub(super) fn read_line(line: &str) -> StreamEvent {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return StreamEvent::Other;
    };
    match (value["type"].as_str(), value["subtype"].as_str()) {
        (Some("system"), Some("init")) => {
            value["session_id"]
                .as_str()
                .map_or(StreamEvent::Other, |id| StreamEvent::Started {
                    session_id: id.to_owned(),
                })
        }
        (Some("result"), _) => {
            let text = value["result"].as_str().unwrap_or_default().to_owned();
            if value["is_error"].as_bool() == Some(true) {
                StreamEvent::Failed {
                    reason: if text.is_empty() {
                        format!("the session ended with {}", value["subtype"])
                    } else {
                        text
                    },
                }
            } else {
                StreamEvent::Answered { text }
            }
        }
        _ => StreamEvent::Other,
    }
}

/// Runs one claimed card to the end. Every outcome lands on the card: its
/// last answer moves it to `review`, anything else back to `triaged` — unless
/// a person stopped the run, who already moved the card.
async fn run(app: AppState, config: LaunchConfig, card: CardRecord, plan: Plan) {
    let (run, stopped) = app.runs.register(&card.id);
    let outcome = tokio::select! {
        biased;
        _ = stopped => None,
        outcome = drive(&app, &config, &card, &plan) => Some(outcome),
    };
    app.runs.unregister(&card.id, run);
    let Some(outcome) = outcome else {
        return;
    };
    {
        let store = app.store();
        // Refused only when a person moved the card meanwhile; theirs stands.
        let _ = match outcome {
            Ok(text) => store.finish_card(&card.id, &text),
            Err(reason) => store.release_card(&card.id, &reason),
        };
    }
    app.hub.cards_changed();
    // A slot is free.
    auto::pump(&app);
}

/// `Ok` with the session's last answer, `Err` with why there is none.
async fn drive(
    app: &AppState,
    config: &LaunchConfig,
    card: &CardRecord,
    plan: &Plan,
) -> Result<String, String> {
    let resume = plan
        .resume
        .as_deref()
        .map(|session| ["--resume", session])
        .into_iter()
        .flatten();
    let mut child = tokio::process::Command::new(&config.argv[0])
        .args(&config.argv[1..])
        .args(resume)
        .current_dir(&plan.project_path)
        .env("DOPPEL_LAUNCHED", "1")
        .env("DOPPEL_CARD", &card.id)
        .env("DOPPEL_SOCKET", &config.socket)
        // Either one makes the child reuse the parent's session id; a card
        // run is always a session of its own.
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CLAUDE_CODE_REMOTE_SESSION_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot launch {}: {e}", config.argv[0]))?;
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take().ok_or("the session has no stdout")?;
    if let Some(input) = stdin.as_mut() {
        input
            .write_all(format!("{}\n", plan.message).as_bytes())
            .await
            .map_err(|e| format!("cannot send the card to the session: {e}"))?;
    }

    let mut lines = BufReader::new(stdout).lines();
    let mut session_id: Option<String> = None;
    let mut last: Result<String, String> = Err("the session ended without an answer".to_owned());
    while let Ok(Some(line)) = lines.next_line().await {
        match read_line(&line) {
            StreamEvent::Started { session_id: id } => {
                let _ = app.store().set_card_session(&card.id, &id);
                app.hub.cards_changed();
                session_id = Some(id);
            }
            StreamEvent::Answered { text } => {
                last = Ok(text);
                // Closing stdin ends a stream-json session; keep it open
                // while a deferred question can still bring a late answer.
                let waiting = session_id
                    .as_deref()
                    .and_then(|id| app.store().pending_late_answers(id).ok())
                    .unwrap_or(0);
                if waiting == 0 {
                    stdin = None;
                }
            }
            StreamEvent::Failed { reason } => {
                last = Err(reason);
                stdin = None;
            }
            StreamEvent::Other => {}
        }
    }
    drop(stdin);
    let status = child
        .wait()
        .await
        .map_err(|e| format!("cannot wait for the session: {e}"))?;
    match last {
        Ok(text) => Ok(text),
        Err(reason) if status.success() => Err(reason),
        Err(reason) => Err(format!("{reason} (exit {status})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_lines_map_to_card_events() {
        assert_eq!(
            read_line(r#"{"type":"system","subtype":"init","session_id":"s-1"}"#),
            StreamEvent::Started {
                session_id: "s-1".into()
            }
        );
        assert_eq!(
            read_line(
                r#"{"type":"result","subtype":"success","is_error":false,"result":"고쳤습니다"}"#
            ),
            StreamEvent::Answered {
                text: "고쳤습니다".into()
            }
        );
        assert_eq!(
            read_line(r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#),
            StreamEvent::Failed {
                reason: "the session ended with \"error_max_turns\"".into()
            }
        );
        assert_eq!(read_line(r#"{"type":"assistant"}"#), StreamEvent::Other);
        assert_eq!(read_line("not json"), StreamEvent::Other);
    }

    #[test]
    fn a_resumed_session_hears_the_note_or_why_it_was_sent_back() {
        let text = |m: String| {
            serde_json::from_str::<Value>(&m).unwrap()["message"]["content"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(
            text(resume_message(Some("테스트도 추가"), Some("x"))),
            "테스트도 추가"
        );
        assert!(
            text(resume_message(None, Some("범위가 넓음"))).contains("되돌린 이유: 범위가 넓음")
        );
        assert_eq!(
            text(resume_message(Some("  "), None)),
            "이어서 진행해 주세요."
        );
    }

    #[test]
    fn the_first_message_carries_title_and_body() {
        let card = CardRecord {
            id: "crd-000001".into(),
            source: "cli".into(),
            external_id: "x".into(),
            title: "캐시 고치기".into(),
            body: "재시작하면 사라진다".into(),
            state: "running".into(),
            project: Some("areum".into()),
            priority: "P1".into(),
            session_id: None,
            claimed_at: None,
            outcome: None,
            last_error: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let message: Value = serde_json::from_str(&first_message(&card)).unwrap();
        assert_eq!(message["type"], "user");
        let text = message["message"]["content"].as_str().unwrap();
        assert!(text.contains("# 캐시 고치기"));
        assert!(text.contains("재시작하면 사라진다"));
    }
}
