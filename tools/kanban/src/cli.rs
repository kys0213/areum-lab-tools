use clap::{Parser, Subcommand, ValueEnum};

/// Local kanban board CLI for AI agents: collect inbound issues, assign them
/// to projects, and let execution agents claim, finish, or release work.
///
/// Output is human-readable text by default. Pass --json to emit the
/// machine-readable envelope; agents should always pass --json.
#[derive(Parser, Debug)]
#[command(name = "kanban", version, about)]
pub struct Cli {
    /// Board database path override (default: ~/.areum/kanban/kanban.db).
    #[arg(long, global = true)]
    pub db: Option<String>,

    /// Emit machine-readable JSON envelope instead of human text.
    /// Agents should always pass this flag.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create the board directory and database (~/.areum/kanban/).
    Init,

    /// Manage the projects that items are assigned to.
    #[command(subcommand)]
    Project(ProjectCommand),

    /// Push an inbound issue onto the board; it lands in `inbox`.
    ///
    /// A repeat of the same --source/--external-id pair is rejected as a
    /// conflict (exit 8) rather than creating a second item.
    Add {
        /// Where the issue came from (discord | github | cli).
        #[arg(long)]
        source: String,
        /// The source's own id for this issue — the dedup key with --source.
        #[arg(long = "external-id")]
        external_id: String,
        /// Issue title.
        #[arg(long)]
        title: String,
        /// Issue body. Pass '-' to read the whole body from stdin.
        #[arg(long)]
        body: String,
    },

    /// List board items, optionally filtered.
    List {
        /// Only items assigned to this project.
        #[arg(long)]
        project: Option<String>,
        /// Only items in this state.
        #[arg(long)]
        state: Option<ItemState>,
        /// Only items carrying a label with this key.
        #[arg(long)]
        label: Option<String>,
    },

    /// Show one item in full, including its body and labels.
    Show {
        /// Item id (itm-000017).
        id: String,
    },

    /// Atomically claim a project's highest-priority backlog item.
    ///
    /// This is a claim, not a query — use `list` to look without taking.
    /// Answers `{"id": null}` when the project's backlog is empty.
    Next {
        /// Project to claim from.
        #[arg(long)]
        project: String,
        /// Claiming session id, recorded as the item's owner.
        #[arg(long)]
        session: String,
        /// Claiming agent name (claude | codex | ...).
        #[arg(long)]
        agent: String,
    },

    /// Complete a claimed item. Keeps session_id/agent so the record of who
    /// did the work survives completion.
    Done {
        /// Item id (itm-000017).
        id: String,
    },

    /// Drop a claim and send the item back to backlog. This is the only
    /// recovery path for an item whose agent died.
    Release {
        /// Item id (itm-000017).
        id: String,
        /// Why the claim was dropped.
        #[arg(long)]
        reason: String,
    },

    /// Assign an item to a project by hand (the `unmatched` correction path).
    Assign {
        /// Item id (itm-000017).
        id: String,
        /// Project to assign it to.
        #[arg(long)]
        project: String,
        /// Also correct the priority while assigning.
        #[arg(long)]
        priority: Option<Priority>,
    },

    /// Correct an item's priority without touching its state.
    Priority {
        /// Item id (itm-000017).
        id: String,
        /// New priority.
        priority: Priority,
    },

    /// Force an item into a state — the escape hatch for transitions the
    /// regular commands refuse.
    Move {
        /// Item id (itm-000017).
        id: String,
        /// Target state.
        state: ItemState,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProjectCommand {
    /// Register a project. Its description is what the classifier reasons
    /// over, so it should describe the kind of work that belongs there.
    Add {
        /// Project name (the id items reference).
        name: String,
        /// Description — doubles as the classification rationale.
        #[arg(long = "desc")]
        description: String,
    },
    /// List registered projects with their descriptions.
    List,
    /// Remove a project. Rejected with a conflict while items still
    /// reference it.
    Rm {
        /// Project name.
        name: String,
    },
}

/// The five board states (docs §4). Parsed by clap so an unknown state is a
/// usage error at the CLI boundary rather than a `CHECK` violation at the
/// storage boundary.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemState {
    Inbox,
    Unmatched,
    Backlog,
    Running,
    Done,
}

impl ItemState {
    /// The exact token stored in `items.state`.
    pub fn as_str(self) -> &'static str {
        match self {
            ItemState::Inbox => "inbox",
            ItemState::Unmatched => "unmatched",
            ItemState::Backlog => "backlog",
            ItemState::Running => "running",
            ItemState::Done => "done",
        }
    }
}

/// Priorities sort lexicographically in priority order, which is why they are
/// stored as these exact tokens (docs §6).
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "verbatim")]
pub enum Priority {
    P0,
    P1,
    P2,
    P3,
}

impl Priority {
    /// The exact token stored in `items.priority`.
    pub fn as_str(self) -> &'static str {
        match self {
            Priority::P0 => "P0",
            Priority::P1 => "P1",
            Priority::P2 => "P2",
            Priority::P3 => "P3",
        }
    }
}

