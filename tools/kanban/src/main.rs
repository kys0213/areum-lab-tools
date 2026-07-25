mod cli;
mod commands;
mod common;
mod output;

use std::io::Read;

use clap::Parser;

use cli::{Cli, Command, ItemState, Priority, ProjectCommand};
use common::config::resolve_db_path;
use output::{AppError, Payload, Sink};

fn main() {
    let cli = Cli::parse();
    let command_name = cli.command.name();
    let json = cli.json;

    let result = run(cli);
    let (sink, line, code) = output::render(command_name, &result, json);
    match sink {
        Sink::Stdout => println!("{line}"),
        Sink::Stderr => eprintln!("{line}"),
    }
    std::process::exit(code);
}

/// Resolves the board path and dispatches to the command handlers. Pure
/// delegation — no business logic lives here.
fn run(cli: Cli) -> Result<Payload, AppError> {
    let db_path = resolve_db_path(cli.db.as_deref(), std::env::var("HOME").ok().as_deref())?;

    match cli.command {
        Command::Init => commands::run_init(&db_path),
        Command::Project(ProjectCommand::Add { name, description }) => {
            commands::run_project_add(&db_path, &name, &description)
        }
        Command::Project(ProjectCommand::List) => commands::run_project_list(&db_path),
        Command::Project(ProjectCommand::Rm { name }) => commands::run_project_rm(&db_path, &name),
        Command::Add {
            source,
            external_id,
            title,
            body,
        } => commands::run_add(&db_path, &source, &external_id, &title, &body, read_stdin),
        Command::List {
            project,
            state,
            label,
        } => commands::run_list(
            &db_path,
            project.as_deref(),
            state.map(ItemState::as_str),
            label.as_deref(),
        ),
        Command::Show { id } => commands::run_show(&db_path, &id),
        Command::Next {
            project,
            session,
            agent,
        } => commands::run_next(&db_path, &project, &session, &agent),
        Command::Done { id } => commands::run_done(&db_path, &id),
        Command::Release { id, reason } => commands::run_release(&db_path, &id, &reason),
        Command::Assign {
            id,
            project,
            priority,
        } => commands::run_assign(&db_path, &id, &project, priority.map(Priority::as_str)),
        Command::Priority { id, priority } => {
            commands::run_priority(&db_path, &id, priority.as_str())
        }
        Command::Move { id, state } => commands::run_move(&db_path, &id, state.as_str()),
    }
}

