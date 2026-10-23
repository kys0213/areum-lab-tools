use std::path::Path;

use serde_json::json;

use crate::commands::remote::call;
use crate::common::error::{AppError, ErrorKind};
use crate::daemon::api::ProjectList;
use crate::output::{Payload, ProjectListData};

/// The path is stored absolute: the daemon launches sessions in it from its
/// own working directory.
pub(crate) fn run_project_add(
    socket: &Path,
    name: &str,
    path: &str,
    description: &str,
) -> Result<Payload, AppError> {
    let absolute = std::fs::canonicalize(path).map_err(|e| {
        AppError::new(
            ErrorKind::Usage,
            format!("project path {path} is not usable: {e}"),
        )
    })?;
    let body = json!({
        "name": name,
        "path": absolute.display().to_string(),
        "description": description,
    });
    Ok(Payload::ProjectAdd(call(
        socket,
        "POST",
        "/v1/projects",
        Some(&body),
    )?))
}

pub(crate) fn run_project_list(socket: &Path) -> Result<Payload, AppError> {
    let list: ProjectList = call(socket, "GET", "/v1/projects", None)?;
    Ok(Payload::ProjectList(ProjectListData {
        count: list.projects.len(),
        projects: list.projects,
    }))
}
