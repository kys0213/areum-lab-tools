use clap::{Parser, Subcommand, ValueEnum};

/// Local assistant that learns your judgment: a ledger of the decisions you
/// made in Claude sessions and plans, a golden set you confirm in batch
/// reviews, and deterministic precedent search over it.
///
/// Output is human-readable text by default. Pass --json to emit the
/// machine-readable envelope; agents should always pass --json.
#[derive(Parser, Debug)]
#[command(name = "doppel", version, about)]
pub struct Cli {
    /// Database path override (default: ~/.areum/doppel/doppel.db).
    #[arg(long, global = true)]
    pub db: Option<String>,

    /// Daemon socket override (default: $DOPPEL_SOCKET, else
    /// ~/.areum/doppel/doppel.sock).
    #[arg(long, global = true)]
    pub socket: Option<String>,

    /// Emit machine-readable JSON envelope instead of human text.
    /// Agents should always pass this flag.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create ~/.areum/doppel/ and the database, and register the seed
    /// categories.
    Init,

    /// Read past decisions into the ledger. Already-ingested records are
    /// counted as skipped_existing, never duplicated.
    #[command(subcommand)]
    Ingest(IngestCommand),

    /// Inspect and categorize ledger entries.
    #[command(subcommand)]
    Decision(DecisionCommand),

    /// Manage the categories decisions are filed under.
    #[command(subcommand)]
    Category(CategoryCommand),

    /// Manage golden precedents.
    #[command(subcommand)]
    Golden(GoldenCommand),

    /// The batch-review queue: golden candidates, overrides first.
    Review,

    /// Search confirmed precedents. Deterministic: the same database and
    /// query always answer the same hits in the same order.
    Search {
        /// Free-text query; each word matches as a prefix.
        query: String,
        /// Only precedents in this category.
        #[arg(long)]
        category: Option<String>,
        /// Only precedents from this project.
        #[arg(long)]
        project: Option<String>,
        /// Maximum hits.
        #[arg(long, default_value_t = 5)]
        limit: u32,
        /// Also search candidates that are not confirmed yet.
        #[arg(long = "include-candidates")]
        include_candidates: bool,
    },

    /// Run or control the resident daemon that sessions connect to.
    #[command(subcommand)]
    Daemon(DaemonCommand),

    /// Stream doppel's messages for one session to stdout, one JSON object
    /// per line, until the daemon closes the connection. The plugin runs
    /// this for the session's life.
    Attach {
        /// Claude Code session id.
        #[arg(long)]
        session: String,
    },

    /// Questions and permission confirmations relayed from sessions.
    #[command(subcommand)]
    Ask(AskCommand),

    /// Sessions connected to the daemon.
    #[command(subcommand)]
    Session(SessionCommand),

    /// How often briefs' recommendations were taken, overall and per
    /// category. Reads the database; no daemon needed.
    Stats,
}

/// Daemon settings shared by `run` and `start`. The thresholds decide the
/// session states (docs/doppel.md §5); the daemon never adjusts them itself.
#[derive(clap::Args, Debug, Clone, PartialEq, Eq)]
pub struct DaemonOptions {
    /// Loopback port of the web UI.
    #[arg(long = "ui-port", default_value_t = 7319)]
    pub ui_port: u16,
    /// Serve no web UI.
    #[arg(long = "no-ui")]
    pub no_ui: bool,
    /// A running turn with no event for this long is `stalled`.
    #[arg(long = "stall-after-secs", default_value_t = 600)]
    pub stall_after_secs: u64,
    /// This many tool failures in one turn is `failing`.
    #[arg(long = "fail-streak", default_value_t = 3)]
    pub fail_streak: u32,
    /// No sign of life for this long is `disconnected`.
    #[arg(long = "heartbeat-timeout-secs", default_value_t = 90)]
    pub heartbeat_timeout_secs: u64,
    /// Write a brief for every new ask with the default brief command
    /// (`claude -p --output-format json --model sonnet /doppel:brief`).
    #[arg(long)]
    pub brief: bool,
    /// Write briefs with this command instead (split on whitespace, no
    /// shell). The bundle arrives on its stdin.
    #[arg(long = "brief-command")]
    pub brief_command: Option<String>,
    /// How long one brief may take before it is marked failed.
    #[arg(long = "brief-timeout-secs", default_value_t = 180)]
    pub brief_timeout_secs: u64,
}

#[derive(Subcommand, Debug)]
pub enum DaemonCommand {
    /// Run the daemon in this process until `daemon stop`, SIGINT or SIGTERM.
    Run(DaemonOptions),
    /// Start the daemon in the background and wait until it answers.
    Start(DaemonOptions),
    /// Ask the running daemon to stop.
    Stop,
    /// Whether a daemon answers on the socket.
    Status,
}

