use std::path::Path;

use crate::common::store::Store;
use crate::output::{AppError, Payload, ProjectData, ProjectListData, ProjectRmData};

/// Registers a project. `description` is the classification rationale, so it
/// is echoed straight back rather than only stored.
pub(crate) fn run_project_add(
    db_path: &Path,
    name: &str,
    description: &str,
) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let project = store.add_project(name, description)?;
    Ok(Payload::ProjectAdd(ProjectData {
        name: project.name,
        description: project.description,
        created_at: project.created_at,
    }))
}

/// Lists registered projects with their descriptions.
pub(crate) fn run_project_list(db_path: &Path) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let projects = store.list_projects()?;
    Ok(Payload::ProjectList(ProjectListData {
        count: projects.len(),
        projects: projects
            .into_iter()
            .map(|p| ProjectData {
                name: p.name,
                description: p.description,
                created_at: p.created_at,
            })
            .collect(),
    }))
}

/// Removes a project. A project still referenced by items is rejected with
/// `conflict` — the foreign key decides, not a pre-check.
pub(crate) fn run_project_rm(db_path: &Path, name: &str) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    store.remove_project(name)?;
    Ok(Payload::ProjectRm(ProjectRmData {
        name: name.to_owned(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::TempBoard;
    use crate::common::store::NewItem;
    use crate::output::{ErrorKind, exit_code};

    #[test]
    fn project_add_registers_and_echoes_the_description() {
        let board = TempBoard::new("project-add");
        let payload = run_project_add(board.db_path(), "belt", "conveyor").unwrap();
        match payload {
            Payload::ProjectAdd(data) => {
                assert_eq!(data.name, "belt");
                assert_eq!(data.description, "conveyor");
            }
            other => panic!("expected Payload::ProjectAdd, got {other:?}"),
        }
    }

    #[test]
    fn adding_a_duplicate_project_name_is_a_conflict() {
        let board = TempBoard::new("project-dup");
        run_project_add(board.db_path(), "belt", "conveyor").unwrap();
        let err = run_project_add(board.db_path(), "belt", "again").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(exit_code(&err), 8);
    }

    #[test]
    fn project_list_reports_every_registered_project_by_name() {
        let board = TempBoard::new("project-list");
        run_project_add(board.db_path(), "belt", "conveyor").unwrap();
        run_project_add(board.db_path(), "areum", "tools").unwrap();
        let payload = run_project_list(board.db_path()).unwrap();
        match payload {
            Payload::ProjectList(data) => {
                assert_eq!(data.count, 2);
                let names: Vec<&str> = data.projects.iter().map(|p| p.name.as_str()).collect();
                assert_eq!(names, vec!["areum", "belt"]);
            }
            other => panic!("expected Payload::ProjectList, got {other:?}"),
        }
    }

    #[test]
    fn project_list_is_empty_for_a_fresh_board() {
        let board = TempBoard::new("project-list-empty");
        let payload = run_project_list(board.db_path()).unwrap();
        match payload {
            Payload::ProjectList(data) => {
                assert_eq!(data.count, 0);
                assert!(data.projects.is_empty());
            }
            other => panic!("expected Payload::ProjectList, got {other:?}"),
        }
    }

    #[test]
    fn project_rm_succeeds_when_unreferenced() {
        let board = TempBoard::new("project-rm-ok");
        run_project_add(board.db_path(), "belt", "conveyor").unwrap();
        let payload = run_project_rm(board.db_path(), "belt").unwrap();
        match payload {
            Payload::ProjectRm(data) => assert_eq!(data.name, "belt"),
            other => panic!("expected Payload::ProjectRm, got {other:?}"),
        }
    }

    #[test]
    fn project_rm_of_an_unknown_project_is_not_found() {
        let board = TempBoard::new("project-rm-missing");
        let err = run_project_rm(board.db_path(), "ghost").unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert_eq!(exit_code(&err), 7);
    }

    #[test]
    fn project_rm_is_refused_while_an_item_references_it() {
        let board = TempBoard::new("project-rm-referenced");
        {
            let mut store = Store::open(board.db_path()).unwrap();
            store.add_project("belt", "conveyor").unwrap();
            let item = store
                .insert_item(&NewItem {
                    source: "discord",
                    external_id: "msg-1",
                    title: "t",
                    body: "b",
                })
                .unwrap();
            store.assign(&item.id, "belt", None).unwrap();
        }
        let err = run_project_rm(board.db_path(), "belt").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(exit_code(&err), 8);
    }
}
