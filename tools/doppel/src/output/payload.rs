//! Result payloads — one variant per subcommand, each carrying the fields
//! that command answers with (docs/doppel.md §7, docs/doppel-judgment.md).
//!
//! These types are the machine-readable contract: their field names and
//! shapes are what `--json` emits, so renaming one is a breaking change for
//! every plugin and skill parsing the envelope.

use std::collections::BTreeMap;

use cli_contract::HumanText;
use serde::Serialize;

use crate::daemon::api::{AskView, SessionView};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OptionData {
    pub label: String,
    pub description: String,
}

/// `init` result. `seeded_categories` lists the seeds this call added —
/// empty on a re-run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InitData {
    pub path: String,
    pub created: bool,
    pub seeded_categories: Vec<String>,
}

/// `ingest sessions|plans` result. `skipped` counts unusable records by
/// reason; records already in the ledger are `skipped_existing`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IngestData {
    pub root: String,
    pub files: usize,
    pub added: usize,
    pub skipped_existing: usize,
    pub skipped: BTreeMap<String, usize>,
}

/// One ledger entry as `decision list` reports it: everything but the long
/// text fields (`options`, `rationale`, `context`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecisionSummaryData {
    pub id: String,
    pub kind: String,
    pub source: String,
    pub project: Option<String>,
    pub question: String,
    pub answers: Vec<String>,
    pub category: Option<String>,
    pub category_by: Option<String>,
    pub decided_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecisionListData {
    pub count: usize,
    pub decisions: Vec<DecisionSummaryData>,
}

/// One ledger entry in full (`decision show`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecisionData {
    pub id: String,
    pub source: String,
    pub external_id: String,
    pub kind: String,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub question: String,
    pub options: Vec<OptionData>,
    pub answers: Vec<String>,
    pub rationale: Option<String>,
    pub context: Option<String>,
    pub category: Option<String>,
    pub category_by: Option<String>,
    pub decided_at: String,
    pub created_at: String,
}

/// `decision categorize` result. `previous_category` is what the filing
/// replaced (`null` for a first filing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CategorizeData {
    pub id: String,
    pub category: String,
    pub category_by: String,
    pub previous_category: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CategoryData {
    pub name: String,
    pub description: String,
    pub state: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CategoryListData {
    pub count: usize,
    pub categories: Vec<CategoryData>,
}

/// One golden entry in full — the precedent as reviewed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoldenData {
    pub id: String,
    pub decision_id: String,
    pub category: String,
    pub project: Option<String>,
    pub question: String,
    pub options: Vec<OptionData>,
    pub answers: Vec<String>,
    pub rationale: Option<String>,
    pub context: Option<String>,
    pub decided_at: String,
    pub state: String,
    pub reason: String,
    pub superseded_by: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
    pub reviewed_at: Option<String>,
}

/// One golden entry as `golden list` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoldenSummaryData {
    pub id: String,
    pub decision_id: String,
    pub category: String,
    pub state: String,
    pub reason: String,
    pub question: String,
    pub answers: Vec<String>,
    pub decided_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoldenListData {
    pub count: usize,
    pub golden: Vec<GoldenSummaryData>,
}

/// `review` — the candidate queue in review order, each in full so the
/// reviewer needs no second call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewData {
    pub count: usize,
    pub candidates: Vec<GoldenData>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchHitData {
    /// Higher is a better match; comparable only within one result.
    pub score: f64,
    pub golden: GoldenData,
}

/// `search` — the precedent bundle a brief is written from.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchData {
    pub query: String,
    pub count: usize,
    pub hits: Vec<SearchHitData>,
}

/// `daemon run|start|stop|status`. `running` is whether a daemon answers on
/// `socket` after the command; `ui_url` is the web UI when it serves one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DaemonData {
    pub running: bool,
    pub socket: String,
    pub ui_url: Option<String>,
    pub pid: Option<u32>,
    pub version: Option<String>,
}

/// `attach` ended: the daemon closed the connection after `messages` lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachData {
    pub session_id: String,
    pub messages: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AskListData {
    pub count: usize,
    pub asks: Vec<AskView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionListData {
    pub count: usize,
    pub sessions: Vec<SessionView>,
}

/// `stats` — how often briefs' recommendations were taken. `acceptance` is
/// `null` until something was judged.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatsData {
    pub briefs: u32,
    pub failed: u32,
    pub new_opinions: u32,
    pub judged: u32,
    pub accepted: u32,
    pub acceptance: Option<f64>,
    pub by_category: Vec<CategoryStatsData>,
}

