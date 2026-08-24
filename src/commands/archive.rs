use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::cli::ArchiveArgs;
use crate::config::Config;
use crate::domain::{
    ArchiveStatus, CommandOutcome, CommandReport, RemovalStatus, WorkspaceArchiveReport,
    WorkspaceRemovalReport,
};
use crate::error::{AppError, Result};
use crate::git::Git;
use crate::workspace;

use super::remove;

pub fn run(config: &Config, git: &Git, arguments: &ArchiveArgs) -> Result<CommandOutcome> {
    let _lock = workspace::lock_mutations(config)?;
    let workspace_path = config.workspace_path(&arguments.workspace)?;
    let archive_path = config.archive_path(&arguments.workspace)?;
    let workspace_metadata = path_metadata(&workspace_path)?;
    let states = workspace::scan(config, git)?;
    if workspace_metadata.is_some()
        && let Some(state) = states.iter().find(|state| {
            state.name != arguments.workspace && paths_match(&state.path, &workspace_path)
        })
    {
        return Ok(conflict(
            arguments,
            workspace_path,
            archive_path,
            format!("workspace name casing does not match; use {:?}", state.name),
        ));
    }
    let state = states
        .into_iter()
        .find(|state| state.name == arguments.workspace);

    if let Some(metadata) = path_metadata(&archive_path)? {
        if metadata.is_dir()
            && workspace_metadata.is_none()
            && state.as_ref().is_none_or(|state| state.members.is_empty())
        {
            return Ok(CommandOutcome::success(CommandReport::WorkspaceArchive(
                WorkspaceArchiveReport {
                    workspace: arguments.workspace.clone(),
                    path: workspace_path,
                    preserved_entries: entries(&archive_path)?,
                    archive_path,
                    repositories: Vec::new(),
                    status: ArchiveStatus::AlreadyArchived,
                    message: None,
                },
            )));
        }

        return Ok(conflict(
            arguments,
            workspace_path,
            archive_path.clone(),
            format!(
                "archive destination {} already exists",
                archive_path.display()
            ),
        ));
    }

    let Some(workspace_metadata) = workspace_metadata else {
        let message = if state
            .as_ref()
            .is_some_and(|state| !state.members.is_empty())
        {
            "workspace directory is missing while registered worktrees remain"
        } else {
            "workspace does not exist"
        };
        return Ok(conflict(
            arguments,
            workspace_path,
            archive_path,
            message.to_owned(),
        ));
    };
    if !workspace_metadata.is_dir() {
        return Ok(conflict(
            arguments,
            workspace_path.clone(),
            archive_path,
            format!(
                "workspace path {} exists and is not a directory",
                workspace_path.display()
            ),
        ));
    }

    let archive_root = config.archive_root();
    if let Some(metadata) = path_metadata(&archive_root)?
        && !metadata.is_dir()
    {
        return Ok(conflict(
            arguments,
            workspace_path,
            archive_path,
            format!(
                "archive root {} exists and is not a directory",
                archive_root.display()
            ),
        ));
    }
    fs::create_dir_all(&archive_root).map_err(|source| AppError::Filesystem {
        context: format!("could not create archive root {}", archive_root.display()),
        source,
    })?;

    if !rename_is_supported() {
        return Ok(conflict(
            arguments,
            workspace_path,
            archive_path,
            "atomic workspace archival is not supported on this platform".to_owned(),
        ));
    }
    if !same_filesystem(&workspace_path, &archive_root)? {
        return Ok(conflict(
            arguments,
            workspace_path,
            archive_path,
            "workspace and archive directories are on different filesystems".to_owned(),
        ));
    }

    if path_metadata(&archive_path)?.is_some() {
        return Ok(conflict(
            arguments,
            workspace_path,
            archive_path.clone(),
            format!(
                "archive destination {} already exists",
                archive_path.display()
            ),
        ));
    }

    let removal = remove::run_for_archive(config, git, &arguments.workspace, arguments.force)?;
    let CommandReport::WorkspaceRemoval(removal_report) = removal.report else {
        unreachable!("archive removal returned a different report type")
    };
    if removal.exit_code != 0 {
        return Ok(failed_archive(arguments, archive_path, removal_report));
    }

    let changed = workspace::scan(config, git)?
        .into_iter()
        .find(|state| state.name == arguments.workspace)
        .is_some_and(|state| !state.members.is_empty());
    if changed {
        return Ok(archive_failure(
            arguments,
            archive_path,
            removal_report,
            ArchiveStatus::Conflict,
            "workspace changed while it was being archived".to_owned(),
        ));
    }

    let preserved_entries = entries(&workspace_path)?
        .into_iter()
        .filter_map(|entry| entry.file_name().map(|name| archive_path.join(name)))
        .collect();
    if let Err(source) = rename_without_replacing(&workspace_path, &archive_path) {
        return Ok(archive_failure(
            arguments,
            archive_path,
            removal_report,
            ArchiveStatus::Failed,
            format!(
                "could not archive workspace {}: {source}",
                workspace_path.display()
            ),
        ));
    }

    Ok(CommandOutcome::success(CommandReport::WorkspaceArchive(
        WorkspaceArchiveReport {
            workspace: arguments.workspace.clone(),
            path: workspace_path,
            archive_path,
            repositories: removal_report.repositories,
            status: ArchiveStatus::Archived,
            preserved_entries,
            message: None,
        },
    )))
}

