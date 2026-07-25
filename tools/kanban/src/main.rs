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
    use output::ErrorKind;

    #[test]
    fn every_remaining_stub_subcommand_dispatches_to_its_loud_stub() {
        // Pins the wiring end to end for the subcommands a later task still
        // owns: each parsed subcommand must reach its handler and surface
        // that handler's "not implemented yet" message, never a panic and
        // never a synthesized success. init/project/add/list/show are
        // implemented (T3) and covered by their own command-level tests
        // instead, since they no longer fail this way.
        let cases: Vec<(Vec<&str>, &str)> = vec![
            (
                vec![
                    "kanban",
                    "next",
                    "--project",
                    "belt",
                    "--session",
                    "sess-abc",
                    "--agent",
                    "claude",
                ],
                "next is not implemented yet",
            ),
            (
                vec!["kanban", "done", "itm-000017"],
                "done is not implemented yet",
            ),
            (
                vec!["kanban", "release", "itm-000017", "--reason", "boom"],
                "release is not implemented yet",
            ),
            (
                vec!["kanban", "assign", "itm-000021", "--project", "belt"],
                "assign is not implemented yet",
            ),
            (
                vec!["kanban", "priority", "itm-000017", "P0"],
                "priority is not implemented yet",
            ),
            (
                vec!["kanban", "move", "itm-000017", "done"],
                "move is not implemented yet",
            ),
        ];

        for (argv, expected) in cases {
            let mut args = argv.clone();
            args.push("--db");
            args.push("/tmp/kanban-dispatch-test.db");
            let cli = Cli::try_parse_from(&args).expect("argv should parse");
            let err = run(cli).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Internal, "for {argv:?}");
            assert_eq!(err.message, expected, "for {argv:?}");
        }
    }

    #[test]
    fn dispatch_renders_a_stub_failure_through_the_json_envelope() {
        // The remaining stubs fail through the normal error path, so --json
        // output is verifiable now rather than after their command bodies
        // land. `done` is still a stub; `show` is not (T3), so it can no
        // longer serve this case.
        let cli = Cli::try_parse_from(["kanban", "--json", "--db", "/tmp/k.db", "done", "itm-1"])
            .unwrap();
        let name = cli.command.name();
        let result = run(cli);
        let (sink, line, code) = output::render(name, &result, true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 1);
        assert_eq!(
            line,
            r#"{"ok":false,"command":"done","error":{"kind":"internal","message":"done is not implemented yet"}}"#
        );
    }
}
