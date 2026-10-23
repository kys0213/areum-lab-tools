use std::io::Read;
use std::path::Path;

use serde_json::{Value, json};

use crate::commands::remote::call;
use crate::common::error::{AppError, ErrorKind};
use crate::daemon::api::{CardList, CardView};
use crate::output::{CardListData, Payload};

pub(crate) struct CardInput<'a> {
    pub(crate) source: &'a str,
    pub(crate) external_id: &'a str,
    pub(crate) title: &'a str,
    /// `-` reads the body from stdin.
    pub(crate) body: &'a str,
}

pub(crate) fn run_card_add(socket: &Path, input: &CardInput<'_>) -> Result<Payload, AppError> {
    let body = resolve_body(input.body, std::io::stdin())?;
    let request = json!({
        "source": input.source,
        "external_id": input.external_id,
        "title": input.title,
        "body": body,
    });
    Ok(Payload::CardAdd(call(
        socket,
        "POST",
        "/v1/cards",
        Some(&request),
    )?))
}

fn resolve_body(body: &str, mut stdin: impl Read) -> Result<String, AppError> {
    if body != "-" {
        return Ok(body.to_owned());
    }
    let mut text = String::new();
    stdin.read_to_string(&mut text).map_err(|e| {
        AppError::new(
            ErrorKind::Usage,
            format!("cannot read the body from stdin: {e}"),
        )
    })?;
    Ok(text)
}

pub(crate) fn run_card_list(socket: &Path, state: Option<&str>) -> Result<Payload, AppError> {
    let path = match state {
        Some(state) => format!("/v1/cards?state={state}"),
        None => "/v1/cards".to_owned(),
    };
    let list: CardList = call(socket, "GET", &path, None)?;
    Ok(Payload::CardList(CardListData {
        count: list.cards.len(),
        cards: list.cards,
    }))
}

pub(crate) fn run_card_show(socket: &Path, id: &str) -> Result<Payload, AppError> {
    Ok(Payload::CardShow(call(
        socket,
        "GET",
        &format!("/v1/cards/{id}"),
        None,
    )?))
}

pub(crate) fn run_card_triage(
    socket: &Path,
    id: &str,
    project: &str,
    priority: Option<&str>,
) -> Result<Payload, AppError> {
    let body = json!({ "project": project, "priority": priority, "by": "human" });
    Ok(Payload::CardTriage(transition(
        socket, id, "triage", &body,
    )?))
}

pub(crate) fn run_card_propose(socket: &Path, id: &str) -> Result<Payload, AppError> {
    Ok(Payload::CardPropose(transition(
        socket,
        id,
        "propose",
        &json!({}),
    )?))
}

pub(crate) fn run_card_dispatch(
    socket: &Path,
    id: &str,
    resume: bool,
    note: Option<&str>,
) -> Result<Payload, AppError> {
    Ok(Payload::CardDispatch(transition(
        socket,
        id,
        "dispatch",
        &json!({ "resume": resume, "note": note }),
    )?))
}

pub(crate) fn run_card_release(
    socket: &Path,
    id: &str,
    reason: Option<&str>,
) -> Result<Payload, AppError> {
    Ok(Payload::CardRelease(transition(
        socket,
        id,
        "release",
        &json!({ "reason": reason }),
    )?))
}

pub(crate) fn run_card_done(socket: &Path, id: &str) -> Result<Payload, AppError> {
    Ok(Payload::CardDone(transition(
        socket,
        id,
        "done",
        &json!({}),
    )?))
}

fn transition(socket: &Path, id: &str, action: &str, body: &Value) -> Result<CardView, AppError> {
    call(
        socket,
        "POST",
        &format!("/v1/cards/{id}/{action}"),
        Some(body),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dash_body_comes_from_stdin() {
        assert_eq!(
            resolve_body("-", "긴 본문\n".as_bytes()).unwrap(),
            "긴 본문\n"
        );
        assert_eq!(
            resolve_body("짧은 본문", "ignored".as_bytes()).unwrap(),
            "짧은 본문"
        );
    }
}
