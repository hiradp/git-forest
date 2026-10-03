use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
use std::fs::File;
use std::path::{Component, Path, PathBuf};

use crate::config::{CheckoutId, Config, RepositoryConfig};
use crate::error::{AppError, Result};
use crate::git::{Git, Worktree};

#[derive(Debug)]
pub struct WorkspaceState {
    pub name: String,
    pub path: PathBuf,
    pub exists: bool,
    pub metadata: WorkspaceMetadata,
    pub members: Vec<MemberState>,
    pub workspace_entries: Vec<PathBuf>,
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

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
pub struct MutationLock {
    _config_directory: File,
}

#[cfg(not(any(target_os = "android", target_os = "linux", target_vendor = "apple")))]
pub struct MutationLock;

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
pub fn lock_mutations(config: &Config) -> Result<Option<MutationLock>> {
    use rustix::fs::{FlockOperation, flock};

    let config_directory =
        File::open(&config.config_dir).map_err(|source| AppError::Filesystem {
            context: format!(
                "could not open configuration directory {} for workspace locking",
                config.config_dir.display()
            ),
            source,
        })?;
    flock(&config_directory, FlockOperation::LockExclusive).map_err(|source| {
        AppError::Filesystem {
            context: format!(
                "could not lock configuration directory {} for workspace mutation",
                config.config_dir.display()
            ),
            source: source.into(),
        }
    })?;
    Ok(Some(MutationLock {
        _config_directory: config_directory,
    }))
}

#[cfg(not(any(target_os = "android", target_os = "linux", target_vendor = "apple")))]
pub fn lock_mutations(_config: &Config) -> Result<Option<MutationLock>> {
    Ok(None)
}

const METADATA_FILE: &str = ".forest-workspace.toml";
const LEGACY_SYMBOL_FILE: &str = ".forest-symbol";
const SYMBOL_KEY: &str = "symbol";
const PARENT_KEY: &str = "parent";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceMetadata {
    pub symbol: Option<String>,
    pub parent: Option<String>,
}

pub fn validate_symbol(symbol: &str) -> std::result::Result<(), &'static str> {
    if symbol.is_empty() || symbol.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(
            "workspace symbol must be nonempty and contain no whitespace or control characters",
        );
    }
    Ok(())
}

fn metadata_file_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(AppError::Operational(format!(
            "workspace metadata {} must be a regular file, not a directory or symlink",
            path.display()
        ))),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(AppError::Filesystem {
            context: format!("could not inspect workspace metadata {}", path.display()),
            source,
        }),
    }
}

pub fn prepare_metadata(
    workspace: &Path,
    symbol: Option<&str>,
    parent: Option<&str>,
) -> Result<()> {
    if let Some(symbol) = symbol {
        validate_symbol(symbol).map_err(|message| AppError::InvalidInput(message.to_owned()))?;
    }
    if let Some(parent) = parent {
        crate::config::validate_workspace_name(parent)?;
    }
    if symbol.is_some() || parent.is_some() {
        // Read everything a write would read, so it cannot fail after mutation.
        let table = read_table(workspace)?;
        if symbol.is_none() && !table.contains_key(SYMBOL_KEY) {
            read_legacy_symbol(workspace)?;
        } else {
            metadata_file_exists(&workspace.join(LEGACY_SYMBOL_FILE))?;
        }
    }
    Ok(())
}

pub fn read_metadata(workspace: &Path) -> Result<WorkspaceMetadata> {
    let (metadata, errors) = read_metadata_fields(workspace)?;
    match errors.into_iter().next() {
        Some(error) => Err(error),
        None => Ok(metadata),
    }
}

/// The readable saved parent, ignoring errors in other fields.
pub fn saved_parent(workspace: &Path) -> Option<String> {
    read_metadata_fields(workspace).ok()?.0.parent
}

