use std::io::{self, IsTerminal, Write};

use serde::Serialize;

use crate::domain::{
    AttachStatus, ChangeAction, ChangeStatus, CommandReport, FetchStatus, RemovalStatus,
    RepositoriesFetchReport, RepositoriesReport, RepositoriesSetupReport, RepositoriesUpdateReport,
    SetupStatus, UpdateStatus, WorkspaceAttachReport, WorkspaceChangeReport, WorkspaceListEntry,
    WorkspaceRemovalReport, WorkspaceStatusEntry,
};
use crate::error::{AppError, Result};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";

#[derive(Clone, Copy)]
struct Styles {
    enabled: bool,
}

impl Styles {
    fn stdout() -> Self {
        Self {
            enabled: io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        }
    }

    fn code(self, code: &'static str) -> &'static str {
        if self.enabled { code } else { "" }
    }

    fn reset(self) -> &'static str {
        self.code(RESET)
    }

    fn bold(self) -> &'static str {
        self.code(BOLD)
    }

    fn dim(self) -> &'static str {
        self.code(DIM)
    }

    fn red(self) -> &'static str {
        self.code(RED)
    }

    fn green(self) -> &'static str {
        self.code(GREEN)
    }

    fn yellow(self) -> &'static str {
        self.code(YELLOW)
    }

    fn cyan(self) -> &'static str {
        self.code(CYAN)
    }
}

pub fn render_error(error: &AppError) -> Result<()> {
    render_error_message(&error.to_string(), error.exit_code())
}

pub fn render_error_message(message: &str, exit_code: u8) -> Result<()> {
    #[derive(Serialize)]
    struct ErrorEnvelope<'a> {
        error: ErrorDetail<'a>,
    }

    #[derive(Serialize)]
    struct ErrorDetail<'a> {
        message: &'a str,
        exit_code: u8,
    }

    let stderr = io::stderr();
    let mut writer = stderr.lock();
    render_json(
        &mut writer,
        &ErrorEnvelope {
            error: ErrorDetail { message, exit_code },
        },
    )?;
    writeln!(writer).map_err(AppError::WriteOutput)
}

pub fn render_blank_line() -> Result<()> {
    let stdout = io::stdout();
    writeln!(stdout.lock()).map_err(AppError::WriteOutput)
}

