//! Board handlers (docs/doppel.md §6): projects and cards. Every change is
//! published as a `cards` notice for the UI.

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};

use crate::common::error::AppError;
use crate::common::store::{CardRecord, NewCard, Store};
use crate::daemon::api::{
    CardList, CardView, DispatchBody, NewCardBody, NewProjectBody, ProjectList, ProjectView,
    ReleaseBody, TriageBody,
};
use crate::daemon::launch::{self, Mode};
use crate::daemon::routes::{ApiResult, AppState, StateQuery, one_of, usage};
use crate::daemon::{auto, proposal};

const PRIORITIES: [&str; 4] = ["P0", "P1", "P2", "P3"];

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/projects", get(list_projects).post(add_project))
        .route("/v1/cards", get(list_cards).post(add_card))
        .route("/v1/cards/{id}", get(get_card))
        .route("/v1/cards/{id}/propose", post(propose_card))
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

fn view(store: &Store, card: CardRecord) -> Result<CardView, AppError> {
    let proposal = store.get_proposal(&card.id)?;
    Ok(CardView::with_proposal(card, proposal))
}

/// The card as the API shows it, read fresh so its proposal is current.
fn current(app: &AppState, id: &str) -> ApiResult<CardView> {
    let store = app.store();
    let card = store.get_card(id)?;
    Ok(Json(view(&store, card)?))
}

async fn list_cards(
    State(app): State<AppState>,
    Query(query): Query<StateQuery>,
) -> ApiResult<CardList> {
    let store = app.store();
    let cards = store
        .list_cards(query.state.as_deref())?
        .into_iter()
        .map(|card| view(&store, card))
        .collect::<Result<_, _>>()?;
    Ok(Json(CardList { cards }))
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
    if app.proposal.is_some() {
        // The card is in; a proposal that cannot start shows as missing.
        let _ = proposal::begin(&app, &card.id);
    }
    current(&app, &card.id)
}

async fn get_card(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<CardView> {
    current(&app, &id)
}

/// Asks for a (new) proposal; it is written in the background.
async fn propose_card(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<CardView> {
    proposal::begin(&app, &id)?;
    current(&app, &id)
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
    auto::pump(&app);
    current(&app, &card.id)
}

/// Claims the card and launches its session in the background; the answer is
/// the card as claimed, before the session has started.
async fn dispatch_card(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<DispatchBody>,
) -> ApiResult<CardView> {
    let mode = if body.resume {
        Mode::Resume { note: body.note }
    } else {
        Mode::Fresh
    };
    let card = launch::start(&app, &id, mode)?;
    current(&app, &card.id)
}

/// Back to `triaged`; a running session is stopped.
async fn release_card(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ReleaseBody>,
) -> ApiResult<CardView> {
    let reason = body.reason.unwrap_or_else(|| "released by hand".to_owned());
    app.store().release_card(&id, &reason)?;
    app.runs.stop(&id);
    app.hub.cards_changed();
    current(&app, &id)
}

async fn done_card(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<CardView> {
    app.store().done_card(&id)?;
    app.runs.stop(&id);
    app.hub.cards_changed();
    auto::pump(&app);
    current(&app, &id)
}

#[cfg(test)]
mod tests;
