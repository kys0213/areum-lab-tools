//! Single owner of the fixtures the command tests and the store tests share:
//! the throwaway board directory and the deterministic clock
//! (`module-structure.md` principle 6).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::common::store::{Clock, Store};

/// Serial number that keeps two boards created in the same process apart even
/// when their tests pass the same label.
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(0);

/// A throwaway board directory that removes itself when it goes out of scope.
///
/// Cleanup lives in `Drop` rather than at the end of each test on purpose: a
/// failing assertion unwinds past any trailing `remove_dir_all` call, so the
/// runs worth re-running would be exactly the ones leaving directories behind.
pub(crate) struct TempBoard {
    dir: PathBuf,
    db_path: PathBuf,
}

impl TempBoard {
    pub(crate) fn new(label: &str) -> TempBoard {
        let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "areum-kanban-test-{}-{serial}-{label}",
            std::process::id()
        ));
        // A recycled pid can point at a directory some earlier run left behind.
        let _ = std::fs::remove_dir_all(&dir);
        let db_path = dir.join("kanban.db");
        TempBoard { dir, db_path }
    }

    /// The board file inside this directory. Nothing has created it yet, so
    /// opening it exercises the store's own "create the parent directory"
    /// path.
    pub(crate) fn db_path(&self) -> &Path {
        &self.db_path
    }
}

impl Drop for TempBoard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A board opened on [`SeqClock`], for arranging rows a command test needs but
/// the command surface cannot produce on its own — an exact `created_at`
/// order, a claimed item, a label.
pub(crate) fn seed_store(db_path: &Path) -> Store {
    Store::open_with_clock(db_path, Box::new(SeqClock::default())).expect("seed board opens")
}

/// Strictly increasing timestamps, one per call, so `created_at` ordering is
/// pinned by insertion order instead of by how fast the test ran. The real
/// clock has one-second resolution, which would leave the claim's
/// `ORDER BY created_at` tie-break untested.
#[derive(Default)]
pub(crate) struct SeqClock {
    ticks: AtomicU64,
}

impl Clock for SeqClock {
    fn now(&self) -> String {
        let n = self.ticks.fetch_add(1, Ordering::SeqCst);
        format!(
            "2024-01-01T{:02}:{:02}:{:02}Z",
            n / 3_600,
            (n / 60) % 60,
            n % 60
        )
    }
}
