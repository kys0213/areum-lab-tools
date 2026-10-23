//! Claude Code session transcripts (`*.jsonl`) → ledger entries
//! (docs/doppel-judgment.md §1).
//!
//! A transcript is one JSON record per line. Two record shapes carry a
//! decision the user made:
//!
//! - an `AskUserQuestion` answer: the assistant record's `tool_use.input`
//!   holds the questions and options, and the user record that answers the
//!   same `tool_use_id` holds `toolUseResult.answers` (question text → a
//!   string, or a list of strings for multi-select);
//! - a permission decision: a user record with `permissionDecision` whose
//!   `source` starts with `user`. Decisions sourced from `config` were made by
//!   rules or the classifier, not by the user, and are not read.
//!
//! Subagent records (`isSidechain: true`) are not read.

use std::collections::HashMap;

use serde_json::Value;

use crate::common::store::{ChoiceOption, NewDecision};
use crate::common::time::canonical_from_rfc3339;
use crate::ingest::redact::redact;
use crate::ingest::tally::SkipTally;
use crate::ingest::text::{head_chars, tail_chars};

/// How much of the assistant text before a question is kept as its context.
const CONTEXT_CHARS: usize = 2_000;
/// How much of a tool input is kept as a permission question.
const INPUT_SUMMARY_CHARS: usize = 500;

const ASK_TOOL: &str = "AskUserQuestion";

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TranscriptScan {
    pub(crate) decisions: Vec<NewDecision>,
    pub(crate) skipped: SkipTally,
}

/// A `tool_use` seen in an assistant record, waiting for its result.
struct PendingTool {
    name: String,
    input: Value,
    /// The assistant text that preceded the call.
    context: Option<String>,
}

/// Fields every decision needs from the record that carries it.
struct RecordMeta<'a> {
    session_id: &'a str,
    decided_at: String,
    project: Option<String>,
}

pub(crate) fn parse_transcript(text: &str) -> TranscriptScan {
    let mut scan = TranscriptScan::default();
    let mut pending: HashMap<String, PendingTool> = HashMap::new();
    let mut last_text: Option<String> = None;

    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            scan.skipped.add("invalid_json");
            continue;
        };
        if record["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        match record["type"].as_str() {
            Some("assistant") => remember_tool_uses(&record, &mut pending, &mut last_text),
            Some("user") => read_user_record(&record, &pending, &mut scan),
            _ => {}
        }
    }
    scan
}

