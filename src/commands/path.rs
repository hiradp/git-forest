use crate::config::Config;
use crate::domain::WorkspacePathReport;
use crate::error::{AppError, Result};

pub fn run(config: &Config, workspace: &str) -> Result<WorkspacePathReport> {
    let path = config.workspace_path(workspace)?;
    if !path.is_dir() {
        return Err(AppError::Operational(format!(
            "workspace {workspace:?} does not exist"
        )));
    }

    Ok(WorkspacePathReport {
        workspace: workspace.to_owned(),
        path,
    })
}
