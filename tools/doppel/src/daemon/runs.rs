//! Card runs in flight, so a person can stop one (`release`, `done`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

/// Card id → (run number, the run's stop signal).
type Stops = HashMap<String, (u64, oneshot::Sender<()>)>;

/// Card runs in flight. Each run is numbered so a run that ends after its
/// card was released and dispatched again cannot unregister the new run.
#[derive(Clone, Default)]
pub(crate) struct Runs {
    next: Arc<AtomicU64>,
    stops: Arc<Mutex<Stops>>,
}

impl Runs {
    pub(super) fn register(&self, card_id: &str) -> (u64, oneshot::Receiver<()>) {
        let run = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.lock().insert(card_id.to_owned(), (run, tx));
        (run, rx)
    }

    pub(super) fn unregister(&self, card_id: &str, run: u64) {
        let mut stops = self.lock();
        if stops.get(card_id).is_some_and(|(r, _)| *r == run) {
            stops.remove(card_id);
        }
    }

    /// Stops the card's run, if one is in flight: its session is killed and
    /// the run leaves the card as the caller put it.
    pub(super) fn stop(&self, card_id: &str) {
        if let Some((_, tx)) = self.lock().remove(card_id) {
            let _ = tx.send(());
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Stops> {
        // Every critical section is one map operation; nothing is half done.
        self.stops
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
