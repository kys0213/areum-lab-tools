//! Brief runs (docs/doppel-judgment.md §5). The daemon's part is mechanical:
//! build the precedent bundle, hand it to the configured command, and check
//! the answer against the contract. What the brief says is the command's
//! (the plugin's `brief` skill) business.

use serde_json::{Value, json};

use crate::common::error::AppError;
use crate::common::store::{
    AskRecord, BriefDraft, SearchFilter, SessionFacts, Store, fts_query, validate_answers,
};
use crate::daemon::model_call::{answer_object, cited, confidence, optional_string};
use crate::daemon::routes::AppState;

/// How many precedents a bundle carries at most.
const PRECEDENTS: u32 = 5;

pub(crate) const DEFAULT_COMMAND: &str =
    "claude -p --output-format json --model sonnet /doppel:brief";

/// The bundle and the precedent ids it carries. The search query is every
/// question and option label of the ask, so a precedent about any of the
/// choices is found.
pub(super) fn bundle(
    store: &Store,
    ask: &AskRecord,
    session: Option<&SessionFacts>,
) -> Result<(Value, Vec<String>), AppError> {
    let questions = ask.questions.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let text: Vec<&str> = questions
        .iter()
        .flat_map(|q| {
            std::iter::once(q["question"].as_str().unwrap_or("")).chain(
                q["options"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                    .iter()
                    .map(|o| o["label"].as_str().unwrap_or("")),
            )
        })
        .collect();
    let hits = match fts_query(&text.join(" ")) {
        Some(expression) => store.search_golden(
            &expression,
            &SearchFilter {
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
                "category": h.golden.category,
                "question": h.golden.question,
                "options": h.golden.options.iter().map(|o| json!({"label": o.label, "description": o.description})).collect::<Vec<_>>(),
                "answers": h.golden.answers,
                "rationale": h.golden.rationale,
                "decided_at": h.golden.decided_at,
                "score": h.score,
            })
        })
        .collect();
    let bundle = json!({
        "v": 1,
        "ask": { "ask_id": ask.id, "kind": ask.kind, "questions": ask.questions },
        "session": {
            "session_id": ask.session_id,
            "cwd": session.and_then(|s| s.cwd.clone()),
            "last_summary": session.and_then(|s| s.last_summary.clone()),
        },
        "precedents": precedents,
    });
    Ok((bundle, ids))
}

/// Reads the command's stdout under the brief contract
/// (docs/doppel-judgment.md §5).
pub(super) fn parse_output(
    stdout: &str,
    questions: &Value,
    precedents: &[String],
) -> Result<BriefDraft, String> {
    let map = answer_object(stdout, "brief")?;
    let confidence = confidence(&map)?;
    let rationale = optional_string(&map, "rationale")?.ok_or("\"rationale\" is missing")?;
    let new_opinion = optional_string(&map, "new_opinion")?;
    let recommendation = match map.get("recommendation") {
        None | Some(Value::Null) => None,
        Some(answers) => {
            validate_answers(questions, answers).map_err(|e| {
                format!(
                    "\"recommendation\" does not fit the questions: {}",
                    e.message
                )
            })?;
            Some(answers.clone())
        }
    };
    if recommendation.is_none() && new_opinion.is_none() {
        return Err("the brief has neither a recommendation nor a new opinion".to_owned());
    }
    let reversible = match map.get("reversible") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err("\"reversible\" must be true, false or null".to_owned()),
    };
    Ok(BriefDraft {
        background: optional_string(&map, "background")?,
        recommendation,
        new_opinion,
        confidence,
        reversible,
        rationale,
        cited: cited(&map, precedents)?,
    })
}

/// What a brief run needs once its pending row is written.
struct Prepared {
    bundle: Value,
    questions: Value,
    cwd: Option<String>,
    precedents: Vec<String>,
}

/// Builds the bundle and records the brief as pending. `None` when the ask
/// already has a brief — a re-sent create must not run the command twice.
fn prepare(store: &Store, ask_id: &str, command: &str) -> Result<Option<Prepared>, AppError> {
    let ask = store.get_ask(ask_id)?;
    if store.get_brief(ask_id)?.is_some() {
        return Ok(None);
    }
    let session = store.get_session(&ask.session_id).ok();
    let (bundle, precedents) = bundle(store, &ask, session.as_ref())?;
    store.start_brief(ask_id, &precedents, command)?;
    Ok(Some(Prepared {
        bundle,
        questions: ask.questions,
        cwd: session.and_then(|s| s.cwd),
        precedents,
    }))
}

