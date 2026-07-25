use std::path::Path;

use crate::common::store::{NewItem, Store};
use crate::output::{AddData, AppError, ErrorKind, Payload};

/// Records an inbound issue in `inbox`. `body` of `"-"` means "read the body
/// from stdin", which is why the reader is injected rather than called
/// directly — the '-' rule stays testable without a real stdin.
///
/// A repeat of the same `(source, external_id)` pair is a `conflict`: the
/// UNIQUE constraint decides, so the same Discord message pushed twice never
/// becomes two items.
pub(crate) fn run_add(
    db_path: &Path,
    source: &str,
    external_id: &str,
    title: &str,
    body: &str,
    read_stdin: impl Fn() -> std::io::Result<String>,
) -> Result<Payload, AppError> {
    let body = resolve_body(body, read_stdin)?;
    let mut store = Store::open(db_path)?;
    let item = store.insert_item(&NewItem {
        source,
        external_id,
        title,
        body: &body,
    })?;
    Ok(Payload::Add(AddData {
        id: item.id,
        source: item.source,
        external_id: item.external_id,
        title: item.title,
        state: item.state.as_str().to_owned(),
        created_at: item.created_at,
    }))
}

/// `--body -` reads the whole body from stdin; any other value is used as-is.
fn resolve_body(
    body: &str,
    read_stdin: impl Fn() -> std::io::Result<String>,
) -> Result<String, AppError> {
    if body == "-" {
        read_stdin()
            .map_err(|e| AppError::new(ErrorKind::Internal, format!("failed to read stdin: {e}")))
    } else {
        Ok(body.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn unique_db_path(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "areum-kanban-add-test-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("kanban.db")
    }

    #[test]
    fn add_lands_the_item_in_inbox_with_the_minted_id() {
        let path = unique_db_path("success");
        let payload = run_add(&path, "discord", "msg-1", "cache drifts", "details", || {
            Ok(String::new())
        })
        .unwrap();
        match payload {
            Payload::Add(data) => {
                assert_eq!(data.id, "itm-000001");
                assert_eq!(data.source, "discord");
                assert_eq!(data.external_id, "msg-1");
                assert_eq!(data.title, "cache drifts");
                assert_eq!(data.state, "inbox");
            }
            other => panic!("expected Payload::Add, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn dash_body_reads_the_body_from_stdin() {
        let path = unique_db_path("stdin-body");
        let payload = run_add(&path, "cli", "local-1", "t", "-", || {
            Ok("piped body".to_owned())
        })
        .unwrap();
        let id = match payload {
            Payload::Add(data) => data.id,
            other => panic!("expected Payload::Add, got {other:?}"),
        };
        let stored = Store::open(&path).unwrap().get_item(&id).unwrap();
        assert_eq!(stored.body, "piped body");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_repeated_source_and_external_id_pair_is_a_conflict() {
        let path = unique_db_path("dup");
        run_add(&path, "discord", "msg-1", "t", "b", || Ok(String::new())).unwrap();
        let err = run_add(&path, "discord", "msg-1", "different title", "b", || {
            Ok(String::new())
        })
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
