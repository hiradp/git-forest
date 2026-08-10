use crate::cli::UpdateArgs;
use crate::config::{Config, RepositoryConfig};
use crate::domain::{
    CommandOutcome, CommandReport, RepositoriesUpdateReport, RepositoryUpdateReport, UpdateStatus,
};
use crate::error::Result;
use crate::git::{Git, failure_message};

use super::fetch::{map_repositories, selected_repositories, validate_repository};

pub fn run(config: &Config, git: &Git, arguments: &UpdateArgs) -> Result<CommandOutcome> {
    let selected = selected_repositories(config, &arguments.repositories)?;
    let fetches = map_repositories(&selected, arguments.jobs.get(), |repository| {
        fetch_default_branch(git, repository)
    });
    let mut failed = false;
    let repositories = selected
        .into_iter()
        .zip(fetches)
        .map(|(repository, fetch)| {
            let result = match fetch {
                DefaultFetch::Ready {
                    branch,
                    default_ref,
                } => update_repository(git, repository, branch, default_ref),
                DefaultFetch::Conflict { branch, message } => {
                    UpdateResult::conflict(branch, message)
                }
                DefaultFetch::Failed { branch, message } => UpdateResult::failed(branch, message),
            };
            if matches!(result.status, UpdateStatus::Conflict | UpdateStatus::Failed) {
                failed = true;
            }
            RepositoryUpdateReport {
                name: repository.name.clone(),
                path: repository.path.clone(),
                branch: result.branch,
                status: result.status,
                message: result.message,
            }
        })
        .collect();

    Ok(CommandOutcome {
        report: CommandReport::RepositoriesUpdate(RepositoriesUpdateReport { repositories }),
        exit_code: u8::from(failed),
    })
}

enum DefaultFetch {
    Ready {
        branch: String,
        default_ref: String,
    },
    Conflict {
        branch: Option<String>,
        message: String,
    },
    Failed {
        branch: Option<String>,
        message: String,
    },
}

struct UpdateResult {
    branch: Option<String>,
    status: UpdateStatus,
    message: Option<String>,
}

impl UpdateResult {
    fn updated(branch: String) -> Self {
        Self {
            branch: Some(branch),
            status: UpdateStatus::Updated,
            message: None,
        }
    }

    fn up_to_date(branch: String) -> Self {
        Self {
            branch: Some(branch),
            status: UpdateStatus::UpToDate,
            message: None,
        }
    }

    fn conflict(branch: Option<String>, message: String) -> Self {
        Self {
            branch,
            status: UpdateStatus::Conflict,
            message: Some(message),
        }
    }

    fn failed(branch: Option<String>, message: String) -> Self {
        Self {
            branch,
            status: UpdateStatus::Failed,
            message: Some(message),
        }
    }
}

fn fetch_default_branch(git: &Git, repository: &RepositoryConfig) -> DefaultFetch {
    if let Err(message) = validate_repository(git, repository) {
        return DefaultFetch::Failed {
            branch: None,
            message,
        };
    }

    let default_ref = match git.default_ref(&repository.path) {
        Ok(Some(default_ref)) => default_ref,
        Ok(None) => {
            return DefaultFetch::Conflict {
                branch: None,
                message: "origin/HEAD does not resolve to an available remote-tracking branch"
                    .to_owned(),
            };
        }
        Err(error) => {
            return DefaultFetch::Failed {
                branch: None,
                message: error.to_string(),
            };
        }
    };
    let Some(branch) = default_ref.strip_prefix("refs/remotes/origin/") else {
        return DefaultFetch::Failed {
            branch: None,
            message: format!("unexpected default reference {default_ref}"),
        };
    };
    let branch = branch.to_owned();
    let output = match git.fetch_origin_branch(&repository.path, &branch) {
        Ok(output) => output,
        Err(error) => {
            return DefaultFetch::Failed {
                branch: Some(branch),
                message: error.to_string(),
            };
        }
    };
    if !output.status.success() {
        return DefaultFetch::Failed {
            branch: Some(branch),
            message: failure_message(&output),
        };
    }

    DefaultFetch::Ready {
        branch,
        default_ref,
    }
}

fn update_repository(
    git: &Git,
    repository: &RepositoryConfig,
    branch: String,
    default_ref: String,
) -> UpdateResult {
    match try_update_repository(git, repository, branch, default_ref) {
        Ok(result) => result,
        Err((branch, message)) => UpdateResult::failed(branch, message),
    }
}

fn try_update_repository(
    git: &Git,
    repository: &RepositoryConfig,
    branch: String,
    default_ref: String,
) -> std::result::Result<UpdateResult, (Option<String>, String)> {
    let local_ref = format!("refs/heads/{branch}");
    let remote_name = default_ref
        .strip_prefix("refs/remotes/")
        .unwrap_or(&default_ref);

    let local_exists = git
        .branch_exists(&repository.path, &branch)
        .map_err(|error| (Some(branch.clone()), error.to_string()))?;
    if !local_exists {
        return Ok(UpdateResult::conflict(
            Some(branch.clone()),
            format!("local default branch {branch} does not exist"),
        ));
    }

    let old_oid = git
        .resolve_reference(&repository.path, &local_ref)
        .map_err(|error| (Some(branch.clone()), error.to_string()))?;
    let new_oid = git
        .resolve_reference(&repository.path, &default_ref)
        .map_err(|error| (Some(branch.clone()), error.to_string()))?;
    let (ahead, behind) = git
        .reference_ahead_behind(&repository.path, &old_oid, &new_oid)
        .map_err(|error| (Some(branch.clone()), error.to_string()))?;
    if ahead > 0 {
        let message = if behind > 0 {
            format!(
                "local branch {branch} has {ahead} local commit(s) and {behind} remote commit(s) not shared with {remote_name}"
            )
        } else {
            format!("local branch {branch} is {ahead} commit(s) ahead of {remote_name}")
        };
        return Ok(UpdateResult::conflict(Some(branch), message));
    }
    if behind == 0 {
        return Ok(UpdateResult::up_to_date(branch));
    }

    let worktrees = git
        .worktrees(&repository.path)
        .map_err(|error| (Some(branch.clone()), error.to_string()))?;
    if let Some(worktree) = worktrees
        .iter()
        .find(|worktree| worktree.branch.as_deref() == Some(local_ref.as_str()))
    {
        let dirty = git
            .worktree_dirty(&worktree.path)
            .map_err(|error| (Some(branch.clone()), error.to_string()))?;
        if dirty {
            return Ok(UpdateResult::conflict(
                Some(branch),
                format!(
                    "default branch has uncommitted changes in {}",
                    worktree.path.display()
                ),
            ));
        }
        let ignored_path = git
            .ignored_path_changed_between(&worktree.path, &old_oid, &new_oid)
            .map_err(|error| (Some(branch.clone()), error.to_string()))?;
        if let Some(path) = ignored_path {
            return Ok(UpdateResult::conflict(
                Some(branch),
                format!(
                    "default branch update overlaps ignored path {path:?} in {}",
                    worktree.path.display()
                ),
            ));
        }
        let output = git
            .fast_forward(&worktree.path, &new_oid)
            .map_err(|error| (Some(branch.clone()), error.to_string()))?;
        if !output.status.success() {
            return Err((Some(branch), failure_message(&output)));
        }
    } else {
        let output = git
            .update_ref(&repository.path, &local_ref, &new_oid, &old_oid)
            .map_err(|error| (Some(branch.clone()), error.to_string()))?;
        if !output.status.success() {
            return Err((Some(branch), failure_message(&output)));
        }
    }

    Ok(UpdateResult::updated(branch))
}
