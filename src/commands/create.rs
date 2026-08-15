use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::CreateArgs;
use crate::config::{CheckoutId, Config, RepositoryConfig};
use crate::domain::{
    ChangeAction, ChangeStatus, CommandOutcome, CommandReport, RepositoryChangeReport,
    WorkspaceChangeReport,
};
use crate::error::{AppError, Result};
use crate::git::{Git, branch_names_conflict, branch_names_equal, failure_message};

#[derive(Debug)]
struct Plan {
    checkout: CheckoutId,
    canonical_path: PathBuf,
    destination: PathBuf,
    branch: String,
    base_ref: Option<String>,
    action: PlannedAction,
    ignore_case: bool,
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
    bases: HashMap<CheckoutId, String>,
    branches: HashMap<CheckoutId, String>,
}

#[derive(Debug, Clone)]
struct Identity {
    checkout: CheckoutId,
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

    let mut preflight = Vec::with_capacity(arguments.checkouts.len());
    for checkout in &arguments.checkouts {
        let repository = config
            .repository(&checkout.repository)
            .expect("requested repositories were validated");
        let destination = workspace_path.join(checkout.to_string());
        let branch_override = overrides.branches.get(checkout);
        let branch = match branch_override {
            Some(branch) => branch.clone(),
            None => config.branch_for_checkout(&arguments.workspace, checkout)?,
        };
        let identity = Identity {
            checkout: checkout.clone(),
            destination,
            branch: branch.clone(),
        };
        let result = preflight_repository(
            git,
            repository,
            identity,
            overrides.bases.get(checkout).map(String::as_str),
            branch_override.is_some(),
        )?;
        preflight.push(result);
    }
    apply_planned_branch_conflicts(&mut preflight);

