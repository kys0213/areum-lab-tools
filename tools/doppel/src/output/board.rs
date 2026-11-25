//! Board results (`project …`, `card …`) and their human text.

use serde::Serialize;

use crate::daemon::api::{CardView, ProjectView, ProposalView};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectListData {
    pub count: usize,
    pub projects: Vec<ProjectView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CardListData {
    pub count: usize,
    pub cards: Vec<CardView>,
}

pub(super) fn format_project(p: &ProjectView) -> String {
    if p.description.is_empty() {
        format!("{} {}", p.name, p.path)
    } else {
        format!("{} {} — {}", p.name, p.path, p.description)
    }
}

pub(super) fn format_project_list(d: &ProjectListData) -> String {
    let mut text = format!("{} project(s)", d.count);
    for p in &d.projects {
        text.push('\n');
        text.push_str(&format_project(p));
    }
    text
}

pub(super) fn format_card_line(c: &CardView) -> String {
    format!(
        "{} [{}] {} {} — {}",
        c.card_id,
        c.state,
        c.priority,
        c.project.as_deref().unwrap_or("-"),
        c.title.split_whitespace().collect::<Vec<_>>().join(" ")
    )
}

pub(super) fn format_card_list(d: &CardListData) -> String {
    let mut text = format!("{} card(s)", d.count);
    for c in &d.cards {
        text.push('\n');
        text.push_str(&format_card_line(c));
    }
    text
}

fn format_proposal(p: &ProposalView) -> String {
    match p.state.as_str() {
        "ready" => {
            let mut text = format!(
                "doppel 제안: {} {} ({}) — {}",
                p.project.as_deref().unwrap_or("맞는 프로젝트 없음"),
                p.priority.as_deref().unwrap_or("-"),
                p.confidence.as_deref().unwrap_or("?"),
                p.rationale
                    .as_deref()
                    .unwrap_or("")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            if p.applied {
                text.push_str(" [자동 배정]");
            }
            match p.accepted {
                Some(true) => text.push_str(" [수락됨]"),
                Some(false) => text.push_str(" [뒤집힘]"),
                None => {}
            }
            text
        }
        "failed" => format!("proposal failed: {}", p.error.as_deref().unwrap_or("")),
        _ => "proposal pending".to_owned(),
    }
}

/// The line plus what the card carries beyond it.
pub(super) fn format_card(c: &CardView) -> String {
    let mut text = format_card_line(c);
    text.push_str(&format!("\n  from {}:{}", c.source, c.external_id));
    if let Some(p) = &c.proposal {
        text.push_str(&format!("\n  {}", format_proposal(p)));
    }
    if let Some(session) = &c.session_id {
        text.push_str(&format!("\n  session {session}"));
    }
    if !c.body.is_empty() {
        text.push_str(&format!("\n\n{}", c.body));
    }
    if let Some(error) = &c.last_error {
        text.push_str(&format!("\n\nlast error: {error}"));
    }
    if let Some(outcome) = &c.outcome {
        text.push_str(&format!("\n\noutcome:\n{outcome}"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_line_shows_state_priority_project_and_title() {
        let card = CardView {
            card_id: "crd-000001".into(),
            source: "cli".into(),
            external_id: "a".into(),
            title: "캐시\n고치기".into(),
            body: String::new(),
            state: "triaged".into(),
            project: Some("areum".into()),
            priority: "P1".into(),
            session_id: None,
            claimed_at: None,
            outcome: None,
            last_error: None,
            created_at: String::new(),
            updated_at: String::new(),
            proposal: None,
        };
        assert_eq!(
            format_card_line(&card),
            "crd-000001 [triaged] P1 areum — 캐시 고치기"
        );
    }
}
