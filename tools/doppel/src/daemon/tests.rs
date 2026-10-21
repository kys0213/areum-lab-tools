//! Black-box tests over a real Unix socket: the daemon is served by
//! `listen::serve` and driven through the same client the CLI uses.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::commands::testutil::{SeqClock, TempDir};
use crate::common::client::{attach, request};
use crate::common::error::{AppError, ErrorKind};
use crate::common::store::Store;
use crate::daemon::listen::{DaemonConfig, serve};
use crate::daemon::state::Thresholds;

struct Running {
    socket: PathBuf,
    task: JoinHandle<Result<(), AppError>>,
}

async fn start(dir: &TempDir) -> Running {
    let socket = dir.path().join("d.sock");
    // The wall clock: the daemon judges session ages against real time.
    let store = Store::open(&dir.db_path()).unwrap();
    let config = DaemonConfig {
        socket: socket.clone(),
        ui_port: None,
        thresholds: Thresholds::default(),
    };
    let (ready_tx, ready_rx) = oneshot::channel();
    let task = tokio::spawn(serve(store, config, Some(ready_tx)));
    ready_rx.await.expect("daemon reports ready");
    Running { socket, task }
}

async fn post(socket: &Path, path: &str, body: Value) -> Result<Value, AppError> {
    request(socket, "POST", path, Some(&body)).await
}

async fn get(socket: &Path, path: &str) -> Result<Value, AppError> {
    request(socket, "GET", path, None).await
}

fn questions() -> Value {
    json!([{"question": "위치는?", "options": [
        {"label": "메모리", "description": ""}, {"label": "디스크", "description": ""}
    ]}])
}

async fn open(socket: &Path, tool_use_id: &str) -> String {
    let ask = post(
        socket,
        "/v1/asks",
        json!({"session_id": "s1", "tool_use_id": tool_use_id, "kind": "ask", "questions": questions()}),
    )
    .await
    .unwrap();
    ask["ask_id"].as_str().unwrap().to_owned()
}