fn read_stdin() -> std::io::Result<String> {
    let mut buffer = String::new();
    std::io::stdin().read_to_string(&mut buffer)?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use commands::testutil::TempBoard;
    use common::store::Store;
    use output::{ErrorKind, NextData};

    /// Parses `argv` and runs it through the real dispatch, so the assertions
    /// below cover the clap→handler wiring itself. Every command test calls
    /// its `run_*` directly and would not notice a mis-wired argument here.
    fn dispatch(argv: &[&str]) -> Result<Payload, AppError> {
        let cli = Cli::try_parse_from(argv).expect("argv should parse");
        run(cli)
    }

    #[test]
    fn every_subcommand_dispatches_to_its_handler_with_arguments_in_order() {
        // `main.rs` hands several same-typed arguments to each handler
        // positionally — (source, external_id, title, body) for `add`,
        // (project, state, label) for `list`, (id, project) for `assign`.
        // Swapping any pair still compiles, so every assertion here uses a
        // value that differs per parameter; asserting only "dispatch
        // returned Ok" would let a swap through.
        let board = TempBoard::new("dispatch-order");
        let db = board.db_path().display().to_string();
        let db = db.as_str();

        match dispatch(&["kanban", "--db", db, "init"]).unwrap() {
            Payload::Init(data) => assert!(data.created),
            other => panic!("expected Payload::Init, got {other:?}"),
        }

        match dispatch(&[
            "kanban", "--db", db, "project", "add", "belt", "--desc", "conveyor",
        ])
        .unwrap()
        {
            Payload::ProjectAdd(data) => {
                assert_eq!(data.name, "belt");
                assert_eq!(data.description, "conveyor");
            }
            other => panic!("expected Payload::ProjectAdd, got {other:?}"),
        }

        match dispatch(&["kanban", "--db", db, "project", "list"]).unwrap() {
            Payload::ProjectList(data) => {
                assert_eq!(data.count, 1);
                assert_eq!(data.projects[0].name, "belt");
            }
            other => panic!("expected Payload::ProjectList, got {other:?}"),
        }

        match dispatch(&[
            "kanban",
            "--db",
            db,
            "add",
            "--source",
            "discord",
            "--external-id",
            "msg-1",
            "--title",
            "cache drifts",
            "--body",
            "steps to reproduce",
        ])
        .unwrap()
        {
            Payload::Add(data) => {
                assert_eq!(data.id, "itm-000001");
                assert_eq!(data.source, "discord");
                assert_eq!(data.external_id, "msg-1");
                assert_eq!(data.title, "cache drifts");
                assert_eq!(data.state, "inbox");
            }
            other => panic!("expected Payload::Add, got {other:?}"),
        }

        // `body` is the one `add` argument its payload does not echo, so
        // `show` is what pins it apart from title/source/external_id.
        match dispatch(&["kanban", "--db", db, "show", "itm-000001"]).unwrap() {
            Payload::Show(data) => {
                assert_eq!(data.id, "itm-000001");
                assert_eq!(data.title, "cache drifts");
                assert_eq!(data.body, "steps to reproduce");
                assert_eq!(data.source, "discord");
                assert_eq!(data.external_id, "msg-1");
            }
            other => panic!("expected Payload::Show, got {other:?}"),
        }

        match dispatch(&[
            "kanban",
            "--db",
            db,
            "assign",
            "itm-000001",
            "--project",
            "belt",
            "--priority",
            "P1",
        ])
        .unwrap()
        {
            Payload::Assign(data) => {
                assert_eq!(data.id, "itm-000001");
                assert_eq!(data.project, "belt");
                assert_eq!(data.priority, "P1");
                assert_eq!(data.previous_state, "inbox");
                assert_eq!(data.state, "backlog");
            }
            other => panic!("expected Payload::Assign, got {other:?}"),
        }

        // Only the classifier writes labels, so the --label filter needs one
        // arranged directly. With all three filters set to different values,
        // any swap among them selects nothing.
        Store::open(board.db_path())
            .unwrap()
            .attach_label("itm-000001", "kind", "bug", None)
            .unwrap();
        match dispatch(&[
            "kanban",
            "--db",
            db,
            "list",
            "--project",
            "belt",
            "--state",
            "backlog",
            "--label",
            "kind",
        ])
        .unwrap()
        {
            Payload::List(data) => {
                assert_eq!(data.count, 1);
                assert_eq!(data.items[0].id, "itm-000001");
                assert_eq!(data.items[0].project.as_deref(), Some("belt"));
                assert_eq!(data.items[0].state, "backlog");
                assert_eq!(data.items[0].labels[0].key, "kind");
            }
            other => panic!("expected Payload::List, got {other:?}"),
        }

        match dispatch(&[
            "kanban",
            "--db",
            db,
            "next",
            "--project",
            "belt",
            "--session",
            "sess-abc",
            "--agent",
            "claude",
        ])
        .unwrap()
        {
            Payload::Next(NextData::Claimed(data)) => {
                assert_eq!(data.id, "itm-000001");
                assert_eq!(data.project, "belt");
                assert_eq!(data.session_id, "sess-abc");
                assert_eq!(data.agent, "claude");
            }
            other => panic!("expected a claim, got {other:?}"),
        }

        match dispatch(&[
            "kanban",
            "--db",
            db,
            "release",
            "itm-000001",
            "--reason",
            "build failed",
        ])
        .unwrap()
        {
            Payload::Release(data) => {
                assert_eq!(data.id, "itm-000001");
                assert_eq!(data.reason, "build failed");
                assert_eq!(data.released_session_id.as_deref(), Some("sess-abc"));
                assert_eq!(data.released_agent.as_deref(), Some("claude"));
            }
            other => panic!("expected Payload::Release, got {other:?}"),
        }

        match dispatch(&["kanban", "--db", db, "priority", "itm-000001", "P0"]).unwrap() {
            Payload::Priority(data) => {
                assert_eq!(data.id, "itm-000001");
                assert_eq!(data.previous_priority, "P1");
                assert_eq!(data.priority, "P0");
            }
            other => panic!("expected Payload::Priority, got {other:?}"),
        }

        // Claim it again so `done` runs from the only state it accepts.
        dispatch(&[
            "kanban",
            "--db",
            db,
            "next",
            "--project",
            "belt",
            "--session",
            "sess-two",
            "--agent",
            "codex",
        ])
        .unwrap();
        match dispatch(&["kanban", "--db", db, "done", "itm-000001"]).unwrap() {
            Payload::Done(data) => {
                assert_eq!(data.id, "itm-000001");
                assert_eq!(data.state, "done");
                assert_eq!(data.session_id.as_deref(), Some("sess-two"));
                assert_eq!(data.agent.as_deref(), Some("codex"));
            }
            other => panic!("expected Payload::Done, got {other:?}"),
        }

        match dispatch(&["kanban", "--db", db, "move", "itm-000001", "unmatched"]).unwrap() {
            Payload::Move(data) => {
                assert_eq!(data.id, "itm-000001");
                assert_eq!(data.from_state, "done");
                assert_eq!(data.to_state, "unmatched");
                assert_eq!(data.project, None);
            }
            other => panic!("expected Payload::Move, got {other:?}"),
        }

        // Nothing references `belt` any more, so the removal goes through.
        match dispatch(&["kanban", "--db", db, "project", "rm", "belt"]).unwrap() {
            Payload::ProjectRm(data) => assert_eq!(data.name, "belt"),
            other => panic!("expected Payload::ProjectRm, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_renders_success_and_failure_through_the_json_envelope() {
        // Both envelope shapes, pinned as literals through the real dispatch
        // rather than through a hand-built payload.
        let board = TempBoard::new("dispatch-envelope");
        let db = board.db_path().display().to_string();

        let cli = Cli::try_parse_from(["kanban", "--json", "--db", &db, "list"]).unwrap();
        let name = cli.command.name();
        let result = run(cli);
        let (sink, line, code) = output::render(name, &result, true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            line,
            r#"{"ok":true,"command":"list","data":{"count":0,"items":[]}}"#
        );

        let cli =
            Cli::try_parse_from(["kanban", "--json", "--db", &db, "done", "itm-999999"]).unwrap();
        let name = cli.command.name();
        let result = run(cli);
        assert_eq!(
            result.as_ref().unwrap_err().kind,
            ErrorKind::NotFound,
            "a missing item must not be reported as an internal failure"
        );
        let (sink, line, code) = output::render(name, &result, true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 7);
        assert_eq!(
            line,
            r#"{"ok":false,"command":"done","error":{"kind":"not_found","message":"no item itm-999999"}}"#
        );
    }
}
