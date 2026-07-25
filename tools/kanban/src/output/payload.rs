//! Result payloads — one variant per subcommand, each carrying the fields
//! that command answers with. Derived from docs/kanban-board.md §4 (state
//! table), §5 (command surface), and §6 (schema).
//!
//! The whole contract is defined here ahead of the command bodies that will
//! construct it, so nothing outside the tests builds a payload in this stage.
#![allow(dead_code)]

use serde::Serialize;

/// A label row (`kind` | `area` | `duplicate-of` | `similar`). `confidence` is
/// `null` when a human attached the label — the classifier is the only source
/// of a score.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LabelData {
    pub key: String,
    pub value: String,
    pub confidence: Option<f64>,
}

/// A registered project. `description` is the classification rationale, so it
/// is echoed on every project response rather than only on `project list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectData {
    pub name: String,
    pub description: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectListData {
    pub count: usize,
    pub projects: Vec<ProjectData>,
}

/// `project rm` result. A project still referenced by items is rejected with
/// `conflict` (docs §6), so reaching this payload means the row is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectRmData {
    pub name: String,
}

/// `init` result — the database file it ensured exists. `created` is `false`
/// when the board was already initialized, mirroring `discord init`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InitData {
    pub path: String,
    pub created: bool,
}

/// `add` result. Carries the dedup key `(source, external_id)` back so the
/// caller can correlate the minted `id` with what it pushed; a repeat of the
/// same pair is a `conflict`, never a second item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddData {
    pub id: String,
    pub source: String,
    pub external_id: String,
    pub title: String,
    /// Always `"inbox"` — classification happens later, not at intake.
    pub state: String,
    pub created_at: String,
}

/// One item as `list` reports it: the full row minus `body`, which would
/// drown a board listing. `show` is the command that returns the body.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemSummaryData {
    pub id: String,
    pub source: String,
    pub external_id: String,
    pub title: String,
    pub state: String,
    /// `null` in `inbox`/`unmatched`; set from `backlog` onward (docs §4).
    pub project: Option<String>,
    pub priority: String,
    /// Set only while `running`, and preserved into `done` when an agent
    /// finished the work.
    pub session_id: Option<String>,
    pub agent: Option<String>,
    pub claimed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub labels: Vec<LabelData>,
}

/// The full item row, as `show` returns it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemData {
    pub id: String,
    pub source: String,
    pub external_id: String,
    pub title: String,
    pub body: String,
    pub state: String,
    pub project: Option<String>,
    pub priority: String,
    pub session_id: Option<String>,
    pub agent: Option<String>,
    pub claimed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub labels: Vec<LabelData>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListData {
    pub count: usize,
    pub items: Vec<ItemSummaryData>,
}

/// The item `next` claimed: exactly the columns the atomic claim returns
/// (docs §6) plus the claim identity it just wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClaimedItemData {
    pub id: String,
    pub title: String,
    pub body: String,
    pub project: String,
    pub priority: String,
    /// Always `"running"` — a successful claim is the `backlog → running`
    /// transition.
    pub state: String,
    pub session_id: String,
    pub agent: String,
    pub claimed_at: String,
}

/// `next` result. An empty backlog answers exactly `{"id": null}` (docs §5),
/// so agents branch on a null `id` rather than on a missing object; untagged
/// so the claimed case serializes as the bare item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum NextData {
    Claimed(ClaimedItemData),
    /// `id` is always `None`; the variant exists to pin the documented
    /// empty-backlog shape.
    Empty {
        id: Option<String>,
    },
}

impl NextData {
    /// The documented empty-backlog answer, `{"id": null}`.
    pub(crate) fn empty() -> Self {
        NextData::Empty { id: None }
    }
}

/// `done` result. `session_id`/`agent` are **preserved**, not cleared: who did
/// the work has to survive completion (docs §4). Both are `null` when a human
/// completed the item without ever claiming it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoneData {
    pub id: String,
    /// Always `"done"`.
    pub state: String,
    pub project: String,
    pub session_id: Option<String>,
    pub agent: Option<String>,
    pub updated_at: String,
}

/// `release` result. The claim identity is echoed *after* being cleared from
/// the row — `release` empties `session_id`/`agent`/`claimed_at` (docs §4), so
/// these fields report what was dropped, not what the item still holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseData {
    pub id: String,
    /// Always `"backlog"` — failure is a released claim, not a state.
    pub state: String,
    pub project: String,
    pub reason: String,
    pub released_session_id: Option<String>,
    pub released_agent: Option<String>,
    pub updated_at: String,
}

/// `assign` result. `previous_state` is what the human corrected from
/// (`inbox` or `unmatched`), which is also the signal the golden set records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssignData {
    pub id: String,
    /// Always `"backlog"`.
    pub state: String,
    pub previous_state: String,
    pub project: String,
    pub priority: String,
    pub updated_at: String,
}

/// `priority` result — both ends of the correction, so a caller can tell a
/// real change from a no-op re-assertion of the same value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PriorityData {
    pub id: String,
    pub priority: String,
    pub previous_priority: String,
    pub updated_at: String,
}

