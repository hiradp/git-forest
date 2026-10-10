use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::error::{AppError, Result};
use crate::git::REPOSITORY_ENVIRONMENT;

const CURRENT_SESSION_VARIABLE: &str = "REX_SESSION";

#[derive(Debug)]
pub struct Rex<'a> {
    /// The program and arguments each new terminal runs. Rex starts the login
    /// shell when this is empty.
    command: &'a [String],
}

#[derive(Debug, Clone)]
pub struct RexSession {
    pub id: String,
    pub label: String,
    pub windows: Vec<RexWindow>,
}

/// A window is a tab; its blocks are the panes in it.
#[derive(Debug, Clone)]
pub struct RexWindow {
    pub id: String,
    pub label: String,
    pub block_ids: Vec<String>,
}

#[derive(Debug)]
pub struct CreatedSession {
    pub session_id: String,
    pub window_id: String,
}

#[derive(Deserialize)]
struct SessionListResult {
    sessions: Option<Vec<SessionEntry>>,
}

#[derive(Deserialize)]
struct SessionEntry {
    session_id: String,
    label: String,
}

#[derive(Deserialize)]
struct WindowListResult {
    windows: Option<Vec<WindowEntry>>,
}

#[derive(Deserialize)]
struct WindowEntry {
    window_id: String,
    label: String,
    layers: Option<Vec<LayerEntry>>,
}

#[derive(Deserialize)]
struct LayerEntry {
    blocks: Option<Vec<BlockEntry>>,
}

#[derive(Deserialize)]
struct BlockEntry {
    block_id: Option<String>,
}

#[derive(Deserialize)]
struct SessionCreateResult {
    session_id: String,
    initial_windows: Option<Vec<WindowCreateResult>>,
}

#[derive(Deserialize)]
struct WindowCreateResult {
    window_id: Option<String>,
}

#[derive(Deserialize)]
struct ProcessResult {
    foreground: Option<ProcessEntry>,
}

#[derive(Deserialize)]
struct ProcessEntry {
    cwd: Option<PathBuf>,
}

impl<'a> Rex<'a> {
    pub fn new(command: &'a [String]) -> Self {
        Self { command }
    }

    pub fn current_session_id() -> Option<String> {
        std::env::var(CURRENT_SESSION_VARIABLE)
            .ok()
            .filter(|id| !id.is_empty())
    }

    /// Every session with its windows. Rex lists windows one session at a
    /// time.
    pub fn sessions(&self) -> Result<Vec<RexSession>> {
        let result: SessionListResult =
            self.request("could not list Rex sessions", ["ls", "--json"])?;
        result
            .sessions
            .unwrap_or_default()
            .into_iter()
            .map(|session| {
                Ok(RexSession {
                    windows: self.windows(&session.session_id)?,
                    id: session.session_id,
                    label: session.label,
                })
            })
            .collect()
    }

    fn windows(&self, session_id: &str) -> Result<Vec<RexWindow>> {
        let result: WindowListResult = self.request(
            "could not list Rex windows",
            ["window", "ls", "--session", session_id, "--json"],
        )?;
        Ok(result
            .windows
            .unwrap_or_default()
            .into_iter()
            .map(|window| RexWindow {
                id: window.window_id,
                label: window.label,
                block_ids: window
                    .layers
                    .into_iter()
                    .flatten()
                    .flat_map(|layer| layer.blocks.unwrap_or_default())
                    .filter_map(|block| block.block_id)
                    .collect(),
            })
            .collect())
    }

    pub fn create_session(
        &self,
        label: &str,
        window_label: &str,
        cwd: &Path,
    ) -> Result<CreatedSession> {
        let context = "could not create a Rex session";
        let mut arguments = vec![
            OsString::from("new"),
            OsString::from(label),
            OsString::from("--window"),
            OsString::from(window_label),
            OsString::from("--cwd"),
            cwd.as_os_str().to_os_string(),
            OsString::from("--json"),
        ];
        arguments.extend(self.terminal_command());
        let result: SessionCreateResult = self.request(context, arguments)?;
        let window_id = result
            .initial_windows
            .into_iter()
            .flatten()
            .find_map(|window| window.window_id)
            .ok_or_else(|| missing_window(context))?;
        Ok(CreatedSession {
            session_id: result.session_id,
            window_id,
        })
    }

