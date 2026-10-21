mod cli;
mod commands;
mod common;
mod daemon;
mod ingest;
mod output;

use std::path::Path;

use clap::Parser;

use cli::{
    AskCommand, CategoryCommand, Cli, Command, DaemonCommand, DecisionCommand, GoldenCommand,
    IngestCommand, SessionCommand,
};
use common::config::{resolve_db_path, resolve_socket_path, resolve_transcript_root};
use common::store::{DecisionFilter, GoldenFilter, SearchFilter};
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

/// Resolves paths and dispatches to the command handlers. Pure delegation —
/// no business logic lives here.
fn run(cli: Cli) -> Result<Payload, AppError> {
    let home = std::env::var("HOME").ok();
    let db_path = resolve_db_path(cli.db.as_deref(), home.as_deref())?;
    let db = db_path.as_path();
    let socket_env = std::env::var("DOPPEL_SOCKET").ok();
    let socket = || {
        resolve_socket_path(
            cli.socket.as_deref(),
            socket_env.as_deref(),
            home.as_deref(),
        )
    };

    match cli.command {
        Command::Init => commands::run_init(db),
        Command::Ingest(IngestCommand::Sessions { root }) => {
            let root = resolve_transcript_root(root.as_deref(), home.as_deref())?;
            commands::run_ingest_sessions(db, &root)
        }
        Command::Ingest(IngestCommand::Plans { repo }) => {
            commands::run_ingest_plans(db, Path::new(&repo))
        }
        Command::Decision(DecisionCommand::List {
            kind,
            category,
            uncategorized,
            limit,
        }) => commands::run_decision_list(
            db,
            &DecisionFilter {
                kind: kind.map(|k| k.as_str()),
                category: category.as_deref(),
                uncategorized,
                limit,
            },
        ),
        Command::Decision(DecisionCommand::Show { id }) => commands::run_decision_show(db, &id),
        Command::Decision(DecisionCommand::Categorize { id, category, by }) => {
            commands::run_decision_categorize(db, &id, &category, by.as_str())
        }
        Command::Category(CategoryCommand::Add { name, description }) => {
            commands::run_category_add(db, &name, &description)
        }
        Command::Category(CategoryCommand::List) => commands::run_category_list(db),
        Command::Category(CategoryCommand::Retire { name }) => {
            commands::run_category_retire(db, &name)
        }
        Command::Golden(GoldenCommand::Propose {
            decision_id,
            reason,
            note,
        }) => commands::run_golden_propose(db, &decision_id, reason.as_str(), note.as_deref()),
        Command::Golden(GoldenCommand::Confirm { id, note }) => {
            commands::run_golden_confirm(db, &id, note.as_deref())
        }
        Command::Golden(GoldenCommand::Retire {
            id,
            superseded_by,
            note,
        }) => commands::run_golden_retire(db, &id, superseded_by.as_deref(), note.as_deref()),
        Command::Golden(GoldenCommand::List { state, category }) => commands::run_golden_list(
            db,
            &GoldenFilter {
                state: state.map(|s| s.as_str()),
                category: category.as_deref(),
            },
        ),
        Command::Review => commands::run_review(db),
        Command::Search {
            query,
            category,
            project,
            limit,
            include_candidates,
        } => commands::run_search(
            db,
            &query,
            &SearchFilter {
                category: category.as_deref(),
                project: project.as_deref(),
                include_candidates,
                limit,
            },
        ),
        Command::Daemon(DaemonCommand::Run(options)) => {
            commands::run_daemon_run(db, &socket()?, &options)
        }
        Command::Daemon(DaemonCommand::Start(options)) => {
            commands::run_daemon_start(db, &socket()?, &options)
        }
        Command::Daemon(DaemonCommand::Stop) => commands::run_daemon_stop(&socket()?),
        Command::Daemon(DaemonCommand::Status) => commands::run_daemon_status(&socket()?),
        Command::Attach { ref session } => commands::run_attach(&socket()?, session),
        Command::Ask(AskCommand::List { ref state }) => {
            commands::run_ask_list(&socket()?, state.as_deref())
        }
        Command::Ask(AskCommand::Wait { ref id, timeout }) => {
            commands::run_ask_wait(&socket()?, id, timeout)
        }
        Command::Ask(AskCommand::Answer {
            ref id,
            ref labels,
            ref answers,
        }) => commands::run_ask_answer(&socket()?, id, labels, answers),
        Command::Session(SessionCommand::List) => commands::run_session_list(&socket()?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use commands::testutil::TempDir;

    /// Parses `argv` and runs it through the real dispatch, so the assertions
    /// cover the clap→handler wiring itself — every command test calls its
    /// `run_*` directly and would not notice a mis-wired argument here.
    fn dispatch(argv: &[&str]) -> Result<Payload, AppError> {
        run(Cli::try_parse_from(argv).expect("argv should parse"))
    }

    #[test]
    fn a_session_answer_flows_from_ingest_to_search() {
        let dir = TempDir::new("dispatch");
        let db = dir.db_path().display().to_string();
        let db = db.as_str();
        dir.write(
            "projects/-work/sess-1.jsonl",
            concat!(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"AskUserQuestion","input":{"questions":[{"question":"테스트 전략은?","options":[{"label":"블랙박스","description":"공개 API 기준"}]}]}}]}}"#,
                "\n",
                r#"{"type":"user","sessionId":"sess-1","uuid":"u-1","cwd":"/work","timestamp":"2026-10-21T01:02:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1"}]},"toolUseResult":{"answers":{"테스트 전략은?":"블랙박스"}}}"#
            ),
        );
        let root = dir.path().join("projects").display().to_string();

        dispatch(&["doppel", "--db", db, "init"]).unwrap();
        match dispatch(&["doppel", "--db", db, "ingest", "sessions", "--root", &root]).unwrap() {
            Payload::Ingest(data) => assert_eq!(data.added, 1),
            other => panic!("expected Ingest, got {other:?}"),
        }
        match dispatch(&[
            "doppel",
            "--db",
            db,
            "decision",
            "list",
            "--kind",
            "ask",
            "--uncategorized",
        ])
        .unwrap()
        {
            Payload::DecisionList(data) => assert_eq!(data.decisions[0].id, "dec-000001"),
            other => panic!("expected DecisionList, got {other:?}"),
        }
        dispatch(&[
            "doppel",
            "--db",
            db,
            "decision",
            "categorize",
            "dec-000001",
            "naming-api-test",
            "--by",
            "model",
        ])
        .unwrap();
        match dispatch(&[
            "doppel",
            "--db",
            db,
            "golden",
            "propose",
            "dec-000001",
            "--reason",
            "ingest",
        ])
        .unwrap()
        {
            Payload::GoldenPropose(data) => {
                assert_eq!(data.category, "naming-api-test");
                assert_eq!(data.reason, "ingest");
            }
            other => panic!("expected GoldenPropose, got {other:?}"),
        }
        match dispatch(&["doppel", "--db", db, "review"]).unwrap() {
            Payload::Review(data) => assert_eq!(data.count, 1),
            other => panic!("expected Review, got {other:?}"),
        }
        dispatch(&["doppel", "--db", db, "golden", "confirm", "gld-000001"]).unwrap();
        match dispatch(&[
            "doppel",
            "--db",
            db,
            "search",
            "테스트는 어떻게",
            "--category",
            "naming-api-test",
            "--limit",
            "3",
        ])
        .unwrap()
        {
            Payload::Search(data) => {
                assert_eq!(data.count, 1);
                assert_eq!(data.hits[0].golden.answers, ["블랙박스"]);
            }
            other => panic!("expected Search, got {other:?}"),
        }
        match dispatch(&[
            "doppel",
            "--db",
            db,
            "golden",
            "list",
            "--state",
            "confirmed",
            "--category",
            "design-scope",
        ])
        .unwrap()
        {
            Payload::GoldenList(data) => assert_eq!(data.count, 0),
            other => panic!("expected GoldenList, got {other:?}"),
        }
    }

    #[test]
    fn category_and_golden_retire_arguments_reach_their_handlers() {
        let dir = TempDir::new("dispatch-retire");
        let db = dir.db_path().display().to_string();
        let db = db.as_str();
        match dispatch(&[
            "doppel", "--db", db, "category", "add", "priority", "--desc", "순서",
        ])
        .unwrap()
        {
            Payload::CategoryAdd(data) => {
                assert_eq!(data.name, "priority");
                assert_eq!(data.description, "순서");
            }
            other => panic!("expected CategoryAdd, got {other:?}"),
        }
        match dispatch(&["doppel", "--db", db, "category", "retire", "priority"]).unwrap() {
            Payload::CategoryRetire(data) => assert_eq!(data.state, "retired"),
            other => panic!("expected CategoryRetire, got {other:?}"),
        }
        let err = dispatch(&[
            "doppel",
            "--db",
            db,
            "golden",
            "retire",
            "gld-000001",
            "--superseded-by",
            "gld-000002",
        ])
        .unwrap_err();
        assert_eq!(err.kind, common::error::ErrorKind::NotFound);
        assert!(err.message.contains("gld-000001"));
    }
}
