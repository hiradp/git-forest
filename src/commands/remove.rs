use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::RemoveArgs;
use crate::config::Config;
use crate::domain::{
    CommandOutcome, CommandReport, RemovalStatus, RepositoryRemoval, WorkspaceRemovalReport,
};
use crate::error::{AppError, Result};
use crate::git::{Git, failure_message};
use crate::workspace::{self, MemberState};

#[derive(Debug)]
enum Preflight {
    Ready {
        name: String,
        canonical_path: PathBuf,
        path: PathBuf,
    },
    AlreadyAbsent {
        name: String,
        path: PathBuf,
    },
    Conflict {
        name: String,
        path: PathBuf,
        message: String,
    },
}

pub fn run(config: &Config, git: &Git, arguments: &RemoveArgs) -> Result<CommandOutcome> {
    let workspace_path = config.workspace_path(&arguments.workspace)?;
    let requested = validate_requested(config, &arguments.repositories)?;
    let mut states = workspace::scan(config, git)?;
    let state = states
        .drain(..)
        .find(|workspace| workspace.name == arguments.workspace);

    let selected = if requested.is_empty() {
        config
            .repositories
            .iter()
            .filter(|repository| {
                state.as_ref().is_some_and(|workspace| {
                    workspace
                        .members
                        .iter()
                        .any(|member| member.name == repository.name)
                })
            })
            .map(|repository| repository.name.clone())
            .collect::<Vec<_>>()
    } else {
        requested
    };

    let mut preflight = Vec::with_capacity(selected.len());
    for name in selected {
        let path = workspace_path.join(&name);
        let member = state
            .as_ref()
            .and_then(|workspace| workspace.members.iter().find(|member| member.name == name));
        preflight.push(preflight_member(git, name, path, member)?);
    }

    if preflight
        .iter()
        .any(|item| matches!(item, Preflight::Conflict { .. }))
    {
        let repositories = preflight
            .into_iter()
            .map(|item| match item {
                Preflight::Ready { name, path, .. } => RepositoryRemoval {
                    name,
                    path,
                    status: RemovalStatus::NotRun,
                    message: Some("not run because workspace preflight failed".to_owned()),
                },
                Preflight::AlreadyAbsent { name, path } => RepositoryRemoval {
                    name,
                    path,
                    status: RemovalStatus::AlreadyAbsent,
                    message: None,
                },
                Preflight::Conflict {
                    name,
                    path,
                    message,
                } => RepositoryRemoval {
                    name,
                    path,
                    status: RemovalStatus::Conflict,
                    message: Some(message),
                },
            })
            .collect();
        return Ok(CommandOutcome {
            report: CommandReport::WorkspaceRemoval(WorkspaceRemovalReport {
                workspace: arguments.workspace.clone(),
                path: workspace_path.clone(),
                repositories,
                workspace_removed: false,
                remaining_entries: remaining_entries(&workspace_path)?,
            }),
            exit_code: 1,
        });
    }

    let mut failed = false;
    let mut repositories = Vec::with_capacity(preflight.len());
    for item in preflight {
        match item {
            Preflight::AlreadyAbsent { name, path } => repositories.push(RepositoryRemoval {
                name,
                path,
                status: RemovalStatus::AlreadyAbsent,
                message: None,
            }),
            Preflight::Ready {
                name,
                canonical_path: _,
                path,
            } if failed => repositories.push(RepositoryRemoval {
                name,
                path,
                status: RemovalStatus::NotRun,
                message: Some("not run because an earlier repository failed".to_owned()),
            }),
            Preflight::Ready {
                name,
                canonical_path,
                path,
            } => {
                let output = git.remove_worktree(&canonical_path, &path)?;
                if output.status.success() {
                    repositories.push(RepositoryRemoval {
                        name,
                        path,
                        status: RemovalStatus::Removed,
                        message: None,
                    });
                } else {
                    failed = true;
                    repositories.push(RepositoryRemoval {
                        name,
                        path,
                        status: RemovalStatus::Failed,
                        message: Some(failure_message(&output)),
                    });
                }
            }
            Preflight::Conflict { .. } => unreachable!("conflicts were handled above"),
        }
    }

    let mut remaining_entries = remaining_entries(&workspace_path)?;
    let workspace_removed = if workspace_path.is_dir() && remaining_entries.is_empty() {
        fs::remove_dir(&workspace_path).map_err(|source| AppError::Filesystem {
            context: format!(
                "could not remove empty workspace directory {}",
                workspace_path.display()
            ),
            source,
        })?;
        true
    } else {
        false
    };
    if workspace_removed {
        remaining_entries.clear();
    }

    Ok(CommandOutcome {
        report: CommandReport::WorkspaceRemoval(WorkspaceRemovalReport {
            workspace: arguments.workspace.clone(),
            path: workspace_path,
            repositories,
            workspace_removed,
            remaining_entries,
        }),
        exit_code: u8::from(failed),
    })
}

fn validate_requested(config: &Config, repositories: &[String]) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    for name in repositories {
        if config.repository(name).is_none() {
            return Err(AppError::InvalidInput(format!(
                "unknown repository {name:?}"
            )));
        }
        if !seen.insert(name.as_str()) {
            return Err(AppError::InvalidInput(format!(
                "repository {name:?} was requested more than once"
            )));
        }
    }
    Ok(repositories.to_vec())
}

fn preflight_member(
    git: &Git,
    name: String,
    path: PathBuf,
    member: Option<&MemberState>,
) -> Result<Preflight> {
    let Some(member) = member else {
        return Ok(Preflight::AlreadyAbsent { name, path });
    };
    if let Some(actual_path) = member.unexpected_worktree_paths.first() {
        let actual_paths = member
            .unexpected_worktree_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(Preflight::Conflict {
            name,
            path: actual_path.clone(),
            message: format!(
                "registered worktree layout does not match: found {actual_paths}; expected {}",
                path.display()
            ),
        });
    }
    if !member.exists && member.registered {
        return Ok(Preflight::Conflict {
            name,
            path,
            message: "registered worktree is missing from disk; repair it with Git before removal"
                .to_owned(),
        });
    }
    if member.exists && !member.registered {
        return Ok(Preflight::Conflict {
            name,
            path,
            message: "path exists but is not registered with the configured canonical repository"
                .to_owned(),
        });
    }
    if !member.exists {
        return Ok(Preflight::AlreadyAbsent { name, path });
    }

    match git.is_clean_for_removal(&member.path)? {
        Ok(false) => Ok(Preflight::Conflict {
            name,
            path,
            message: "worktree has modified, untracked, or ignored files".to_owned(),
        }),
        Ok(true) => Ok(Preflight::Ready {
            name,
            canonical_path: member.canonical_path.clone(),
            path,
        }),
        Err(message) => Ok(Preflight::Conflict {
            name,
            path,
            message: format!("could not inspect worktree before removal: {message}"),
        }),
    }
}

fn remaining_entries(workspace_path: &Path) -> Result<Vec<PathBuf>> {
    if !workspace_path.is_dir() {
        return Ok(workspace_path
            .exists()
            .then(|| workspace_path.to_path_buf())
            .into_iter()
            .collect());
    }

    let entries = fs::read_dir(workspace_path).map_err(|source| AppError::Filesystem {
        context: format!("could not read workspace {}", workspace_path.display()),
        source,
    })?;
    let mut paths = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|source| AppError::Filesystem {
                    context: format!("could not read an entry in {}", workspace_path.display()),
                    source,
                })
        })
        .collect::<Result<Vec<_>>>()?;
    paths.sort();
    Ok(paths)
}
