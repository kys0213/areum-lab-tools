use std::path::Path;

use crate::commands::convert::golden_data;
use crate::common::error::ErrorKind;
use crate::common::store::{SearchFilter, Store, fts_query};
use crate::output::{AppError, Payload, SearchData, SearchHitData};

/// The precedent bundle for `query` (docs/doppel-judgment.md §4). A query
/// with no word in it is a usage error rather than "no precedents": an empty
/// bundle would read as "this is new" to the brief writer.
pub(crate) fn run_search(
    db_path: &Path,
    query: &str,
    filter: &SearchFilter<'_>,
) -> Result<Payload, AppError> {
    let expression = fts_query(query)
        .ok_or_else(|| AppError::new(ErrorKind::Usage, "search query has no words to match"))?;
    let store = Store::open(db_path)?;
    let hits: Vec<_> = store
        .search_golden(&expression, filter)?
        .into_iter()
        .map(|hit| SearchHitData {
            score: hit.score,
            golden: golden_data(hit.golden),
        })
        .collect();
    Ok(Payload::Search(SearchData {
        query: query.to_owned(),
        count: hits.len(),
        hits,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::{TempDir, sample_decision, seed_store};

    #[test]
    fn search_answers_confirmed_precedents_with_scores() {
        let dir = TempDir::new("search");
        let mut store = seed_store(&dir.db_path());
        store.seed_categories().unwrap();
        store
            .insert_decisions(&[sample_decision(
                "s:1:0",
                "테스트 전략",
                "2026-01-01T00:00:00Z",
            )])
            .unwrap();
        store
            .categorize("dec-000001", "naming-api-test", "human")
            .unwrap();
        store.propose_golden("dec-000001", "manual", None).unwrap();
        store.confirm_golden("gld-000001", None).unwrap();

        let filter = SearchFilter {
            limit: 5,
            ..SearchFilter::default()
        };
        match run_search(&dir.db_path(), "테스트는", &filter).unwrap() {
            Payload::Search(data) => {
                assert_eq!(data.count, 1);
                assert_eq!(data.hits[0].golden.id, "gld-000001");
                assert!(data.hits[0].score > 0.0);
            }
            other => panic!("expected Search, got {other:?}"),
        }
    }

    #[test]
    fn a_query_without_words_is_a_usage_error() {
        let dir = TempDir::new("search-empty");
        let err = run_search(&dir.db_path(), " ?! ", &SearchFilter::default()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage);
    }
}
