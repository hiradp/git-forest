use crate::config::Config;
use crate::domain::{RepositoriesReport, RepositoryReport};
use crate::error::Result;
use crate::git::Git;

pub fn run(config: &Config, git: &Git) -> Result<RepositoriesReport> {
    let repositories = config
        .repositories
        .iter()
        .map(|repository| {
            let exists = repository.path.exists();
            if !exists {
                return Ok(RepositoryReport {
                    name: repository.name.clone(),
                    path: repository.path.clone(),
                    exists: false,
                    is_git_worktree: false,
                    origin_url: None,
                    default_ref: None,
                });
            }

            let inspection = git.inspect_repository(&repository.path)?;
            Ok(RepositoryReport {
                name: repository.name.clone(),
                path: repository.path.clone(),
                exists: true,
                is_git_worktree: inspection.is_git_worktree,
                origin_url: inspection.origin_url,
                default_ref: inspection.default_ref,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(RepositoriesReport { repositories })
}
