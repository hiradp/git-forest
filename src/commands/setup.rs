use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tempfile::Builder;

use crate::config::{Config, RepositoryConfig};
use crate::domain::{
    CommandOutcome, CommandReport, RepositoriesSetupReport, RepositorySetupReport, SetupStatus,
};
use crate::error::Result;
use crate::git::{Git, failure_message};

#[derive(Debug)]
enum Preflight {
    Reuse {
        name: String,
        path: PathBuf,
        remote: Option<String>,
    },
    Clone {
        name: String,
        path: PathBuf,
        remote: String,
    },
    Conflict {
        name: String,
        path: PathBuf,
        remote: Option<String>,
        message: String,
    },
}

pub fn run(config: &Config, git: &Git) -> Result<CommandOutcome> {
    let mut preflight = Vec::with_capacity(config.repositories.len());
    for repository in &config.repositories {
        preflight.push(preflight_repository(git, repository)?);
    }

    if preflight
        .iter()
        .any(|item| matches!(item, Preflight::Conflict { .. }))
    {
        let repositories = preflight
            .into_iter()
            .map(|item| match item {
                Preflight::Reuse { name, path, remote } => RepositorySetupReport {
                    name,
                    path,
                    remote,
                    status: SetupStatus::NotRun,
                    message: Some("not run because repository setup preflight failed".to_owned()),
                },
                Preflight::Clone { name, path, remote } => RepositorySetupReport {
                    name,
                    path,
                    remote: Some(remote),
                    status: SetupStatus::NotRun,
                    message: Some("not run because repository setup preflight failed".to_owned()),
                },
                Preflight::Conflict {
                    name,
                    path,
                    remote,
                    message,
                } => RepositorySetupReport {
                    name,
                    path,
                    remote,
                    status: SetupStatus::Conflict,
                    message: Some(message),
                },
            })
            .collect();
        return Ok(CommandOutcome {
            report: CommandReport::RepositoriesSetup(RepositoriesSetupReport { repositories }),
            exit_code: 1,
        });
    }

    let mut failed = false;
    let mut repositories = Vec::with_capacity(preflight.len());
    for item in preflight {
        match item {
            Preflight::Reuse { name, path, remote } => repositories.push(RepositorySetupReport {
                name,
                path,
                remote,
                status: SetupStatus::Reused,
                message: None,
            }),
            Preflight::Clone { name, path, remote } if failed => {
                repositories.push(RepositorySetupReport {
                    name,
                    path,
                    remote: Some(remote),
                    status: SetupStatus::NotRun,
                    message: Some("not run because an earlier repository failed".to_owned()),
                });
            }
            Preflight::Clone { name, path, remote } => {
                let result = clone_repository(git, &config.config_dir, &remote, &path);

                let (status, message) = match result {
                    Ok(()) => (SetupStatus::Cloned, None),
                    Err(message) => {
                        failed = true;
                        (SetupStatus::Failed, Some(message))
                    }
                };
                repositories.push(RepositorySetupReport {
                    name,
                    path,
                    remote: Some(remote),
                    status,
                    message,
                });
            }
            Preflight::Conflict { .. } => unreachable!("conflicts were handled above"),
        }
    }

    Ok(CommandOutcome {
        report: CommandReport::RepositoriesSetup(RepositoriesSetupReport { repositories }),
        exit_code: u8::from(failed),
    })
}

fn clone_repository(
    git: &Git,
    working_directory: &Path,
    remote: &str,
    destination: &Path,
) -> std::result::Result<(), String> {
    let parent = destination.parent().ok_or_else(|| {
        format!(
            "repository path {} has no parent directory",
            destination.display()
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "could not create repository root {}: {error}",
            parent.display()
        )
    })?;

    let staging = Builder::new()
        .prefix(".git-forest-clone-")
        .tempdir_in(parent)
        .map_err(|error| {
            format!(
                "could not create a clone staging directory in {}: {error}",
                parent.display()
            )
        })?;
    let output = git
        .clone_repository(working_directory, remote, staging.path())
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(failure_message(&output));
    }

    publish_repository(staging.path(), destination).map_err(|error| {
        format!(
            "could not publish cloned repository at {}: {error}",
            destination.display()
        )
    })
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
fn publish_repository(source: &Path, destination: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE).map_err(Into::into)
}

#[cfg(target_os = "windows")]
fn publish_repository(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(not(any(
    target_os = "android",
    target_os = "linux",
    target_vendor = "apple",
    target_os = "windows"
)))]
fn publish_repository(source: &Path, destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "cannot atomically rename {} to {} without replacing an existing path on this platform",
            source.display(),
            destination.display()
        ),
    ))
}

fn preflight_repository(git: &Git, repository: &RepositoryConfig) -> Result<Preflight> {
    match fs::symlink_metadata(&repository.path) {
        Ok(_) => {
            let inspection = git.inspect_repository(&repository.path)?;
            if inspection.is_git_worktree {
                Ok(Preflight::Reuse {
                    name: repository.name.clone(),
                    path: repository.path.clone(),
                    remote: repository.remote.clone(),
                })
            } else {
                Ok(conflict(
                    repository,
                    format!(
                        "canonical path {} exists but is not a Git worktree",
                        repository.path.display()
                    ),
                ))
            }
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            if let Some(remote) = &repository.remote {
                Ok(Preflight::Clone {
                    name: repository.name.clone(),
                    path: repository.path.clone(),
                    remote: remote.clone(),
                })
            } else {
                Ok(conflict(
                    repository,
                    "repositories.remote is not configured for this missing repository".to_owned(),
                ))
            }
        }
        Err(source) => Ok(conflict(
            repository,
            format!(
                "could not inspect canonical path {}: {source}",
                repository.path.display()
            ),
        )),
    }
}

fn conflict(repository: &RepositoryConfig, message: String) -> Preflight {
    Preflight::Conflict {
        name: repository.name.clone(),
        path: repository.path.clone(),
        remote: repository.remote.clone(),
        message,
    }
}
