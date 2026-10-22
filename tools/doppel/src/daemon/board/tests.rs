//! The board over a real socket, with scripts standing in for Claude: they
//! speak just enough stream-json for the launcher.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::commands::testutil::TempDir;
use crate::common::error::ErrorKind;
use crate::daemon::launch::LaunchConfig;
use crate::daemon::tests::{Running, get, post, serve_in, stop};

/// Answers once with what it was asked, the card id and its working
/// directory, then ends when stdin closes.
const ANSWERS: &str = r#"#!/bin/sh
read -r first
echo '{"type":"system","subtype":"init","session_id":"sess-launched"}'
echo "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"done $DOPPEL_CARD in $(pwd) launched=$DOPPEL_LAUNCHED\"}"
cat > /dev/null
"#;

const FAILS: &str = "#!/bin/sh\nread -r first\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-x\"}'\nexit 3\n";

const HANGS: &str = "#!/bin/sh\nread -r first\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-h\"}'\nexec sleep 30\n";

async fn board(dir: &TempDir, script: &str, max_running: u32) -> Running {
    let path = dir.write("session.sh", script);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let launch = LaunchConfig {
        argv: vec![path.display().to_string()],
        max_running,
        socket: dir.path().join("d.sock"),
    };
    serve_in(dir, None, Some(launch)).await
}

async fn project(socket: &Path, dir: &TempDir) -> String {
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let work = std::fs::canonicalize(work).unwrap().display().to_string();
    post(
        socket,
        "/v1/projects",
        json!({"name": "areum", "path": work, "description": "도구 모음"}),
    )
    .await
    .unwrap();
    work
}

async fn triaged_card(socket: &Path, external_id: &str) -> String {
    let card = post(
        socket,
        "/v1/cards",
        json!({"source": "cli", "external_id": external_id, "title": "캐시 고치기", "body": "재시작하면 사라진다"}),
    )
    .await
    .unwrap();
    let id = card["card_id"].as_str().unwrap().to_owned();
    let triaged = post(
        socket,
        &format!("/v1/cards/{id}/triage"),
        json!({"project": "areum", "priority": "P1"}),
    )
    .await
    .unwrap();
    assert_eq!(triaged["state"], "triaged");
    id
}

async fn card_in(socket: &Path, id: &str, state: &str) -> Value {
    for _ in 0..100 {
        let card = get(socket, &format!("/v1/cards/{id}")).await.unwrap();
        if card["state"] == state {
            return card;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{id} never reached {state}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dispatched_card_runs_in_its_project_and_waits_for_review() {
    let dir = TempDir::new("board-run");
    let d = board(&dir, ANSWERS, 2).await;
    let work = project(&d.socket, &dir).await;
    let id = triaged_card(&d.socket, "1").await;

    let claimed = post(&d.socket, &format!("/v1/cards/{id}/dispatch"), json!({}))
        .await
        .unwrap();
    assert_eq!(claimed["state"], "running");
    let card = card_in(&d.socket, &id, "review").await;
    assert_eq!(card["session_id"], "sess-launched");
    assert_eq!(
        card["outcome"],
        format!("done {id} in {work} launched=1").as_str()
    );

    let done = post(&d.socket, &format!("/v1/cards/{id}/done"), json!({}))
        .await
        .unwrap();
    assert_eq!(done["state"], "done");
    stop(d).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_without_an_answer_puts_the_card_back() {
    let dir = TempDir::new("board-fail");
    let d = board(&dir, FAILS, 2).await;
    project(&d.socket, &dir).await;
    let id = triaged_card(&d.socket, "1").await;
    post(&d.socket, &format!("/v1/cards/{id}/dispatch"), json!({}))
        .await
        .unwrap();
    // Running first, then back: wait for the error to land.
    let mut card = card_in(&d.socket, &id, "triaged").await;
    for _ in 0..100 {
        if !card["last_error"].is_null() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        card = get(&d.socket, &format!("/v1/cards/{id}")).await.unwrap();
    }
    let error = card["last_error"].as_str().unwrap();
    assert!(error.contains("without an answer"), "{error}");
    assert!(error.contains("exit"), "{error}");
    stop(d).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_running_limit_holds_and_release_stops_the_session() {
    let dir = TempDir::new("board-limit");
    let d = board(&dir, HANGS, 1).await;
    project(&d.socket, &dir).await;
    let first = triaged_card(&d.socket, "1").await;
    let second = triaged_card(&d.socket, "2").await;
    post(&d.socket, &format!("/v1/cards/{first}/dispatch"), json!({}))
        .await
        .unwrap();
    let refused = post(
        &d.socket,
        &format!("/v1/cards/{second}/dispatch"),
        json!({}),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.kind, ErrorKind::Conflict);

    let released = post(
        &d.socket,
        &format!("/v1/cards/{first}/release"),
        json!({"reason": "다시 볼게"}),
    )
    .await
    .unwrap();
    assert_eq!(released["state"], "triaged");
    // The stopped run leaves the person's reason in place.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let card = get(&d.socket, &format!("/v1/cards/{first}")).await.unwrap();
    assert_eq!(card["state"], "triaged");
    assert_eq!(card["last_error"], "다시 볼게");
    post(
        &d.socket,
        &format!("/v1/cards/{second}/dispatch"),
        json!({}),
    )
    .await
    .unwrap();
    stop(d).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_requests_are_refused_with_their_kind() {
    let dir = TempDir::new("board-bad");
    let d = serve_in(&dir, None, None).await;
    let missing_dir = post(
        &d.socket,
        "/v1/projects",
        json!({"name": "x", "path": dir.path().join("nope").display().to_string()}),
    )
    .await
    .unwrap_err();
    assert_eq!(missing_dir.kind, ErrorKind::Usage);
    project(&d.socket, &dir).await;
    let id = triaged_card(&d.socket, "1").await;
    let duplicate = post(
        &d.socket,
        "/v1/cards",
        json!({"source": "cli", "external_id": "1", "title": "또"}),
    )
    .await
    .unwrap_err();
    assert_eq!(duplicate.kind, ErrorKind::Conflict);
    let bad_priority = post(
        &d.socket,
        &format!("/v1/cards/{id}/triage"),
        json!({"project": "areum", "priority": "P9"}),
    )
    .await
    .unwrap_err();
    assert_eq!(bad_priority.kind, ErrorKind::Usage);
    // No launch command: dispatch is a configuration problem, not a claim.
    let no_launch = post(&d.socket, &format!("/v1/cards/{id}/dispatch"), json!({}))
        .await
        .unwrap_err();
    assert_eq!(no_launch.kind, ErrorKind::Config);
    let card = get(&d.socket, &format!("/v1/cards/{id}")).await.unwrap();
    assert_eq!(card["state"], "triaged");
    stop(d).await;
}
