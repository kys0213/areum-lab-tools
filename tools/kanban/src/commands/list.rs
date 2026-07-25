use std::path::Path;

use crate::common::store::{ItemFilter, Store};
use crate::output::{AppError, ItemSummaryData, LabelData, ListData, Payload};

/// Lists board items. Every filter is optional and they compose; `label`
/// matches a label key, not a value.
///
/// Unlike `next`, this only looks — it never claims.
pub(crate) fn run_list(
    db_path: &Path,
    project: Option<&str>,
    state: Option<&str>,
    label: Option<&str>,
) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let filter = ItemFilter {
        project,
        state,
        label,
    };
    let items = store.list_items(&filter)?;
    let mut summaries = Vec::with_capacity(items.len());
    for item in items {
        let labels = store.labels_for(&item.id)?;
        summaries.push(ItemSummaryData {
            id: item.id,
            source: item.source,
            external_id: item.external_id,
            title: item.title,
            state: item.state.as_str().to_owned(),
            project: item.project,
            priority: item.priority.as_str().to_owned(),
            session_id: item.session_id,
            agent: item.agent,
            claimed_at: item.claimed_at,
            created_at: item.created_at,
            updated_at: item.updated_at,
            labels: labels
                .into_iter()
                .map(|l| LabelData {
                    key: l.key,
                    value: l.value,
                    confidence: l.confidence,
                })
                .collect(),
        });
    }
    Ok(Payload::List(ListData {
        count: summaries.len(),
        items: summaries,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::store::NewItem;
    use std::path::PathBuf;

    fn unique_db_path(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "areum-kanban-list-test-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("kanban.db")
    }

    #[test]
    fn list_is_empty_for_a_fresh_board() {
        let path = unique_db_path("empty");
        let payload = run_list(&path, None, None, None).unwrap();
        match payload {
            Payload::List(data) => {
                assert_eq!(data.count, 0);
                assert!(data.items.is_empty());
            }
            other => panic!("expected Payload::List, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn list_filters_by_project_state_and_label_and_reads_labels_back() {
        let path = unique_db_path("filters");
        {
            let mut store = Store::open(&path).unwrap();
            store.add_project("belt", "conveyor").unwrap();
            let assigned = store
                .insert_item(&NewItem {
                    source: "discord",
                    external_id: "msg-1",
                    title: "cache drifts",
                    body: "b",
                })
                .unwrap();
            store.assign(&assigned.id, "belt", Some("P1")).unwrap();
            store
                .attach_label(&assigned.id, "kind", "bug", Some(0.9))
                .unwrap();
            store
                .insert_item(&NewItem {
                    source: "discord",
                    external_id: "msg-2",
                    title: "unrelated, still in inbox",
                    body: "b",
                })
                .unwrap();
        }

        let by_project = run_list(&path, Some("belt"), None, None).unwrap();
        match by_project {
            Payload::List(data) => {
                assert_eq!(data.count, 1);
                assert_eq!(data.items[0].id, "itm-000001");
                assert_eq!(data.items[0].project.as_deref(), Some("belt"));
                assert_eq!(data.items[0].labels.len(), 1);
                assert_eq!(data.items[0].labels[0].key, "kind");
                assert_eq!(data.items[0].labels[0].value, "bug");
            }
            other => panic!("expected Payload::List, got {other:?}"),
        }

        let by_state = run_list(&path, None, Some("inbox"), None).unwrap();
        match by_state {
            Payload::List(data) => assert_eq!(data.count, 1),
            other => panic!("expected Payload::List, got {other:?}"),
        }

        let by_label = run_list(&path, None, None, Some("kind")).unwrap();
        match by_label {
            Payload::List(data) => assert_eq!(data.count, 1),
            other => panic!("expected Payload::List, got {other:?}"),
        }

        let no_match = run_list(&path, Some("belt"), Some("inbox"), None).unwrap();
        match no_match {
            Payload::List(data) => assert_eq!(data.count, 0),
            other => panic!("expected Payload::List, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn label_filter_matches_the_key_not_the_value() {
        let path = unique_db_path("label-key-not-value");
        {
            let mut store = Store::open(&path).unwrap();
            let item = store
                .insert_item(&NewItem {
                    source: "discord",
                    external_id: "msg-1",
                    title: "cache drifts",
                    body: "b",
                })
                .unwrap();
            store
                .attach_label(&item.id, "kind", "bug", Some(0.9))
                .unwrap();
        }

        // "kind" is the label's key, so it matches.
        let by_key = run_list(&path, None, None, Some("kind")).unwrap();
        match by_key {
            Payload::List(data) => assert_eq!(data.count, 1),
            other => panic!("expected Payload::List, got {other:?}"),
        }

        // "bug" is the label's value, not its key, so it must not match —
        // this is what proves the filter checks the key and not the value.
        let by_value = run_list(&path, None, None, Some("bug")).unwrap();
        match by_value {
            Payload::List(data) => assert_eq!(data.count, 0),
            other => panic!("expected Payload::List, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
