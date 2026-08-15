use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::RemoveArgs;
use crate::config::{CheckoutId, Config};
use crate::domain::{
    CommandOutcome, CommandReport, RemovalStatus, RepositoryRemoval, WorkspaceRemovalReport,
};
use crate::error::{AppError, Result};
use crate::git::{Git, failure_message};
use crate::workspace::{self, MemberState};

#[derive(Debug)]
enum Preflight {
    Ready {
        checkout: CheckoutId,
        canonical_path: PathBuf,
        path: PathBuf,
    },
    AlreadyAbsent {
        checkout: CheckoutId,
        path: PathBuf,
    },
    Conflict {
        checkout: CheckoutId,
        path: PathBuf,
        message: String,
    },
}

pub fn run(config: &Config, git: &Git, arguments: &RemoveArgs) -> Result<CommandOutcome> {
    let workspace_path = config.workspace_path(&arguments.workspace)?;
    let requested = validate_requested(config, &arguments.checkouts)?;
    let mut states = workspace::scan(config, git)?;
    let state = states
        .drain(..)
        .find(|workspace| workspace.name == arguments.workspace);

    let selected = if requested.is_empty() {
        state
            .as_ref()
            .map(|workspace| {
                workspace
                    .members
                    .iter()
                    .map(|member| member.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    } else {
        requested
    };

    let mut preflight = Vec::with_capacity(selected.len());
    for checkout in selected {
        let path = workspace_path.join(checkout.to_string());
        let member = state.as_ref().and_then(|workspace| {
            workspace
                .members
                .iter()
                .find(|member| member.id == checkout)
        });
        preflight.push(preflight_member(git, checkout, path, member)?);
    }

    if preflight
        .iter()
        .any(|item| matches!(item, Preflight::Conflict { .. }))
    {
        let repositories = preflight
            .into_iter()
            .map(|item| match item {
                Preflight::Ready { checkout, path, .. } => removal_report(
                    checkout,
                    path,
                    RemovalStatus::NotRun,
                    Some("not run because workspace preflight failed".to_owned()),
                ),
                Preflight::AlreadyAbsent { checkout, path } => {
                    removal_report(checkout, path, RemovalStatus::AlreadyAbsent, None)
                }
                Preflight::Conflict {
                    checkout,
                    path,
                    message,
                } => removal_report(checkout, path, RemovalStatus::Conflict, Some(message)),
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
            Preflight::AlreadyAbsent { checkout, path } => repositories.push(removal_report(
                checkout,
                path,
                RemovalStatus::AlreadyAbsent,
                None,
            )),
            Preflight::Ready { checkout, path, .. } if failed => {
                repositories.push(removal_report(
                    checkout,
                    path,
                    RemovalStatus::NotRun,
                    Some("not run because an earlier checkout failed".to_owned()),
                ));
            }
            Preflight::Ready {
                checkout,
                canonical_path,
                path,
            } => {
                let output = git.remove_worktree(&canonical_path, &path)?;
                if output.status.success() {
                    repositories.push(removal_report(checkout, path, RemovalStatus::Removed, None));
                } else {
                    failed = true;
                    repositories.push(removal_report(
                        checkout,
                        path,
                        RemovalStatus::Failed,
                        Some(failure_message(&output)),
                    ));
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

fn removal_report(
    checkout: CheckoutId,
    path: PathBuf,
    status: RemovalStatus,
    message: Option<String>,
) -> RepositoryRemoval {
    RepositoryRemoval {
        name: checkout.repository.clone(),
        checkout: checkout.to_string(),
        slot: checkout.slot,
        path,
        status,
        message,
    }
}

fn validate_requested(config: &Config, checkouts: &[CheckoutId]) -> Result<Vec<CheckoutId>> {
    let mut seen = HashSet::new();
    let mut destinations = HashMap::new();
    for checkout in checkouts {
        if config.repository(&checkout.repository).is_none() {
            return Err(AppError::InvalidInput(format!(
                "unknown repository {:?}",
                checkout.repository
            )));
        }
        if !seen.insert(checkout) {
            return Err(AppError::InvalidInput(format!(
                "checkout {:?} was requested more than once",
                checkout.to_string()
            )));
        }
        let path_key = checkout.to_string().to_ascii_lowercase();
        if let Some(existing) = destinations.insert(path_key, checkout.to_string()) {
            return Err(AppError::InvalidInput(format!(
                "checkouts {existing:?} and {:?} may resolve to the same destination on a case-insensitive filesystem",
                checkout.to_string()
            )));
        }
    }
    Ok(checkouts.to_vec())
}

fn preflight_member(
    git: &Git,
    checkout: CheckoutId,
    path: PathBuf,
    member: Option<&MemberState>,
) -> Result<Preflight> {
    let Some(member) = member else {
        return Ok(Preflight::AlreadyAbsent { checkout, path });
    };
    if let Some(actual_path) = member.unexpected_worktree_paths.first() {
        let actual_paths = member
            .unexpected_worktree_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(Preflight::Conflict {
            checkout,
            path: actual_path.clone(),
            message: format!(
                "registered worktree layout does not match: found {actual_paths}; expected {}",
                path.display()
            ),
        });
    }
    if !member.exists && member.registered {
        return Ok(Preflight::Ready {
            checkout,
            canonical_path: member.canonical_path.clone(),
            path,
        });
    }
    if member.exists && !member.registered {
        return Ok(Preflight::Conflict {
            checkout,
            path,
            message: "path exists but is not registered with the configured canonical repository"
                .to_owned(),
        });
    }
    if !member.exists {
        return Ok(Preflight::AlreadyAbsent { checkout, path });
    }

    match git.is_clean_for_removal(&member.path)? {
        Ok(false) => Ok(Preflight::Conflict {
            checkout,
            path,
            message: "worktree has modified, untracked, or ignored files".to_owned(),
        }),
        Ok(true) => Ok(Preflight::Ready {
            checkout,
            canonical_path: member.canonical_path.clone(),
            path,
        }),
        Err(message) => Ok(Preflight::Conflict {
            checkout,
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
