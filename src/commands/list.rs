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
            workspace_entries: workspace.workspace_entries,
            inconsistencies: workspace.inconsistencies,
        })
        .collect();

    Ok(WorkspacesListReport { workspaces })
}

pub fn archived(config: &Config) -> Result<crate::domain::ArchivesListReport> {
    let archives = crate::archives::scan(config)?
        .into_iter()
        .map(|archive| crate::domain::ArchiveListEntry {
            workspace: archive.workspace,
            archive_id: archive.archive_id,
            archive_path: archive.path,
        })
        .collect();
    Ok(crate::domain::ArchivesListReport { archives })
}

fn short_branch(branch: Option<&str>) -> Option<String> {
    branch.map(|branch| {
        branch
            .strip_prefix("refs/heads/")
            .unwrap_or(branch)
            .to_owned()
    })
}
