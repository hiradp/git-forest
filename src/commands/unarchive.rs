use crate::archives;
use crate::cli::{CreateArgs, UnarchiveArgs};
use crate::config::Config;
use crate::domain::{CommandOutcome, CommandReport, UnarchiveStatus, WorkspaceUnarchiveReport};
use crate::error::Result;
use crate::git::Git;
use crate::workspace;

use super::{archive, create};

pub fn run(config: &Config, git: &Git, arguments: &UnarchiveArgs) -> Result<CommandOutcome> {
    let _lock = workspace::lock_mutations(config)?;
    let source_workspace = &arguments.creation.workspace;
    let destination = arguments.destination.as_deref().unwrap_or(source_workspace);
    let path = config.workspace_path(destination)?;
    let candidates = archives::for_workspace(config, source_workspace)?;
    let selected = match arguments.archive.as_deref() {
        Some(id) => candidates.iter().find(|entry| entry.archive_id == id),
        None if candidates.len() == 1 => candidates.first(),
        None => None,
    };
    let mut report = WorkspaceUnarchiveReport {
        source_workspace: source_workspace.clone(),
        workspace: destination.to_owned(),
        path: path.clone(),
        archive_id: selected
            .map(|entry| entry.archive_id.clone())
            .or_else(|| arguments.archive.clone()),
        archive_path: selected.map(|entry| entry.path.clone()),
        repositories: Vec::new(),
        status: UnarchiveStatus::Unarchived,
        message: None,
    };
    if let Some(id) = &arguments.archive {
        if !archives::valid_id(id) {
            return Ok(conflict(
                report,
                format!("invalid archive ID {id:?}; use an ID from `git forest list --archived`"),
            ));
        }
    } else if candidates.len() > 1 {
        return Ok(conflict(
            report,
            format!(
                "workspace {source_workspace:?} has multiple archives; select one with --archive <ID>: {}",
                candidates
                    .iter()
                    .map(|entry| entry.archive_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    let active = archive::path_metadata(&path)?;
    if let Some(metadata) = &active {
        if !metadata.is_dir() || selected.is_some() {
            return Ok(conflict(
                report,
                format!(
                    "active destination {} already exists; choose another name with --as <WORKSPACE>",
                    path.display()
                ),
            ));
        }
        // Without a manifest, a consumed archive cannot be distinguished from an
        // already-active workspace. Reconcile only when no source will be moved.
        report.status = UnarchiveStatus::AlreadyActive;
    } else if selected.is_none() {
        return Ok(conflict(
            report,
            format!(
                "no matching archive for workspace {source_workspace:?}; use `git forest list --archived`"
            ),
        ));
    }

    if let Some(selected) = selected {
        if let Some(symbol) = arguments.creation.symbol.as_deref() {
            workspace::prepare_symbol(&selected.path, symbol)?;
        }
        if !archive::rename_is_supported()
            || !archive::same_filesystem(&selected.path, &config.workspaces_root)?
        {
            return Ok(conflict(
                report,
                "atomic restoration requires supported paths on the same filesystem".to_owned(),
            ));
        }
        for repository in &config.repositories {
            if !repository.path.exists()
                || !git.inspect_repository(&repository.path)?.is_git_worktree
            {
                continue;
            }
            for worktree in git.worktrees(&repository.path)? {
                if workspace::path_is_within(&worktree.path, &selected.path)
                    || workspace::path_is_within(&worktree.path, &path)
                {
                    return Ok(conflict(
                        report,
                        format!(
                            "registered worktree {} conflicts with restoration; repair or remove its registration first",
                            worktree.path.display()
                        ),
                    ));
                }
            }
        }
        for checkout in &arguments.creation.checkouts {
            let entry = selected.path.join(checkout.to_string());
            if archive::path_metadata(&entry)?.is_some() {
                return Ok(conflict(
                    report,
                    format!(
                        "archived entry {} conflicts with requested checkout {checkout}",
                        entry.display()
                    ),
                ));
            }
        }
    }

    let creation = CreateArgs {
        workspace: destination.to_owned(),
        symbol: arguments.creation.symbol.clone(),
        checkouts: arguments.creation.checkouts.clone(),
        bases: arguments.creation.bases.clone(),
        branches: arguments.creation.branches.clone(),
        output: arguments.creation.output.clone(),
    };
    let plan = match create::prepare_create(config, git, &creation)? {
        create::Preparation::Ready(plan) => plan,
        create::Preparation::Conflict(outcome) => {
            return Ok(with_creation(report, outcome, UnarchiveStatus::Conflict));
        }
    };
    if let Some(selected) = selected
        && let Err(source) = archive::rename_without_replacing(&selected.path, &path)
    {
        report.status = UnarchiveStatus::Failed;
        report.message = Some(format!(
            "could not restore {}: {source}",
            selected.path.display()
        ));
        return Ok(CommandOutcome {
            report: CommandReport::WorkspaceUnarchive(report),
            exit_code: 1,
        });
    }
    let outcome = create::apply(git, plan)?;
    let status = if outcome.exit_code == 0 {
        report.status
    } else {
        UnarchiveStatus::Failed
    };
    if outcome.exit_code != 0 {
        report.message = Some("workspace files were restored, but checkout creation failed; repeat this command to retry".to_owned());
    }
    Ok(with_creation(report, outcome, status))
}

fn conflict(mut report: WorkspaceUnarchiveReport, message: String) -> CommandOutcome {
    report.status = UnarchiveStatus::Conflict;
    report.message = Some(message);
    CommandOutcome {
        report: CommandReport::WorkspaceUnarchive(report),
        exit_code: 1,
    }
}

fn with_creation(
    mut report: WorkspaceUnarchiveReport,
    outcome: CommandOutcome,
    status: UnarchiveStatus,
) -> CommandOutcome {
    let CommandReport::WorkspaceChange(creation) = outcome.report else {
        unreachable!("creation returned a different report type")
    };
    report.repositories = creation.repositories;
    report.status = status;
    CommandOutcome {
        report: CommandReport::WorkspaceUnarchive(report),
        exit_code: outcome.exit_code,
    }
}
