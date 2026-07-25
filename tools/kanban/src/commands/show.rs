use std::path::Path;

use crate::common::store::Store;
use crate::output::{AppError, ItemData, LabelData, Payload};

/// Returns one item in full, including its body and labels. An unknown id is
/// `not_found`.
pub(crate) fn run_show(db_path: &Path, id: &str) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let item = store.get_item(id)?;
    let labels = store.labels_for(&item.id)?;
    Ok(Payload::Show(ItemData {
        id: item.id,
        source: item.source,
        external_id: item.external_id,
        title: item.title,
        body: item.body,
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
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::TempBoard;
    use crate::common::store::NewItem;
    use crate::output::{ErrorKind, exit_code};

    #[test]
    fn show_returns_the_full_item_including_body_and_labels() {
        let board = TempBoard::new("show-success");
        let id = {
            let mut store = Store::open(board.db_path()).unwrap();
            let item = store
                .insert_item(&NewItem {
                    source: "discord",
                    external_id: "msg-1",
                    title: "cache drifts",
                    body: "full body text",
                })
                .unwrap();
            store
                .attach_label(&item.id, "kind", "bug", Some(0.9))
                .unwrap();
            item.id
        };

        let payload = run_show(board.db_path(), &id).unwrap();
        match payload {
            Payload::Show(data) => {
                assert_eq!(data.id, id);
                assert_eq!(data.body, "full body text");
                assert_eq!(data.state, "inbox");
                assert_eq!(data.labels.len(), 1);
                assert_eq!(data.labels[0].key, "kind");
                assert_eq!(data.labels[0].value, "bug");
            }
            other => panic!("expected Payload::Show, got {other:?}"),
        }
    }

    #[test]
    fn show_of_a_missing_id_is_not_found() {
        let board = TempBoard::new("show-missing");
        let err = run_show(board.db_path(), "itm-999999").unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert_eq!(exit_code(&err), 7);
    }
}
