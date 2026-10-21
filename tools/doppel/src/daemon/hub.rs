//! In-process broadcast streams. Handlers publish what changed; waiters,
//! attach connections and UI sockets subscribe and filter for what they care
//! about — the reactive shape the daemon is built around. Nothing here is
//! durable: a subscriber that was not connected re-reads the store.

use serde_json::Value;
use tokio::sync::broadcast;

use crate::daemon::api::UiNotice;

/// Big enough that a burst of events does not lag a UI socket; a lagging
/// subscriber re-reads the store anyway.
const CAPACITY: usize = 256;

#[derive(Clone)]
pub(super) struct Hub {
    /// Ids of asks whose state changed.
    pub(super) asks: broadcast::Sender<String>,
    /// Messages for one session's attach connection: `(session_id, message)`.
    pub(super) sessions: broadcast::Sender<(String, Value)>,
    pub(super) ui: broadcast::Sender<UiNotice>,
}

impl Hub {
    pub(super) fn new() -> Hub {
        Hub {
            asks: broadcast::channel(CAPACITY).0,
            sessions: broadcast::channel(CAPACITY).0,
            ui: broadcast::channel(CAPACITY).0,
        }
    }

    /// `send` fails only when nobody listens, which is not an error here.
    pub(super) fn ask_changed(&self, ask_id: &str) {
        let _ = self.asks.send(ask_id.to_owned());
        let _ = self.ui.send(UiNotice::ASKS);
        let _ = self.ui.send(UiNotice::SESSIONS);
    }

    pub(super) fn sessions_changed(&self) {
        let _ = self.ui.send(UiNotice::SESSIONS);
    }

    pub(super) fn to_session(&self, session_id: &str, message: Value) {
        let _ = self.sessions.send((session_id.to_owned(), message));
    }
}