/// Writes the brief for one ask, start to finish. Every outcome ends in the
/// store (`ready` or `failed`); nothing is reported back to the request that
/// opened the ask.
pub(super) async fn run(app: AppState, ask_id: String) {
    let Some(config) = app.brief.clone() else {
        return;
    };
    let prepared = prepare(&app.store(), &ask_id, &config.argv.join(" "));
    let Ok(Some(prepared)) = prepared else {
        return;
    };
    app.hub.ask_changed(&ask_id);

    let outcome = config
        .run(&prepared.bundle, prepared.cwd.as_deref(), "brief")
        .await
        .and_then(|stdout| parse_output(&stdout, &prepared.questions, &prepared.precedents));
    {
        let store = app.store();
        let _ = match &outcome {
            Ok(draft) => store.finish_brief(&ask_id, draft),
            Err(reason) => store.fail_brief(&ask_id, reason),
        };
    }
    app.hub.ask_changed(&ask_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn questions() -> Value {
        json!([{"question": "위치는?", "options": [{"label": "메모리"}, {"label": "디스크"}]}])
    }

    fn brief() -> Value {
        json!({
            "background": "캐시 설계 중",
            "recommendation": {"위치는?": "디스크"},
            "new_opinion": null,
            "confidence": "high",
            "reversible": true,
            "rationale": "지난 판례도 디스크",
            "cited": ["gld-000001"]
        })
    }

    fn ids() -> Vec<String> {
        vec!["gld-000001".into()]
    }

    #[test]
    fn a_claude_result_envelope_is_unwrapped() {
        let stdout =
            json!({"type": "result", "is_error": false, "result": brief().to_string()}).to_string();
        let draft = parse_output(&stdout, &questions(), &ids()).unwrap();
        assert_eq!(draft.recommendation, Some(json!({"위치는?": "디스크"})));
        assert_eq!(draft.reversible, Some(true));
        assert_eq!(draft.cited, ["gld-000001"]);
    }

    #[test]
    fn a_bare_brief_and_one_code_fence_are_accepted() {
        assert!(parse_output(&brief().to_string(), &questions(), &ids()).is_ok());
        let fenced = format!("```json\n{}\n```", brief());
        let stdout = json!({"type": "result", "result": fenced}).to_string();
        assert!(parse_output(&stdout, &questions(), &ids()).is_ok());
    }

    #[test]
    fn contract_violations_fail_with_a_reason() {
        let prose = json!({"type": "result", "result": "디스크가 좋겠습니다"}).to_string();
        assert!(
            parse_output(&prose, &questions(), &ids())
                .unwrap_err()
                .contains("not one JSON object")
        );

        let mut unknown = brief();
        unknown["cited"] = json!(["gld-000099"]);
        assert!(
            parse_output(&unknown.to_string(), &questions(), &ids())
                .unwrap_err()
                .contains("gld-000099")
        );

        let mut misfit = brief();
        misfit["recommendation"] = json!({"다른 질문": "x"});
        assert!(
            parse_output(&misfit.to_string(), &questions(), &ids())
                .unwrap_err()
                .contains("does not fit")
        );

        let mut empty = brief();
        empty["recommendation"] = Value::Null;
        assert!(
            parse_output(&empty.to_string(), &questions(), &ids())
                .unwrap_err()
                .contains("neither")
        );

        let mut sure = brief();
        sure["confidence"] = json!("certain");
        assert!(parse_output(&sure.to_string(), &questions(), &ids()).is_err());

        let failed =
            json!({"type": "result", "is_error": true, "result": "rate limited"}).to_string();
        assert!(
            parse_output(&failed, &questions(), &ids())
                .unwrap_err()
                .contains("rate limited")
        );
    }

    #[test]
    fn a_new_opinion_alone_is_a_valid_brief() {
        let mut opinion = brief();
        opinion["recommendation"] = Value::Null;
        opinion["new_opinion"] = json!("판례가 맞지 않아 메모리를 제안");
        opinion["cited"] = json!([]);
        let draft = parse_output(&opinion.to_string(), &questions(), &[]).unwrap();
        assert_eq!(draft.recommendation, None);
        assert!(draft.new_opinion.is_some());
    }
}
