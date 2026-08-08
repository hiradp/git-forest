use crate::cli::StatusArgs;
use crate::config::Config;
use crate::domain::{RepositoryStatus, WorkspaceStatusEntry, WorkspacesStatusReport};
use crate::error::{AppError, Result};
use crate::git::Git;
use crate::workspace::{self, MemberState};

pub fn run(config: &Config, git: &Git, arguments: &StatusArgs) -> Result<WorkspacesStatusReport> {
    if let Some(name) = &arguments.workspace {
        config.workspace_path(name)?;
    }

    let mut states = workspace::scan(config, git)?;
    if let Some(name) = &arguments.workspace {
        states.retain(|workspace| &workspace.name == name);
        if states.is_empty() {
            return Err(AppError::Operational(format!(
                "workspace {name:?} does not exist"
            )));
        }
    }

    let mut workspaces = Vec::with_capacity(states.len());
    for state in states {
        let mut repositories = Vec::with_capacity(state.members.len());
        for member in state.members {
            repositories.push(repository_status(git, member)?);
        }
        workspaces.push(WorkspaceStatusEntry {
            name: state.name,
            path: state.path,
            exists: state.exists,
            repositories,
            unexpected_entries: state.unexpected_entries,
            inconsistencies: state.inconsistencies,
        });
    }

    Ok(WorkspacesStatusReport { workspaces })
}

fn repository_status(git: &Git, member: MemberState) -> Result<RepositoryStatus> {
    let mut inconsistencies = member.inconsistencies;
    let mut branch = None;
    let mut detached = None;
    let mut head = None;
    let mut dirty = None;
    let mut upstream = None;
    let mut ahead = None;
    let mut behind = None;

    if member.exists {
        match git.inspect_worktree(&member.path)? {
            Ok(inspection) => {
                detached = Some(inspection.branch.is_none());
                branch = inspection.branch;
                head = Some(inspection.head);
                dirty = Some(inspection.dirty);
                upstream = inspection.upstream;
                ahead = inspection.ahead;
                behind = inspection.behind;
            }
            Err(message) => inconsistencies.push(format!(
                "could not inspect worktree {}: {message}",
                member.path.display()
            )),
        }
    } else if let Some(metadata) = &member.metadata {
        branch = metadata.branch.as_deref().map(short_branch);
        detached = Some(metadata.detached || metadata.branch.is_none());
        head = Some(metadata.head.clone());
    }

    Ok(RepositoryStatus {
        name: member.name,
        path: member.path,
        exists: member.exists,
        registered: member.registered,
        branch,
        detached,
        head,
        dirty,
        upstream,
        ahead,
        behind,
        inconsistencies,
    })
}

fn short_branch(branch: &str) -> String {
    branch
        .strip_prefix("refs/heads/")
        .unwrap_or(branch)
        .to_owned()
}
