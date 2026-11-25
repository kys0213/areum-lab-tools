//! The board over a real socket, with scripts standing in for Claude: they
//! speak just enough stream-json for the launcher.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::commands::testutil::TempDir;
use crate::common::error::ErrorKind;
use crate::daemon::auto::{AutoConfig, TriageGate};
use crate::daemon::launch::LaunchConfig;
use crate::daemon::listen::DaemonConfig;
use crate::daemon::model_call::ModelCommand;
use crate::daemon::tests::{Running, config_in, get, post, serve_config, serve_in, stop};

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

/// Proposes areum/P1 with high confidence, after checking the bundle lists
/// the projects.
const PROPOSES: &str = r#"#!/bin/sh
bundle=$(cat)
case "$bundle" in *'"name":"areum"'*) ;; *) echo "no projects in $bundle" >&2; exit 1;; esac
echo '{"type":"result","result":"{\"project\":\"areum\",\"priority\":\"P1\",\"confidence\":\"high\",\"rationale\":\"캐시는 areum 의 일\",\"cited\":[]}"}'
"#;

/// Answers with its arguments and whether it heard the note.
const ECHOES: &str = r#"#!/bin/sh
read -r first
case "$first" in *테스트도*) heard=note;; *) heard=card;; esac
echo '{"type":"system","subtype":"init","session_id":"sess-echo"}'
echo "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"args=$* heard=$heard\"}"
cat > /dev/null
"#;

fn script(dir: &TempDir, name: &str, body: &str) -> String {
    let path = dir.write(name, body);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

fn launch(dir: &TempDir, body: &str, max_running: u32) -> LaunchConfig {
    LaunchConfig {
        argv: vec![script(dir, "session.sh", body), "--fixed".into()],
        max_running,
        socket: dir.path().join("d.sock"),
    }
}

async fn board(dir: &TempDir, body: &str, max_running: u32) -> Running {
    serve_in(dir, None, Some(launch(dir, body, max_running))).await
}

fn proposer(dir: &TempDir) -> ModelCommand {
    ModelCommand {
        argv: vec![script(dir, "propose.sh", PROPOSES)],
        timeout: Duration::from_secs(10),
    }
}

async fn inbox_card(socket: &Path, external_id: &str) -> String {
    let card = post(
        socket,
        "/v1/cards",
        json!({"source": "cli", "external_id": external_id, "title": "캐시 고치기", "body": "재시작하면 사라진다"}),
    )
    .await
    .unwrap();
    card["card_id"].as_str().unwrap().to_owned()
}

async fn proposal_in(socket: &Path, id: &str, state: &str) -> Value {
    let mut card = Value::Null;
    for _ in 0..100 {
        card = get(socket, &format!("/v1/cards/{id}")).await.unwrap();
        if card["proposal"]["state"] == state {
            return card;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the proposal for {id} never reached {state}: {card}");
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
    let id = inbox_card(socket, external_id).await;
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
    let mut card = Value::Null;
    for _ in 0..100 {
        card = get(socket, &format!("/v1/cards/{id}")).await.unwrap();
        if card["state"] == state {
            return card;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{id} never reached {state}: {card}");
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

#[tokio::test(flavor = "multi_thread")]
async fn a_new_card_gets_a_proposal_a_person_can_take() {
    let dir = TempDir::new("board-propose");
    let d = serve_config(DaemonConfig {
        proposal: Some(proposer(&dir)),
        ..config_in(&dir)
    })
    .await;
    project(&d.socket, &dir).await;
    let id = inbox_card(&d.socket, "1").await;
    let card = proposal_in(&d.socket, &id, "ready").await;
    assert_eq!(card["state"], "inbox");
    assert_eq!(card["proposal"]["project"], "areum");
    assert_eq!(card["proposal"]["priority"], "P1");

    let taken = post(
        &d.socket,
        &format!("/v1/cards/{id}/triage"),
        json!({"project": "areum", "priority": "P1"}),
    )
    .await
    .unwrap();
    assert_eq!(taken["proposal"]["accepted"], true);
    // Past the inbox, no new proposal.
    let late = post(&d.socket, &format!("/v1/cards/{id}/propose"), json!({}))
        .await
        .unwrap_err();
    assert_eq!(late.kind, ErrorKind::Conflict);
    stop(d).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn proposing_needs_a_daemon_that_writes_them() {
    let dir = TempDir::new("board-no-propose");
    let d = serve_in(&dir, None, None).await;
    let id = inbox_card(&d.socket, "1").await;
    assert!(get(&d.socket, &format!("/v1/cards/{id}")).await.unwrap()["proposal"].is_null());
    let refused = post(&d.socket, &format!("/v1/cards/{id}/propose"), json!({}))
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ErrorKind::Config);
    stop(d).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn with_both_switched_on_a_sure_proposal_is_assigned_and_run() {
    let dir = TempDir::new("board-auto");
    let d = serve_config(DaemonConfig {
        proposal: Some(proposer(&dir)),
        launch: Some(launch(&dir, ANSWERS, 2)),
        auto: AutoConfig {
            triage: Some(TriageGate {
                min_judged: 0,
                min_acceptance: 0.0,
            }),
            dispatch: true,
        },
        ..config_in(&dir)
    })
    .await;
    project(&d.socket, &dir).await;
    let id = inbox_card(&d.socket, "1").await;
    let card = card_in(&d.socket, &id, "review").await;
    assert_eq!(card["project"], "areum");
    assert_eq!(card["proposal"]["applied"], true);
    // doppel's own assignment is not a judgment.
    assert!(card["proposal"]["accepted"].is_null());

    // Sent back by a person, it waits for them instead of running again.
    post(
        &d.socket,
        &format!("/v1/cards/{id}/release"),
        json!({"reason": "범위가 넓음"}),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        get(&d.socket, &format!("/v1/cards/{id}")).await.unwrap()["state"],
        "triaged"
    );
    stop(d).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_run_continues_the_last_session_with_the_note() {
    let dir = TempDir::new("board-resume");
    let d = board(&dir, ECHOES, 2).await;
    project(&d.socket, &dir).await;
    let id = triaged_card(&d.socket, "1").await;
    let no_session = post(
        &d.socket,
        &format!("/v1/cards/{id}/dispatch"),
        json!({"resume": true}),
    )
    .await
    .unwrap_err();
    assert_eq!(no_session.kind, ErrorKind::Usage);

    post(&d.socket, &format!("/v1/cards/{id}/dispatch"), json!({}))
        .await
        .unwrap();
    let first = card_in(&d.socket, &id, "review").await;
    assert_eq!(first["outcome"], "args=--fixed heard=card");

    post(
        &d.socket,
        &format!("/v1/cards/{id}/release"),
        json!({"reason": "다시"}),
    )
    .await
    .unwrap();
    post(
        &d.socket,
        &format!("/v1/cards/{id}/dispatch"),
        json!({"resume": true, "note": "테스트도 추가해 주세요"}),
    )
    .await
    .unwrap();
    let again = card_in(&d.socket, &id, "review").await;
    assert_eq!(
        again["outcome"],
        "args=--fixed --resume sess-echo heard=note"
    );
    stop(d).await;
}