    if preflight
        .iter()
        .any(|item| matches!(item, Preflight::Conflict(_, _)))
    {
        let repositories = preflight
            .into_iter()
            .map(|item| match item {
                Preflight::Ready(plan) => change_report(
                    plan.checkout,
                    plan.destination,
                    plan.branch,
                    plan.base_ref,
                    Some(plan.action.report_action()),
                    ChangeStatus::NotRun,
                    Some("not run because workspace preflight failed".to_owned()),
                ),
                Preflight::Conflict(identity, message) => change_report(
                    identity.checkout,
                    identity.destination,
                    identity.branch,
                    None,
                    None,
                    ChangeStatus::Conflict,
                    Some(message),
                ),
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
            repositories.push(change_report(
                plan.checkout,
                plan.destination,
                plan.branch,
                plan.base_ref,
                Some(action),
                ChangeStatus::NotRun,
                Some("not run because an earlier checkout failed".to_owned()),
            ));
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
        repositories.push(change_report(
            plan.checkout,
            plan.destination,
            plan.branch,
            plan.base_ref,
            Some(action),
            status,
            message,
        ));
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

fn apply_planned_branch_conflicts(preflight: &mut [Preflight]) {
    let mut selected: Vec<(String, String, String, bool)> = Vec::new();
    for item in preflight {
        let replacement = match item {
            Preflight::Ready(plan) => {
                let existing = selected
                    .iter()
                    .find(|(repository, branch, _, ignore_case)| {
                        repository == &plan.checkout.repository
                            && branch_names_conflict(
                                branch,
                                &plan.branch,
                                *ignore_case || plan.ignore_case,
                            )
                    });
                if let Some((_, existing_branch, existing_checkout, ignore_case)) = existing {
                    let ignore_case = *ignore_case || plan.ignore_case;
                    let message = if branch_names_equal(existing_branch, &plan.branch, ignore_case)
                    {
                        format!(
                            "checkouts {existing_checkout} and {} select the same branch; a branch can be checked out only once",
                            plan.checkout
                        )
                    } else {
                        format!(
                            "branches {existing_branch} and {} selected by checkouts {existing_checkout} and {} conflict in Git's branch namespace",
                            plan.branch, plan.checkout
                        )
                    };
                    Some(Preflight::Conflict(
                        Identity {
                            checkout: plan.checkout.clone(),
                            destination: plan.destination.clone(),
                            branch: plan.branch.clone(),
                        },
                        message,
                    ))
                } else {
                    selected.push((
                        plan.checkout.repository.clone(),
                        plan.branch.clone(),
                        plan.checkout.to_string(),
                        plan.ignore_case,
                    ));
                    None
                }
            }
            Preflight::Conflict(_, _) => None,
        };
        if let Some(replacement) = replacement {
            *item = replacement;
        }
    }
}

fn validate_arguments(config: &Config, arguments: &CreateArgs) -> Result<Overrides> {
    let mut requested = HashSet::new();
    let mut destinations = HashMap::new();
    for checkout in &arguments.checkouts {
        if !requested.insert(checkout.clone()) {
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
        if config.repository(&checkout.repository).is_none() {
            return Err(AppError::InvalidInput(format!(
                "unknown repository {:?}",
                checkout.repository
            )));
        }
    }

    let mut branches = HashMap::new();
    for branch in &arguments.branches {
        if !requested.contains(&branch.checkout) {
            return Err(AppError::InvalidInput(format!(
                "branch override provided for unrequested checkout {:?}",
                branch.checkout.to_string()
            )));
        }
        if branches
            .insert(branch.checkout.clone(), branch.branch.clone())
            .is_some()
        {
            return Err(AppError::InvalidInput(format!(
                "multiple branch overrides provided for checkout {:?}",
                branch.checkout.to_string()
            )));
        }
    }

    let mut bases = HashMap::new();
    for base in &arguments.bases {
        if !requested.contains(&base.checkout) {
            return Err(AppError::InvalidInput(format!(
                "base override provided for unrequested checkout {:?}",
                base.checkout.to_string()
            )));
        }
        if branches.contains_key(&base.checkout) {
            return Err(AppError::InvalidInput(format!(
                "branch and base overrides cannot both be provided for checkout {:?}",
                base.checkout.to_string()
            )));
        }
        if bases
            .insert(base.checkout.clone(), base.reference.clone())
            .is_some()
        {
            return Err(AppError::InvalidInput(format!(
                "multiple base overrides provided for checkout {:?}",
                base.checkout.to_string()
            )));
        }
    }
    Ok(Overrides { bases, branches })
}

#[allow(clippy::too_many_arguments)]
fn change_report(
    checkout: CheckoutId,
    path: PathBuf,
    branch: String,
    base_ref: Option<String>,
    action: Option<ChangeAction>,
    status: ChangeStatus,
    message: Option<String>,
) -> RepositoryChangeReport {
    RepositoryChangeReport {
        name: checkout.repository.clone(),
        checkout: checkout.to_string(),
        slot: checkout.slot,
        path,
        branch,
        base_ref,
        action,
        status,
        message,
    }
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
    let ignore_case = git.ref_names_ignore_case(&repository.path)?;

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
            checkout: identity.checkout,
            canonical_path: repository.path.clone(),
            destination: identity.destination,
            branch: identity.branch,
            base_ref: None,
            action: PlannedAction::Reuse,
            ignore_case,
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

    if let Some(existing) =
        git.branch_namespace_conflict(&repository.path, &identity.branch, ignore_case)?
    {
        return Ok(conflict(format!(
            "branch {} conflicts with existing branch {existing}",
            identity.branch
        )));
    }
    if git.branch_exists(&repository.path, &identity.branch)? {
        return Ok(Preflight::Ready(Plan {
            checkout: identity.checkout,
            canonical_path: repository.path.clone(),
            destination: identity.destination,
            branch: identity.branch,
            base_ref: None,
            action: PlannedAction::AddExistingBranch,
            ignore_case,
        }));
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
            checkout: identity.checkout,
            canonical_path: repository.path.clone(),
            destination: identity.destination,
            branch: identity.branch,
            base_ref: Some(remote_ref),
            action: PlannedAction::CreateTrackingBranch,
            ignore_case,
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
                    identity.checkout
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
        checkout: identity.checkout,
        canonical_path: repository.path.clone(),
        destination: identity.destination,
        branch: identity.branch,
        base_ref: Some(base_ref),
        action: PlannedAction::CreateBranch,
        ignore_case,
    }))
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left == right
        || match (left.canonicalize(), right.canonicalize()) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(checkout: &str, branch: &str, ignore_case: bool) -> Preflight {
        Preflight::Ready(Plan {
            checkout: checkout.parse().unwrap(),
            canonical_path: PathBuf::from("/repo"),
            destination: PathBuf::from("/workspace").join(checkout),
            branch: branch.to_owned(),
            base_ref: Some("origin/main".to_owned()),
            action: PlannedAction::CreateBranch,
            ignore_case,
        })
    }

    #[test]
    fn detects_case_folded_planned_branch_conflicts() {
        let mut preflight = vec![
            ready("alpha@one", "Foo", true),
            ready("alpha@two", "foo/child", true),
        ];

        apply_planned_branch_conflicts(&mut preflight);

        assert!(matches!(preflight[0], Preflight::Ready(_)));
        assert!(matches!(preflight[1], Preflight::Conflict(_, _)));
    }

    #[test]
    fn preserves_case_distinct_planned_branches_when_refs_are_case_sensitive() {
        let mut preflight = vec![
            ready("alpha@one", "Foo", false),
            ready("alpha@two", "foo/child", false),
        ];

        apply_planned_branch_conflicts(&mut preflight);

        assert!(
            preflight
                .iter()
                .all(|item| matches!(item, Preflight::Ready(_)))
        );
    }
}
