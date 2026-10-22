//! Card runs (docs/doppel.md §6 "세션 실행"): launch the configured command in
//! the card's project, hand it the card as the first message over
//! stream-json, and move the card when the session is done.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

use crate::common::store::CardRecord;
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

/// Card id → (run number, the run's stop signal).
type Stops = HashMap<String, (u64, oneshot::Sender<()>)>;

/// Card runs in flight. Each run is numbered so a run that ends after its
/// card was released and dispatched again cannot unregister the new run.
#[derive(Clone, Default)]
pub(crate) struct Runs {
    next: Arc<AtomicU64>,
    stops: Arc<Mutex<Stops>>,
}

impl Runs {
    fn register(&self, card_id: &str) -> (u64, oneshot::Receiver<()>) {
        let run = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.lock().insert(card_id.to_owned(), (run, tx));
        (run, rx)
    }

    fn unregister(&self, card_id: &str, run: u64) {
        let mut stops = self.lock();
        if stops.get(card_id).is_some_and(|(r, _)| *r == run) {
            stops.remove(card_id);
        }
    }

    /// Stops the card's run, if one is in flight: its session is killed and
    /// the run leaves the card as the caller put it.
    pub(super) fn stop(&self, card_id: &str) {
        if let Some((_, tx)) = self.lock().remove(card_id) {
            let _ = tx.send(());
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Stops> {
        // Every critical section is one map operation; nothing is half done.
        self.stops
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The first message a launched session reads.
pub(super) fn first_message(card: &CardRecord) -> String {
    let text = if card.body.trim().is_empty() {
        format!("다음 작업을 처리해 주세요.\n\n# {}", card.title)
    } else {
        format!(
            "다음 작업을 처리해 주세요.\n\n# {}\n\n{}",
            card.title, card.body
        )
    };
    json!({ "type": "user", "message": { "role": "user", "content": text } }).to_string()
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
pub(super) async fn run(
    app: AppState,
    config: LaunchConfig,
    card: CardRecord,
    project_path: String,
) {
    let (run, stopped) = app.runs.register(&card.id);
    let outcome = tokio::select! {
        biased;
        _ = stopped => None,
        outcome = drive(&app, &config, &card, &project_path) => Some(outcome),
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
}

/// `Ok` with the session's last answer, `Err` with why there is none.
async fn drive(
    app: &AppState,
    config: &LaunchConfig,
    card: &CardRecord,
    project_path: &str,
) -> Result<String, String> {
    let mut child = tokio::process::Command::new(&config.argv[0])
        .args(&config.argv[1..])
        .current_dir(project_path)
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
            .write_all(format!("{}\n", first_message(card)).as_bytes())
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