fn content_blocks(record: &Value) -> &[Value] {
    record["message"]["content"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn remember_tool_uses(
    record: &Value,
    pending: &mut HashMap<String, PendingTool>,
    last_text: &mut Option<String>,
) {
    for block in content_blocks(record) {
        match block["type"].as_str() {
            Some("text") => {
                if let Some(text) = block["text"].as_str().filter(|t| !t.trim().is_empty()) {
                    *last_text = Some(text.to_owned());
                }
            }
            Some("tool_use") => {
                if let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) {
                    pending.insert(
                        id.to_owned(),
                        PendingTool {
                            name: name.to_owned(),
                            input: block["input"].clone(),
                            context: last_text.clone(),
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

fn read_user_record(
    record: &Value,
    pending: &HashMap<String, PendingTool>,
    scan: &mut TranscriptScan,
) {
    let results: Vec<&str> = content_blocks(record)
        .iter()
        .filter(|b| b["type"].as_str() == Some("tool_result"))
        .filter_map(|b| b["tool_use_id"].as_str())
        .collect();
    let answers_ask = results
        .iter()
        .find(|id| pending.get(**id).is_some_and(|t| t.name == ASK_TOOL));
    let user_permission = record["permissionDecision"]["source"]
        .as_str()
        .is_some_and(|s| s.starts_with("user"));
    if answers_ask.is_none() && !user_permission {
        return;
    }

    let Some(meta) = record_meta(record, &mut scan.skipped) else {
        return;
    };
    if let Some(tool_use_id) = answers_ask {
        read_ask(record, tool_use_id, &pending[*tool_use_id], &meta, scan);
    } else {
        read_permission(record, &results, pending, &meta, scan);
    }
}

fn record_meta<'a>(record: &'a Value, skipped: &mut SkipTally) -> Option<RecordMeta<'a>> {
    let Some(session_id) = record["sessionId"].as_str() else {
        skipped.add("missing_session_id");
        return None;
    };
    let Some(decided_at) = record["timestamp"]
        .as_str()
        .and_then(canonical_from_rfc3339)
    else {
        skipped.add("bad_timestamp");
        return None;
    };
    Some(RecordMeta {
        session_id,
        decided_at,
        project: record["cwd"].as_str().map(str::to_owned),
    })
}

/// One entry per question of the call.
fn read_ask(
    record: &Value,
    tool_use_id: &str,
    tool: &PendingTool,
    meta: &RecordMeta<'_>,
    scan: &mut TranscriptScan,
) {
    let Some(answers) = record["toolUseResult"]["answers"].as_object() else {
        // Declined or interrupted: the call has a result but no answer.
        scan.skipped.add("ask_without_answers");
        return;
    };
    let Some(questions) = tool.input["questions"].as_array() else {
        scan.skipped.add("malformed_ask");
        return;
    };
    let context = tool
        .context
        .as_deref()
        .map(|c| redact(&tail_chars(c, CONTEXT_CHARS)));

    for (index, question) in questions.iter().enumerate() {
        let Some(text) = question["question"].as_str() else {
            scan.skipped.add("malformed_ask");
            continue;
        };
        let Some(chosen) = answers.get(text).and_then(answer_list) else {
            scan.skipped.add("ask_unanswered");
            continue;
        };
        scan.decisions.push(NewDecision {
            source: "session",
            external_id: format!("{}:{tool_use_id}:{index}", meta.session_id),
            kind: "ask",
            project: meta.project.clone(),
            session_id: Some(meta.session_id.to_owned()),
            question: redact(text),
            options: options_of(question),
            answers: chosen.iter().map(|a| redact(a)).collect(),
            rationale: None,
            context: context.clone(),
            decided_at: meta.decided_at.clone(),
        });
    }
}

/// A single answer is a string, a multi-select answer a list of strings.
fn answer_list(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::String(s) => Some(vec![s.clone()]),
        Value::Array(items) => items
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => None,
    }
}

fn options_of(question: &Value) -> Vec<ChoiceOption> {
    question["options"]
        .as_array()
        .map(|options| {
            options
                .iter()
                .filter_map(|o| {
                    Some(ChoiceOption {
                        label: redact(o["label"].as_str()?),
                        description: redact(o["description"].as_str().unwrap_or("")),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn read_permission(
    record: &Value,
    results: &[&str],
    pending: &HashMap<String, PendingTool>,
    meta: &RecordMeta<'_>,
    scan: &mut TranscriptScan,
) {
    let Some(tool) = results.iter().find_map(|id| pending.get(*id)) else {
        scan.skipped.add("permission_unmatched");
        return;
    };
    let (Some(uuid), Some(decision)) = (
        record["uuid"].as_str(),
        record["permissionDecision"]["decision"].as_str(),
    ) else {
        scan.skipped.add("malformed_permission");
        return;
    };
    scan.decisions.push(NewDecision {
        source: "session",
        external_id: format!("{}:{uuid}", meta.session_id),
        kind: "permission",
        project: meta.project.clone(),
        session_id: Some(meta.session_id.to_owned()),
        question: redact(&format!(
            "{} 실행 허용: {}",
            tool.name,
            input_summary(&tool.name, &tool.input)
        )),
        options: Vec::new(),
        answers: vec![decision.to_owned()],
        rationale: None,
        context: tool
            .context
            .as_deref()
            .map(|c| redact(&tail_chars(c, CONTEXT_CHARS))),
        decided_at: meta.decided_at.clone(),
    });
}

/// What the user was asked to allow: the command for Bash, the path for file
/// tools, compact JSON otherwise.
fn input_summary(tool: &str, input: &Value) -> String {
    let field = match tool {
        "Bash" => input["command"].as_str(),
        "Edit" | "Write" | "Read" | "MultiEdit" => input["file_path"].as_str(),
        "NotebookEdit" => input["notebook_path"].as_str(),
        _ => None,
    };
    let summary = field.map_or_else(|| input.to_string(), str::to_owned);
    head_chars(&summary, INPUT_SUMMARY_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(value: Value) -> String {
        value.to_string()
    }

    fn ask_call(id: &str) -> String {
        line(json!({
            "type": "assistant", "isSidechain": false,
            "message": {"content": [
                {"type": "text", "text": "배경 설명: 캐시를 어디에 둘지 정해야 합니다."},
                {"type": "tool_use", "id": id, "name": "AskUserQuestion", "input": {"questions": [
                    {"question": "캐시 위치는?", "header": "캐시", "multiSelect": false, "options": [
                        {"label": "메모리 (Recommended)", "description": "빠름"},
                        {"label": "디스크", "description": "영속"}
                    ]},
                    {"question": "적용 범위는?", "header": "범위", "multiSelect": true, "options": [
                        {"label": "API", "description": ""},
                        {"label": "CLI", "description": ""}
                    ]}
                ]}}
            ]}
        }))
    }

    fn ask_answer(id: &str, answers: Value) -> String {
        line(json!({
            "type": "user", "isSidechain": false, "sessionId": "sess-1", "uuid": "u-1",
            "cwd": "/work/repo", "timestamp": "2026-10-21T14:47:37.935Z",
            "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": "The user answered"}]},
            "toolUseResult": {"questions": [], "answers": answers}
        }))
    }

    #[test]
    fn each_answered_question_becomes_one_entry() {
        let text = [
            ask_call("toolu_1"),
            ask_answer(
                "toolu_1",
                json!({"캐시 위치는?": "메모리 (Recommended)", "적용 범위는?": ["API", "직접 입력"]}),
            ),
        ]
        .join("\n");
        let scan = parse_transcript(&text);
        assert_eq!(scan.skipped, SkipTally::default());
        assert_eq!(scan.decisions.len(), 2);

        let first = &scan.decisions[0];
        assert_eq!(first.external_id, "sess-1:toolu_1:0");
        assert_eq!(first.kind, "ask");
        assert_eq!(first.question, "캐시 위치는?");
        assert_eq!(first.answers, ["메모리 (Recommended)"]);
        assert_eq!(first.options[1].label, "디스크");
        assert_eq!(first.project.as_deref(), Some("/work/repo"));
        assert_eq!(first.decided_at, "2026-10-21T14:47:37Z");
        assert_eq!(
            first.context.as_deref(),
            Some("배경 설명: 캐시를 어디에 둘지 정해야 합니다.")
        );

        let second = &scan.decisions[1];
        assert_eq!(second.external_id, "sess-1:toolu_1:1");
        assert_eq!(second.answers, ["API", "직접 입력"]);
    }

    #[test]
    fn a_question_without_an_answer_is_counted() {
        let text = [
            ask_call("toolu_1"),
            ask_answer("toolu_1", json!({"캐시 위치는?": "디스크"})),
        ]
        .join("\n");
        let scan = parse_transcript(&text);
        assert_eq!(scan.decisions.len(), 1);
        assert_eq!(scan.skipped.get("ask_unanswered"), 1);
    }

    #[test]
    fn a_declined_question_is_counted_not_recorded() {
        let declined = line(json!({
            "type": "user", "sessionId": "sess-1", "timestamp": "2026-10-21T14:47:37Z",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_1", "is_error": true}]},
            "toolUseResult": "User rejected"
        }));
        let scan = parse_transcript(&[ask_call("toolu_1"), declined].join("\n"));
        assert!(scan.decisions.is_empty());
        assert_eq!(scan.skipped.get("ask_without_answers"), 1);
    }

    #[test]
    fn user_permission_decisions_are_read_and_config_ones_are_not() {
        let call = line(json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "text", "text": "배포 스크립트를 돌립니다."},
                {"type": "tool_use", "id": "toolu_b", "name": "Bash", "input": {"command": "make deploy"}},
                {"type": "tool_use", "id": "toolu_c", "name": "Bash", "input": {"command": "ls"}}
            ]}
        }));
        let by_user = line(json!({
            "type": "user", "sessionId": "sess-1", "uuid": "u-9", "cwd": "/work/repo",
            "timestamp": "2026-10-21T15:00:00.000Z",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_b"}]},
            "permissionDecision": {"decision": "accept", "source": "user_temporary"}
        }));
        let by_config = line(json!({
            "type": "user", "sessionId": "sess-1", "uuid": "u-10",
            "timestamp": "2026-10-21T15:00:01.000Z",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_c"}]},
            "permissionDecision": {"decision": "accept", "source": "config", "reasonType": "rule"}
        }));
        let scan = parse_transcript(&[call, by_user, by_config].join("\n"));
        assert_eq!(scan.decisions.len(), 1);
        let decision = &scan.decisions[0];
        assert_eq!(decision.kind, "permission");
        assert_eq!(decision.external_id, "sess-1:u-9");
        assert_eq!(decision.question, "Bash 실행 허용: make deploy");
        assert_eq!(decision.answers, ["accept"]);
        assert_eq!(
            decision.context.as_deref(),
            Some("배포 스크립트를 돌립니다.")
        );
    }

    #[test]
    fn permission_without_a_known_tool_call_is_counted() {
        let orphan = line(json!({
            "type": "user", "sessionId": "sess-1", "uuid": "u-1",
            "timestamp": "2026-10-21T15:00:00Z",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_x"}]},
            "permissionDecision": {"decision": "accept", "source": "user_permanent"}
        }));
        let scan = parse_transcript(&orphan);
        assert!(scan.decisions.is_empty());
        assert_eq!(scan.skipped.get("permission_unmatched"), 1);
    }

    #[test]
    fn sidechain_records_and_unrelated_lines_are_ignored() {
        let sidechain_answer = line(json!({
            "type": "user", "isSidechain": true, "sessionId": "sess-1",
            "timestamp": "2026-10-21T15:00:00Z",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_1"}]},
            "toolUseResult": {"answers": {"캐시 위치는?": "디스크"}}
        }));
        let summary = line(json!({"type": "summary", "summary": "x"}));
        let scan = parse_transcript(&[ask_call("toolu_1"), sidechain_answer, summary].join("\n"));
        assert!(scan.decisions.is_empty());
        assert_eq!(scan.skipped, SkipTally::default());
    }

    #[test]
    fn broken_lines_and_bad_metadata_are_counted() {
        let no_session = line(json!({
            "type": "user", "timestamp": "2026-10-21T15:00:00Z",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_1"}]},
            "toolUseResult": {"answers": {"캐시 위치는?": "디스크"}}
        }));
        let text = ["{not json".to_owned(), ask_call("toolu_1"), no_session].join("\n");
        let scan = parse_transcript(&text);
        assert!(scan.decisions.is_empty());
        assert_eq!(scan.skipped.get("invalid_json"), 1);
        assert_eq!(scan.skipped.get("missing_session_id"), 1);
    }

    #[test]
    fn secrets_in_answers_and_context_are_masked() {
        let call = line(json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "text", "text": "토큰 ghp_0123456789abcdef 로 접근합니다."},
                {"type": "tool_use", "id": "toolu_1", "name": "AskUserQuestion", "input": {"questions": [
                    {"question": "이 토큰을 쓸까요?", "options": []}
                ]}}
            ]}
        }));
        let answer = ask_answer(
            "toolu_1",
            json!({"이 토큰을 쓸까요?": "sk-ant-api03-abcdefgh 로 바꿔"}),
        );
        let scan = parse_transcript(&[call, answer].join("\n"));
        let decision = &scan.decisions[0];
        assert_eq!(decision.answers, ["[REDACTED] 로 바꿔"]);
        assert_eq!(
            decision.context.as_deref(),
            Some("토큰 [REDACTED] 로 접근합니다.")
        );
    }

    #[test]
    fn non_bash_inputs_summarize_as_paths_or_json() {
        assert_eq!(
            input_summary("Edit", &json!({"file_path": "/a/b.rs", "old_string": "x"})),
            "/a/b.rs"
        );
        assert_eq!(
            input_summary("WebFetch", &json!({"url": "https://x"})),
            r#"{"url":"https://x"}"#
        );
    }
}