/// `category` is `null` for asks whose ledger entries are not filed yet.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CategoryStatsData {
    pub category: Option<String>,
    pub judged: u32,
    pub accepted: u32,
    pub acceptance: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Payload {
    Init(InitData),
    Ingest(IngestData),
    DecisionList(DecisionListData),
    DecisionShow(DecisionData),
    DecisionCategorize(CategorizeData),
    CategoryAdd(CategoryData),
    CategoryList(CategoryListData),
    CategoryRetire(CategoryData),
    GoldenPropose(GoldenData),
    GoldenConfirm(GoldenData),
    GoldenRetire(GoldenData),
    GoldenList(GoldenListData),
    Review(ReviewData),
    Search(SearchData),
    DaemonRun(DaemonData),
    DaemonStart(DaemonData),
    DaemonStop(DaemonData),
    DaemonStatus(DaemonData),
    Attach(AttachData),
    AskList(AskListData),
    AskWait(AskView),
    AskAnswer(AskView),
    SessionList(SessionListData),
    Stats(StatsData),
}

impl HumanText for Payload {
    fn to_human(&self) -> String {
        match self {
            Payload::Init(d) => {
                let verb = if d.created {
                    "initialized"
                } else {
                    "already initialized"
                };
                let mut text = format!("doppel {verb}: {}", d.path);
                if !d.seeded_categories.is_empty() {
                    text.push_str(&format!(
                        "\nseeded categories: {}",
                        d.seeded_categories.join(", ")
                    ));
                }
                text
            }
            Payload::Ingest(d) => {
                let mut text = format!(
                    "read {} file(s) under {}: {} added, {} already in the ledger",
                    d.files, d.root, d.added, d.skipped_existing
                );
                for (reason, count) in &d.skipped {
                    text.push_str(&format!("\nskipped {count} ({reason})"));
                }
                text
            }
            Payload::DecisionList(d) => {
                let mut text = format!("{} decision(s)", d.count);
                for decision in &d.decisions {
                    text.push_str(&format!(
                        "\n{} [{}] {} {} → {}",
                        decision.id,
                        decision.category.as_deref().unwrap_or("-"),
                        decision.decided_at,
                        one_line(&decision.question),
                        one_line(&decision.answers.join(", "))
                    ));
                }
                text
            }
            Payload::DecisionShow(d) => format_decision(d),
            Payload::DecisionCategorize(d) => match &d.previous_category {
                Some(previous) => format!(
                    "{} moved from {} to {} (by {})",
                    d.id, previous, d.category, d.category_by
                ),
                None => format!("{} filed under {} (by {})", d.id, d.category, d.category_by),
            },
            Payload::CategoryAdd(d) => format!("added category {}: {}", d.name, d.description),
            Payload::CategoryList(d) => {
                let mut text = format!("{} category(ies)", d.count);
                for category in &d.categories {
                    text.push_str(&format!(
                        "\n{} [{}]: {}",
                        category.name, category.state, category.description
                    ));
                }
                text
            }
            Payload::CategoryRetire(d) => format!("retired category {}", d.name),
            Payload::GoldenPropose(d) => format!(
                "{} proposed from {} in {} ({})",
                d.id, d.decision_id, d.category, d.reason
            ),
            Payload::GoldenConfirm(d) => format!("{} confirmed in {}", d.id, d.category),
            Payload::GoldenRetire(d) => match &d.superseded_by {
                Some(by) => format!("{} retired, superseded by {by}", d.id),
                None => format!("{} retired", d.id),
            },
            Payload::GoldenList(d) => {
                let mut text = format!("{} golden entr(ies)", d.count);
                for golden in &d.golden {
                    text.push_str(&format!(
                        "\n{} [{}/{}] {} → {}",
                        golden.id,
                        golden.category,
                        golden.state,
                        one_line(&golden.question),
                        one_line(&golden.answers.join(", "))
                    ));
                }
                text
            }
            Payload::Review(d) => {
                let mut text = format!("{} candidate(s) to review", d.count);
                for golden in &d.candidates {
                    text.push_str("\n\n");
                    text.push_str(&format_golden(golden));
                }
                text
            }
            Payload::DaemonRun(d) => format!("doppel daemon stopped ({})", d.socket),
            Payload::DaemonStart(d) | Payload::DaemonStatus(d) if d.running => {
                let mut text = format!(
                    "doppel daemon running on {} (pid {}, v{})",
                    d.socket,
                    d.pid.map_or("?".to_owned(), |p| p.to_string()),
                    d.version.as_deref().unwrap_or("?")
                );
                if let Some(url) = &d.ui_url {
                    text.push_str(&format!("\nweb UI: {url}"));
                }
                text
            }
            Payload::DaemonStart(d) | Payload::DaemonStatus(d) => {
                format!("doppel daemon is not running ({})", d.socket)
            }
            Payload::DaemonStop(d) => format!("doppel daemon stopped ({})", d.socket),
            Payload::Attach(d) => format!(
                "attach for {} closed after {} message(s)",
                d.session_id, d.messages
            ),
            Payload::AskList(d) => {
                let mut text = format!("{} ask(s)", d.count);
                for ask in &d.asks {
                    text.push('\n');
                    text.push_str(&format_ask(ask));
                }
                text
            }
            Payload::AskWait(ask) | Payload::AskAnswer(ask) => format_ask(ask),
            Payload::SessionList(d) => {
                let mut text = format!("{} session(s)", d.count);
                for s in &d.sessions {
                    text.push_str(&format!(
                        "\n{} [{}] {} {}{}",
                        s.session_id,
                        s.state,
                        s.cwd.as_deref().unwrap_or("-"),
                        s.last_event_at,
                        if s.open_asks > 0 {
                            format!(" ({} waiting)", s.open_asks)
                        } else {
                            String::new()
                        }
                    ));
                }
                text
            }
            Payload::Stats(d) => {
                let percent = |rate: Option<f64>| {
                    rate.map_or("-".to_owned(), |r| format!("{:.0}%", r * 100.0))
                };
                let mut text = format!(
                    "{} brief(s), {} failed, {} new opinion(s); {} of {} recommendation(s) taken ({})",
                    d.briefs,
                    d.failed,
                    d.new_opinions,
                    d.accepted,
                    d.judged,
                    percent(d.acceptance)
                );
                for c in &d.by_category {
                    text.push_str(&format!(
                        "\n{}: {} of {} ({})",
                        c.category.as_deref().unwrap_or("(uncategorized)"),
                        c.accepted,
                        c.judged,
                        percent(c.acceptance)
                    ));
                }
                text
            }
            Payload::Search(d) => {
                let mut text = format!("{} precedent(s) for \"{}\"", d.count, d.query);
                for hit in &d.hits {
                    text.push_str(&format!("\n\nscore {:.3}\n", hit.score));
                    text.push_str(&format_golden(&hit.golden));
                }
                text
            }
        }
    }
}