/// Reads each field independently so that one invalid field cannot hide the
/// others, such as a parent link that protects a child from retirement.
fn read_metadata_fields(workspace: &Path) -> Result<(WorkspaceMetadata, Vec<AppError>)> {
    let table = read_table(workspace)?;
    let path = workspace.join(METADATA_FILE);
    let mut metadata = WorkspaceMetadata::default();
    let mut errors = Vec::new();
    if let Some(value) = table.get(PARENT_KEY) {
        match value.as_str() {
            None => errors.push(invalid_metadata(&path, "parent must be a string")),
            Some(parent) => match crate::config::validate_workspace_name(parent) {
                Ok(()) => metadata.parent = Some(parent.to_owned()),
                Err(error) => {
                    let message = error.to_string();
                    errors.push(invalid_metadata(
                        &path,
                        message.strip_prefix("invalid input: ").unwrap_or(&message),
                    ));
                }
            },
        }
    }
    match table.get(SYMBOL_KEY) {
        Some(value) => match value.as_str() {
            None => errors.push(invalid_metadata(&path, "symbol must be a string")),
            Some(symbol) => match validate_symbol(symbol) {
                Ok(()) => metadata.symbol = Some(symbol.to_owned()),
                Err(message) => errors.push(invalid_metadata(&path, message)),
            },
        },
        None => match read_legacy_symbol(workspace) {
            Ok(symbol) => metadata.symbol = symbol,
            Err(error) => errors.push(error),
        },
    }
    Ok((metadata, errors))
}

/// Sets the given fields, preserving every other key, and retires the legacy
/// symbol file.
pub fn update_metadata(workspace: &Path, symbol: Option<&str>, parent: Option<&str>) -> Result<()> {
    use std::io::Write;

    if symbol.is_none() && parent.is_none() {
        return Ok(());
    }
    prepare_metadata(workspace, symbol, parent)?;
    let mut table = read_table(workspace)?;
    if let Some(symbol) = symbol {
        table.insert(SYMBOL_KEY.to_owned(), symbol.into());
    } else if !table.contains_key(SYMBOL_KEY)
        && let Some(symbol) = read_legacy_symbol(workspace)?
    {
        table.insert(SYMBOL_KEY.to_owned(), symbol.into());
    }
    if let Some(parent) = parent {
        table.insert(PARENT_KEY.to_owned(), parent.into());
    }

    let path = workspace.join(METADATA_FILE);
    let contents = toml::to_string(&table)
        .map_err(|error| invalid_metadata(&path, &format!("could not serialize: {error}")))?;
    let write = || -> std::io::Result<()> {
        let mut temporary = tempfile::NamedTempFile::new_in(workspace)?;
        temporary.write_all(contents.as_bytes())?;
        temporary.persist(&path).map_err(|error| error.error)?;
        Ok(())
    };
    write().map_err(|source| AppError::Filesystem {
        context: format!("could not write workspace metadata {}", path.display()),
        source,
    })?;

    let legacy = workspace.join(LEGACY_SYMBOL_FILE);
    if metadata_file_exists(&legacy)? {
        fs::remove_file(&legacy).map_err(|source| AppError::Filesystem {
            context: format!("could not remove legacy symbol file {}", legacy.display()),
            source,
        })?;
    }
    Ok(())
}

fn read_table(workspace: &Path) -> Result<toml::Table> {
    let path = workspace.join(METADATA_FILE);
    if !metadata_file_exists(&path)? {
        return Ok(toml::Table::new());
    }
    let contents = fs::read_to_string(&path).map_err(|source| AppError::Filesystem {
        context: format!("could not read workspace metadata {}", path.display()),
        source,
    })?;
    contents
        .parse::<toml::Table>()
        .map_err(|error| invalid_metadata(&path, error.message()))
}

fn read_legacy_symbol(workspace: &Path) -> Result<Option<String>> {
    let path = workspace.join(LEGACY_SYMBOL_FILE);
    if !metadata_file_exists(&path)? {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path).map_err(|source| AppError::Filesystem {
        context: format!("could not read workspace symbol {}", path.display()),
        source,
    })?;
    let symbol = contents.trim_end();
    validate_symbol(symbol).map_err(|message| {
        AppError::Operational(format!("invalid symbol file {}: {message}", path.display()))
    })?;
    Ok(Some(symbol.to_owned()))
}

