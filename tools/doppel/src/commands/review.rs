use std::path::Path;

use crate::commands::convert::golden_data;
use crate::common::store::Store;
use crate::output::{AppError, Payload, ReviewData};

/// The batch-review queue: every candidate in full, overrides first.
pub(crate) fn run_review(db_path: &Path) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let candidates: Vec<_> = store.review_queue()?.into_iter().map(golden_data).collect();
    Ok(Payload::Review(ReviewData {
        count: candidates.len(),
        candidates,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::{TempDir, sample_decision, seed_store};

    #[test]
    fn review_lists_candidates_overrides_first() {
        let dir = TempDir::new("review");
        let mut store = seed_store(&dir.db_path());
        store.seed_categories().unwrap();
        store
            .insert_decisions(&[
                sample_decision("s:1:0", "캐시", "2026-01-01T00:00:00Z"),
                sample_decision("s:2:0", "네이밍", "2026-01-02T00:00:00Z"),
            ])
            .unwrap();
        for id in ["dec-000001", "dec-000002"] {
            store.categorize(id, "design-scope", "human").unwrap();
        }
        store.propose_golden("dec-000001", "ingest", None).unwrap();
        store
            .propose_golden("dec-000002", "override", None)
            .unwrap();

        match run_review(&dir.db_path()).unwrap() {
            Payload::Review(data) => {
                assert_eq!(data.count, 2);
                assert_eq!(data.candidates[0].decision_id, "dec-000002");
                assert_eq!(data.candidates[0].options.len(), 2);
            }
            other => panic!("expected Review, got {other:?}"),
        }
    }
}
