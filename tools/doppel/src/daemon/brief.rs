//! Brief runs (docs/doppel-judgment.md §5). The daemon's part is mechanical:
//! build the precedent bundle, hand it to the configured command, and check
//! the answer against the contract. What the brief says is the command's
//! (the plugin's `brief` skill) business.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use crate::common::error::AppError;
use crate::common::store::{
    AskRecord, BriefDraft, SearchFilter, SessionFacts, Store, fts_query, validate_answers,
};
use crate::daemon::routes::AppState;

/// How many precedents a bundle carries at most.
const PRECEDENTS: u32 = 5;
/// How much of the command's stderr a failed brief keeps.
const STDERR_TAIL: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BriefConfig {
    pub(crate) argv: Vec<String>,
    pub(crate) timeout: Duration,
}

impl BriefConfig {
    pub(crate) const DEFAULT_COMMAND: &'static str =
        "claude -p --output-format json --model sonnet /doppel:brief";

    /// Splits on whitespace — no shell, no quoting. `None` for a blank
    /// command.
    pub(crate) fn from_command(command: &str, timeout: Duration) -> Option<BriefConfig> {
        let argv: Vec<String> = command.split_whitespace().map(str::to_owned).collect();
        (!argv.is_empty()).then_some(BriefConfig { argv, timeout })
    }
}

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

/// Reads the command's stdout under the brief contract. `claude -p
/// --output-format json` wraps the model's text in `{"type":"result",
/// "result": …}`; another command may print the brief directly. One layer of
/// code fence around the object is removed — models add it unasked —
/// anything else that is not one JSON object is a contract violation.
pub(super) fn parse_output(
    stdout: &str,
    questions: &Value,
    precedents: &[String],
) -> Result<BriefDraft, String> {
    let outer: Option<Value> = serde_json::from_str(stdout.trim()).ok();
    let text = match &outer {
        Some(Value::Object(map)) if map.get("type").and_then(Value::as_str) == Some("result") => {
            if map.get("is_error").and_then(Value::as_bool) == Some(true) {
                return Err(format!(
                    "the brief command reported an error: {}",
                    map.get("result").and_then(Value::as_str).unwrap_or("")
                ));
            }
            map.get("result")
                .and_then(Value::as_str)
                .ok_or("the result envelope has no text")?
                .to_owned()
        }
        _ => stdout.to_owned(),
    };
    let body = strip_fence(text.trim());
    let brief: Value =
        serde_json::from_str(body).map_err(|e| format!("the brief is not one JSON object: {e}"))?;
    let map = brief.as_object().ok_or("the brief is not a JSON object")?;

    let string = |key: &str| -> Result<Option<String>, String> {
        match map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
            Some(Value::String(_)) => Ok(None),
            Some(_) => Err(format!("\"{key}\" must be a string")),
        }
    };
    let confidence = string("confidence")?.ok_or("\"confidence\" is missing")?;
    if !matches!(confidence.as_str(), "low" | "medium" | "high") {
        return Err(format!(
            "\"confidence\" must be low, medium or high, not {confidence}"
        ));
    }
    let rationale = string("rationale")?.ok_or("\"rationale\" is missing")?;
    let new_opinion = string("new_opinion")?;
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
    let cited: Vec<String> = match map.get("cited") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| {
                i.as_str()
                    .map(str::to_owned)
                    .ok_or("\"cited\" must list ids")
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("\"cited\" must be a list".to_owned()),
    };
    if let Some(unknown) = cited.iter().find(|id| !precedents.contains(id)) {
        return Err(format!("cites {unknown}, which the bundle did not carry"));
    }
    Ok(BriefDraft {
        background: string("background")?,
        recommendation,
        new_opinion,
        confidence,
        reversible,
        rationale,
        cited,
    })
}

fn strip_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let after_info = rest.split_once('\n').map_or("", |(_, body)| body);
    after_info
        .trim_end()
        .strip_suffix("```")
        .unwrap_or(after_info)
        .trim()
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

    let outcome = execute(&config, &prepared.bundle, prepared.cwd.as_deref())
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

/// Runs the command with the bundle on stdin. `DOPPEL_DISABLE` keeps the
/// brief's own session from connecting back to doppel; the parent's
/// `CLAUDE_CODE_SESSION_ID` is dropped because a nested `claude` takes it
/// over as its own id.
async fn execute(
    config: &BriefConfig,
    bundle: &Value,
    cwd: Option<&str>,
) -> Result<String, String> {
    let mut command = tokio::process::Command::new(&config.argv[0]);
    command
        .args(&config.argv[1..])
        .env("DOPPEL_DISABLE", "1")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = cwd.filter(|d| Path::new(d).is_dir()) {
        command.current_dir(dir);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", config.argv[0]))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(bundle.to_string().as_bytes())
            .await
            .map_err(|e| format!("cannot write the bundle: {e}"))?;
    }
    let output = tokio::time::timeout(config.timeout, child.wait_with_output())
        .await
        .map_err(|_| format!("timed out after {}s", config.timeout.as_secs()))?
        .map_err(|e| format!("cannot read the brief: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .chars()
            .rev()
            .take(STDERR_TAIL)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return Err(format!(
            "the brief command exited with {}: {}",
            output.status,
            tail.trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| "the brief is not UTF-8".to_owned())
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

    #[test]
    fn commands_split_on_whitespace() {
        let config =
            BriefConfig::from_command("  claude -p  /doppel:brief ", Duration::from_secs(1))
                .unwrap();
        assert_eq!(config.argv, ["claude", "-p", "/doppel:brief"]);
        assert_eq!(
            BriefConfig::from_command("   ", Duration::from_secs(1)),
            None
        );
    }
}
