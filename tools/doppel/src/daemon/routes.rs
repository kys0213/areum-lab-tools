//! HTTP and WebSocket handlers (docs/doppel-protocol.md §6). Handlers apply
//! the request to the store and publish what changed on the hub; they hold
//! the store lock only for the query itself, never across an await.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};

use crate::common::error::{AppError, ErrorKind};
use crate::common::store::{SessionEvent, Store};
use crate::common::time::now_epoch_secs;
use crate::daemon::api::{
    AnswerBody, AskList, AskView, AttachMessage, CloseBody, EventBody, Health, OpenAskBody,
    SessionList, SessionView,
};
use crate::daemon::hub::Hub;
use crate::daemon::state::Thresholds;

/// Longest a single `wait` may hold the connection; the hook's own limit
/// (`$.process.run`, ten minutes) sits above it.
const MAX_WAIT: Duration = Duration::from_secs(590);
/// Keeps idle WebSocket connections from being reaped by anything between.
const PING_EVERY: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(super) struct AppState {
    pub(super) store: Arc<Mutex<Store>>,
    pub(super) hub: Hub,
    pub(super) thresholds: Thresholds,
    /// Flips to `true` once to stop the daemon: both listeners drain and
    /// every WebSocket loop returns so the drain can finish.
    pub(super) stop: watch::Sender<bool>,
}