    /// Returns the new window's identifier.
    pub fn create_window(&self, session_id: &str, label: &str, cwd: &Path) -> Result<String> {
        let context = "could not create a Rex window";
        let mut arguments = vec![
            OsString::from("window"),
            OsString::from("new"),
            OsString::from("--session"),
            OsString::from(session_id),
            OsString::from(label),
            OsString::from("--cwd"),
            cwd.as_os_str().to_os_string(),
            OsString::from("--focus=false"),
            OsString::from("--json"),
        ];
        arguments.extend(self.terminal_command());
        let result: WindowCreateResult = self.request(context, arguments)?;
        result.window_id.ok_or_else(|| missing_window(context))
    }

    pub fn rename_session(&self, session_id: &str, label: &str) -> Result<()> {
        self.request_value(
            "could not rename a Rex session",
            ["session", "rename", session_id, label],
        )
    }

    pub fn rename_window(&self, session_id: &str, window_id: &str, label: &str) -> Result<()> {
        self.request_value(
            "could not rename a Rex window",
            [
                "window",
                "rename",
                "--session",
                session_id,
                window_id,
                label,
            ],
        )
    }

    pub fn focus_window(&self, session_id: &str, window_id: &str) -> Result<()> {
        self.request_value(
            "could not focus a Rex window",
            ["window", "focus", "--session", session_id, window_id],
        )
    }

    /// Asks the Rex app to show a session. Only a client can do this, and Rex
    /// picks the app when exactly one is connected.
    pub fn show_session(&self, session_id: &str, window_id: &str) -> Result<()> {
        self.request_value(
            "could not show a Rex session",
            [
                OsString::from("do"),
                OsString::from("session.select"),
                OsString::from(format!("session_id={session_id}")),
                OsString::from(format!("window_id={window_id}")),
            ],
        )
    }

    /// The working directory of a block's foreground process, when the block
    /// is a terminal that reports one.
    pub fn block_cwd(&self, session_id: &str, block_id: &str) -> Option<PathBuf> {
        self.request::<ProcessResult, _, _>(
            "could not inspect a Rex block",
            [
                "block",
                "call",
                "--session",
                session_id,
                "--block",
                block_id,
                "process",
            ],
        )
        .ok()?
        .foreground?
        .cwd
    }

    fn terminal_command(&self) -> Vec<OsString> {
        if self.command.is_empty() {
            return Vec::new();
        }
        std::iter::once(OsString::from("--"))
            .chain(self.command.iter().map(OsString::from))
            .collect()
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
        serde_json::from_slice(&output.stdout).map_err(|source| AppError::ParseRex {
            context: context.to_owned(),
            source,
        })
    }

    fn run<I, S>(&self, context: &str, arguments: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new("rex");
        // Rex starts a missing server unless told otherwise.
        command.arg("--autostart=false").args(arguments);
        for variable in REPOSITORY_ENVIRONMENT {
            command.env_remove(variable);
        }
        let output = command.output().map_err(AppError::StartRex)?;
        if !output.status.success() {
            return Err(AppError::Rex {
                context: context.to_owned(),
                message: failure_message(&output),
            });
        }
        Ok(output)
    }
}

fn missing_window(context: &str) -> AppError {
    AppError::Rex {
        context: context.to_owned(),
        message: "the response names no window".to_owned(),
    }
}

/// Rex prints `Error: <message>` followed by usage text.
fn failure_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    match stderr.lines().find(|line| !line.trim().is_empty()) {
        Some(line) => line
            .trim()
            .strip_prefix("Error: ")
            .unwrap_or(line.trim())
            .to_owned(),
        None => format!("rex exited with {}", output.status),
    }
}
