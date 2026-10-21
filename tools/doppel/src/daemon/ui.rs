//! The web UI: one self-contained page (`ui.html`) that lists sessions and
//! the question inbox, re-fetching when `/v1/ui` says something changed.

use axum::response::Html;

pub(super) async fn page() -> Html<&'static str> {
    Html(include_str!("ui.html"))
}
