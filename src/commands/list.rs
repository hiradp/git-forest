use crate::config::Config;
use crate::domain::{WorkspaceListEntry, WorkspaceListRepository, WorkspacesListReport};
use crate::error::Result;
use crate::git::Git;
use crate::workspace;

pub fn run(config: &Config, git: &Git) -> Result<WorkspacesListReport> {
    let workspaces = workspace::scan(config, git)?
        .into_iter()
        .map(|workspace| WorkspaceListEntry {
            name: workspace.name,
            path: workspace.path,
            exists: workspace.exists,
            repositories: workspace
                .members
                .into_iter()
                .map(|member| WorkspaceListRepository {
                    name: member.id.repository.clone(),
                    checkout: member.id.to_string(),
                    slot: member.id.slot,
                    path: member.path,
                    exists: member.exists,
                    registered: member.registered,
                    branch: member
                        .metadata
                        .as_ref()
                        .and_then(|metadata| short_branch(metadata.branch.as_deref())),
                    head: member
                        .metadata
                        .as_ref()
                        .map(|metadata| metadata.head.clone()),
                    inconsistencies: member.inconsistencies,
                })
                .collect(),
            unexpected_entries: workspace.unexpected_entries,
            inconsistencies: workspace.inconsistencies,
        })
        .collect();

    Ok(WorkspacesListReport { workspaces })
}

fn short_branch(branch: Option<&str>) -> Option<String> {
    branch.map(|branch| {
        branch
            .strip_prefix("refs/heads/")
            .unwrap_or(branch)
            .to_owned()
    })
}