pub fn render(report: &CommandReport, json: bool) -> Result<()> {
    let stdout = io::stdout();
    let styles = Styles::stdout();
    let mut writer = stdout.lock();

    if json {
        match report {
            CommandReport::RepositoriesSetup(report) => render_json(&mut writer, report)?,
            CommandReport::Repositories(report) => render_json(&mut writer, report)?,
            CommandReport::RepositoriesFetch(report) => render_json(&mut writer, report)?,
            CommandReport::RepositoriesUpdate(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspaceChange(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspacesList(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspacesStatus(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspacePath(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspaceAttach(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspaceRemoval(report) => render_json(&mut writer, report)?,
        }
        writeln!(writer).map_err(AppError::WriteOutput)?;
        return Ok(());
    }

    match report {
        CommandReport::RepositoriesSetup(report) => {
            render_repositories_setup(&mut writer, report, styles)
        }
        CommandReport::Repositories(report) => render_repositories(&mut writer, report, styles),
        CommandReport::RepositoriesFetch(report) => {
            render_repositories_fetch(&mut writer, report, styles)
        }
        CommandReport::RepositoriesUpdate(report) => {
            render_repositories_update(&mut writer, report, styles)
        }
        CommandReport::WorkspaceChange(report) => {
            render_workspace_change(&mut writer, report, styles)
        }
        CommandReport::WorkspacesList(report) => {
            if report.workspaces.is_empty() {
                return writeln!(
                    writer,
                    "{}No workspaces found.{}",
                    styles.dim(),
                    styles.reset()
                )
                .map_err(AppError::WriteOutput);
            }
            for (index, workspace) in report.workspaces.iter().enumerate() {
                if index > 0 {
                    writeln!(writer).map_err(AppError::WriteOutput)?;
                }
                render_workspace_list(&mut writer, workspace, styles)?;
            }
            Ok(())
        }
        CommandReport::WorkspacesStatus(report) => {
            if report.workspaces.is_empty() {
                return writeln!(
                    writer,
                    "{}No workspaces found.{}",
                    styles.dim(),
                    styles.reset()
                )
                .map_err(AppError::WriteOutput);
            }
            for (index, workspace) in report.workspaces.iter().enumerate() {
                if index > 0 {
                    writeln!(writer).map_err(AppError::WriteOutput)?;
                }
                render_workspace_status(&mut writer, workspace, styles)?;
            }
            Ok(())
        }
        CommandReport::WorkspacePath(report) => {
            writeln!(writer, "{}", report.path.display()).map_err(AppError::WriteOutput)
        }
        CommandReport::WorkspaceAttach(report) => {
            render_workspace_attach(&mut writer, report, styles)
        }
        CommandReport::WorkspaceRemoval(report) => {
            render_workspace_removal(&mut writer, report, styles)
        }
    }
}

fn render_json(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer_pretty(writer, value).map_err(AppError::SerializeJson)
}

fn render_repositories_setup(
    writer: &mut impl Write,
    report: &RepositoriesSetupReport,
    styles: Styles,
) -> Result<()> {
    writeln!(
        writer,
        "{}Setup repositories{}",
        styles.bold(),
        styles.reset()
    )
    .map_err(AppError::WriteOutput)?;
    writeln!(writer).map_err(AppError::WriteOutput)?;

    let name_width = report
        .repositories
        .iter()
        .map(|repository| repository.name.chars().count())
        .max()
        .unwrap_or(0);
    for repository in &report.repositories {
        let (status, symbol, color) = match repository.status {
            SetupStatus::Cloned => ("cloned", "✓", styles.green()),
            SetupStatus::Reused => ("reused", "✓", styles.green()),
            SetupStatus::Conflict => ("conflict", "✗", styles.red()),
            SetupStatus::Failed => ("failed", "✗", styles.red()),
            SetupStatus::NotRun => ("not run", "–", styles.yellow()),
        };
        writeln!(
            writer,
            "  {color}{symbol}{} {}{:name_width$}{}  {color}{status}{}  {}",
            styles.reset(),
            styles.bold(),
            repository.name,
            styles.reset(),
            styles.reset(),
            repository.path.display(),
        )
        .map_err(AppError::WriteOutput)?;
        if let Some(message) = &repository.message {
            render_message(writer, message, styles)?;
        }
    }
    Ok(())
}

fn render_repositories(
    writer: &mut impl Write,
    report: &RepositoriesReport,
    styles: Styles,
) -> Result<()> {
    writeln!(writer, "{}Repositories{}", styles.bold(), styles.reset())
        .map_err(AppError::WriteOutput)?;
    if report.repositories.is_empty() {
        return writeln!(
            writer,
            "\n  {}No repositories configured.{}",
            styles.dim(),
            styles.reset()
        )
        .map_err(AppError::WriteOutput);
    }

    writeln!(writer).map_err(AppError::WriteOutput)?;
    let name_width = report
        .repositories
        .iter()
        .map(|repository| repository.name.chars().count())
        .max()
        .unwrap_or(0);

    for repository in &report.repositories {
        let (symbol, color, state) = if !repository.exists {
            ("✗", styles.red(), Some("missing"))
        } else if !repository.is_git_worktree {
            ("!", styles.yellow(), Some("not a Git worktree"))
        } else {
            ("✓", styles.green(), None)
        };
        write!(
            writer,
            "  {color}{symbol}{} {}{:name_width$}{}  {}",
            styles.reset(),
            styles.bold(),
            repository.name,
            styles.reset(),
            repository.path.display(),
        )
        .map_err(AppError::WriteOutput)?;
        if let Some(state) = state {
            write!(writer, "  {color}{state}{}", styles.reset()).map_err(AppError::WriteOutput)?;
        }
        writeln!(writer).map_err(AppError::WriteOutput)?;

        if repository.exists && repository.is_git_worktree {
            render_detail_field(
                writer,
                "Origin",
                repository.origin_url.as_deref().unwrap_or("—"),
                styles,
            )?;
            render_detail_field(
                writer,
                "Default",
                repository.default_ref.as_deref().unwrap_or("—"),
                styles,
            )?;
        }
    }
    Ok(())
}

fn render_repositories_fetch(
    writer: &mut impl Write,
    report: &RepositoriesFetchReport,
    styles: Styles,
) -> Result<()> {
    writeln!(writer, "{}Fetch origin{}", styles.bold(), styles.reset())
        .map_err(AppError::WriteOutput)?;
    writeln!(writer).map_err(AppError::WriteOutput)?;

    let name_width = report
        .repositories
        .iter()
        .map(|repository| repository.name.chars().count())
        .max()
        .unwrap_or(0);
    for repository in &report.repositories {
        let (status, symbol, color) = match repository.status {
            FetchStatus::Fetched => ("fetched", "✓", styles.green()),
            FetchStatus::Failed => ("failed", "✗", styles.red()),
        };
        writeln!(
            writer,
            "  {color}{symbol}{} {}{:name_width$}{}  {color}{status}{}",
            styles.reset(),
            styles.bold(),
            repository.name,
            styles.reset(),
            styles.reset(),
        )
        .map_err(AppError::WriteOutput)?;
        if let Some(message) = &repository.message {
            render_message(writer, message, styles)?;
        }
    }
    Ok(())
}

fn render_repositories_update(
    writer: &mut impl Write,
    report: &RepositoriesUpdateReport,
    styles: Styles,
) -> Result<()> {
    writeln!(
        writer,
        "{}Update default branches{}",
        styles.bold(),
        styles.reset()
    )
    .map_err(AppError::WriteOutput)?;
    writeln!(writer).map_err(AppError::WriteOutput)?;

    let name_width = report
        .repositories
        .iter()
        .map(|repository| repository.name.chars().count())
        .max()
        .unwrap_or(0);
    let branch_width = report
        .repositories
        .iter()
        .filter_map(|repository| repository.branch.as_deref())
        .map(str::len)
        .max()
        .unwrap_or(1);
    for repository in &report.repositories {
        let (status, symbol, color) = match repository.status {
            UpdateStatus::Updated => ("updated", "✓", styles.green()),
            UpdateStatus::UpToDate => ("up to date", "✓", styles.green()),
            UpdateStatus::Conflict => ("conflict", "!", styles.yellow()),
            UpdateStatus::Failed => ("failed", "✗", styles.red()),
        };
        writeln!(
            writer,
            "  {color}{symbol}{} {}{:name_width$}{}  {:branch_width$}  {color}{status}{}",
            styles.reset(),
            styles.bold(),
            repository.name,
            styles.reset(),
            repository.branch.as_deref().unwrap_or("—"),
            styles.reset(),
        )
        .map_err(AppError::WriteOutput)?;
        if let Some(message) = &repository.message {
            render_message(writer, message, styles)?;
        }
    }
    Ok(())
}

fn render_workspace_change(
    writer: &mut impl Write,
    report: &WorkspaceChangeReport,
    styles: Styles,
) -> Result<()> {
    let common_branch = report.repositories.first().and_then(|first| {
        report
            .repositories
            .iter()
            .all(|repository| repository.branch == first.branch)
            .then_some(first.branch.as_str())
    });
    render_workspace_header(
        writer,
        &report.workspace,
        report.path.display(),
        common_branch,
        styles,
    )?;

    let name_width = report
        .repositories
        .iter()
        .map(|repository| repository.name.chars().count())
        .max()
        .unwrap_or(0);
    let status_width = report
        .repositories
        .iter()
        .map(|repository| change_status_name(repository.status).chars().count())
        .max()
        .unwrap_or(0);

    for repository in &report.repositories {
        let status = change_status_name(repository.status);
        let (symbol, color) = change_status_style(repository.status, styles);
        write!(
            writer,
            "  {color}{symbol}{} {}{:name_width$}{}  {color}{status}{}",
            styles.reset(),
            styles.bold(),
            repository.name,
            styles.reset(),
            styles.reset(),
        )
        .map_err(AppError::WriteOutput)?;

        let action = change_action_detail(repository.status, repository.action);
        let branch = common_branch
            .is_none()
            .then_some(repository.branch.as_str());
        if action.is_some() || branch.is_some() {
            let padding = status_width.saturating_sub(status.chars().count()) + 2;
            write!(writer, "{:padding$}", "").map_err(AppError::WriteOutput)?;
            if let Some(action) = action {
                write!(writer, "{}{action}{}", styles.dim(), styles.reset())
                    .map_err(AppError::WriteOutput)?;
            }
            if let Some(branch) = branch {
                if action.is_some() {
                    write!(writer, " {}·{} ", styles.dim(), styles.reset())
                        .map_err(AppError::WriteOutput)?;
                }
                write!(writer, "{}{branch}{}", styles.cyan(), styles.reset())
                    .map_err(AppError::WriteOutput)?;
            }
        }
        writeln!(writer).map_err(AppError::WriteOutput)?;

        if let Some(message) = &repository.message {
            render_message(writer, message, styles)?;
        }
    }
    Ok(())
}

fn render_workspace_list(
    writer: &mut impl Write,
    workspace: &WorkspaceListEntry,
    styles: Styles,
) -> Result<()> {
    render_workspace_header(
        writer,
        &workspace.name,
        workspace.path.display(),
        None,
        styles,
    )?;

    if workspace.repositories.is_empty() {
        writeln!(writer, "  {}No worktrees.{}", styles.dim(), styles.reset())
            .map_err(AppError::WriteOutput)?;
    }
    let name_width = workspace
        .repositories
        .iter()
        .map(|repository| repository.name.chars().count())
        .max()
        .unwrap_or(0);

    for repository in &workspace.repositories {
        let healthy =
            repository.exists && repository.registered && repository.inconsistencies.is_empty();
        let (symbol, color) = if healthy {
            ("✓", styles.green())
        } else {
            ("!", styles.yellow())
        };
        write!(
            writer,
            "  {color}{symbol}{} {}{:name_width$}{}  {}",
            styles.reset(),
            styles.bold(),
            repository.name,
            styles.reset(),
            repository.branch.as_deref().unwrap_or("detached"),
        )
        .map_err(AppError::WriteOutput)?;
        if !repository.exists {
            write!(
                writer,
                " {}·{} {}missing{}",
                styles.dim(),
                styles.reset(),
                styles.yellow(),
                styles.reset(),
            )
            .map_err(AppError::WriteOutput)?;
        }
        if !repository.registered {
            write!(
                writer,
                " {}·{} {}unregistered{}",
                styles.dim(),
                styles.reset(),
                styles.yellow(),
                styles.reset(),
            )
            .map_err(AppError::WriteOutput)?;
        }
        writeln!(writer).map_err(AppError::WriteOutput)?;
        render_inconsistencies(writer, &repository.inconsistencies, "      ", styles)?;
    }

    for inconsistency in &workspace.inconsistencies {
        if !workspace.repositories.iter().any(|repository| {
            repository
                .inconsistencies
                .iter()
                .any(|repository_issue| repository_issue == inconsistency)
        }) {
            render_inconsistency(writer, inconsistency, "  ", styles)?;
        }
    }
    Ok(())
}

fn render_workspace_status(
    writer: &mut impl Write,
    workspace: &WorkspaceStatusEntry,
    styles: Styles,
) -> Result<()> {
    render_workspace_header(
        writer,
        &workspace.name,
        workspace.path.display(),
        None,
        styles,
    )?;

    if workspace.repositories.is_empty() {
        writeln!(writer, "  {}No worktrees.{}", styles.dim(), styles.reset())
            .map_err(AppError::WriteOutput)?;
    }
    let name_width = workspace
        .repositories
        .iter()
        .map(|repository| repository.name.chars().count())
        .max()
        .unwrap_or(0);

    for repository in &workspace.repositories {
        let (symbol, symbol_color) = if !repository.exists
            || !repository.registered
            || !repository.inconsistencies.is_empty()
        {
            ("!", styles.yellow())
        } else if repository.dirty == Some(true) {
            ("●", styles.yellow())
        } else {
            ("✓", styles.green())
        };
        write!(
            writer,
            "  {symbol_color}{symbol}{} {}{:name_width$}{}  {}",
            styles.reset(),
            styles.bold(),
            repository.name,
            styles.reset(),
            repository.branch.as_deref().unwrap_or("detached"),
        )
        .map_err(AppError::WriteOutput)?;

        write_separator(writer, styles)?;
        let (state, state_color) = if !repository.exists {
            ("missing", styles.yellow())
        } else {
            match repository.dirty {
                Some(true) => ("dirty", styles.yellow()),
                Some(false) => ("clean", styles.green()),
                None => ("unknown", styles.yellow()),
            }
        };
        write!(writer, "{state_color}{state}{}", styles.reset()).map_err(AppError::WriteOutput)?;

        if let Some(head) = &repository.head {
            write_separator(writer, styles)?;
            write!(
                writer,
                "{}{}{}",
                styles.dim(),
                short_head(head),
                styles.reset(),
            )
            .map_err(AppError::WriteOutput)?;
        }
        if let Some(upstream) = &repository.upstream {
            write_separator(writer, styles)?;
            write!(writer, "{}{upstream}{}", styles.cyan(), styles.reset())
                .map_err(AppError::WriteOutput)?;
            if repository.ahead.unwrap_or(0) > 0 {
                write!(
                    writer,
                    " {}↑{}{}",
                    styles.yellow(),
                    repository.ahead.unwrap_or(0),
                    styles.reset(),
                )
                .map_err(AppError::WriteOutput)?;
            }
            if repository.behind.unwrap_or(0) > 0 {
                write!(
                    writer,
                    " {}↓{}{}",
                    styles.yellow(),
                    repository.behind.unwrap_or(0),
                    styles.reset(),
                )
                .map_err(AppError::WriteOutput)?;
            }
        }
        if !repository.registered {
            write_separator(writer, styles)?;
            write!(writer, "{}unregistered{}", styles.yellow(), styles.reset(),)
                .map_err(AppError::WriteOutput)?;
        }
        writeln!(writer).map_err(AppError::WriteOutput)?;
        render_inconsistencies(writer, &repository.inconsistencies, "      ", styles)?;
    }

    for inconsistency in &workspace.inconsistencies {
        if !workspace.repositories.iter().any(|repository| {
            repository
                .inconsistencies
                .iter()
                .any(|repository_issue| repository_issue == inconsistency)
        }) {
            render_inconsistency(writer, inconsistency, "  ", styles)?;
        }
    }
    Ok(())
}

fn render_workspace_attach(
    writer: &mut impl Write,
    report: &WorkspaceAttachReport,
    styles: Styles,
) -> Result<()> {
    render_header_field(
        writer,
        "Workspace",
        &report.workspace,
        styles.bold(),
        styles,
    )?;
    render_header_field(writer, "Path", report.path.display(), "", styles)?;
    render_header_field(
        writer,
        "Herdr",
        &report.herdr_workspace_id,
        styles.cyan(),
        styles,
    )?;
    writeln!(writer).map_err(AppError::WriteOutput)?;

    let label_width = report
        .tabs
        .iter()
        .map(|tab| tab.label.chars().count())
        .max()
        .unwrap_or(0);
    let status_width = report
        .tabs
        .iter()
        .map(|tab| attach_status_name(tab.status).chars().count())
        .max()
        .unwrap_or(0);
    for tab in &report.tabs {
        let status = attach_status_name(tab.status);
        writeln!(
            writer,
            "  {}✓{} {}{:label_width$}{}  {}{status:<status_width$}{}  {}",
            styles.green(),
            styles.reset(),
            styles.bold(),
            tab.label,
            styles.reset(),
            styles.green(),
            styles.reset(),
            tab.path.display(),
        )
        .map_err(AppError::WriteOutput)?;
    }
    Ok(())
}

fn render_workspace_removal(
    writer: &mut impl Write,
    report: &WorkspaceRemovalReport,
    styles: Styles,
) -> Result<()> {
    render_workspace_header(
        writer,
        &report.workspace,
        report.path.display(),
        None,
        styles,
    )?;

    let name_width = report
        .repositories
        .iter()
        .map(|repository| repository.name.chars().count())
        .max()
        .unwrap_or(0);
    for repository in &report.repositories {
        let status = removal_status_name(repository.status);
        let (symbol, color) = removal_status_style(repository.status, styles);
        writeln!(
            writer,
            "  {color}{symbol}{} {}{:name_width$}{}  {color}{status}{}",
            styles.reset(),
            styles.bold(),
            repository.name,
            styles.reset(),
            styles.reset(),
        )
        .map_err(AppError::WriteOutput)?;
        if let Some(message) = &repository.message {
            render_message(writer, message, styles)?;
        }
    }

    if report.workspace_removed {
        writeln!(
            writer,
            "\n  {}✓{} Workspace directory removed.",
            styles.green(),
            styles.reset(),
        )
        .map_err(AppError::WriteOutput)?;
    } else if !report.remaining_entries.is_empty() {
        writeln!(writer, "\n{}Preserved{}", styles.bold(), styles.reset())
            .map_err(AppError::WriteOutput)?;
        for entry in &report.remaining_entries {
            writeln!(writer, "  {}", entry.display()).map_err(AppError::WriteOutput)?;
        }
    } else if report.repositories.is_empty() {
        writeln!(
            writer,
            "  {}Nothing to remove.{}",
            styles.dim(),
            styles.reset()
        )
        .map_err(AppError::WriteOutput)?;
    }
    Ok(())
}

fn render_workspace_header(
    writer: &mut impl Write,
    workspace: &str,
    path: impl std::fmt::Display,
    branch: Option<&str>,
    styles: Styles,
) -> Result<()> {
    render_header_field(writer, "Workspace", workspace, styles.bold(), styles)?;
    render_header_field(writer, "Path", path, "", styles)?;
    if let Some(branch) = branch {
        render_header_field(writer, "Branch", branch, styles.cyan(), styles)?;
    }
    writeln!(writer).map_err(AppError::WriteOutput)
}

fn render_header_field(
    writer: &mut impl Write,
    label: &str,
    value: impl std::fmt::Display,
    value_style: &str,
    styles: Styles,
) -> Result<()> {
    let value_reset = if value_style.is_empty() {
        ""
    } else {
        styles.reset()
    };
    writeln!(
        writer,
        "{}{label:<9}{}  {value_style}{value}{value_reset}",
        styles.dim(),
        styles.reset(),
    )
    .map_err(AppError::WriteOutput)
}

fn render_detail_field(
    writer: &mut impl Write,
    label: &str,
    value: &str,
    styles: Styles,
) -> Result<()> {
    writeln!(
        writer,
        "      {}{label:<7}{}  {value}",
        styles.dim(),
        styles.reset(),
    )
    .map_err(AppError::WriteOutput)
}

fn render_message(writer: &mut impl Write, message: &str, styles: Styles) -> Result<()> {
    for (index, line) in message.split('\n').enumerate() {
        if index == 0 {
            writeln!(writer, "      {}└{} {line}", styles.dim(), styles.reset())
                .map_err(AppError::WriteOutput)?;
        } else {
            writeln!(writer, "        {line}").map_err(AppError::WriteOutput)?;
        }
    }
    Ok(())
}

fn render_inconsistencies(
    writer: &mut impl Write,
    inconsistencies: &[String],
    indent: &str,
    styles: Styles,
) -> Result<()> {
    for inconsistency in inconsistencies {
        render_inconsistency(writer, inconsistency, indent, styles)?;
    }
    Ok(())
}

fn render_inconsistency(
    writer: &mut impl Write,
    inconsistency: &str,
    indent: &str,
    styles: Styles,
) -> Result<()> {
    for (index, line) in inconsistency.split('\n').enumerate() {
        if index == 0 {
            writeln!(
                writer,
                "{indent}{}!{} {line}",
                styles.yellow(),
                styles.reset(),
            )
            .map_err(AppError::WriteOutput)?;
        } else {
            writeln!(writer, "{indent}  {line}").map_err(AppError::WriteOutput)?;
        }
    }
    Ok(())
}

fn write_separator(writer: &mut impl Write, styles: Styles) -> Result<()> {
    write!(writer, " {}·{} ", styles.dim(), styles.reset()).map_err(AppError::WriteOutput)
}

fn short_head(head: &str) -> String {
    head.chars().take(8).collect()
}

fn change_action_detail(
    status: ChangeStatus,
    action: Option<ChangeAction>,
) -> Option<&'static str> {
    if !matches!(status, ChangeStatus::Created) {
        return None;
    }
    match action {
        Some(ChangeAction::CreateBranch) => Some("new branch"),
        Some(ChangeAction::AddExistingBranch) => Some("existing branch"),
        Some(ChangeAction::Reuse) | None => None,
    }
}

fn change_status_name(status: ChangeStatus) -> &'static str {
    match status {
        ChangeStatus::Reused => "reused",
        ChangeStatus::Created => "created",
        ChangeStatus::Conflict => "conflict",
        ChangeStatus::Failed => "failed",
        ChangeStatus::NotRun => "not run",
    }
}

fn change_status_style(status: ChangeStatus, styles: Styles) -> (&'static str, &'static str) {
    match status {
        ChangeStatus::Reused | ChangeStatus::Created => ("✓", styles.green()),
        ChangeStatus::Conflict | ChangeStatus::Failed => ("✗", styles.red()),
        ChangeStatus::NotRun => ("–", styles.yellow()),
    }
}

fn attach_status_name(status: AttachStatus) -> &'static str {
    match status {
        AttachStatus::Created => "created",
        AttachStatus::Reused => "reused",
        AttachStatus::Reconciled => "reconciled",
    }
}

fn removal_status_name(status: RemovalStatus) -> &'static str {
    match status {
        RemovalStatus::Removed => "removed",
        RemovalStatus::AlreadyAbsent => "already absent",
        RemovalStatus::Conflict => "conflict",
        RemovalStatus::Failed => "failed",
        RemovalStatus::NotRun => "not run",
    }
}

fn removal_status_style(status: RemovalStatus, styles: Styles) -> (&'static str, &'static str) {
    match status {
        RemovalStatus::Removed | RemovalStatus::AlreadyAbsent => ("✓", styles.green()),
        RemovalStatus::Conflict | RemovalStatus::Failed => ("✗", styles.red()),
        RemovalStatus::NotRun => ("–", styles.yellow()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::{AttachedTabReport, RepositoryChangeReport};

    #[test]
    fn renders_workspace_attachment_as_a_compact_summary() {
        let report = WorkspaceAttachReport {
            workspace: "topic".to_owned(),
            path: PathBuf::from("/workspaces/topic"),
            herdr_workspace_id: "w1".to_owned(),
            status: AttachStatus::Reconciled,
            tabs: vec![
                AttachedTabReport {
                    label: "1-main".to_owned(),
                    path: PathBuf::from("/workspaces/topic"),
                    herdr_tab_id: "w1:t1".to_owned(),
                    status: AttachStatus::Reused,
                },
                AttachedTabReport {
                    label: "2-alpha".to_owned(),
                    path: PathBuf::from("/workspaces/topic/alpha"),
                    herdr_tab_id: "w1:t2".to_owned(),
                    status: AttachStatus::Reconciled,
                },
            ],
        };
        let mut output = Vec::new();

        render_workspace_attach(&mut output, &report, Styles { enabled: false }).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            concat!(
                "Workspace  topic\n",
                "Path       /workspaces/topic\n",
                "Herdr      w1\n",
                "\n",
                "  ✓ 1-main   reused      /workspaces/topic\n",
                "  ✓ 2-alpha  reconciled  /workspaces/topic/alpha\n",
            )
        );
    }

    #[test]
    fn renders_workspace_changes_as_a_compact_summary() {
        let report = WorkspaceChangeReport {
            workspace: "topic".to_owned(),
            path: PathBuf::from("/workspaces/topic"),
            repositories: vec![
                RepositoryChangeReport {
                    name: "alpha".to_owned(),
                    path: PathBuf::from("/workspaces/topic/alpha"),
                    branch: "user/topic".to_owned(),
                    base_ref: Some("origin/main".to_owned()),
                    action: Some(ChangeAction::CreateBranch),
                    status: ChangeStatus::Created,
                    message: None,
                },
                RepositoryChangeReport {
                    name: "beta".to_owned(),
                    path: PathBuf::from("/workspaces/topic/beta"),
                    branch: "user/topic".to_owned(),
                    base_ref: None,
                    action: Some(ChangeAction::Reuse),
                    status: ChangeStatus::Reused,
                    message: None,
                },
            ],
        };
        let mut output = Vec::new();

        render_workspace_change(&mut output, &report, Styles { enabled: false }).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            concat!(
                "Workspace  topic\n",
                "Path       /workspaces/topic\n",
                "Branch     user/topic\n",
                "\n",
                "  ✓ alpha  created  new branch\n",
                "  ✓ beta   reused\n",
            )
        );
    }
}
