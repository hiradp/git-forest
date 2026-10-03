use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::cli::RenameArgs;
use crate::config::{CheckoutId, Config};
use crate::domain::{
    CommandOutcome, CommandReport, RenameStatus, RepositoryRename, WorkspaceRenameReport,
    WorkspaceRenameStatus,
};
use crate::error::{AppError, Result};
use crate::git::{Git, failure_message};
use crate::workspace::{self, WorkspaceState};

#[derive(Debug)]
struct Plan {
    checkout: CheckoutId,
    canonical_path: PathBuf,
    old_path: PathBuf,
    path: PathBuf,
    branch: Option<String>,
    already_repaired: bool,
}

pub fn run(config: &Config, git: &Git, arguments: &RenameArgs) -> Result<CommandOutcome> {
    let _lock = workspace::lock_mutations(config)?;
    let old_path = config.workspace_path(&arguments.workspace)?;
    let path = config.workspace_path(&arguments.new_workspace)?;

    if arguments.workspace == arguments.new_workspace {
        return Ok(conflict(
            arguments,
            old_path,
            path,
            "current and new workspace names are the same".to_owned(),
        ));
    }

    let old_metadata = path_metadata(&old_path)?;
    let new_metadata = path_metadata(&path)?;
    let states = workspace::scan(config, git)?;

    if old_metadata.is_some()
        && let Some(state) = states
            .iter()
            .find(|state| state.name != arguments.workspace && paths_match(&state.path, &old_path))
    {
        return Ok(conflict(
            arguments,
            old_path,
            path,
            format!("workspace name casing does not match; use {:?}", state.name),
        ));
    }

    let old_state = states
        .iter()
        .find(|state| state.name == arguments.workspace);
    let new_state = states
        .iter()
        .find(|state| state.name == arguments.new_workspace);

    let (plans, move_directory) = match (old_metadata, new_metadata) {
        (Some(metadata), None) => {
            if !metadata.is_dir() {
                return Ok(conflict(
                    arguments,
                    old_path.clone(),
                    path,
                    format!(
                        "workspace path {} exists and is not a directory",
                        old_path.display()
                    ),
                ));
            }
            if new_state.is_some_and(|state| !state.members.is_empty()) {
                return Ok(conflict(
                    arguments,
                    old_path,
                    path.clone(),
                    format!(
                        "destination {} has registered Git worktrees",
                        path.display()
                    ),
                ));
            }
            match initial_plans(old_state, &path) {
                Ok(plans) => (plans, true),
                Err(message) => return Ok(conflict(arguments, old_path, path, message)),
            }
        }
        (None, Some(metadata)) if metadata.is_dir() => {
            match resume_plans(git, old_state, new_state, &path)? {
                Ok(plans) => (plans, false),
                Err(message) => return Ok(conflict(arguments, old_path, path, message)),
            }
        }
        (None, Some(_)) => {
            return Ok(conflict(
                arguments,
                old_path,
                path.clone(),
                format!(
                    "destination {} exists and is not a directory",
                    path.display()
                ),
            ));
        }
        (Some(_), Some(_)) => {
            return Ok(conflict(
                arguments,
                old_path,
                path.clone(),
                format!("destination {} already exists", path.display()),
            ));
        }
        (None, None) => {
            return Ok(conflict(
                arguments,
                old_path,
                path,
                format!("workspace {:?} does not exist", arguments.workspace),
            ));
        }
    };

    // Relink children before moving anything so that every interruption leaves
    // a state the same command can resume.
    let children = workspace::children(&states, &arguments.workspace)
        .map(|child| child.path.clone())
        .collect::<Vec<_>>();
    relink(&children, &arguments.workspace, &arguments.new_workspace)?;

    if move_directory && let Err(source) = rename_without_replacing(&old_path, &path) {
        relink(&children, &arguments.new_workspace, &arguments.workspace)?;
        let repositories = plans
            .into_iter()
            .map(|plan| rename_report(plan, RenameStatus::NotRun, None))
            .collect();
        return Ok(CommandOutcome {
            report: CommandReport::WorkspaceRename(WorkspaceRenameReport {
                old_workspace: arguments.workspace.clone(),
                old_path,
                workspace: arguments.new_workspace.clone(),
                path: path.clone(),
                repositories,
                status: WorkspaceRenameStatus::Failed,
                message: Some(format!(
                    "could not rename workspace directory to {}: {source}",
                    path.display()
                )),
            }),
            exit_code: 1,
        });
    }

    let mut failed = false;
    let mut repositories = Vec::with_capacity(plans.len());
    for plan in plans {
        if plan.already_repaired {
            repositories.push(rename_report(plan, RenameStatus::AlreadyRepaired, None));
            continue;
        }

        let output = git.repair_worktree(&plan.canonical_path, &plan.path)?;
        if output.status.success() {
            repositories.push(rename_report(plan, RenameStatus::Repaired, None));
        } else {
            failed = true;
            let message = failure_message(&output);
            repositories.push(rename_report(plan, RenameStatus::Failed, Some(message)));
        }
    }

    if !failed {
        let expected = repositories
            .iter()
            .map(|repository| repository.checkout.as_str())
            .collect::<HashSet<_>>();
        let states = workspace::scan(config, git)?;
        let old_registrations_remain = states
            .iter()
            .find(|state| state.name == arguments.workspace)
            .is_some_and(|state| state.members.iter().any(|member| member.registered));
        let destination_is_consistent = states
            .iter()
            .find(|state| state.name == arguments.new_workspace)
            .is_some_and(|state| {
                state.exists
                    && state.inconsistencies.is_empty()
                    && state.members.len() == expected.len()
                    && state.members.iter().all(|member| {
                        expected.contains(member.id.to_string().as_str())
                            && member.exists
                            && member.registered
                            && member.inconsistencies.is_empty()
                            && member.unexpected_worktree_paths.is_empty()
                    })
            });
        failed = old_registrations_remain || !destination_is_consistent;
    }
    let (status, message) = if failed {
        (
            WorkspaceRenameStatus::Failed,
            Some(format!(
                "workspace directory is at {}, but one or more Git worktrees could not be repaired; repeat `git forest rename {} {}` to resume",
                path.display(),
                arguments.workspace,
                arguments.new_workspace
            )),
        )
    } else {
        (WorkspaceRenameStatus::Renamed, None)
    };

    Ok(CommandOutcome {
        report: CommandReport::WorkspaceRename(WorkspaceRenameReport {
            old_workspace: arguments.workspace.clone(),
            old_path,
            workspace: arguments.new_workspace.clone(),
            path,
            repositories,
            status,
            message,
        }),
        exit_code: u8::from(failed),
    })
}

