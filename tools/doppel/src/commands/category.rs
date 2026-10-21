use std::path::Path;

use crate::commands::convert::category_data;
use crate::common::store::Store;
use crate::output::{AppError, CategoryListData, Payload};

pub(crate) fn run_category_add(
    db_path: &Path,
    name: &str,
    description: &str,
) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    Ok(Payload::CategoryAdd(category_data(
        store.add_category(name, description)?,
    )))
}

pub(crate) fn run_category_list(db_path: &Path) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let categories: Vec<_> = store
        .list_categories()?
        .into_iter()
        .map(category_data)
        .collect();
    Ok(Payload::CategoryList(CategoryListData {
        count: categories.len(),
        categories,
    }))
}

pub(crate) fn run_category_retire(db_path: &Path, name: &str) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    Ok(Payload::CategoryRetire(category_data(
        store.retire_category(name)?,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::TempDir;
    use crate::common::error::ErrorKind;

    #[test]
    fn add_list_retire_round_trip() {
        let dir = TempDir::new("category");
        run_category_add(&dir.db_path(), "priority", "작업 우선순위").unwrap();
        match run_category_list(&dir.db_path()).unwrap() {
            Payload::CategoryList(data) => {
                assert_eq!(data.count, 1);
                assert_eq!(data.categories[0].state, "active");
            }
            other => panic!("expected CategoryList, got {other:?}"),
        }
        match run_category_retire(&dir.db_path(), "priority").unwrap() {
            Payload::CategoryRetire(data) => assert_eq!(data.state, "retired"),
            other => panic!("expected CategoryRetire, got {other:?}"),
        }
        let err = run_category_add(&dir.db_path(), "priority", "again").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
    }
}
