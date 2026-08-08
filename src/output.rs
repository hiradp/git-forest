use std::io::{self, Write};

use serde::Serialize;

use crate::domain::{
    ChangeAction, ChangeStatus, CommandReport, RemovalStatus, RepositoriesReport,
    WorkspaceChangeReport, WorkspaceListEntry, WorkspaceRemovalReport, WorkspaceStatusEntry,
};
use crate::error::{AppError, Result};

pub fn render_error(error: &AppError) -> Result<()> {
    render_error_message(&error.to_string(), error.exit_code())
}

pub fn render_error_message(message: &str, exit_code: u8) -> Result<()> {
    #[derive(Serialize)]
    struct ErrorEnvelope<'a> {
        error: ErrorDetail<'a>,
    }

    #[derive(Serialize)]
    struct ErrorDetail<'a> {
        message: &'a str,
        exit_code: u8,
    }

    let stderr = io::stderr();
    let mut writer = stderr.lock();
    render_json(
        &mut writer,
        &ErrorEnvelope {
            error: ErrorDetail { message, exit_code },
        },
    )?;
    writeln!(writer).map_err(AppError::WriteOutput)
}

pub fn render(report: &CommandReport, json: bool) -> Result<()> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    if json {
        match report {
            CommandReport::Repositories(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspaceChange(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspacesList(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspacesStatus(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspacePath(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspaceRemoval(report) => render_json(&mut writer, report)?,
        }
        writeln!(writer).map_err(AppError::WriteOutput)?;
        return Ok(());
    }

    match report {
        CommandReport::Repositories(report) => render_repositories(&mut writer, report),
        CommandReport::WorkspaceChange(report) => render_workspace_change(&mut writer, report),
        CommandReport::WorkspacesList(report) => {
            for workspace in &report.workspaces {
                render_workspace_list(&mut writer, workspace)?;
            }
            Ok(())
        }
        CommandReport::WorkspacesStatus(report) => {
            for workspace in &report.workspaces {
                render_workspace_status(&mut writer, workspace)?;
            }
            Ok(())
        }
        CommandReport::WorkspacePath(report) => {
            writeln!(writer, "{}", report.path.display()).map_err(AppError::WriteOutput)
        }
        CommandReport::WorkspaceRemoval(report) => render_workspace_removal(&mut writer, report),
    }
}

fn render_json(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer_pretty(writer, value).map_err(AppError::SerializeJson)
}

fn render_repositories(writer: &mut impl Write, report: &RepositoriesReport) -> Result<()> {
    for repository in &report.repositories {
        let state = if !repository.exists {
            "missing"
        } else if repository.is_git_worktree {
            "git"
        } else {
            "not-git"
        };
        writeln!(
            writer,
            "{}\t{}\t{}\torigin={}\tdefault={}",
            repository.name,
            repository.path.display(),
            state,
            repository.origin_url.as_deref().unwrap_or("-"),
            repository.default_ref.as_deref().unwrap_or("-")
        )
        .map_err(AppError::WriteOutput)?;
    }
    Ok(())
}

fn render_workspace_change(writer: &mut impl Write, report: &WorkspaceChangeReport) -> Result<()> {
    writeln!(writer, "{}\t{}", report.workspace, report.path.display())
        .map_err(AppError::WriteOutput)?;
    for repository in &report.repositories {
        write!(
            writer,
            "{}\t{}\t{}\tbranch={}\taction={}",
            repository.name,
            change_status_name(repository.status),
            repository.path.display(),
            repository.branch,
            repository.action.map(action_name).unwrap_or("none")
        )
        .map_err(AppError::WriteOutput)?;
        if let Some(message) = &repository.message {
            write!(writer, "\t{message}").map_err(AppError::WriteOutput)?;
        }
        writeln!(writer).map_err(AppError::WriteOutput)?;
    }
    Ok(())
}

fn render_workspace_list(writer: &mut impl Write, workspace: &WorkspaceListEntry) -> Result<()> {
    writeln!(writer, "{}\t{}", workspace.name, workspace.path.display())
        .map_err(AppError::WriteOutput)?;
    for repository in &workspace.repositories {
        writeln!(
            writer,
            "  {}\t{}\tregistered={}\tbranch={}",
            repository.name,
            repository.path.display(),
            repository.registered,
            repository.branch.as_deref().unwrap_or("detached")
        )
        .map_err(AppError::WriteOutput)?;
        render_inconsistencies(writer, &repository.inconsistencies, "    ")?;
    }
    render_inconsistencies(writer, &workspace.inconsistencies, "  ")
}

fn render_workspace_status(
    writer: &mut impl Write,
    workspace: &WorkspaceStatusEntry,
) -> Result<()> {
    writeln!(writer, "{}\t{}", workspace.name, workspace.path.display())
        .map_err(AppError::WriteOutput)?;
    for repository in &workspace.repositories {
        let dirty = match repository.dirty {
            Some(true) => "dirty",
            Some(false) => "clean",
            None => "unknown",
        };
        write!(
            writer,
            "  {}\t{}\t{}\t{}\tregistered={}",
            repository.name,
            repository.branch.as_deref().unwrap_or("detached"),
            repository.head.as_deref().unwrap_or("-"),
            dirty,
            repository.registered
        )
        .map_err(AppError::WriteOutput)?;
        if let Some(upstream) = &repository.upstream {
            write!(
                writer,
                "\tupstream={}\tahead={}\tbehind={}",
                upstream,
                repository.ahead.unwrap_or(0),
                repository.behind.unwrap_or(0)
            )
            .map_err(AppError::WriteOutput)?;
        }
        writeln!(writer).map_err(AppError::WriteOutput)?;
        render_inconsistencies(writer, &repository.inconsistencies, "    ")?;
    }
    render_inconsistencies(writer, &workspace.inconsistencies, "  ")
}

fn render_workspace_removal(
    writer: &mut impl Write,
    report: &WorkspaceRemovalReport,
) -> Result<()> {
    writeln!(writer, "{}\t{}", report.workspace, report.path.display())
        .map_err(AppError::WriteOutput)?;
    for repository in &report.repositories {
        write!(
            writer,
            "{}\t{}\t{}",
            repository.name,
            removal_status_name(repository.status),
            repository.path.display()
        )
        .map_err(AppError::WriteOutput)?;
        if let Some(message) = &repository.message {
            write!(writer, "\t{message}").map_err(AppError::WriteOutput)?;
        }
        writeln!(writer).map_err(AppError::WriteOutput)?;
    }
    for entry in &report.remaining_entries {
        writeln!(writer, "preserved\t{}", entry.display()).map_err(AppError::WriteOutput)?;
    }
    Ok(())
}

fn render_inconsistencies(
    writer: &mut impl Write,
    inconsistencies: &[String],
    indent: &str,
) -> Result<()> {
    for inconsistency in inconsistencies {
        writeln!(writer, "{indent}! {inconsistency}").map_err(AppError::WriteOutput)?;
    }
    Ok(())
}

fn action_name(action: ChangeAction) -> &'static str {
    match action {
        ChangeAction::Reuse => "reuse",
        ChangeAction::AddExistingBranch => "add-existing-branch",
        ChangeAction::CreateBranch => "create-branch",
    }
}

fn change_status_name(status: ChangeStatus) -> &'static str {
    match status {
        ChangeStatus::Reused => "reused",
        ChangeStatus::Created => "created",
        ChangeStatus::Conflict => "conflict",
        ChangeStatus::Failed => "failed",
        ChangeStatus::NotRun => "not-run",
    }
}

fn removal_status_name(status: RemovalStatus) -> &'static str {
    match status {
        RemovalStatus::Removed => "removed",
        RemovalStatus::AlreadyAbsent => "already-absent",
        RemovalStatus::Conflict => "conflict",
        RemovalStatus::Failed => "failed",
        RemovalStatus::NotRun => "not-run",
    }
}