fn failed_archive(
    arguments: &ArchiveArgs,
    archive_path: PathBuf,
    removal: WorkspaceRemovalReport,
) -> CommandOutcome {
    let status = if removal
        .repositories
        .iter()
        .any(|repository| repository.status == RemovalStatus::Conflict)
    {
        ArchiveStatus::Conflict
    } else {
        ArchiveStatus::Failed
    };
    archive_failure(
        arguments,
        archive_path,
        removal,
        status,
        "workspace was not archived because worktree removal failed".to_owned(),
    )
}

fn archive_failure(
    arguments: &ArchiveArgs,
    archive_path: PathBuf,
    removal: WorkspaceRemovalReport,
    status: ArchiveStatus,
    message: String,
) -> CommandOutcome {
    CommandOutcome {
        report: CommandReport::WorkspaceArchive(WorkspaceArchiveReport {
            workspace: arguments.workspace.clone(),
            path: removal.path,
            archive_path,
            repositories: removal.repositories,
            status,
            preserved_entries: Vec::new(),
            message: Some(message),
        }),
        exit_code: 1,
    }
}

fn conflict(
    arguments: &ArchiveArgs,
    workspace_path: PathBuf,
    archive_path: PathBuf,
    message: String,
) -> CommandOutcome {
    CommandOutcome {
        report: CommandReport::WorkspaceArchive(WorkspaceArchiveReport {
            workspace: arguments.workspace.clone(),
            path: workspace_path,
            archive_path,
            repositories: Vec::new(),
            status: ArchiveStatus::Conflict,
            preserved_entries: Vec::new(),
            message: Some(message),
        }),
        exit_code: 1,
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

fn entries(path: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(path).map_err(|source| AppError::Filesystem {
        context: format!("could not read archived workspace {}", path.display()),
        source,
    })?;
    let mut entries = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|source| AppError::Filesystem {
                    context: format!("could not read an entry in {}", path.display()),
                    source,
                })
        })
        .collect::<Result<Vec<_>>>()?;
    entries.sort();
    Ok(entries)
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
fn rename_is_supported() -> bool {
    true
}

#[cfg(not(any(target_os = "android", target_os = "linux", target_vendor = "apple")))]
fn rename_is_supported() -> bool {
    false
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
fn same_filesystem(left: &Path, right: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let left_metadata = fs::metadata(left).map_err(|source| AppError::Filesystem {
        context: format!("could not inspect workspace path {}", left.display()),
        source,
    })?;
    let right_metadata = fs::metadata(right).map_err(|source| AppError::Filesystem {
        context: format!("could not inspect archive root {}", right.display()),
        source,
    })?;
    Ok(left_metadata.dev() == right_metadata.dev())
}

#[cfg(not(any(target_os = "android", target_os = "linux", target_vendor = "apple")))]
fn same_filesystem(_left: &Path, _right: &Path) -> Result<bool> {
    Ok(false)
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