impl Command {
    /// Stable command name used as the `command` field in the JSON envelope.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Init => "init",
            Command::Project(_) => "project",
            Command::Add { .. } => "add",
            Command::List { .. } => "list",
            Command::Show { .. } => "show",
            Command::Next { .. } => "next",
            Command::Done { .. } => "done",
            Command::Release { .. } => "release",
            Command::Assign { .. } => "assign",
            Command::Priority { .. } => "priority",
            Command::Move { .. } => "move",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_parses_without_arguments() {
        let cli = Cli::try_parse_from(["kanban", "init"]).unwrap();
        assert!(matches!(cli.command, Command::Init));
        assert_eq!(cli.command.name(), "init");
        assert!(!cli.json);
        assert_eq!(cli.db, None);
    }

    #[test]
    fn global_flags_parse_before_and_after_the_subcommand() {
        // Both are global=true, so an agent may append --json at the end.
        let before =
            Cli::try_parse_from(["kanban", "--json", "--db", "/tmp/k.db", "list"]).unwrap();
        assert!(before.json);
        assert_eq!(before.db.as_deref(), Some("/tmp/k.db"));

        let after = Cli::try_parse_from(["kanban", "list", "--json", "--db", "/tmp/k.db"]).unwrap();
        assert!(after.json);
        assert_eq!(after.db.as_deref(), Some("/tmp/k.db"));
    }

    #[test]
    fn project_add_parses_name_and_desc() {
        let cli = Cli::try_parse_from(["kanban", "project", "add", "belt", "--desc", "conveyor"])
            .unwrap();
        match cli.command {
            Command::Project(ProjectCommand::Add { name, description }) => {
                assert_eq!(name, "belt");
                assert_eq!(description, "conveyor");
            }
            other => panic!("expected Project(Add), got {other:?}"),
        }
        assert_eq!(
            Cli::try_parse_from(["kanban", "project", "list"])
                .unwrap()
                .command
                .name(),
            "project"
        );
    }

    #[test]
    fn project_add_requires_desc() {
        assert!(Cli::try_parse_from(["kanban", "project", "add", "belt"]).is_err());
    }

    #[test]
    fn project_rm_parses_name() {
        let cli = Cli::try_parse_from(["kanban", "project", "rm", "belt"]).unwrap();
        match cli.command {
            Command::Project(ProjectCommand::Rm { name }) => assert_eq!(name, "belt"),
            other => panic!("expected Project(Rm), got {other:?}"),
        }
    }

    #[test]
    fn project_requires_a_subcommand() {
        assert!(Cli::try_parse_from(["kanban", "project"]).is_err());
    }

    #[test]
    fn add_parses_every_required_flag() {
        let cli = Cli::try_parse_from([
            "kanban",
            "add",
            "--source",
            "discord",
            "--external-id",
            "msg-1",
            "--title",
            "cache drifts",
            "--body",
            "details",
        ])
        .unwrap();
        match cli.command {
            Command::Add {
                source,
                external_id,
                title,
                body,
            } => {
                assert_eq!(source, "discord");
                assert_eq!(external_id, "msg-1");
                assert_eq!(title, "cache drifts");
                assert_eq!(body, "details");
            }
            other => panic!("expected Add, got {other:?}"),
        }
    }

    #[test]
    fn add_accepts_dash_body_for_stdin() {
        // '-' is a legal --body value; the stdin read happens downstream.
        let cli = Cli::try_parse_from([
            "kanban",
            "add",
            "--source",
            "cli",
            "--external-id",
            "local-3",
            "--title",
            "t",
            "--body",
            "-",
        ])
        .unwrap();
        match cli.command {
            Command::Add { body, .. } => assert_eq!(body, "-"),
            other => panic!("expected Add, got {other:?}"),
        }
    }

    #[test]
    fn add_requires_all_four_flags() {
        assert!(Cli::try_parse_from(["kanban", "add", "--source", "cli", "--title", "t"]).is_err());
    }