/// Points every child at `to`, leaving all of them at `from` on failure.
fn relink(children: &[PathBuf], from: &str, to: &str) -> Result<()> {
    for child in children {
        workspace::prepare_metadata(child, None, Some(to))?;
    }
    for (index, child) in children.iter().enumerate() {
        if let Err(error) = workspace::update_metadata(child, None, Some(to)) {
            // The failing write may have committed before its cleanup failed.
            for written in &children[..=index] {
                // Best effort; the original error explains the failure.
                let _ = workspace::update_metadata(written, None, Some(from));
            }
            return Err(error);
        }
    }
    Ok(())
}

fn initial_plans(
    state: Option<&WorkspaceState>,
    new_path: &Path,
) -> std::result::Result<Vec<Plan>, String> {
    let Some(state) = state else {
        return Err(
            "workspace directory exists but was not discovered during reconciliation".to_owned(),
        );
    };
    if !state.exists {
        return Err("workspace directory is missing".to_owned());
    }

    let mut issues = state.inconsistencies.clone();
    for member in &state.members {
        issues.extend(member.inconsistencies.iter().cloned());
        if !member.unexpected_worktree_paths.is_empty() {
            issues.push(format!(
                "checkout {} has an unexpected registered worktree layout",
                member.id
            ));
        }
        if !member.exists || !member.registered || member.metadata.is_none() {
            issues.push(format!(
                "checkout {} is not a present registered worktree",
                member.id
            ));
        }
    }
    issues.sort();
    issues.dedup();
    if !issues.is_empty() {
        return Err(format!("workspace is inconsistent: {}", issues.join("; ")));
    }

    Ok(state
        .members
        .iter()
        .map(|member| {
            let metadata = member
                .metadata
                .as_ref()
                .expect("consistent workspace members have Git metadata");
            Plan {
                checkout: member.id.clone(),
                canonical_path: member.canonical_path.clone(),
                old_path: member.path.clone(),
                path: new_path.join(member.id.to_string()),
                branch: short_branch(metadata.branch.as_deref()),
                already_repaired: false,
            }
        })
        .collect())
}

