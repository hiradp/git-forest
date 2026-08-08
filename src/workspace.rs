use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::config::{Config, RepositoryConfig};
use crate::error::{AppError, Result};
use crate::git::{Git, Worktree};

#[derive(Debug)]
pub struct WorkspaceState {
    pub name: String,
    pub path: PathBuf,
    pub exists: bool,
    pub members: Vec<MemberState>,
    pub unexpected_entries: Vec<PathBuf>,
    pub inconsistencies: Vec<String>,
}

#[derive(Debug)]
pub struct MemberState {
    pub name: String,
    pub canonical_path: PathBuf,
    pub path: PathBuf,
    pub exists: bool,
    pub registered: bool,
    pub metadata: Option<Worktree>,
    pub unexpected_worktree_paths: Vec<PathBuf>,
    pub inconsistencies: Vec<String>,
}

struct RepositoryRegistry<'a> {
    repository: &'a RepositoryConfig,
    worktrees: Vec<Worktree>,
    issue: Option<String>,
}

pub fn scan(config: &Config, git: &Git) -> Result<Vec<WorkspaceState>> {
    let registries = load_registries(config, git)?;
    let mut candidates = BTreeMap::new();

    if config.workspaces_root.exists() {
        let entries =
            fs::read_dir(&config.workspaces_root).map_err(|source| AppError::Filesystem {
                context: format!(
                    "could not read workspace root {}",
                    config.workspaces_root.display()
                ),
                source,
            })?;
        for entry in entries {
            let entry = entry.map_err(|source| AppError::Filesystem {
                context: format!(
                    "could not read an entry in {}",
                    config.workspaces_root.display()
                ),
                source,
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            candidates.insert(name, entry.path());
        }
    }

    for registry in &registries {
        for worktree in &registry.worktrees {
            if let Some(name) = workspace_name_for_path(&worktree.path, &config.workspaces_root) {
                candidates
                    .entry(name.clone())
                    .or_insert_with(|| config.workspaces_root.join(name));
            }
        }
    }

    candidates
        .into_iter()
        .map(|(name, path)| build_workspace(config, &registries, name, path))
        .collect()
}

fn load_registries<'a>(config: &'a Config, git: &Git) -> Result<Vec<RepositoryRegistry<'a>>> {
    config
        .repositories
        .iter()
        .map(|repository| {
            if !repository.path.exists() {
                return Ok(RepositoryRegistry {
                    repository,
                    worktrees: Vec::new(),
                    issue: Some(format!(
                        "canonical repository {} does not exist",
                        repository.path.display()
                    )),
                });
            }
            let inspection = git.inspect_repository(&repository.path)?;
            if !inspection.is_git_worktree {
                return Ok(RepositoryRegistry {
                    repository,
                    worktrees: Vec::new(),
                    issue: Some(format!(
                        "canonical repository {} is not a Git worktree",
                        repository.path.display()
                    )),
                });
            }
            Ok(RepositoryRegistry {
                repository,
                worktrees: git.worktrees(&repository.path)?,
                issue: None,
            })
        })
        .collect()
}

fn build_workspace(
    config: &Config,
    registries: &[RepositoryRegistry<'_>],
    name: String,
    path: PathBuf,
) -> Result<WorkspaceState> {
    let exists = path.is_dir();
    let mut inconsistencies = Vec::new();
    if path.exists() && !exists {
        inconsistencies.push("workspace path exists but is not a directory".to_owned());
    } else if !path.exists() {
        inconsistencies.push("workspace directory is missing".to_owned());
    }

    let configured_names = config
        .repositories
        .iter()
        .map(|repository| repository.name.as_str())
        .collect::<HashSet<_>>();
    let mut unexpected_entries = Vec::new();
    if exists {
        let entries = fs::read_dir(&path).map_err(|source| AppError::Filesystem {
            context: format!("could not read workspace {}", path.display()),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| AppError::Filesystem {
                context: format!("could not read an entry in {}", path.display()),
                source,
            })?;
            if !entry
                .file_name()
                .to_str()
                .is_some_and(|entry_name| configured_names.contains(entry_name))
            {
                unexpected_entries.push(entry.path());
            }
        }
        unexpected_entries.sort();
    }

    let mut members = Vec::new();
    for registry in registries {
        let destination = path.join(&registry.repository.name);
        let expected_metadata = registry
            .worktrees
            .iter()
            .find(|worktree| paths_match(&worktree.path, &destination))
            .cloned();
        let mut unexpected_worktrees = registry
            .worktrees
            .iter()
            .filter(|worktree| {
                workspace_name_for_path(&worktree.path, &config.workspaces_root).as_deref()
                    == Some(name.as_str())
                    && !paths_match(&worktree.path, &destination)
            })
            .cloned()
            .collect::<Vec<_>>();
        unexpected_worktrees.sort_by(|left, right| left.path.cmp(&right.path));
        let unexpected_worktree_paths = unexpected_worktrees
            .iter()
            .map(|worktree| worktree.path.clone())
            .collect::<Vec<_>>();

        let destination_exists = destination.exists();
        let (member_path, metadata) = if destination_exists || expected_metadata.is_some() {
            (destination.clone(), expected_metadata)
        } else if let Some(worktree) = unexpected_worktrees.first() {
            (worktree.path.clone(), Some(worktree.clone()))
        } else {
            (destination.clone(), None)
        };
        let member_exists = member_path.exists();

        if member_exists || metadata.is_some() || !unexpected_worktree_paths.is_empty() {
            let mut member_inconsistencies = Vec::new();
            if destination_exists && metadata.is_none() {
                member_inconsistencies.push(format!(
                    "{} is not registered with canonical repository {}",
                    destination.display(),
                    registry.repository.path.display()
                ));
            } else if !member_exists && metadata.is_some() {
                member_inconsistencies.push("registered worktree is missing from disk".to_owned());
            }
            for unexpected_path in &unexpected_worktree_paths {
                let message = format!(
                    "repository {} has a registered worktree at {}; expected {}",
                    registry.repository.name,
                    unexpected_path.display(),
                    destination.display()
                );
                member_inconsistencies.push(message.clone());
                inconsistencies.push(message);
            }
            if let Some(issue) = &registry.issue {
                member_inconsistencies.push(issue.clone());
            }
            members.push(MemberState {
                name: registry.repository.name.clone(),
                canonical_path: registry.repository.path.clone(),
                path: member_path,
                exists: member_exists,
                registered: metadata.is_some(),
                metadata,
                unexpected_worktree_paths,
                inconsistencies: member_inconsistencies,
            });
        }
    }

    if !unexpected_entries.is_empty() {
        inconsistencies.push(format!(
            "workspace contains {} unexpected entr{}",
            unexpected_entries.len(),
            if unexpected_entries.len() == 1 {
                "y"
            } else {
                "ies"
            }
        ));
    }

    Ok(WorkspaceState {
        name,
        path,
        exists,
        members,
        unexpected_entries,
        inconsistencies,
    })
}

fn workspace_name_for_path(path: &Path, workspace_root: &Path) -> Option<String> {
    let relative = path.strip_prefix(workspace_root).ok().or_else(|| {
        let canonical_root = workspace_root.canonicalize().ok()?;
        path.strip_prefix(canonical_root).ok()
    })?;
    match relative.components().next()? {
        Component::Normal(name) => name.to_str().map(str::to_owned),
        _ => None,
    }
}

pub fn paths_match(left: &Path, right: &Path) -> bool {
    left == right
        || match (left.canonicalize(), right.canonicalize()) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
}