#[derive(Subcommand, Debug)]
pub enum AskCommand {
    /// List asks, newest first.
    List {
        /// Only asks in this state (open | deferred | answered | closed).
        #[arg(long)]
        state: Option<String>,
    },
    /// Wait until the ask leaves `open`, or the timeout runs out (the ask is
    /// then answered still open — call again).
    Wait {
        /// Ask id (ask-000001).
        id: String,
        /// Seconds to wait, at most 590.
        #[arg(long, default_value_t = 590)]
        timeout: u64,
    },
    /// Answer an ask from the terminal.
    Answer {
        /// Ask id (ask-000001).
        id: String,
        /// The chosen label(s), for an ask with a single question.
        labels: Vec<String>,
        /// `<question>=<label>`; repeat for several questions or several
        /// labels of a multi-select question.
        #[arg(long = "answer")]
        answers: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionCommand {
    /// List sessions with their derived state, most recently active first.
    List,
}

#[derive(Subcommand, Debug)]
pub enum IngestCommand {
    /// AskUserQuestion answers and user-made permission decisions from
    /// Claude Code session transcripts (*.jsonl under --root).
    Sessions {
        /// Transcript root (default: ~/.claude/projects).
        #[arg(long)]
        root: Option<String>,
    },
    /// Decision records from <repo>/plans/*.md.
    Plans {
        /// Repository root that holds a plans/ directory.
        repo: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum DecisionCommand {
    /// List ledger entries, newest decision first.
    List {
        /// Only decisions of this kind.
        #[arg(long)]
        kind: Option<DecisionKind>,
        /// Only decisions filed under this category.
        #[arg(long, conflicts_with = "uncategorized")]
        category: Option<String>,
        /// Only decisions without a category.
        #[arg(long)]
        uncategorized: bool,
        /// Maximum entries.
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Show one decision in full.
    Show {
        /// Decision id (dec-000001).
        id: String,
    },
    /// File a decision under a category.
    Categorize {
        /// Decision id (dec-000001).
        id: String,
        /// Category name.
        category: String,
        /// Who made the call. A model's filing is a proposal until a human
        /// confirms the precedent in review.
        #[arg(long, value_enum, default_value_t = CategoryBy::Human)]
        by: CategoryBy,
    },
}

#[derive(Subcommand, Debug)]
pub enum CategoryCommand {
    /// Register a category.
    Add {
        /// Category name (kebab-case).
        name: String,
        /// What kind of decision belongs here.
        #[arg(long = "desc")]
        description: String,
    },
    /// List categories, active and retired.
    List,
    /// Retire a category. Retired categories accept no new filings.
    Retire {
        /// Category name.
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum GoldenCommand {
    /// Make a categorized decision a golden candidate.
    Propose {
        /// Decision id (dec-000001).
        decision_id: String,
        /// Why it became a candidate.
        #[arg(long, value_enum, default_value_t = GoldenReason::Manual)]
        reason: GoldenReason,
        /// Free-form note.
        #[arg(long)]
        note: Option<String>,
    },
    /// Confirm a candidate as a precedent.
    Confirm {
        /// Golden id (gld-000001).
        id: String,
        /// Free-form note.
        #[arg(long)]
        note: Option<String>,
    },
    /// Retire a candidate or a precedent.
    Retire {
        /// Golden id (gld-000001).
        id: String,
        /// The confirmed precedent that replaces this one.
        #[arg(long = "superseded-by")]
        superseded_by: Option<String>,
        /// Free-form note.
        #[arg(long)]
        note: Option<String>,
    },
    /// List golden entries.
    List {
        /// Only entries in this state.
        #[arg(long)]
        state: Option<GoldenState>,
        /// Only entries in this category.
        #[arg(long)]
        category: Option<String>,
    },
}

/// What a ledger entry records (docs/doppel-judgment.md §1).
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionKind {
    Ask,
    Permission,
    Plan,
}

impl DecisionKind {
    /// The exact token stored in `decisions.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionKind::Ask => "ask",
            DecisionKind::Permission => "permission",
            DecisionKind::Plan => "plan",
        }
    }
}

/// Who filed a decision under its category.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CategoryBy {
    Human,
    Model,
}

impl CategoryBy {
    /// The exact token stored in `decisions.category_by`.
    pub fn as_str(self) -> &'static str {
        match self {
            CategoryBy::Human => "human",
            CategoryBy::Model => "model",
        }
    }
}

/// Golden lifecycle (docs/doppel-judgment.md §3).
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoldenState {
    Candidate,
    Confirmed,
    Retired,
}

impl GoldenState {
    /// The exact token stored in `golden.state`.
    pub fn as_str(self) -> &'static str {
        match self {
            GoldenState::Candidate => "candidate",
            GoldenState::Confirmed => "confirmed",
            GoldenState::Retired => "retired",
        }
    }
}

/// Why a decision became a golden candidate. `override` — a recommendation
/// the user turned down — sorts first in review.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoldenReason {
    Override,
    Ingest,
    Manual,
}

impl GoldenReason {
    /// The exact token stored in `golden.reason`.
    pub fn as_str(self) -> &'static str {
        match self {
            GoldenReason::Override => "override",
            GoldenReason::Ingest => "ingest",
            GoldenReason::Manual => "manual",
        }
    }
}

