//! Board handlers (docs/doppel.md §6): projects and cards. Every change is
//! published as a `cards` notice for the UI.

use std::path::Path as FsPath;

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};

use crate::common::error::{AppError, ErrorKind};
use crate::common::store::NewCard;
use crate::daemon::api::{
    CardList, CardView, NewCardBody, NewProjectBody, ProjectList, ProjectView, ReleaseBody,
    TriageBody,
};
use crate::daemon::launch;
use crate::daemon::routes::{ApiResult, AppState, StateQuery, one_of, usage};

const PRIORITIES: [&str; 4] = ["P0", "P1", "P2", "P3"];

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/projects", get(list_projects).post(add_project))
        .route("/v1/cards", get(list_cards).post(add_card))
        .route("/v1/cards/{id}", get(get_card))
        .route("/v1/cards/{id}/triage", post(triage_card))
        .route("/v1/cards/{id}/dispatch", post(dispatch_card))
        .route("/v1/cards/{id}/release", post(release_card))
        .route("/v1/cards/{id}/done", post(done_card))
}

async fn list_projects(State(app): State<AppState>) -> ApiResult<ProjectList> {
    let projects = app.store().list_projects()?;
    Ok(Json(ProjectList {
        projects: projects.into_iter().map(ProjectView::from).collect(),
    }))
}

async fn add_project(
    State(app): State<AppState>,
    Json(body): Json<NewProjectBody>,
) -> ApiResult<ProjectView> {
    let project = app
        .store()
        .add_project(&body.name, &body.path, &body.description)?;
    app.hub.cards_changed();
    Ok(Json(project.into()))
}

async fn list_cards(
    State(app): State<AppState>,
    Query(query): Query<StateQuery>,
) -> ApiResult<CardList> {
    let cards = app.store().list_cards(query.state.as_deref())?;
    Ok(Json(CardList {
        cards: cards.into_iter().map(CardView::from).collect(),
    }))
}

async fn add_card(
    State(app): State<AppState>,
    Json(body): Json<NewCardBody>,
) -> ApiResult<CardView> {
    if body.title.trim().is_empty() {
        return Err(usage("a card needs a title"));
    }
    let card = app.store().add_card(&NewCard {
        source: &body.source,
        external_id: &body.external_id,
        title: &body.title,
        body: &body.body,
    })?;
    app.hub.cards_changed();
    Ok(Json(card.into()))
}

async fn get_card(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<CardView> {
    Ok(Json(app.store().get_card(&id)?.into()))
}

async fn triage_card(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<TriageBody>,
) -> ApiResult<CardView> {
    let by = one_of(body.by.as_deref(), &["human", "model"], "human")
        .ok_or_else(|| usage("by must be human or model"))?;
    let card = {
        let mut store = app.store();
        let priority = match body.priority {
            Some(p) if PRIORITIES.contains(&p.as_str()) => p,
            Some(p) => return Err(usage(format!("priority {p} is not one of P0..P3"))),
            None => store.get_card(&id)?.priority,
        };
        store.triage_card(&id, &body.project, &priority, &by)?
    };
    app.hub.cards_changed();
    Ok(Json(card.into()))
}

/// Claims the card and launches its session in the background; the answer is
/// the card as claimed, before the session has started.
async fn dispatch_card(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<CardView> {
    let config = app.launch.clone().ok_or_else(|| {
        AppError::new(
            ErrorKind::Config,
            "this daemon launches no sessions (empty --launch-command)",
        )
    })?;
    let (card, path) = {
        let store = app.store();
        let card = store.get_card(&id)?;
        let project = card
            .project
            .as_deref()
            .ok_or_else(|| usage(format!("{id} has no project; triage it first")))?;
        let path = store.get_project(project)?.path;
        if !FsPath::new(&path).is_dir() {
            return Err(usage(format!(
                "project {project} path {path} is not a directory"
            )));
        }
        (store.claim_card(&id, config.max_running)?, path)
    };
    app.hub.cards_changed();
    tokio::spawn(launch::run(app.clone(), config, card.clone(), path));
    Ok(Json(card.into()))
}

/// Back to `triaged`; a running session is stopped.
async fn release_card(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ReleaseBody>,
) -> ApiResult<CardView> {
    let reason = body.reason.unwrap_or_else(|| "released by hand".to_owned());
    let card = app.store().release_card(&id, &reason)?;
    app.runs.stop(&id);
    app.hub.cards_changed();
    Ok(Json(card.into()))
}

async fn done_card(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<CardView> {
    let card = app.store().done_card(&id)?;
    app.runs.stop(&id);
    app.hub.cards_changed();
    Ok(Json(card.into()))
}

#[cfg(test)]
mod tests;
