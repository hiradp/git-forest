use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::error::{AppError, Result};
use crate::git::REPOSITORY_ENVIRONMENT;

pub const WORKSPACE_PATH_TOKEN: &str = "git_forest_path";
pub const TAB_TOKEN: &str = "git_forest_tab";

const METADATA_SOURCE: &str = "git-forest";

#[derive(Debug, Default)]
pub struct Herdr;

#[derive(Debug, Clone, Deserialize)]
pub struct HerdrWorkspace {
    #[serde(rename = "workspace_id")]
    pub id: String,
    pub label: Option<String>,
    #[serde(default)]
    pub tokens: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HerdrTab {
    #[serde(rename = "tab_id")]
    pub id: String,
    pub label: String,
    pub number: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HerdrPane {
    #[serde(rename = "pane_id")]
    pub id: String,
    pub tab_id: String,
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub tokens: HashMap<String, String>,
}

#[derive(Debug)]
pub struct CreatedWorkspace {
    pub workspace: HerdrWorkspace,
    pub tab: HerdrTab,
    pub root_pane: HerdrPane,
}

#[derive(Debug)]
pub struct CreatedTab {
    pub tab: HerdrTab,
    pub root_pane: HerdrPane,
}

#[derive(Deserialize)]
struct Envelope<T> {
    result: T,
}

#[derive(Deserialize)]
struct WorkspaceListResult {
    workspaces: Vec<HerdrWorkspace>,
}

#[derive(Deserialize)]
struct WorkspaceCreateResult {
    workspace: HerdrWorkspace,
    tab: HerdrTab,
    root_pane: HerdrPane,
}

#[derive(Deserialize)]
struct TabListResult {
    tabs: Vec<HerdrTab>,
}

#[derive(Deserialize)]
struct TabCreateResult {
    tab: HerdrTab,
    root_pane: HerdrPane,
}

#[derive(Deserialize)]
struct PaneListResult {
    panes: Vec<HerdrPane>,
}

impl Herdr {
    pub fn workspaces(&self) -> Result<Vec<HerdrWorkspace>> {
        self.request(
            "could not list Herdr workspaces",
            [OsString::from("workspace"), OsString::from("list")],
        )
        .map(|result: WorkspaceListResult| result.workspaces)
    }

    pub fn create_workspace(&self, cwd: &Path, label: &str) -> Result<CreatedWorkspace> {
        let result: WorkspaceCreateResult = self.request(
            "could not create a Herdr workspace",
            [
                OsString::from("workspace"),
                OsString::from("create"),
                OsString::from("--cwd"),
                cwd.as_os_str().to_os_string(),
                OsString::from("--label"),
                OsString::from(label),
                OsString::from("--no-focus"),
            ],
        )?;
        Ok(CreatedWorkspace {
            workspace: result.workspace,
            tab: result.tab,
            root_pane: result.root_pane,
        })
    }

    pub fn report_workspace_path(&self, workspace_id: &str, path: &str) -> Result<()> {
        self.request_value(
            "could not identify a Herdr workspace",
            [
                OsString::from("workspace"),
                OsString::from("report-metadata"),
                OsString::from(workspace_id),
                OsString::from("--source"),
                OsString::from(METADATA_SOURCE),
                OsString::from("--token"),
                token(WORKSPACE_PATH_TOKEN, path),
            ],
        )
    }

    pub fn tabs(&self, workspace_id: &str) -> Result<Vec<HerdrTab>> {
        self.request(
            "could not list Herdr tabs",
            [
                OsString::from("tab"),
                OsString::from("list"),
                OsString::from("--workspace"),
                OsString::from(workspace_id),
            ],
        )
        .map(|result: TabListResult| result.tabs)
    }

    pub fn create_tab(&self, workspace_id: &str, cwd: &Path, label: &str) -> Result<CreatedTab> {
        let result: TabCreateResult = self.request(
            "could not create a Herdr tab",
            [
                OsString::from("tab"),
                OsString::from("create"),
                OsString::from("--workspace"),
                OsString::from(workspace_id),
                OsString::from("--cwd"),
                cwd.as_os_str().to_os_string(),
                OsString::from("--label"),
                OsString::from(label),
                OsString::from("--no-focus"),
            ],
        )?;
        Ok(CreatedTab {
            tab: result.tab,
            root_pane: result.root_pane,
        })
    }

    pub fn rename_tab(&self, tab_id: &str, label: &str) -> Result<()> {
        self.request_value(
            "could not rename a Herdr tab",
            [
                OsString::from("tab"),
                OsString::from("rename"),
                OsString::from(tab_id),
                OsString::from(label),
            ],
        )
    }

    pub fn focus_tab(&self, tab_id: &str) -> Result<()> {
        self.request_value(
            "could not focus a Herdr tab",
            [
                OsString::from("tab"),
                OsString::from("focus"),
                OsString::from(tab_id),
            ],
        )
    }

    pub fn panes(&self, workspace_id: &str) -> Result<Vec<HerdrPane>> {
        self.request(
            "could not list Herdr panes",
            [
                OsString::from("pane"),
                OsString::from("list"),
                OsString::from("--workspace"),
                OsString::from(workspace_id),
            ],
        )
        .map(|result: PaneListResult| result.panes)
    }

    pub fn report_tab_role(&self, pane_id: &str, role: &str) -> Result<()> {
        self.request_value(
            "could not identify a Herdr tab",
            [
                OsString::from("pane"),
                OsString::from("report-metadata"),
                OsString::from(pane_id),
                OsString::from("--source"),
                OsString::from(METADATA_SOURCE),
                OsString::from("--token"),
                token(TAB_TOKEN, role),
            ],
        )
    }

    pub fn focus_workspace(&self, workspace_id: &str) -> Result<()> {
        self.request_value(
            "could not focus a Herdr workspace",
            [
                OsString::from("workspace"),
                OsString::from("focus"),
                OsString::from(workspace_id),
            ],
        )
    }

    fn request_value<I, S>(&self, context: &str, arguments: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run(context, arguments).map(|_| ())
    }

    fn request<T, I, S>(&self, context: &str, arguments: I) -> Result<T>
    where
        T: DeserializeOwned,
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.run(context, arguments)?;
        serde_json::from_slice::<Envelope<T>>(&output.stdout)
            .map(|response| response.result)
            .map_err(|source| AppError::ParseHerdr {
                context: context.to_owned(),
                source,
            })
    }

    fn run<I, S>(&self, context: &str, arguments: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new("herdr");
        command.args(arguments);
        for variable in REPOSITORY_ENVIRONMENT {
            command.env_remove(variable);
        }
        let output = command.output().map_err(AppError::StartHerdr)?;
        if !output.status.success() {
            return Err(AppError::Herdr {
                context: context.to_owned(),
                message: failure_message(&output),
            });
        }
        Ok(output)
    }
}

fn token(name: &str, value: &str) -> OsString {
    OsString::from(format!("{name}={value}"))
}

fn failure_message(output: &Output) -> String {
    #[derive(Deserialize)]
    struct ErrorEnvelope {
        error: ErrorBody,
    }

    #[derive(Deserialize)]
    struct ErrorBody {
        message: String,
    }

    if let Ok(error) = serde_json::from_slice::<ErrorEnvelope>(&output.stderr) {
        return error.error.message;
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !stdout.is_empty() {
        return stdout;
    }
    format!("herdr exited with {}", output.status)
}
