//! Triage proposals (docs/doppel-judgment.md §5 "배정 제안"). Like briefs,
//! the daemon's part is mechanical: build the bundle, run the command, check
//! the answer against the contract. Which project fits is the command's (the
//! plugin's `triage` skill) business.

use serde_json::{Value, json};

use crate::common::error::{AppError, ErrorKind};
use crate::common::store::{
    CardRecord, ProjectRecord, ProposalDraft, SearchFilter, Store, TRIAGE_CATEGORY, fts_query,
};
use crate::daemon::auto;
use crate::daemon::model_call::{answer_object, cited, confidence, optional_string};
use crate::daemon::routes::AppState;

/// How many precedents a bundle carries at most.
const PRECEDENTS: u32 = 5;
const PRIORITIES: [&str; 4] = ["P0", "P1", "P2", "P3"];

pub(crate) const DEFAULT_COMMAND: &str =
    "claude -p --output-format json --model sonnet /doppel:triage";

/// The bundle and the precedent ids it carries: the card, every project,
/// and confirmed assignments whose card reads like this one.
pub(super) fn bundle(
    store: &Store,
    card: &CardRecord,
    projects: &[ProjectRecord],
) -> Result<(Value, Vec<String>), AppError> {
    let hits = match fts_query(&format!("{} {}", card.title, card.body)) {
        Some(expression) => store.search_golden(
            &expression,
            &SearchFilter {
                category: Some(TRIAGE_CATEGORY),
                limit: PRECEDENTS,
                ..SearchFilter::default()
            },
        )?,
        None => Vec::new(),
    };
    let ids = hits.iter().map(|h| h.golden.id.clone()).collect();
    let precedents: Vec<Value> = hits
        .into_iter()
        .map(|h| {
            json!({
                "golden_id": h.golden.id,
                "question": h.golden.question,
                "context": h.golden.context,
                "answers": h.golden.answers,
                "rationale": h.golden.rationale,
                "decided_at": h.golden.decided_at,
                "score": h.score,
            })
        })
        .collect();
    let bundle = json!({
        "v": 1,
        "card": {
            "card_id": card.id,
            "source": card.source,
            "title": card.title,
            "body": card.body,
        },
        "projects": projects
            .iter()
            .map(|p| json!({ "name": p.name, "path": p.path, "description": p.description }))
            .collect::<Vec<_>>(),
        "precedents": precedents,
    });
    Ok((bundle, ids))
}

/// Reads the command's stdout under the proposal contract.
pub(super) fn parse_output(
    stdout: &str,
    projects: &[String],
    precedents: &[String],
) -> Result<ProposalDraft, String> {
    let map = answer_object(stdout, "proposal")?;
    let project = optional_string(&map, "project")?;
    if let Some(name) = project
        .as_deref()
        .filter(|p| !projects.iter().any(|k| k == p))
    {
        return Err(format!(
            "proposes project {name}, which the bundle did not list"
        ));
    }
    let priority = optional_string(&map, "priority")?.ok_or("\"priority\" is missing")?;
    if !PRIORITIES.contains(&priority.as_str()) {
        return Err(format!("\"priority\" must be P0..P3, not {priority}"));
    }
    Ok(ProposalDraft {
        project,
        priority,
        confidence: confidence(&map)?,
        rationale: optional_string(&map, "rationale")?.ok_or("\"rationale\" is missing")?,
        cited: cited(&map, precedents)?,
    })
}

/// What a proposal run needs once its pending row is written.
struct Prepared {
    bundle: Value,
    projects: Vec<String>,
    precedents: Vec<String>,
}

/// Records the proposal as pending and writes it in the background. Fails
/// right away when this daemon writes no proposals, the card is past the
/// inbox, or a proposal is already being written.
pub(super) fn begin(app: &AppState, card_id: &str) -> Result<(), AppError> {
    let config = app.proposal.clone().ok_or_else(|| {
        AppError::new(
            ErrorKind::Config,
            "this daemon writes no proposals (start it with --propose)",
        )
    })?;
    let prepared = {
        let store = app.store();
        let card = store.get_card(card_id)?;
        let projects = store.list_projects()?;
        let (bundle, precedents) = bundle(&store, &card, &projects)?;
        store.start_proposal(card_id, &precedents, &config.argv.join(" "))?;
        Prepared {
            bundle,
            projects: projects.into_iter().map(|p| p.name).collect(),
            precedents,
        }
    };
    app.hub.cards_changed();
    let app = app.clone();
    let card_id = card_id.to_owned();
    tokio::spawn(async move {
        let outcome = config
            .run(&prepared.bundle, None, "proposal")
            .await
            .and_then(|stdout| parse_output(&stdout, &prepared.projects, &prepared.precedents));
        let ready = {
            let store = app.store();
            match &outcome {
                Ok(draft) => store.finish_proposal(&card_id, draft).is_ok(),
                Err(reason) => {
                    let _ = store.fail_proposal(&card_id, reason);
                    false
                }
            }
        };
        app.hub.cards_changed();
        if ready {
            auto::apply_proposal(&app, &card_id);
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projects() -> Vec<String> {
        vec!["areum".into(), "blog".into()]
    }

    fn proposal() -> Value {
        json!({
            "project": "areum",
            "priority": "P1",
            "confidence": "high",
            "rationale": "캐시는 areum 의 일",
            "cited": ["gld-000001"]
        })
    }

    #[test]
    fn a_proposal_within_the_contract_is_read() {
        let stdout = json!({"type": "result", "result": proposal().to_string()}).to_string();
        let draft = parse_output(&stdout, &projects(), &["gld-000001".into()]).unwrap();
        assert_eq!(draft.project.as_deref(), Some("areum"));
        assert_eq!(draft.priority, "P1");
        assert_eq!(draft.cited, ["gld-000001"]);
    }

    #[test]
    fn no_fitting_project_is_a_valid_answer() {
        let mut none = proposal();
        none["project"] = Value::Null;
        none["cited"] = json!([]);
        let draft = parse_output(&none.to_string(), &projects(), &[]).unwrap();
        assert_eq!(draft.project, None);
    }

    #[test]
    fn contract_violations_fail_with_a_reason() {
        let check = |value: Value, needle: &str| {
            let err =
                parse_output(&value.to_string(), &projects(), &["gld-000001".into()]).unwrap_err();
            assert!(err.contains(needle), "{err}");
        };
        let mut unknown = proposal();
        unknown["project"] = json!("mail");
        check(unknown, "did not list");
        let mut priority = proposal();
        priority["priority"] = json!("urgent");
        check(priority, "P0..P3");
        let mut missing = proposal();
        missing["rationale"] = Value::Null;
        check(missing, "rationale");
        let mut cited = proposal();
        cited["cited"] = json!(["gld-000009"]);
        check(cited, "gld-000009");
    }
}
