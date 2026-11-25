//! Single owner of the fixtures the command tests and the store tests share:
//! throwaway directories, the deterministic clock and a sample ledger entry
//! (`module-structure.md` principle 6).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::common::store::{ChoiceOption, Clock, NewDecision, Store};

/// Keeps two directories created in the same process apart even when their
/// tests pass the same label.
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(0);

/// A throwaway directory that removes itself when it goes out of scope.
/// Cleanup lives in `Drop` so a failing assertion does not leave it behind.
pub(crate) struct TempDir {
    dir: PathBuf,
}

impl TempDir {
    pub(crate) fn new(label: &str) -> TempDir {
        let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "areum-doppel-test-{}-{serial}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        TempDir { dir }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.dir
    }

    /// The database file inside this directory. Nothing has created it yet,
    /// so opening it exercises the store's own "create the parent" path.
    pub(crate) fn db_path(&self) -> PathBuf {
        self.dir.join("doppel.db")
    }

    /// Writes `contents` at `relative`, creating parent directories.
    pub(crate) fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.dir.join(relative);
        std::fs::create_dir_all(path.parent().expect("relative path has a parent"))
            .expect("fixture directory is creatable");
        std::fs::write(&path, contents).expect("fixture file is writable");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Strictly increasing timestamps, one per call, so every "ordered by time"
/// tie-break is pinned by call order instead of by how fast the test ran.
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

pub(crate) fn memory_store() -> Store {
    Store::open_in_memory(Box::new(SeqClock::default())).expect("in-memory store opens")
}

/// A store at `db_path` on [`SeqClock`], for arranging rows a command test
/// needs before running the command against the same file.
pub(crate) fn seed_store(db_path: &Path) -> Store {
    Store::open_with_clock(db_path, Box::new(SeqClock::default())).expect("seed store opens")
}

/// An `ask` decision whose text fields carry `topic`, so search tests can
/// tell entries apart.
pub(crate) fn sample_decision(external_id: &str, topic: &str, decided_at: &str) -> NewDecision {
    NewDecision {
        source: "session",
        external_id: external_id.to_owned(),
        kind: "ask",
        project: Some("/work/areum-lab-tools".to_owned()),
        session_id: Some("sess-1".to_owned()),
        question: format!("{topic} 어떻게 할까요?"),
        options: vec![
            ChoiceOption {
                label: "A".to_owned(),
                description: format!("{topic} 첫 안"),
            },
            ChoiceOption {
                label: "B".to_owned(),
                description: format!("{topic} 둘째 안"),
            },
        ],
        answers: vec!["A".to_owned()],
        rationale: None,
        context: Some(format!("{topic} 배경")),
        decided_at: decided_at.to_owned(),
    }
}
