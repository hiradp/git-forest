use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct RepositoriesReport {
    pub repositories: Vec<RepositoryReport>,
}

#[derive(Debug, Serialize)]
pub struct RepositoryReport {
    pub name: String,
    pub path: PathBuf,
    pub exists: bool,
    pub is_git_worktree: bool,
    pub origin_url: Option<String>,
    pub default_ref: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RepositoriesSetupReport {
    pub repositories: Vec<RepositorySetupReport>,
}

#[derive(Debug, Serialize)]
pub struct RepositorySetupReport {
    pub name: String,
    pub path: PathBuf,
    pub remote: Option<String>,
    pub status: SetupStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupStatus {
    Cloned,
    Reused,
    Conflict,
    Failed,
    NotRun,
}

#[derive(Debug, Serialize)]
pub struct RepositoriesFetchReport {
    pub repositories: Vec<RepositoryFetchReport>,
}

#[derive(Debug, Serialize)]
pub struct RepositoryFetchReport {
    pub name: String,
    pub path: PathBuf,
    pub status: FetchStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchStatus {
    Fetched,
    Failed,
}

#[derive(Debug, Serialize)]
pub struct RepositoriesUpdateReport {
    pub repositories: Vec<RepositoryUpdateReport>,
}

#[derive(Debug, Serialize)]
pub struct RepositoryUpdateReport {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub status: UpdateStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    Updated,
    UpToDate,
    Conflict,
    Failed,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceChangeReport {
    pub workspace: String,
    pub path: PathBuf,
    pub repositories: Vec<RepositoryChangeReport>,
}

#[derive(Debug, Serialize)]
pub struct RepositoryChangeReport {
    pub name: String,
    pub path: PathBuf,
    pub branch: String,
    pub base_ref: Option<String>,
    pub action: Option<ChangeAction>,
    pub status: ChangeStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeAction {
    Reuse,
    AddExistingBranch,
    CreateBranch,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Reused,
    Created,
    Conflict,
    Failed,
    NotRun,
}

#[derive(Debug, Serialize)]
pub struct WorkspacesListReport {
    pub workspaces: Vec<WorkspaceListEntry>,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceListEntry {
    pub name: String,
    pub path: PathBuf,
    pub exists: bool,
    pub repositories: Vec<WorkspaceListRepository>,
    pub unexpected_entries: Vec<PathBuf>,
    pub inconsistencies: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceListRepository {
    pub name: String,
    pub path: PathBuf,
    pub exists: bool,
    pub registered: bool,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub inconsistencies: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkspacesStatusReport {
    pub workspaces: Vec<WorkspaceStatusEntry>,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceStatusEntry {
    pub name: String,
    pub path: PathBuf,
    pub exists: bool,
    pub repositories: Vec<RepositoryStatus>,
    pub unexpected_entries: Vec<PathBuf>,
    pub inconsistencies: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RepositoryStatus {
    pub name: String,
    pub path: PathBuf,
    pub exists: bool,
    pub registered: bool,
    pub branch: Option<String>,
    pub detached: Option<bool>,
    pub head: Option<String>,
    pub dirty: Option<bool>,
    pub upstream: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub inconsistencies: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkspacePathReport {
    pub workspace: String,
    pub path: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceAttachReport {
    pub workspace: String,
    pub path: PathBuf,
    pub herdr_workspace_id: String,
    pub status: AttachStatus,
    pub tabs: Vec<AttachedTabReport>,
}

#[derive(Debug, Serialize)]
pub struct AttachedTabReport {
    pub label: String,
    pub path: PathBuf,
    pub herdr_tab_id: String,
    pub status: AttachStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachStatus {
    Created,
    Reused,
    Reconciled,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceRemovalReport {
    pub workspace: String,
    pub path: PathBuf,
    pub repositories: Vec<RepositoryRemoval>,
    pub workspace_removed: bool,
    pub remaining_entries: Vec<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct RepositoryRemoval {
    pub name: String,
    pub path: PathBuf,
    pub status: RemovalStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalStatus {
    Removed,
    AlreadyAbsent,
    Conflict,
    Failed,
    NotRun,
}

#[derive(Debug)]
pub struct CommandOutcome {
    pub report: CommandReport,
    pub exit_code: u8,
}

impl CommandOutcome {
    pub fn success(report: CommandReport) -> Self {
        Self {
            report,
            exit_code: 0,
        }
    }
}

#[derive(Debug)]
pub enum CommandReport {
    RepositoriesSetup(RepositoriesSetupReport),
    Repositories(RepositoriesReport),
    RepositoriesFetch(RepositoriesFetchReport),
    RepositoriesUpdate(RepositoriesUpdateReport),
    WorkspaceChange(WorkspaceChangeReport),
    WorkspacesList(WorkspacesListReport),
    WorkspacesStatus(WorkspacesStatusReport),
    WorkspacePath(WorkspacePathReport),
    WorkspaceAttach(WorkspaceAttachReport),
    WorkspaceRemoval(WorkspaceRemovalReport),
}