fn resume_plans(
    git: &Git,
    old_state: Option<&WorkspaceState>,
    new_state: Option<&WorkspaceState>,
    new_path: &Path,
) -> Result<std::result::Result<Vec<Plan>, String>> {
    let Some(old_state) = old_state else {
        return Ok(Err(
            "source workspace does not exist and no incomplete Git worktree registrations were found"
                .to_owned(),
        ));
    };
    if old_state.exists || old_state.members.is_empty() {
        return Ok(Err(
            "source workspace does not identify an incomplete rename".to_owned(),
        ));
    }

    let mut stale = HashMap::new();
    for member in &old_state.members {
        if member.exists
            || !member.registered
            || member.metadata.is_none()
            || !member.unexpected_worktree_paths.is_empty()
        {
            return Ok(Err(format!(
                "source workspace has inconsistent checkout {}",
                member.id
            )));
        }
        stale.insert(member.id.clone(), member);
    }

    let Some(new_state) = new_state else {
        return Ok(Err(
            "destination directory exists but was not discovered during reconciliation".to_owned(),
        ));
    };
    if !new_state.exists || !new_state.inconsistencies.is_empty() {
        return Ok(Err(
            "destination workspace is inconsistent and cannot resume the rename".to_owned(),
        ));
    }

    let mut found = HashSet::new();
    let mut plans = Vec::with_capacity(new_state.members.len());
    for member in &new_state.members {
        if !member.exists || !member.unexpected_worktree_paths.is_empty() {
            return Ok(Err(format!(
                "destination checkout {} is missing or has an unexpected layout",
                member.id
            )));
        }
        if !git.worktree_belongs_to(&member.canonical_path, &member.path)? {
            return Ok(Err(format!(
                "destination {} does not belong to canonical repository {}",
                member.path.display(),
                member.canonical_path.display()
            )));
        }

        let was_stale = stale.contains_key(&member.id);
        if member.registered == was_stale {
            return Ok(Err(format!(
                "Git registration for destination checkout {} does not match an incomplete rename",
                member.id
            )));
        }
        found.insert(member.id.clone());
        plans.push(Plan {
            checkout: member.id.clone(),
            canonical_path: member.canonical_path.clone(),
            old_path: old_state.path.join(member.id.to_string()),
            path: new_path.join(member.id.to_string()),
            branch: short_branch(git.current_branch_ref(&member.path)?.as_deref()),
            already_repaired: member.registered,
        });
    }

    if stale.keys().any(|checkout| !found.contains(checkout)) {
        return Ok(Err(
            "one or more stale source registrations have no matching destination checkout"
                .to_owned(),
        ));
    }

    Ok(Ok(plans))
}

fn rename_report(plan: Plan, status: RenameStatus, message: Option<String>) -> RepositoryRename {
    RepositoryRename {
        name: plan.checkout.repository.clone(),
        checkout: plan.checkout.to_string(),
        slot: plan.checkout.slot,
        old_path: plan.old_path,
        path: plan.path,
        branch: plan.branch,
        status,
        message,
    }
}

fn short_branch(reference: Option<&str>) -> Option<String> {
    reference.map(|reference| {
        reference
            .strip_prefix("refs/heads/")
            .unwrap_or(reference)
            .to_owned()
    })
}

fn conflict(
    arguments: &RenameArgs,
    old_path: PathBuf,
    path: PathBuf,
    message: String,
) -> CommandOutcome {
    CommandOutcome {
        report: CommandReport::WorkspaceRename(WorkspaceRenameReport {
            old_workspace: arguments.workspace.clone(),
            old_path,
            workspace: arguments.new_workspace.clone(),
            path,
            repositories: Vec::new(),
            status: WorkspaceRenameStatus::Conflict,
            message: Some(message),
        }),
        exit_code: 1,
    }
}

fn path_metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(AppError::Filesystem {
            context: format!("could not inspect path {}", path.display()),
            source,
        }),
    }
}

fn paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    left.canonicalize()
        .ok()
        .zip(right.canonicalize().ok())
        .is_some_and(|(left, right)| left == right)
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
fn rename_without_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE).map_err(Into::into)
}

#[cfg(not(any(target_os = "android", target_os = "linux", target_vendor = "apple")))]
fn rename_without_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "cannot atomically rename {} to {} without replacing an existing path on this platform",
            source.display(),
            destination.display()
        ),
    ))
}
