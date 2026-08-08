use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::CreateArgs;
use crate::config::{Config, RepositoryConfig};
use crate::domain::{
    ChangeAction, ChangeStatus, CommandOutcome, CommandReport, RepositoryChangeReport,
    WorkspaceChangeReport,
};
use crate::error::{AppError, Result};
use crate::git::{Git, failure_message};

#[derive(Debug)]
struct Plan {
    name: String,
    canonical_path: PathBuf,
    destination: PathBuf,
    branch: String,
    base_ref: Option<String>,
    action: PlannedAction,
}

#[derive(Debug, Clone, Copy)]
enum PlannedAction {
    Reuse,
    AddExistingBranch,
    CreateBranch,
    CreateTrackingBranch,
}

impl PlannedAction {
    fn report_action(self) -> ChangeAction {
        match self {
            Self::Reuse => ChangeAction::Reuse,
            Self::AddExistingBranch => ChangeAction::AddExistingBranch,
            Self::CreateBranch | Self::CreateTrackingBranch => ChangeAction::CreateBranch,
        }
    }
}

#[derive(Debug)]
struct Overrides {
    bases: HashMap<String, String>,
    branches: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct Identity {
    name: String,
    destination: PathBuf,
    branch: String,
}

#[derive(Debug)]
enum Preflight {
    Ready(Plan),
    Conflict(Identity, String),
}

pub fn run(
    config: &Config,
    git: &Git,
    arguments: &CreateArgs,
    require_existing_workspace: bool,
) -> Result<CommandOutcome> {
    let workspace_path = config.workspace_path(&arguments.workspace)?;
    let rendered_branch = config.branch_for(&arguments.workspace)?;
    let overrides = validate_arguments(config, arguments)?;

    if workspace_path.exists() && !workspace_path.is_dir() {
        return Err(AppError::Operational(format!(
            "workspace path {} exists and is not a directory",
            workspace_path.display()
        )));
    }
    if require_existing_workspace && !workspace_path.is_dir() {
        return Err(AppError::Operational(format!(
            "workspace {:?} does not exist; use `git forest create` first",
            arguments.workspace
        )));
    }

    let mut preflight = Vec::with_capacity(arguments.repositories.len());
    for name in &arguments.repositories {
        let repository = config
            .repository(name)
            .expect("requested repositories were validated");
        let destination = workspace_path.join(name);
        let branch_override = overrides.branches.get(name);
        let identity = Identity {
            name: name.clone(),
            destination,
            branch: branch_override.unwrap_or(&rendered_branch).clone(),
        };
        let result = preflight_repository(
            git,
            repository,
            identity,
            overrides.bases.get(name).map(String::as_str),
            branch_override.is_some(),
        )?;
        preflight.push(result);
    }

    if preflight
        .iter()
        .any(|item| matches!(item, Preflight::Conflict(_, _)))
    {
        let repositories = preflight
            .into_iter()
            .map(|item| match item {
                Preflight::Ready(plan) => RepositoryChangeReport {
                    name: plan.name,
                    path: plan.destination,
                    branch: plan.branch,
                    base_ref: plan.base_ref,
                    action: Some(plan.action.report_action()),
                    status: ChangeStatus::NotRun,
                    message: Some("not run because workspace preflight failed".to_owned()),
                },
                Preflight::Conflict(identity, message) => RepositoryChangeReport {
                    name: identity.name,
                    path: identity.destination,
                    branch: identity.branch,
                    base_ref: None,
                    action: None,
                    status: ChangeStatus::Conflict,
                    message: Some(message),
                },
            })
            .collect();
        return Ok(CommandOutcome {
            report: CommandReport::WorkspaceChange(WorkspaceChangeReport {
                workspace: arguments.workspace.clone(),
                path: workspace_path,
                repositories,
            }),
            exit_code: 1,
        });
    }

    let plans = preflight
        .into_iter()
        .map(|item| match item {
            Preflight::Ready(plan) => plan,
            Preflight::Conflict(_, _) => unreachable!("conflicts were handled above"),
        })
        .collect::<Vec<_>>();
    if plans
        .iter()
        .any(|plan| !matches!(plan.action, PlannedAction::Reuse))
    {
        fs::create_dir_all(&workspace_path).map_err(|source| AppError::Filesystem {
            context: format!(
                "could not create workspace directory {}",
                workspace_path.display()
            ),
            source,
        })?;
    }

    let mut failed = false;
    let mut repositories = Vec::with_capacity(plans.len());
    for plan in plans {
        let action = plan.action.report_action();
        if failed {
            repositories.push(RepositoryChangeReport {
                name: plan.name,
                path: plan.destination,
                branch: plan.branch,
                base_ref: plan.base_ref,
                action: Some(action),
                status: ChangeStatus::NotRun,
                message: Some("not run because an earlier repository failed".to_owned()),
            });
            continue;
        }

        let result = match plan.action {
            PlannedAction::Reuse => None,
            PlannedAction::AddExistingBranch => Some(git.add_existing_branch(
                &plan.canonical_path,
                &plan.destination,
                &plan.branch,
            )?),
            PlannedAction::CreateBranch => Some(
                git.add_new_branch(
                    &plan.canonical_path,
                    &plan.destination,
                    &plan.branch,
                    plan.base_ref
                        .as_deref()
                        .expect("new branches always have a base ref"),
                )?,
            ),
            PlannedAction::CreateTrackingBranch => Some(
                git.add_tracking_branch(
                    &plan.canonical_path,
                    &plan.destination,
                    &plan.branch,
                    plan.base_ref
                        .as_deref()
                        .expect("tracking branches always have a remote ref"),
                )?,
            ),
        };

        let (status, message) = match result {
            None => (ChangeStatus::Reused, None),
            Some(output) if output.status.success() => (ChangeStatus::Created, None),
            Some(output) => {
                failed = true;
                (ChangeStatus::Failed, Some(failure_message(&output)))
            }
        };
        repositories.push(RepositoryChangeReport {
            name: plan.name,
            path: plan.destination,
            branch: plan.branch,
            base_ref: plan.base_ref,
            action: Some(action),
            status,
            message,
        });
    }

    Ok(CommandOutcome {
        report: CommandReport::WorkspaceChange(WorkspaceChangeReport {
            workspace: arguments.workspace.clone(),
            path: workspace_path,
            repositories,
        }),
        exit_code: u8::from(failed),
    })
}

fn validate_arguments(config: &Config, arguments: &CreateArgs) -> Result<Overrides> {
    let mut requested = HashSet::new();
    for name in &arguments.repositories {
        if !requested.insert(name.as_str()) {
            return Err(AppError::InvalidInput(format!(
                "repository {name:?} was requested more than once"
            )));
        }
        if config.repository(name).is_none() {
            return Err(AppError::InvalidInput(format!(
                "unknown repository {name:?}"
            )));
        }
    }

    let mut branches = HashMap::new();
    for branch in &arguments.branches {
        if !requested.contains(branch.repository.as_str()) {
            return Err(AppError::InvalidInput(format!(
                "branch override provided for unrequested repository {:?}",
                branch.repository
            )));
        }
        if branches
            .insert(branch.repository.clone(), branch.branch.clone())
            .is_some()
        {
            return Err(AppError::InvalidInput(format!(
                "multiple branch overrides provided for repository {:?}",
                branch.repository
            )));
        }
    }

    let mut bases = HashMap::new();
    for base in &arguments.bases {
        if !requested.contains(base.repository.as_str()) {
            return Err(AppError::InvalidInput(format!(
                "base override provided for unrequested repository {:?}",
                base.repository
            )));
        }
        if branches.contains_key(&base.repository) {
            return Err(AppError::InvalidInput(format!(
                "branch and base overrides cannot both be provided for repository {:?}",
                base.repository
            )));
        }
        if bases
            .insert(base.repository.clone(), base.reference.clone())
            .is_some()
        {
            return Err(AppError::InvalidInput(format!(
                "multiple base overrides provided for repository {:?}",
                base.repository
            )));
        }
    }
    Ok(Overrides { bases, branches })
}

fn preflight_repository(
    git: &Git,
    repository: &RepositoryConfig,
    identity: Identity,
    base_override: Option<&str>,
    explicit_branch: bool,
) -> Result<Preflight> {
    let conflict = |message: String| Preflight::Conflict(identity.clone(), message);

    if !repository.path.exists() {
        return Ok(conflict(format!(
            "canonical repository {} does not exist",
            repository.path.display()
        )));
    }

    let inspection = git.inspect_repository(&repository.path)?;
    if !inspection.is_git_worktree {
        return Ok(conflict(format!(
            "canonical repository {} is not a Git worktree",
            repository.path.display()
        )));
    }

    if !git.check_branch_name(&repository.path, &identity.branch)? {
        return Ok(conflict(format!(
            "branch name {:?} is not valid according to Git",
            identity.branch
        )));
    }

    let branch_ref = format!("refs/heads/{}", identity.branch);
    let worktrees = git.worktrees(&repository.path)?;
    let destination_worktree = worktrees
        .iter()
        .find(|worktree| paths_match(&worktree.path, &identity.destination));

    if identity.destination.exists() {
        let Some(worktree) = destination_worktree else {
            return Ok(conflict(format!(
                "destination {} exists but is not registered with canonical repository {}",
                identity.destination.display(),
                repository.path.display()
            )));
        };
        if worktree.branch.as_deref() != Some(branch_ref.as_str()) {
            return Ok(conflict(format!(
                "destination {} is registered on {}; expected branch {}",
                identity.destination.display(),
                worktree.branch.as_deref().unwrap_or(if worktree.detached {
                    "detached HEAD"
                } else {
                    "an unknown ref"
                }),
                identity.branch
            )));
        }
        if !git.worktree_belongs_to(&repository.path, &identity.destination)? {
            return Ok(conflict(format!(
                "destination {} is registered but does not belong to canonical repository {}",
                identity.destination.display(),
                repository.path.display()
            )));
        }
        let actual_branch = git.current_branch_ref(&identity.destination)?;
        if actual_branch.as_deref() != Some(branch_ref.as_str()) {
            return Ok(conflict(format!(
                "destination {} is actually on {}; expected branch {}",
                identity.destination.display(),
                actual_branch.as_deref().unwrap_or("detached HEAD"),
                identity.branch
            )));
        }
        return Ok(Preflight::Ready(Plan {
            name: identity.name,
            canonical_path: repository.path.clone(),
            destination: identity.destination,
            branch: identity.branch,
            base_ref: None,
            action: PlannedAction::Reuse,
        }));
    }

    if destination_worktree.is_some() {
        return Ok(conflict(format!(
            "destination {} is registered with Git but is missing from the filesystem",
            identity.destination.display()
        )));
    }

    if let Some(worktree) = worktrees
        .iter()
        .find(|worktree| worktree.branch.as_deref() == Some(branch_ref.as_str()))
    {
        return Ok(conflict(format!(
            "branch {} is already checked out at {}",
            identity.branch,
            worktree.path.display()
        )));
    }

    if git.branch_exists(&repository.path, &identity.branch)? {
        return Ok(Preflight::Ready(Plan {
            name: identity.name,
            canonical_path: repository.path.clone(),
            destination: identity.destination,
            branch: identity.branch,
            base_ref: None,
            action: PlannedAction::AddExistingBranch,
        }));
    }
    if let Some(existing) = git.branch_namespace_conflict(&repository.path, &identity.branch)? {
        return Ok(conflict(format!(
            "branch {} conflicts with existing branch {existing}",
            identity.branch
        )));
    }

    if explicit_branch {
        let remote_ref = format!("refs/remotes/origin/{}", identity.branch);
        if !git.resolves_to_commit(&repository.path, &remote_ref)? {
            return Ok(conflict(format!(
                "branch {} does not exist locally and remote-tracking branch origin/{} is unavailable in {}; run `git forest fetch {}`",
                identity.branch,
                identity.branch,
                repository.path.display(),
                repository.name,
            )));
        }
        return Ok(Preflight::Ready(Plan {
            name: identity.name,
            canonical_path: repository.path.clone(),
            destination: identity.destination,
            branch: identity.branch,
            base_ref: Some(remote_ref),
            action: PlannedAction::CreateTrackingBranch,
        }));
    }

    let base_ref = match base_override {
        Some(reference) => reference.to_owned(),
        None => match inspection.default_ref {
            Some(reference) => reference,
            None => {
                return Ok(conflict(format!(
                    "origin/HEAD is unavailable in {}; pass --base {}=<ref>",
                    repository.path.display(),
                    repository.name
                )));
            }
        },
    };
    if !git.resolves_to_commit(&repository.path, &base_ref)? {
        return Ok(conflict(format!(
            "base ref {base_ref:?} does not resolve to a commit in {}",
            repository.path.display()
        )));
    }

    Ok(Preflight::Ready(Plan {
        name: identity.name,
        canonical_path: repository.path.clone(),
        destination: identity.destination,
        branch: identity.branch,
        base_ref: Some(base_ref),
        action: PlannedAction::CreateBranch,
    }))
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left == right
        || match (left.canonicalize(), right.canonicalize()) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
}
