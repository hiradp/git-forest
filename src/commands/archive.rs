use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::archives;
pub(crate) use crate::archives::{entries, path_metadata};
use crate::cli::ArchiveArgs;
use crate::config::Config;
use crate::domain::{
    ArchiveStatus, CommandOutcome, CommandReport, RemovalStatus, RepositoryRemoval,
    WorkspaceArchiveReport,
};
use crate::error::{AppError, Result};
use crate::git::Git;
use crate::workspace;

use super::remove;

pub(crate) struct Retirement {
    pub repositories: Vec<RepositoryRemoval>,
    pub status: ArchiveStatus,
    pub preserved_entries: Vec<PathBuf>,
    pub message: Option<String>,
}

impl Retirement {
    fn conflict(message: String) -> Self {
        Self {
            repositories: Vec::new(),
            status: ArchiveStatus::Conflict,
            preserved_entries: Vec::new(),
            message: Some(message),
        }
    }
}

pub fn run(config: &Config, git: &Git, arguments: &ArchiveArgs) -> Result<CommandOutcome> {
    let _lock = workspace::lock_mutations(config)?;
    let workspace_path = config.workspace_path(&arguments.workspace)?;
    let workspace_missing = path_metadata(&workspace_path)?.is_none();
    let existing = archives::for_workspace(config, &arguments.workspace)?;
    let destination = match (workspace_missing, existing.last()) {
        (true, Some(archive)) => archive.clone(),
        _ => archives::destination(config, &arguments.workspace)?,
    };
    let already_archived = workspace_missing
        && !existing.is_empty()
        && workspace::scan(config, git)?
            .iter()
            .find(|state| state.name == arguments.workspace)
            .is_none_or(|state| state.members.is_empty());
    let retirement = if already_archived {
        Retirement {
            repositories: Vec::new(),
            status: ArchiveStatus::AlreadyArchived,
            preserved_entries: entries(&destination.path)?,
            message: None,
        }
    } else {
        retire_locked(config, git, arguments, &destination.path)?
    };
    let exit_code = u8::from(!matches!(
        retirement.status,
        ArchiveStatus::Archived | ArchiveStatus::AlreadyArchived
    ));
    Ok(CommandOutcome {
        report: CommandReport::WorkspaceArchive(WorkspaceArchiveReport {
            workspace: arguments.workspace.clone(),
            path: workspace_path,
            archive_id: destination.archive_id,
            archive_path: destination.path,
            repositories: retirement.repositories,
            status: retirement.status,
            preserved_entries: retirement.preserved_entries,
            message: retirement.message,
        }),
        exit_code,
    })
}