impl Command {
    /// Stable command name used as the `command` field in the JSON envelope.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Init => "init",
            Command::Ingest(IngestCommand::Sessions { .. }) => "ingest sessions",
            Command::Ingest(IngestCommand::Plans { .. }) => "ingest plans",
            Command::Decision(DecisionCommand::List { .. }) => "decision list",
            Command::Decision(DecisionCommand::Show { .. }) => "decision show",
            Command::Decision(DecisionCommand::Categorize { .. }) => "decision categorize",
            Command::Category(CategoryCommand::Add { .. }) => "category add",
            Command::Category(CategoryCommand::List) => "category list",
            Command::Category(CategoryCommand::Retire { .. }) => "category retire",
            Command::Golden(GoldenCommand::Propose { .. }) => "golden propose",
            Command::Golden(GoldenCommand::Confirm { .. }) => "golden confirm",
            Command::Golden(GoldenCommand::Retire { .. }) => "golden retire",
            Command::Golden(GoldenCommand::List { .. }) => "golden list",
            Command::Review => "review",
            Command::Search { .. } => "search",
            Command::Daemon(DaemonCommand::Run(_)) => "daemon run",
            Command::Daemon(DaemonCommand::Start(_)) => "daemon start",
            Command::Daemon(DaemonCommand::Stop) => "daemon stop",
            Command::Daemon(DaemonCommand::Status) => "daemon status",
            Command::Attach { .. } => "attach",
            Command::Ask(AskCommand::List { .. }) => "ask list",
            Command::Ask(AskCommand::Wait { .. }) => "ask wait",
            Command::Ask(AskCommand::Answer { .. }) => "ask answer",
            Command::Session(SessionCommand::List) => "session list",
            Command::Stats => "stats",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_flags_parse_before_and_after_the_subcommand() {
        let before =
            Cli::try_parse_from(["doppel", "--json", "--db", "/tmp/d.db", "review"]).unwrap();
        assert!(before.json);
        assert_eq!(before.db.as_deref(), Some("/tmp/d.db"));

        let after =
            Cli::try_parse_from(["doppel", "review", "--json", "--db", "/tmp/d.db"]).unwrap();
        assert!(after.json);
        assert_eq!(after.db.as_deref(), Some("/tmp/d.db"));
    }

    #[test]
    fn category_and_uncategorized_filters_conflict() {
        let err = Cli::try_parse_from([
            "doppel",
            "decision",
            "list",
            "--category",
            "design-scope",
            "--uncategorized",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn categorize_defaults_to_a_human_filing() {
        let cli = Cli::try_parse_from([
            "doppel",
            "decision",
            "categorize",
            "dec-000001",
            "design-scope",
        ])
        .unwrap();
        match cli.command {
            Command::Decision(DecisionCommand::Categorize { by, .. }) => {
                assert_eq!(by, CategoryBy::Human)
            }
            other => panic!("expected categorize, got {other:?}"),
        }
    }

    #[test]
    fn propose_defaults_to_a_manual_reason() {
        let cli = Cli::try_parse_from(["doppel", "golden", "propose", "dec-000001"]).unwrap();
        match cli.command {
            Command::Golden(GoldenCommand::Propose { reason, .. }) => {
                assert_eq!(reason, GoldenReason::Manual)
            }
            other => panic!("expected propose, got {other:?}"),
        }
    }

    #[test]
    fn search_takes_the_query_positionally_with_a_default_limit() {
        let cli = Cli::try_parse_from(["doppel", "search", "테스트 전략"]).unwrap();
        match cli.command {
            Command::Search {
                query,
                limit,
                include_candidates,
                ..
            } => {
                assert_eq!(query, "테스트 전략");
                assert_eq!(limit, 5);
                assert!(!include_candidates);
            }
            other => panic!("expected search, got {other:?}"),
        }
    }

    #[test]
    fn daemon_options_default_to_the_documented_values() {
        let cli = Cli::try_parse_from(["doppel", "daemon", "run"]).unwrap();
        match cli.command {
            Command::Daemon(DaemonCommand::Run(options)) => {
                assert_eq!(options.ui_port, 7319);
                assert!(!options.no_ui);
                assert_eq!(options.stall_after_secs, 600);
                assert_eq!(options.fail_streak, 3);
                assert_eq!(options.heartbeat_timeout_secs, 90);
            }
            other => panic!("expected daemon run, got {other:?}"),
        }
    }

    #[test]
    fn ask_answer_takes_labels_or_question_pairs() {
        let cli = Cli::try_parse_from([
            "doppel",
            "ask",
            "answer",
            "ask-000001",
            "A",
            "--answer",
            "범위는?=API",
        ])
        .unwrap();
        match cli.command {
            Command::Ask(AskCommand::Answer {
                id,
                labels,
                answers,
            }) => {
                assert_eq!(id, "ask-000001");
                assert_eq!(labels, ["A"]);
                assert_eq!(answers, ["범위는?=API"]);
            }
            other => panic!("expected ask answer, got {other:?}"),
        }
    }

    #[test]
    fn nested_command_names_are_space_joined() {
        let cli = Cli::try_parse_from(["doppel", "golden", "list"]).unwrap();
        assert_eq!(cli.command.name(), "golden list");
    }
}
