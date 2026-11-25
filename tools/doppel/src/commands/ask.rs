use std::path::Path;

use serde_json::{Map, Value, json};

use crate::commands::runtime::block_on;
use crate::common::client::request;
use crate::common::error::{AppError, ErrorKind};
use crate::daemon::api::{AskList, AskView};
use crate::output::{AskListData, Payload};

/// The daemon holds one wait for at most this long.
const MAX_WAIT_SECS: u64 = 590;

pub(crate) fn run_ask_list(socket: &Path, state: Option<&str>) -> Result<Payload, AppError> {
    let path = match state {
        Some(state) => format!("/v1/asks?state={state}"),
        None => "/v1/asks".to_owned(),
    };
    let list: AskList = decode(block_on(request(socket, "GET", &path, None))?)?;
    Ok(Payload::AskList(AskListData {
        count: list.asks.len(),
        asks: list.asks,
    }))
}

/// Exits 0 whenever the daemon answered, the ask still `open` included —
/// the caller (the plugin hook) loops on that.
pub(crate) fn run_ask_wait(
    socket: &Path,
    id: &str,
    timeout_secs: u64,
) -> Result<Payload, AppError> {
    let timeout_ms = timeout_secs.min(MAX_WAIT_SECS) * 1_000;
    let path = format!("/v1/asks/{id}/wait?timeout_ms={timeout_ms}");
    Ok(Payload::AskWait(decode(block_on(request(
        socket, "GET", &path, None,
    ))?)?))
}

pub(crate) fn run_ask_answer(
    socket: &Path,
    id: &str,
    labels: &[String],
    pairs: &[String],
) -> Result<Payload, AppError> {
    let ask: AskView = decode(block_on(request(
        socket,
        "GET",
        &format!("/v1/asks/{id}"),
        None,
    ))?)?;
    let answers = build_answers(&ask.questions, labels, pairs)?;
    let body = json!({ "answers": answers, "answered_by": "doppel-cli" });
    let path = format!("/v1/asks/{id}/answer");
    Ok(Payload::AskAnswer(decode(block_on(request(
        socket,
        "POST",
        &path,
        Some(&body),
    ))?)?))
}

/// Positional labels answer a single-question ask; `<question>=<label>`
/// pairs answer any. A multi-select question collects every label given for
/// it; a single-select question takes exactly one.
fn build_answers(
    questions: &Value,
    labels: &[String],
    pairs: &[String],
) -> Result<Value, AppError> {
    let questions = questions.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut chosen: Vec<(String, String)> = Vec::new();
    if !labels.is_empty() {
        let [only] = questions else {
            return Err(usage(
                "labels without --answer need an ask with exactly one question",
            ));
        };
        let text = only["question"].as_str().unwrap_or_default().to_owned();
        chosen.extend(labels.iter().map(|l| (text.clone(), l.clone())));
    }
    for pair in pairs {
        let (question, label) = pair
            .split_once('=')
            .ok_or_else(|| usage(format!("--answer {pair} is not <question>=<label>")))?;
        chosen.push((question.to_owned(), label.to_owned()));
    }
    let mut answers = Map::new();
    for question in questions {
        let text = question["question"].as_str().unwrap_or_default();
        let picks: Vec<&String> = chosen
            .iter()
            .filter(|(q, _)| q == text)
            .map(|(_, l)| l)
            .collect();
        let value = match (
            question["multiSelect"].as_bool() == Some(true),
            picks.as_slice(),
        ) {
            (_, []) => return Err(usage(format!("no answer for \"{text}\""))),
            (false, [one]) => Value::String((*one).clone()),
            (false, _) => return Err(usage(format!("\"{text}\" takes one answer"))),
            (true, many) => {
                Value::Array(many.iter().map(|l| Value::String((*l).clone())).collect())
            }
        };
        answers.insert(text.to_owned(), value);
    }
    if let Some((unknown, _)) = chosen.iter().find(|(q, _)| !answers.contains_key(q)) {
        return Err(usage(format!("the ask has no question \"{unknown}\"")));
    }
    Ok(Value::Object(answers))
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, AppError> {
    serde_json::from_value(value).map_err(|e| {
        AppError::new(
            ErrorKind::Internal,
            format!("unexpected daemon answer: {e}"),
        )
    })
}

fn usage(message: impl Into<String>) -> AppError {
    AppError::new(ErrorKind::Usage, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn questions() -> Value {
        json!([
            {"question": "위치는?", "multiSelect": false, "options": []},
            {"question": "범위는?", "multiSelect": true, "options": []}
        ])
    }

    #[test]
    fn pairs_fill_single_and_multi_select_questions() {
        let answers = build_answers(
            &questions(),
            &[],
            &[
                "위치는?=메모리".into(),
                "범위는?=API".into(),
                "범위는?=CLI".into(),
            ],
        )
        .unwrap();
        assert_eq!(
            answers,
            json!({"위치는?": "메모리", "범위는?": ["API", "CLI"]})
        );
    }

    #[test]
    fn labels_answer_a_single_question_ask_only() {
        let one = json!([{"question": "위치는?", "options": []}]);
        assert_eq!(
            build_answers(&one, &["디스크".into()], &[]).unwrap(),
            json!({"위치는?": "디스크"})
        );
        let err = build_answers(&questions(), &["디스크".into()], &[]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage);
    }

    #[test]
    fn missing_extra_or_doubled_answers_are_usage_errors() {
        let missing = build_answers(&questions(), &[], &["위치는?=메모리".into()]);
        assert_eq!(missing.unwrap_err().kind, ErrorKind::Usage);
        let doubled = build_answers(
            &questions(),
            &[],
            &["위치는?=a".into(), "위치는?=b".into(), "범위는?=API".into()],
        );
        assert_eq!(doubled.unwrap_err().kind, ErrorKind::Usage);
        let unknown = build_answers(
            &questions(),
            &[],
            &["위치는?=a".into(), "범위는?=API".into(), "그밖=x".into()],
        );
        assert_eq!(unknown.unwrap_err().kind, ErrorKind::Usage);
    }
}
