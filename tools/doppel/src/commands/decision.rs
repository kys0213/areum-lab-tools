use std::path::Path;

use crate::commands::convert::decision_data;
use crate::common::store::{DecisionFilter, Store};
use crate::output::{AppError, CategorizeData, DecisionListData, DecisionSummaryData, Payload};

pub(crate) fn run_decision_list(
    db_path: &Path,
    filter: &DecisionFilter<'_>,
) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let decisions: Vec<DecisionSummaryData> = store
        .list_decisions(filter)?
        .into_iter()
        .map(|d| DecisionSummaryData {
            id: d.id,
            kind: d.kind,
            source: d.source,
            project: d.project,
            question: d.question,
            answers: d.answers,
            category: d.category,
            category_by: d.category_by,
            decided_at: d.decided_at,
        })
        .collect();
    Ok(Payload::DecisionList(DecisionListData {
        count: decisions.len(),
        decisions,
    }))
}

pub(crate) fn run_decision_show(db_path: &Path, id: &str) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    Ok(Payload::DecisionShow(decision_data(
        store.get_decision(id)?,
    )))
}

pub(crate) fn run_decision_categorize(
    db_path: &Path,
    id: &str,
    category: &str,
    by: &str,
) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let change = store.categorize(id, category, by)?;
    Ok(Payload::DecisionCategorize(CategorizeData {
        id: change.after.id,
        category: category.to_owned(),
        category_by: by.to_owned(),
        previous_category: change.previous_category,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::{TempDir, sample_decision, seed_store};
    use crate::common::error::ErrorKind;

    fn ledger(label: &str) -> TempDir {
        let dir = TempDir::new(label);
        let mut store = seed_store(&dir.db_path());
        store.seed_categories().unwrap();
        store
            .insert_decisions(&[
                sample_decision("s:1:0", "캐시", "2026-01-01T00:00:00Z"),
                sample_decision("s:2:0", "네이밍", "2026-02-01T00:00:00Z"),
            ])
            .unwrap();
        dir
    }

    #[test]
    fn list_answers_summaries_newest_first() {
        let dir = ledger("decision-list");
        let filter = DecisionFilter {
            limit: 10,
            ..DecisionFilter::default()
        };
        match run_decision_list(&dir.db_path(), &filter).unwrap() {
            Payload::DecisionList(data) => {
                assert_eq!(data.count, 2);
                assert_eq!(data.decisions[0].id, "dec-000002");
            }
            other => panic!("expected DecisionList, got {other:?}"),
        }
    }

    #[test]
    fn show_answers_the_full_entry_and_unknown_is_not_found() {
        let dir = ledger("decision-show");
        match run_decision_show(&dir.db_path(), "dec-000001").unwrap() {
            Payload::DecisionShow(data) => {
                assert_eq!(data.options.len(), 2);
                assert_eq!(data.context.as_deref(), Some("캐시 배경"));
            }
            other => panic!("expected DecisionShow, got {other:?}"),
        }
        let err = run_decision_show(&dir.db_path(), "dec-000404").unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
    }

    #[test]
    fn categorize_reports_the_previous_filing() {
        let dir = ledger("decision-categorize");
        run_decision_categorize(&dir.db_path(), "dec-000001", "design-scope", "model").unwrap();
        match run_decision_categorize(&dir.db_path(), "dec-000001", "naming-api-test", "human")
            .unwrap()
        {
            Payload::DecisionCategorize(data) => {
                assert_eq!(data.previous_category.as_deref(), Some("design-scope"));
                assert_eq!(data.category, "naming-api-test");
                assert_eq!(data.category_by, "human");
            }
            other => panic!("expected DecisionCategorize, got {other:?}"),
        }
    }
}