/// Caller must hold the workspace mutation lock.
pub(crate) fn retire_locked(
    config: &Config,
    git: &Git,
    arguments: &ArchiveArgs,
    destination: &Path,
) -> Result<Retirement> {
    let workspace_path = config.workspace_path(&arguments.workspace)?;
    let workspace_metadata = path_metadata(&workspace_path)?;
    let states = workspace::scan(config, git)?;
    if workspace_metadata.is_some()
        && let Some(state) = states.iter().find(|state| {
            state.name != arguments.workspace && paths_match(&state.path, &workspace_path)
        })
    {
        return Ok(Retirement::conflict(format!(
            "workspace name casing does not match; use {:?}",
            state.name
        )));
    }
    let state = states
        .iter()
        .find(|state| state.name == arguments.workspace);
    let Some(workspace_metadata) = workspace_metadata else {
        let message = if state.is_some_and(|state| !state.members.is_empty()) {
            "workspace directory is missing while registered worktrees remain"
        } else {
            "workspace does not exist"
        };
        return Ok(Retirement::conflict(message.to_owned()));
    };
    if !workspace_metadata.is_dir() {
        return Ok(Retirement::conflict(format!(
            "workspace path {} exists and is not a directory",
            workspace_path.display()
        )));
    }

    let storage_root = destination
        .parent()
        .expect("retirement destination has a parent");
    archives::validate_storage_root(config, storage_root)?;
    fs::create_dir_all(storage_root).map_err(|source| AppError::Filesystem {
        context: format!(
            "could not create retirement storage {}",
            storage_root.display()
        ),
        source,
    })?;
    archives::validate_storage_root(config, storage_root)?;

    if !rename_is_supported() {
        return Ok(Retirement::conflict(
            "atomic workspace retirement is not supported on this platform".to_owned(),
        ));
    }
    if !same_filesystem(&workspace_path, storage_root)? {
        return Ok(Retirement::conflict(
            "workspace and retirement storage are on different filesystems".to_owned(),
        ));
    }
    if path_metadata(destination)?.is_some() {
        return Ok(Retirement::conflict(format!(
            "retirement destination {} already exists",
            destination.display()
        )));
    }

    let removal = remove::run_for_archive(config, git, &arguments.workspace, arguments.force)?;
    let CommandReport::WorkspaceRemoval(removal_report) = removal.report else {
        unreachable!("archive removal returned a different report type")
    };
    if removal.exit_code != 0 {
        let status = if removal_report
            .repositories
            .iter()
            .any(|repository| repository.status == RemovalStatus::Conflict)
        {
            ArchiveStatus::Conflict
        } else {
            ArchiveStatus::Failed
        };
        return Ok(Retirement {
            repositories: removal_report.repositories,
            status,
            preserved_entries: Vec::new(),
            message: Some("workspace was not retired because worktree removal failed".to_owned()),
        });
    }

    let mut retirement = Retirement {
        repositories: removal_report.repositories,
        status: ArchiveStatus::Archived,
        preserved_entries: Vec::new(),
        message: None,
    };
    let changed = workspace::scan(config, git)?
        .into_iter()
        .find(|state| state.name == arguments.workspace)
        .is_some_and(|state| !state.members.is_empty());
    if changed {
        retirement.status = ArchiveStatus::Conflict;
        retirement.message = Some("workspace changed while it was being retired".to_owned());
        return Ok(retirement);
    }

    let preserved_entries = entries(&workspace_path)?
        .into_iter()
        .filter_map(|entry| entry.file_name().map(|name| destination.join(name)))
        .collect();
    if let Err(source) = rename_without_replacing(&workspace_path, destination) {
        retirement.status = ArchiveStatus::Failed;
        retirement.message = Some(format!(
            "could not retire workspace {}: {source}",
            workspace_path.display()
        ));
    } else {
        retirement.preserved_entries = preserved_entries;
    }
    Ok(retirement)
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
pub(crate) fn rename_is_supported() -> bool {
    true
}

#[cfg(not(any(target_os = "android", target_os = "linux", target_vendor = "apple")))]
pub(crate) fn rename_is_supported() -> bool {
    false
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
pub(crate) fn same_filesystem(left: &Path, right: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let left_metadata = fs::metadata(left).map_err(|source| AppError::Filesystem {
        context: format!("could not inspect workspace path {}", left.display()),
        source,
    })?;
    let right_metadata = fs::metadata(right).map_err(|source| AppError::Filesystem {
        context: format!("could not inspect storage root {}", right.display()),
        source,
    })?;
    Ok(left_metadata.dev() == right_metadata.dev())
}

#[cfg(not(any(target_os = "android", target_os = "linux", target_vendor = "apple")))]
pub(crate) fn same_filesystem(_left: &Path, _right: &Path) -> Result<bool> {
    Ok(false)
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
pub(crate) fn rename_without_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE).map_err(Into::into)
}

#[cfg(not(any(target_os = "android", target_os = "linux", target_vendor = "apple")))]
pub(crate) fn rename_without_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "cannot atomically rename {} to {} without replacing an existing path on this platform",
            source.display(),
            destination.display()
        ),
    ))
}