    #[test]
    fn list_filters_default_to_none() {
        let cli = Cli::try_parse_from(["kanban", "list"]).unwrap();
        match cli.command {
            Command::List {
                project,
                state,
                label,
            } => {
                assert_eq!(project, None);
                assert_eq!(state, None);
                assert_eq!(label, None);
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn list_parses_every_filter() {
        let cli = Cli::try_parse_from([
            "kanban",
            "list",
            "--project",
            "belt",
            "--state",
            "unmatched",
            "--label",
            "duplicate-of",
        ])
        .unwrap();
        match cli.command {
            Command::List {
                project,
                state,
                label,
            } => {
                assert_eq!(project.as_deref(), Some("belt"));
                assert_eq!(state, Some(ItemState::Unmatched));
                assert_eq!(label.as_deref(), Some("duplicate-of"));
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn list_rejects_an_unknown_state() {
        // Fail fast at the CLI boundary rather than at the schema CHECK.
        assert!(Cli::try_parse_from(["kanban", "list", "--state", "failed"]).is_err());
    }

    #[test]
    fn every_state_token_round_trips_through_the_cli() {
        let states = [
            ("inbox", ItemState::Inbox),
            ("unmatched", ItemState::Unmatched),
            ("backlog", ItemState::Backlog),
            ("running", ItemState::Running),
            ("done", ItemState::Done),
        ];
        for (token, expected) in states {
            let cli = Cli::try_parse_from(["kanban", "move", "itm-000017", token]).unwrap();
            match cli.command {
                Command::Move { id, state } => {
                    assert_eq!(id, "itm-000017");
                    assert_eq!(state, expected);
                    assert_eq!(state.as_str(), token);
                }
                other => panic!("expected Move, got {other:?}"),
            }
        }
    }

    #[test]
    fn show_parses_the_item_id() {
        let cli = Cli::try_parse_from(["kanban", "show", "itm-000017"]).unwrap();
        match cli.command {
            Command::Show { id } => assert_eq!(id, "itm-000017"),
            other => panic!("expected Show, got {other:?}"),
        }
    }

    #[test]
    fn next_requires_project_session_and_agent() {
        let cli = Cli::try_parse_from([
            "kanban",
            "next",
            "--project",
            "belt",
            "--session",
            "sess-abc",
            "--agent",
            "claude",
        ])
        .unwrap();
        match cli.command {
            Command::Next {
                project,
                session,
                agent,
            } => {
                assert_eq!(project, "belt");
                assert_eq!(session, "sess-abc");
                assert_eq!(agent, "claude");
            }
            other => panic!("expected Next, got {other:?}"),
        }
        assert!(Cli::try_parse_from(["kanban", "next", "--project", "belt"]).is_err());
    }

    #[test]
    fn done_parses_the_item_id() {
        let cli = Cli::try_parse_from(["kanban", "done", "itm-000017"]).unwrap();
        match cli.command {
            Command::Done { id } => assert_eq!(id, "itm-000017"),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn release_requires_a_reason() {
        let cli = Cli::try_parse_from([
            "kanban",
            "release",
            "itm-000017",
            "--reason",
            "build failed",
        ])
        .unwrap();
        match cli.command {
            Command::Release { id, reason } => {
                assert_eq!(id, "itm-000017");
                assert_eq!(reason, "build failed");
            }
            other => panic!("expected Release, got {other:?}"),
        }
        assert!(Cli::try_parse_from(["kanban", "release", "itm-000017"]).is_err());
    }

    #[test]
    fn assign_priority_is_optional() {
        let cli =
            Cli::try_parse_from(["kanban", "assign", "itm-000021", "--project", "belt"]).unwrap();
        match cli.command {
            Command::Assign {
                id,
                project,
                priority,
            } => {
                assert_eq!(id, "itm-000021");
                assert_eq!(project, "belt");
                assert_eq!(priority, None);
            }
            other => panic!("expected Assign, got {other:?}"),
        }
    }

    #[test]
    fn assign_parses_priority_when_given() {
        let cli = Cli::try_parse_from([
            "kanban",
            "assign",
            "itm-000021",
            "--project",
            "belt",
            "--priority",
            "P1",
        ])
        .unwrap();
        match cli.command {
            Command::Assign { priority, .. } => assert_eq!(priority, Some(Priority::P1)),
            other => panic!("expected Assign, got {other:?}"),
        }
    }

    #[test]
    fn every_priority_token_round_trips_through_the_cli() {
        // Priorities are uppercase in the schema; lowercase must be rejected
        // rather than silently normalized.
        let priorities = [
            ("P0", Priority::P0),
            ("P1", Priority::P1),
            ("P2", Priority::P2),
            ("P3", Priority::P3),
        ];
        for (token, expected) in priorities {
            let cli = Cli::try_parse_from(["kanban", "priority", "itm-000017", token]).unwrap();
            match cli.command {
                Command::Priority { id, priority } => {
                    assert_eq!(id, "itm-000017");
                    assert_eq!(priority, expected);
                    assert_eq!(priority.as_str(), token);
                }
                other => panic!("expected Priority, got {other:?}"),
            }
        }
        assert!(Cli::try_parse_from(["kanban", "priority", "itm-000017", "p1"]).is_err());
        assert!(Cli::try_parse_from(["kanban", "priority", "itm-000017", "P9"]).is_err());
    }

    #[test]
    fn move_requires_both_id_and_state() {
        assert!(Cli::try_parse_from(["kanban", "move", "itm-000017"]).is_err());
        assert_eq!(
            Cli::try_parse_from(["kanban", "move", "itm-000017", "done"])
                .unwrap()
                .command
                .name(),
            "move"
        );
    }

    #[test]
    fn missing_subcommand_is_a_usage_error() {
        assert!(Cli::try_parse_from(["kanban"]).is_err());
    }

    #[test]
    fn reason_flag_is_rejected_outside_release() {
        // --reason is release-only; clap must reject it on done rather than
        // accepting and ignoring it.
        assert!(Cli::try_parse_from(["kanban", "done", "itm-000017", "--reason", "x"]).is_err());
    }
}
