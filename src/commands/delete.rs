use std::fs;

use crate::cli::{ArchiveArgs, DeleteArgs, OutputArgs};
use crate::config::Config;
use crate::domain::{
    ArchiveStatus, CommandOutcome, CommandReport, DeleteStatus, WorkspaceDeleteReport,
};
use crate::error::Result;
use crate::git::Git;
use crate::workspace;

use super::archive;

pub fn run(config: &Config, git: &Git, arguments: &DeleteArgs) -> Result<CommandOutcome> {
    let _lock = workspace::lock_mutations(config)?;
    let path = config.workspace_path(&arguments.workspace)?;
    let archive_path = config.archive_path(&arguments.workspace)?;

    if fs::symlink_metadata(&path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        && fs::symlink_metadata(&archive_path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        && workspace::scan(config, git)?
            .into_iter()
            .all(|state| state.name != arguments.workspace)
    {
        return Ok(CommandOutcome::success(CommandReport::WorkspaceDelete(
            WorkspaceDeleteReport {
                workspace: arguments.workspace.clone(),
                path,
                repositories: Vec::new(),
                status: DeleteStatus::AlreadyDeleted,
                deleted_entries: Vec::new(),
                message: None,
            },
        )));
    }

    let archived = archive::run_locked(
        config,
        git,
        &ArchiveArgs {
            workspace: arguments.workspace.clone(),
            force: arguments.force,
            output: OutputArgs { json: false },
        },
    )?;
    let CommandReport::WorkspaceArchive(archive_report) = archived.report else {
        unreachable!("archive returned a different report type")
    };

    if !matches!(
        archive_report.status,
        ArchiveStatus::Archived | ArchiveStatus::AlreadyArchived
    ) {
        let status = match archive_report.status {
            ArchiveStatus::Conflict => DeleteStatus::Conflict,
            ArchiveStatus::Failed => DeleteStatus::Failed,
            ArchiveStatus::Archived | ArchiveStatus::AlreadyArchived => unreachable!(),
        };
        return Ok(CommandOutcome {
            report: CommandReport::WorkspaceDelete(WorkspaceDeleteReport {
                workspace: archive_report.workspace,
                path: archive_report.path,
                repositories: archive_report.repositories,
                status,
                deleted_entries: Vec::new(),
                message: archive_report.message,
            }),
            exit_code: archived.exit_code,
        });
    }

    let report = WorkspaceDeleteReport {
        workspace: archive_report.workspace,
        path: archive_report.path,
        repositories: archive_report.repositories,
        status: DeleteStatus::Deleted,
        deleted_entries: archive_report.preserved_entries,
        message: None,
    };

    match fs::remove_dir_all(&archive_report.archive_path) {
        Ok(()) => Ok(CommandOutcome::success(CommandReport::WorkspaceDelete(
            report,
        ))),
        Err(source) => Ok(CommandOutcome {
            report: CommandReport::WorkspaceDelete(WorkspaceDeleteReport {
                status: DeleteStatus::Failed,
                deleted_entries: Vec::new(),
                message: Some(format!(
                    "workspace was retired, but its archived files could not be deleted from {}: {source}",
                    archive_report.archive_path.display()
                )),
                ..report
            }),
            exit_code: 1,
        }),
    }
}