impl AppState {
    pub(super) fn new(store: Store, thresholds: Thresholds) -> AppState {
        AppState {
            store: Arc::new(Mutex::new(store)),
            hub: Hub::new(),
            thresholds,
            stop: watch::channel(false).0,
        }
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        // A handler that panicked while holding the lock left no partial
        // write behind (every write is one statement or one transaction),
        // so the data is still consistent.
        self.store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// An [`AppError`] as an HTTP response: the status of its kind and the error
/// envelope the CLI reads back.
pub(super) struct ApiError(pub(super) AppError);

impl From<AppError> for ApiError {
    fn from(err: AppError) -> Self {
        ApiError(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0.kind {
            ErrorKind::Usage => StatusCode::BAD_REQUEST,
            ErrorKind::NotFound => StatusCode::NOT_FOUND,
            ErrorKind::Conflict => StatusCode::CONFLICT,
            ErrorKind::Config | ErrorKind::Network | ErrorKind::Internal => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        (status, Json(json!({ "ok": false, "error": self.0 }))).into_response()
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

/// Routes both listeners serve.
pub(super) fn shared_routes() -> Router<AppState> {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/events", post(post_event))
        .route("/v1/sessions", get(list_sessions))
        .route("/v1/asks", get(list_asks).post(open_ask))
        .route("/v1/asks/{id}", get(get_ask))
        .route("/v1/asks/{id}/wait", get(wait_ask))
        .route("/v1/asks/{id}/answer", post(answer_ask))
        .route("/v1/asks/{id}/defer", post(defer_ask))
        .route("/v1/asks/{id}/close", post(close_ask))
        .route("/v1/attach/{session_id}", get(attach))
        .route("/v1/ui", get(ui_socket))
        .route("/", get(super::ui::page))
}

/// The Unix socket additionally takes `shutdown`; a web page must never be
/// able to stop the daemon.
pub(super) fn socket_routes() -> Router<AppState> {
    shared_routes().route("/v1/shutdown", post(shutdown))
}

async fn health() -> Json<Health> {
    Json(Health {
        ok: true,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        pid: std::process::id(),
    })
}

async fn shutdown(State(app): State<AppState>) -> Json<Value> {
    app.stop.send_replace(true);
    Json(json!({ "ok": true }))
}

/// Resolves once the daemon is stopping.
pub(super) async fn stopped(mut stop: watch::Receiver<bool>) {
    // An `Err` means the sender is gone, which also means stopping.
    let _ = stop.wait_for(|stopping| *stopping).await;
}

fn usage(message: impl Into<String>) -> ApiError {
    ApiError(AppError::new(ErrorKind::Usage, message))
}

/// Maps the posted event to the store's event, checking the fields its kind
/// needs (docs/doppel-protocol.md §2).
fn session_event(body: &EventBody) -> Result<SessionEvent, ApiError> {
    let event = match body.kind.as_str() {
        "session.start" => SessionEvent::Start {
            cwd: body.cwd.clone(),
            surface: one_of(
                body.surface.as_deref(),
                &["terminal", "headless", "unknown"],
                "unknown",
            )
            .ok_or_else(|| usage("surface must be terminal, headless or unknown"))?,
            launched_by: one_of(body.launched_by.as_deref(), &["doppel", "user"], "user")
                .ok_or_else(|| usage("launched_by must be doppel or user"))?,
        },
        "session.end" => SessionEvent::End {
            reason: body.reason.clone(),
        },
        "heartbeat" => SessionEvent::Heartbeat,
        "turn.start" => SessionEvent::TurnStart,
        "turn.complete" => SessionEvent::TurnComplete {
            summary: body.summary.clone(),
        },
        "tool.failure" => SessionEvent::ToolFailure,
        other => return Err(usage(format!("unknown event kind {other}"))),
    };
    Ok(event)
}

/// `value` when it is one of `allowed`, `default` when absent, `None` when it
/// is something else.
fn one_of(value: Option<&str>, allowed: &[&str], default: &str) -> Option<String> {
    match value {
        None => Some(default.to_owned()),
        Some(v) if allowed.contains(&v) => Some(v.to_owned()),
        Some(_) => None,
    }
}

async fn post_event(
    State(app): State<AppState>,
    Json(body): Json<EventBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let event = session_event(&body)?;
    app.store().record_session_event(&body.session_id, &event)?;
    if matches!(event, SessionEvent::End { .. }) {
        app.hub.ask_changed("");
    }
    app.hub.sessions_changed();
    Ok((StatusCode::ACCEPTED, Json(json!({ "ok": true }))))
}

async fn list_sessions(State(app): State<AppState>) -> ApiResult<SessionList> {
    let now = now_epoch_secs();
    let sessions = app
        .store()
        .list_sessions()?
        .into_iter()
        .map(|facts| SessionView::derive(facts, now, &app.thresholds))
        .collect();
    Ok(Json(SessionList { sessions }))
}

#[derive(Deserialize)]
struct StateQuery {
    state: Option<String>,
}

async fn list_asks(
    State(app): State<AppState>,
    Query(query): Query<StateQuery>,
) -> ApiResult<AskList> {
    let asks = app
        .store()
        .list_asks(query.state.as_deref())?
        .into_iter()
        .map(AskView::from)
        .collect();
    Ok(Json(AskList { asks }))
}

async fn open_ask(
    State(app): State<AppState>,
    Json(body): Json<OpenAskBody>,
) -> ApiResult<AskView> {
    let ask = app.store().open_ask(
        &body.session_id,
        &body.tool_use_id,
        &body.kind,
        &body.questions,
    )?;
    app.hub.ask_changed(&ask.id);
    Ok(Json(ask.into()))
}

async fn get_ask(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<AskView> {
    Ok(Json(app.store().get_ask(&id)?.into()))
}

#[derive(Deserialize)]
struct WaitQuery {
    timeout_ms: Option<u64>,
}

/// Answers as soon as the ask leaves `open`, or with the still-open ask when
/// the timeout runs out — the caller loops.
async fn wait_ask(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<WaitQuery>,
) -> ApiResult<AskView> {
    let timeout = query
        .timeout_ms
        .map_or(MAX_WAIT, Duration::from_millis)
        .min(MAX_WAIT);
    let deadline = tokio::time::Instant::now() + timeout;
    // Subscribe before the first read so a change between the read and the
    // subscription is not missed.
    let mut changes = app.hub.asks.subscribe();
    loop {
        let ask = app.store().get_ask(&id)?;
        if ask.state != "open" {
            return Ok(Json(ask.into()));
        }
        match tokio::time::timeout_at(deadline, changes.recv()).await {
            Err(_elapsed) => return Ok(Json(ask.into())),
            Ok(Ok(_)) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) => return Ok(Json(ask.into())),
        }
    }
}

async fn answer_ask(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AnswerBody>,
) -> ApiResult<AskView> {
    let ask = app
        .store()
        .answer_ask(&id, &body.answers, &body.answered_by)?;
    app.hub.ask_changed(&ask.id);
    if ask.deferred_at.is_some() {
        // The session stopped waiting; its hook takes the answer from the
        // attach connection instead of `wait`.
        app.hub.to_session(
            &ask.session_id,
            deferred_answer(&ask.id, &ask.tool_use_id, &body.answers, &body.answered_by),
        );
    }
    Ok(Json(ask.into()))
}

fn deferred_answer(ask_id: &str, tool_use_id: &str, answers: &Value, answered_by: &str) -> Value {
    serde_json::to_value(AttachMessage::Answer {
        ask_id: ask_id.to_owned(),
        tool_use_id: tool_use_id.to_owned(),
        answers: answers.clone(),
        answered_by: answered_by.to_owned(),
    })
    .expect("attach message serializes")
}

async fn defer_ask(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<AskView> {
    let ask = app.store().defer_ask(&id)?;
    app.hub.ask_changed(&ask.id);
    Ok(Json(ask.into()))
}

async fn close_ask(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CloseBody>,
) -> ApiResult<AskView> {
    let ask = app
        .store()
        .close_ask(&id, body.answers.as_ref(), &body.answered_by)?;
    app.hub.ask_changed(&ask.id);
    Ok(Json(ask.into()))
}

async fn attach(
    State(app): State<AppState>,
    Path(session_id): Path<String>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    app.store().touch_session(&session_id)?;
    app.hub.sessions_changed();
    Ok(upgrade.on_upgrade(move |socket| serve_attach(app, session_id, socket)))
}

/// Pushes this session's messages until either side goes away. Answers that
/// arrived while the session was not connected are sent first.
async fn serve_attach(app: AppState, session_id: String, mut socket: WebSocket) {
    let mut messages = app.hub.sessions.subscribe();
    let pending: Vec<Value> = match app.store().deliverable_answers(&session_id) {
        Ok(asks) => asks
            .into_iter()
            .filter_map(|a| {
                let answers = a.answers?;
                let by = a.answered_by?;
                Some(deferred_answer(&a.id, &a.tool_use_id, &answers, &by))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    for message in pending {
        if socket
            .send(Message::Text(message.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
    }
    let mut ping = tokio::time::interval(PING_EVERY);
    let stopping = stopped(app.stop.subscribe());
    tokio::pin!(stopping);
    loop {
        tokio::select! {
            _ = &mut stopping => return,
            received = messages.recv() => match received {
                Ok((target, message)) if target == session_id => {
                    if socket.send(Message::Text(message.to_string().into())).await.is_err() {
                        return;
                    }
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {
                    if let Ok(store) = app.store.lock() {
                        let _ = store.touch_session(&session_id);
                    }
                }
            },
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

async fn ui_socket(State(app): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| serve_ui(app, socket))
}

async fn serve_ui(app: AppState, mut socket: WebSocket) {
    let mut notices = app.hub.ui.subscribe();
    let stopping = stopped(app.stop.subscribe());
    tokio::pin!(stopping);
    loop {
        tokio::select! {
            _ = &mut stopping => return,
            notice = notices.recv() => match notice {
                Ok(notice) => {
                    let text = serde_json::to_string(&notice).expect("notice serializes");
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
            incoming = socket.recv() => if matches!(incoming, None | Some(Err(_)) | Some(Ok(Message::Close(_)))) {
                return;
            },
        }
    }
}
