use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

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
                let result = path
                    .parent()
                    .ok_or_else(|| {
                        format!("repository path {} has no parent directory", path.display())
                    })
                    .and_then(|parent| {
                        fs::create_dir_all(parent).map_err(|error| {
                            format!(
                                "could not create repository root {}: {error}",
                                parent.display()
                            )
                        })
                    })
                    .and_then(|()| {
                        let output = git
                            .clone_repository(&remote, &path)
                            .map_err(|error| error.to_string())?;
                        if output.status.success() {
                            Ok(())
                        } else {
                            Err(failure_message(&output))
                        }
                    });

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
        Err(source) if source.kind() == ErrorKind::NotFound => {
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
