use std::collections::HashSet;

use crate::cli::FetchArgs;
use crate::config::{Config, RepositoryConfig};
use crate::domain::{
    CommandOutcome, CommandReport, FetchStatus, RepositoriesFetchReport, RepositoryFetchReport,
};
use crate::error::{AppError, Result};
use crate::git::{Git, failure_message};

pub fn run(config: &Config, git: &Git, arguments: &FetchArgs) -> Result<CommandOutcome> {
    let selected = selected_repositories(config, arguments)?;
    let mut failed = false;
    let mut repositories = Vec::with_capacity(selected.len());

    for repository in selected {
        let result = if !repository.path.exists() {
            Err(format!(
                "canonical repository {} does not exist",
                repository.path.display()
            ))
        } else {
            let inspection = git.inspect_repository(&repository.path)?;
            if !inspection.is_git_worktree {
                Err(format!(
                    "canonical repository {} is not a Git worktree",
                    repository.path.display()
                ))
            } else {
                let output = git.fetch_origin(&repository.path)?;
                if output.status.success() {
                    Ok(())
                } else {
                    Err(failure_message(&output))
                }
            }
        };

        let (status, message) = match result {
            Ok(()) => (FetchStatus::Fetched, None),
            Err(message) => {
                failed = true;
                (FetchStatus::Failed, Some(message))
            }
        };
        repositories.push(RepositoryFetchReport {
            name: repository.name.clone(),
            path: repository.path.clone(),
            status,
            message,
        });
    }

    Ok(CommandOutcome {
        report: CommandReport::RepositoriesFetch(RepositoriesFetchReport { repositories }),
        exit_code: u8::from(failed),
    })
}

fn selected_repositories<'a>(
    config: &'a Config,
    arguments: &FetchArgs,
) -> Result<Vec<&'a RepositoryConfig>> {
    if arguments.repositories.is_empty() {
        return Ok(config.repositories.iter().collect());
    }

    let mut selected = Vec::with_capacity(arguments.repositories.len());
    let mut seen = HashSet::new();
    for name in &arguments.repositories {
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
