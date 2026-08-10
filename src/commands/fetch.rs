use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::cli::FetchArgs;
use crate::config::{Config, RepositoryConfig};
use crate::domain::{
    CommandOutcome, CommandReport, FetchStatus, RepositoriesFetchReport, RepositoryFetchReport,
};
use crate::error::{AppError, Result};
use crate::git::{Git, failure_message};

pub fn run(config: &Config, git: &Git, arguments: &FetchArgs) -> Result<CommandOutcome> {
    let selected = selected_repositories(config, &arguments.repositories)?;
    let results = fetch_repositories(git, &selected, arguments.jobs.get());
    let mut failed = false;
    let repositories = selected
        .into_iter()
        .zip(results)
        .map(|(repository, result)| {
            let (status, message) = match result {
                Ok(()) => (FetchStatus::Fetched, None),
                Err(message) => {
                    failed = true;
                    (FetchStatus::Failed, Some(message))
                }
            };
            RepositoryFetchReport {
                name: repository.name.clone(),
                path: repository.path.clone(),
                status,
                message,
            }
        })
        .collect();

    Ok(CommandOutcome {
        report: CommandReport::RepositoriesFetch(RepositoriesFetchReport { repositories }),
        exit_code: u8::from(failed),
    })
}

pub(crate) fn selected_repositories<'a>(
    config: &'a Config,
    requested: &[String],
) -> Result<Vec<&'a RepositoryConfig>> {
    if requested.is_empty() {
        return Ok(config.repositories.iter().collect());
    }

    let mut selected = Vec::with_capacity(requested.len());
    let mut seen = HashSet::new();
    for name in requested {
        if !seen.insert(name.as_str()) {
            return Err(AppError::InvalidInput(format!(
                "repository {name:?} was requested more than once"
            )));
        }
        let repository = config
            .repository(name)
            .ok_or_else(|| AppError::InvalidInput(format!("unknown repository {name:?}")))?;
        selected.push(repository);
    }
    Ok(selected)
}

pub(crate) fn fetch_repositories(
    git: &Git,
    repositories: &[&RepositoryConfig],
    jobs: usize,
) -> Vec<std::result::Result<(), String>> {
    map_repositories(repositories, jobs, |repository| {
        fetch_repository(git, repository)
    })
}

pub(crate) fn map_repositories<T, F>(
    repositories: &[&RepositoryConfig],
    jobs: usize,
    operation: F,
) -> Vec<T>
where
    T: Send,
    F: Fn(&RepositoryConfig) -> T + Sync,
{
    if repositories.is_empty() {
        return Vec::new();
    }

    let next = AtomicUsize::new(0);
    let completed = Mutex::new(Vec::with_capacity(repositories.len()));
    let worker_count = jobs.min(repositories.len());

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let next = &next;
            let completed = &completed;
            let operation = &operation;
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(repository) = repositories.get(index) else {
                        break;
                    };
                    let result = operation(repository);
                    completed.lock().unwrap().push((index, result));
                }
            });
        }
    });

    let mut ordered: Vec<Option<T>> = (0..repositories.len()).map(|_| None).collect();
    for (index, result) in completed.into_inner().unwrap() {
        ordered[index] = Some(result);
    }
    ordered
        .into_iter()
        .map(|result| result.expect("every repository worker returns a result"))
        .collect()
}

pub(crate) fn validate_repository(
    git: &Git,
    repository: &RepositoryConfig,
) -> std::result::Result<(), String> {
    if !repository.path.exists() {
        return Err(format!(
            "canonical repository {} does not exist",
            repository.path.display()
        ));
    }
    if !git
        .is_worktree_root(&repository.path)
        .map_err(|error| error.to_string())?
    {
        return Err(format!(
            "canonical repository {} is not a Git worktree",
            repository.path.display()
        ));
    }
    Ok(())
}

fn fetch_repository(git: &Git, repository: &RepositoryConfig) -> std::result::Result<(), String> {
    validate_repository(git, repository)?;
    let output = git
        .fetch_origin(&repository.path)
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(failure_message(&output))
    }
}