/// `move` result — the escape-hatch transition, reported as both ends.
/// `project` is `null` when the target state is `inbox`/`unmatched`, which the
/// schema's CHECK requires to be project-less.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MoveData {
    pub id: String,
    pub from_state: String,
    pub to_state: String,
    pub project: Option<String>,
    pub updated_at: String,
}

/// Command result payload. `untagged` so each variant serializes as its inner
/// object directly under the envelope `data` key.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Payload {
    Init(InitData),
    ProjectAdd(ProjectData),
    ProjectList(ProjectListData),
    ProjectRm(ProjectRmData),
    Add(AddData),
    List(ListData),
    Show(ItemData),
    Next(NextData),
    Done(DoneData),
    Release(ReleaseData),
    Assign(AssignData),
    Priority(PriorityData),
    Move(MoveData),
}

impl Payload {
    // Called from `crate::output::render`, the parent module.
    pub(super) fn to_human(&self) -> String {
        match self {
            Payload::Init(d) if d.created => format!("board initialized: {}", d.path),
            Payload::Init(d) => format!("board already initialized: {}", d.path),
            Payload::ProjectAdd(d) => format!("added project {}: {}", d.name, d.description),
            Payload::ProjectList(d) => {
                let mut text = format!("{} project(s)", d.count);
                for project in &d.projects {
                    text.push_str(&format!("\n{}: {}", project.name, project.description));
                }
                text
            }
            Payload::ProjectRm(d) => format!("removed project {}", d.name),
            Payload::Add(d) => format!(
                "added {} to {} ({}/{}): {}",
                d.id, d.state, d.source, d.external_id, d.title
            ),
            Payload::List(d) => {
                let mut text = format!("{} item(s)", d.count);
                for item in &d.items {
                    text.push('\n');
                    text.push_str(&format_summary(item));
                }
                text
            }
            Payload::Show(d) => format_item(d),
            Payload::Next(NextData::Claimed(d)) => format!(
                "claimed {} [{}] in {} as {}/{}: {}",
                d.id, d.priority, d.project, d.agent, d.session_id, d.title
            ),
            Payload::Next(NextData::Empty { .. }) => "no backlog item to claim".to_owned(),
            Payload::Done(d) => format!(
                "{} done in {} ({})",
                d.id,
                d.project,
                format_claim(d.session_id.as_deref(), d.agent.as_deref())
            ),
            Payload::Release(d) => format!(
                "{} released to {} in {} ({}): {}",
                d.id,
                d.state,
                d.project,
                format_claim(
                    d.released_session_id.as_deref(),
                    d.released_agent.as_deref()
                ),
                d.reason
            ),
            Payload::Assign(d) => format!(
                "{} assigned to {} [{}] ({} -> {})",
                d.id, d.project, d.priority, d.previous_state, d.state
            ),
            Payload::Priority(d) => format!(
                "{} priority {} -> {}",
                d.id, d.previous_priority, d.priority
            ),
            Payload::Move(d) => format!("{} moved {} -> {}", d.id, d.from_state, d.to_state),
        }
    }
}

/// Renders a claim identity for human output. `done` keeps its claim and
/// `release` reports the one it dropped, so both need the "no claim" wording.
fn format_claim(session_id: Option<&str>, agent: Option<&str>) -> String {
    match (session_id, agent) {
        (Some(session), Some(agent)) => format!("{agent}/{session}"),
        (Some(session), None) => format!("session {session}"),
        (None, Some(agent)) => format!("agent {agent}"),
        (None, None) => "no session".to_owned(),
    }
}

fn format_project(project: Option<&str>) -> &str {
    project.unwrap_or("-")
}

fn format_summary(item: &ItemSummaryData) -> String {
    let mut line = format!(
        "{}  [{}/{}]  {}  {}",
        item.id,
        item.state,
        item.priority,
        format_project(item.project.as_deref()),
        item.title
    );
    if !item.labels.is_empty() {
        line.push_str(&format!("\n  labels: {}", format_labels(&item.labels)));
    }
    line
}

fn format_item(item: &ItemData) -> String {
    let mut text = format!(
        "{}  [{}/{}]  {}\ntitle: {}\nsource: {}/{}\ncreated: {}  updated: {}",
        item.id,
        item.state,
        item.priority,
        format_project(item.project.as_deref()),
        item.title,
        item.source,
        item.external_id,
        item.created_at,
        item.updated_at
    );
    if item.session_id.is_some() || item.agent.is_some() {
        text.push_str(&format!(
            "\nclaim: {}",
            format_claim(item.session_id.as_deref(), item.agent.as_deref())
        ));
    }
    if !item.labels.is_empty() {
        text.push_str(&format!("\nlabels: {}", format_labels(&item.labels)));
    }
    text.push_str(&format!("\n\n{}", item.body));
    text
}