fn invalid_metadata(path: &Path, message: &str) -> AppError {
    AppError::Operational(format!(
        "invalid workspace metadata {}: {message}",
        path.display()
    ))
}

struct RepositoryRegistry<'a> {
    repository: &'a RepositoryConfig,
    worktrees: Vec<Worktree>,
    issue: Option<String>,
}

pub fn scan(config: &Config, git: &Git) -> Result<Vec<WorkspaceState>> {
    let registries = load_registries(config, git)?;
    let archive_root = config.archive_root();
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
            if paths_match(&entry.path(), &archive_root) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            candidates.insert(name, entry.path());
        }
    }

    for registry in &registries {
        for worktree in &registry.worktrees {
            if path_is_within(&worktree.path, &archive_root) {
                continue;
            }
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
    let mut workspace_entries = Vec::new();
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
                workspace_entries.push(entry.path());
            }
        }
        workspace_entries.sort();
    }
    let metadata = if exists {
        match read_metadata_fields(&path) {
            Ok((metadata, errors)) => {
                inconsistencies.extend(errors.iter().map(ToString::to_string));
                metadata
            }
            Err(error) => {
                inconsistencies.push(error.to_string());
                WorkspaceMetadata::default()
            }
        }
    } else {
        WorkspaceMetadata::default()
    };

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
            let member_exists = path_occupied(&destination)?;
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
                    exists: path_occupied(&worktree.path)?,
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

    Ok(WorkspaceState {
        name,
        path,
        exists,
        metadata,
        members,
        workspace_entries,
        inconsistencies,
    })
}

pub fn children<'a>(
    states: &'a [WorkspaceState],
    name: &'a str,
) -> impl Iterator<Item = &'a WorkspaceState> {
    states.iter().filter(move |state| {
        state.exists && state.name != name && state.metadata.parent.as_deref() == Some(name)
    })
}

pub enum Ancestry<'a> {
    /// Parents from nearest to the root; every one is an active workspace.
    Linked(Vec<&'a WorkspaceState>),
    /// The chain names a workspace that is not active.
    MissingParent {
        ancestors: Vec<&'a WorkspaceState>,
        parent: String,
    },
    Cycle(Vec<String>),
}

pub fn ancestry<'a>(states: &'a [WorkspaceState], name: &str) -> Ancestry<'a> {
    let mut visited = vec![name.to_owned()];
    let mut ancestors = Vec::new();
    let mut current = states.iter().find(|state| state.name == name);
    while let Some(parent) = current.and_then(|state| state.metadata.parent.as_deref()) {
        if visited.iter().any(|seen| seen == parent) {
            visited.push(parent.to_owned());
            return Ancestry::Cycle(visited);
        }
        visited.push(parent.to_owned());
        let Some(state) = states
            .iter()
            .find(|state| state.exists && state.name == parent)
        else {
            return Ancestry::MissingParent {
                ancestors,
                parent: parent.to_owned(),
            };
        };
        ancestors.push(state);
        current = Some(state);
    }
    Ancestry::Linked(ancestors)
}

pub(crate) fn paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    left.canonicalize()
        .ok()
        .zip(right.canonicalize().ok())
        .is_some_and(|(left, right)| left == right)
}

pub(crate) fn path_is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
        || resolve_existing_ancestor(path)
            .zip(resolve_existing_ancestor(root))
            .is_some_and(|(path, root)| path.starts_with(root))
}

fn resolve_existing_ancestor(path: &Path) -> Option<PathBuf> {
    // Git records resolved paths, even when a worktree has since been deleted.
    path.ancestors().find_map(|ancestor| {
        let resolved = ancestor.canonicalize().ok()?;
        Some(resolved.join(path.strip_prefix(ancestor).ok()?))
    })
}

fn path_occupied(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(AppError::Filesystem {
            context: format!("could not inspect workspace path {}", path.display()),
            source,
        }),
    }
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