async fn stop(running: Running) {
    post(&running.socket, "/v1/shutdown", json!({}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), running.task)
        .await
        .expect("daemon stops")
        .unwrap()
        .unwrap();
    assert!(!running.socket.exists(), "the socket is removed on stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn events_shape_the_listed_session_state() {
    let dir = TempDir::new("daemon-events");
    let d = start(&dir).await;
    post(
        &d.socket,
        "/v1/events",
        json!({"session_id": "s1", "kind": "session.start", "cwd": "/w", "surface": "headless"}),
    )
    .await
    .unwrap();
    post(
        &d.socket,
        "/v1/events",
        json!({"session_id": "s1", "kind": "turn.start"}),
    )
    .await
    .unwrap();
    let sessions = get(&d.socket, "/v1/sessions").await.unwrap();
    assert_eq!(sessions["sessions"][0]["state"], "running");
    assert_eq!(sessions["sessions"][0]["surface"], "headless");

    open(&d.socket, "t1").await;
    let sessions = get(&d.socket, "/v1/sessions").await.unwrap();
    assert_eq!(sessions["sessions"][0]["state"], "waiting_ask");

    let bad = post(
        &d.socket,
        "/v1/events",
        json!({"session_id": "s1", "kind": "teleport"}),
    )
    .await;
    assert_eq!(bad.unwrap_err().kind, ErrorKind::Usage);
    let bad = post(
        &d.socket,
        "/v1/events",
        json!({"session_id": "s1", "kind": "session.start", "surface": "tv"}),
    )
    .await;
    assert_eq!(bad.unwrap_err().kind, ErrorKind::Usage);
    stop(d).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wait_wakes_as_soon_as_the_ask_is_answered() {
    let dir = TempDir::new("daemon-wait");
    let d = start(&dir).await;
    let id = open(&d.socket, "t1").await;

    let socket = d.socket.clone();
    let wait_path = format!("/v1/asks/{id}/wait?timeout_ms=10000");
    let waiter = tokio::spawn(async move { get(&socket, &wait_path).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    post(
        &d.socket,
        &format!("/v1/asks/{id}/answer"),
        json!({"answers": {"위치는?": "디스크"}, "answered_by": "doppel-ui"}),
    )
    .await
    .unwrap();
    let woke = tokio::time::timeout(Duration::from_secs(3), waiter)
        .await
        .expect("wait returns well before its timeout")
        .unwrap()
        .unwrap();
    assert_eq!(woke["state"], "answered");
    assert_eq!(woke["answered_by"], "doppel-ui");

    let timed_out = get(
        &d.socket,
        &format!(
            "/v1/asks/{}/wait?timeout_ms=50",
            open(&d.socket, "t2").await
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        timed_out["state"], "open",
        "a timed-out wait answers the open ask"
    );

    let closed = post(
        &d.socket,
        &format!("/v1/asks/{id}/close"),
        json!({"answers": {"위치는?": "메모리"}, "answered_by": "session"}),
    )
    .await
    .unwrap();
    assert_eq!(closed["answers"]["위치는?"], "메모리");
    let again = post(
        &d.socket,
        &format!("/v1/asks/{id}/answer"),
        json!({"answers": {"위치는?": "디스크"}, "answered_by": "slack"}),
    )
    .await;
    assert_eq!(again.unwrap_err().kind, ErrorKind::Conflict);
    stop(d).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deferred_answers_reach_the_session_live_and_on_reconnect() {
    let dir = TempDir::new("daemon-attach");
    let d = start(&dir).await;
    let id = open(&d.socket, "t1").await;
    post(&d.socket, &format!("/v1/asks/{id}/defer"), json!({}))
        .await
        .unwrap();

    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let socket = d.socket.clone();
    let live = tokio::spawn(async move {
        attach(&socket, "s1", |line| {
            let _ = line_tx.send(line.to_owned());
        })
        .await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    post(
        &d.socket,
        &format!("/v1/asks/{id}/answer"),
        json!({"answers": {"위치는?": "디스크"}, "answered_by": "doppel-ui"}),
    )
    .await
    .unwrap();
    let line = tokio::time::timeout(Duration::from_secs(3), line_rx.recv())
        .await
        .expect("the answer is pushed")
        .unwrap();
    let message: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        message,
        json!({"kind": "answer", "ask_id": id, "tool_use_id": "t1", "answers": {"위치는?": "디스크"}, "answered_by": "doppel-ui"})
    );

    // A second connection (the plugin re-spawned attach) is sent the
    // still-unclosed answer first.
    let (replay_tx, mut replay_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let socket = d.socket.clone();
    let replay = tokio::spawn(async move {
        attach(&socket, "s1", |line| {
            let _ = replay_tx.send(line.to_owned());
        })
        .await
    });
    let replayed = tokio::time::timeout(Duration::from_secs(3), replay_rx.recv())
        .await
        .expect("the pending answer is replayed")
        .unwrap();
    assert_eq!(replayed, line);

    stop(d).await;
    // Stopping closes attach connections, so both clients end cleanly.
    tokio::time::timeout(Duration::from_secs(3), live)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), replay)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_daemon_is_refused_and_a_stale_socket_is_replaced() {
    let dir = TempDir::new("daemon-single");
    let d = start(&dir).await;
    let store = Store::open_with_clock(&dir.db_path(), Box::new(SeqClock::default())).unwrap();
    let second = serve(
        store,
        DaemonConfig {
            socket: d.socket.clone(),
            ui_port: None,
            thresholds: Thresholds::default(),
        },
        None,
    )
    .await;
    assert_eq!(second.unwrap_err().kind, ErrorKind::Conflict);
    let socket = d.socket.clone();
    stop(d).await;

    // A socket file nobody listens on is what a crashed daemon leaves.
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    drop(listener);
    assert!(socket.exists());
    let d = start(&dir).await;
    assert_eq!(get(&d.socket, "/v1/health").await.unwrap()["ok"], true);
    stop(d).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_daemon_is_a_network_error() {
    let dir = TempDir::new("daemon-absent");
    let err = get(&dir.path().join("none.sock"), "/v1/health")
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Network);
    assert!(err.message.contains("doppel daemon start"));
}