fn format_ask(ask: &AskView) -> String {
    let brief = ask
        .brief
        .as_ref()
        .map_or(String::new(), |b| match b.state.as_str() {
            "ready" => match (&b.recommendation, &b.new_opinion) {
                (Some(r), _) => format!(
                    "\n  doppel 추천: {} ({}) — {}",
                    one_line(&r.to_string()),
                    b.confidence.as_deref().unwrap_or("?"),
                    one_line(b.rationale.as_deref().unwrap_or(""))
                ),
                (None, Some(opinion)) => format!("\n  doppel 새 의견: {}", one_line(opinion)),
                (None, None) => String::new(),
            },
            "failed" => format!("\n  brief failed: {}", b.error.as_deref().unwrap_or("")),
            _ => "\n  brief pending".to_owned(),
        });
    format!("{}{brief}", format_ask_line(ask))
}

fn format_ask_line(ask: &AskView) -> String {
    let questions = ask
        .questions
        .as_array()
        .map(|qs| {
            qs.iter()
                .filter_map(|q| q["question"].as_str())
                .map(one_line)
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .unwrap_or_default();
    let mut text = format!(
        "{} [{}/{}] {} — {}",
        ask.ask_id, ask.kind, ask.state, ask.session_id, questions
    );
    if let (Some(answers), Some(by)) = (&ask.answers, &ask.answered_by) {
        text.push_str(&format!(" → {} (by {by})", one_line(&answers.to_string())));
    }
    text
}

/// Collapses newlines so a list row stays one row.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn format_options(options: &[OptionData]) -> String {
    options
        .iter()
        .map(|o| {
            if o.description.is_empty() {
                format!("\n  - {}", o.label)
            } else {
                format!("\n  - {}: {}", o.label, o.description)
            }
        })
        .collect()
}

fn format_decision(d: &DecisionData) -> String {
    let mut text = format!(
        "{} ({}/{}) decided {}\ncategory: {}{}\nproject: {}\nquestion: {}",
        d.id,
        d.source,
        d.kind,
        d.decided_at,
        d.category.as_deref().unwrap_or("-"),
        d.category_by
            .as_deref()
            .map(|by| format!(" (by {by})"))
            .unwrap_or_default(),
        d.project.as_deref().unwrap_or("-"),
        d.question
    );
    if !d.options.is_empty() {
        text.push_str("\noptions:");
        text.push_str(&format_options(&d.options));
    }
    text.push_str(&format!("\nanswer: {}", d.answers.join(", ")));
    if let Some(rationale) = &d.rationale {
        text.push_str(&format!("\nrationale:\n{rationale}"));
    }
    if let Some(context) = &d.context {
        text.push_str(&format!("\ncontext:\n{context}"));
    }
    text
}

fn format_golden(g: &GoldenData) -> String {
    let mut text = format!(
        "{} [{}/{}] from {} ({}), decided {}\nquestion: {}",
        g.id, g.category, g.state, g.decision_id, g.reason, g.decided_at, g.question
    );
    if !g.options.is_empty() {
        text.push_str("\noptions:");
        text.push_str(&format_options(&g.options));
    }
    text.push_str(&format!("\nanswer: {}", g.answers.join(", ")));
    if let Some(rationale) = &g.rationale {
        text.push_str(&format!("\nrationale:\n{rationale}"));
    }
    if let Some(note) = &g.note {
        text.push_str(&format!("\nnote: {note}"));
    }
    text
}