fn format_labels(labels: &[LabelData]) -> String {
    labels
        .iter()
        .map(|l| format!("{}={}", l.key, l.value))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::{Sink, render};

    fn sample_summary() -> ItemSummaryData {
        ItemSummaryData {
            id: "itm-000017".into(),
            source: "discord".into(),
            external_id: "msg-1".into(),
            title: "cache keeps drifting".into(),
            state: "backlog".into(),
            project: Some("belt".into()),
            priority: "P1".into(),
            session_id: None,
            agent: None,
            claimed_at: None,
            created_at: "2024-01-01T00:00:00Z".into(),
            updated_at: "2024-01-02T00:00:00Z".into(),
            labels: vec![],
        }
    }

    #[test]
    fn human_init_reports_created_and_existing_boards_differently() {
        let fresh = Payload::Init(InitData {
            path: "/home/user/.areum/kanban/kanban.db".into(),
            created: true,
        });
        let (sink, text, code) = render("init", &Ok(fresh), false);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            text,
            "board initialized: /home/user/.areum/kanban/kanban.db"
        );

        let existing = Payload::Init(InitData {
            path: "/home/user/.areum/kanban/kanban.db".into(),
            created: false,
        });
        let (_, text, _) = render("init", &Ok(existing), false);
        assert_eq!(
            text,
            "board already initialized: /home/user/.areum/kanban/kanban.db"
        );
    }

    #[test]
    fn human_next_empty_backlog_is_not_an_error() {
        let payload = Payload::Next(NextData::empty());
        let (sink, text, code) = render("next", &Ok(payload), false);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(text, "no backlog item to claim");
    }

    #[test]
    fn human_next_claim_names_the_holder() {
        let payload = Payload::Next(NextData::Claimed(ClaimedItemData {
            id: "itm-000017".into(),
            title: "fix the build".into(),
            body: "steps".into(),
            project: "belt".into(),
            priority: "P1".into(),
            state: "running".into(),
            session_id: "sess-abc".into(),
            agent: "claude".into(),
            claimed_at: "2024-01-02T00:00:00Z".into(),
        }));
        let (_, text, _) = render("next", &Ok(payload), false);
        assert_eq!(
            text,
            "claimed itm-000017 [P1] in belt as claude/sess-abc: fix the build"
        );
    }

    #[test]
    fn human_done_without_a_session_reads_as_no_session() {
        // A human-completed item has no claim; the wording must not imply one.
        let payload = Payload::Done(DoneData {
            id: "itm-000017".into(),
            state: "done".into(),
            project: "belt".into(),
            session_id: None,
            agent: None,
            updated_at: "2024-01-03T00:00:00Z".into(),
        });
        let (_, text, _) = render("done", &Ok(payload), false);
        assert_eq!(text, "itm-000017 done in belt (no session)");
    }

    #[test]
    fn human_release_reports_the_dropped_claim_and_reason() {
        let payload = Payload::Release(ReleaseData {
            id: "itm-000017".into(),
            state: "backlog".into(),
            project: "belt".into(),
            reason: "build failed".into(),
            released_session_id: Some("sess-abc".into()),
            released_agent: Some("claude".into()),
            updated_at: "2024-01-03T00:00:00Z".into(),
        });
        let (_, text, _) = render("release", &Ok(payload), false);
        assert_eq!(
            text,
            "itm-000017 released to backlog in belt (claude/sess-abc): build failed"
        );
    }

    #[test]
    fn human_list_renders_one_line_per_item_with_a_count_header() {
        let payload = Payload::List(ListData {
            count: 1,
            items: vec![sample_summary()],
        });
        let (_, text, code) = render("list", &Ok(payload), false);
        assert_eq!(code, 0);
        assert_eq!(
            text,
            "1 item(s)\nitm-000017  [backlog/P1]  belt  cache keeps drifting"
        );
    }

    #[test]
    fn human_list_shows_a_dash_for_an_unassigned_project() {
        let mut item = sample_summary();
        item.state = "inbox".into();
        item.project = None;
        let payload = Payload::List(ListData {
            count: 1,
            items: vec![item],
        });
        let (_, text, _) = render("list", &Ok(payload), false);
        assert_eq!(
            text,
            "1 item(s)\nitm-000017  [inbox/P1]  -  cache keeps drifting"
        );
    }

    #[test]
    fn human_success_output_never_contains_the_json_envelope_marker() {
        // json=false success must be plain text, never the {"ok":...} shape —
        // that boundary is how agents tell the two modes apart.
        let payloads = [
            Payload::Init(InitData {
                path: "/db".into(),
                created: true,
            }),
            Payload::ProjectRm(ProjectRmData {
                name: "belt".into(),
            }),
            Payload::Next(NextData::empty()),
            Payload::Move(MoveData {
                id: "itm-000017".into(),
                from_state: "running".into(),
                to_state: "done".into(),
                project: Some("belt".into()),
                updated_at: "2024-01-03T00:00:00Z".into(),
            }),
        ];
        for payload in payloads {
            let (sink, text, code) = render("cmd", &Ok(payload), false);
            assert_eq!(sink, Sink::Stdout);
            assert_eq!(code, 0);
            assert!(
                !text.contains(r#"{"ok"#),
                "human output leaked json envelope: {text}"
            );
        }
    }
}
