use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::config::{CheckoutId, Config, RepositoryConfig};
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
    pub id: CheckoutId,
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
    let mut filesystem_checkouts = BTreeMap::new();
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
            let checkout = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<CheckoutId>().ok())
                .filter(|checkout| configured_names.contains(checkout.repository.as_str()));
            if let Some(checkout) = checkout {
                filesystem_checkouts.insert(checkout, entry.path());
            } else {
                unexpected_entries.push(entry.path());
            }
        }
        unexpected_entries.sort();
    }

    let mut members = Vec::new();
    for registry in registries {
        let mut checkout_ids = filesystem_checkouts
            .keys()
            .filter(|checkout| checkout.repository == registry.repository.name)
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut metadata_by_checkout = BTreeMap::new();
        let mut unexpected_worktrees = Vec::new();

        for worktree in &registry.worktrees {
            if workspace_name_for_path(&worktree.path, &config.workspaces_root).as_deref()
                != Some(name.as_str())
            {
                continue;
            }
            match checkout_for_path(&worktree.path, &path) {
                Some(checkout) if checkout.repository == registry.repository.name => {
                    checkout_ids.insert(checkout.clone());
                    if metadata_by_checkout
                        .insert(checkout, worktree.clone())
                        .is_some()
                    {
                        unexpected_worktrees.push(worktree.clone());
                    }
                }
                _ => unexpected_worktrees.push(worktree.clone()),
            }
        }
        unexpected_worktrees.sort_by(|left, right| left.path.cmp(&right.path));

        let member_start = members.len();
        for checkout in checkout_ids {
            let destination = path.join(checkout.to_string());
            let metadata = metadata_by_checkout.remove(&checkout);
            let member_exists = destination.exists();
            let mut member_inconsistencies = Vec::new();
            if member_exists && metadata.is_none() {
                member_inconsistencies.push(format!(
                    "{} is not registered with canonical repository {}",
                    destination.display(),
                    registry.repository.path.display()
                ));
            } else if !member_exists && metadata.is_some() {
                member_inconsistencies.push("registered worktree is missing from disk".to_owned());
            }
            if let Some(issue) = &registry.issue {
                member_inconsistencies.push(issue.clone());
            }
            members.push(MemberState {
                id: checkout,
                canonical_path: registry.repository.path.clone(),
                path: destination,
                exists: member_exists,
                registered: metadata.is_some(),
                metadata,
                unexpected_worktree_paths: Vec::new(),
                inconsistencies: member_inconsistencies,
            });
        }

        if !unexpected_worktrees.is_empty() {
            let unexpected_worktree_paths = unexpected_worktrees
                .iter()
                .map(|worktree| worktree.path.clone())
                .collect::<Vec<_>>();
            let messages = unexpected_worktree_paths
                .iter()
                .map(|unexpected_path| {
                    let message = format!(
                        "repository {} has a registered worktree at {}; expected a direct child named {} or {}@<slot>",
                        registry.repository.name,
                        unexpected_path.display(),
                        registry.repository.name,
                        registry.repository.name,
                    );
                    inconsistencies.push(message.clone());
                    message
                })
                .collect::<Vec<_>>();

            if member_start == members.len() {
                let worktree = unexpected_worktrees.remove(0);
                let mut member_inconsistencies = messages;
                if let Some(issue) = &registry.issue {
                    member_inconsistencies.push(issue.clone());
                }
                members.push(MemberState {
                    id: CheckoutId::primary(&registry.repository.name),
                    canonical_path: registry.repository.path.clone(),
                    exists: worktree.path.exists(),
                    registered: true,
                    path: worktree.path.clone(),
                    metadata: Some(worktree),
                    unexpected_worktree_paths,
                    inconsistencies: member_inconsistencies,
                });
            } else {
                let member = &mut members[member_start];
                member.unexpected_worktree_paths = unexpected_worktree_paths;
                member.inconsistencies.extend(messages);
            }
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

fn checkout_for_path(path: &Path, workspace_path: &Path) -> Option<CheckoutId> {
    let relative = path.strip_prefix(workspace_path).ok().or_else(|| {
        let canonical_workspace = workspace_path.canonicalize().ok()?;
        path.strip_prefix(canonical_workspace).ok()
    })?;
    let mut components = relative.components();
    let Component::Normal(name) = components.next()? else {
        return None;
    };
    if components.next().is_some() {
        return None;
    }
    name.to_str()?.parse().ok()
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
