use std::io::{self, IsTerminal, Write};

use serde::Serialize;

use crate::domain::{
    ArchiveStatus, AttachStatus, ChangeAction, ChangeStatus, CleanStatus, CommandReport,
    DeleteStatus, FetchStatus, RemovalStatus, RenameStatus, RepositoriesFetchReport,
    RepositoriesReport, RepositoriesSetupReport, RepositoriesUpdateReport, RepositoryRemoval,
    SetupStatus, UnarchiveStatus, UpdateStatus, WorkspaceArchiveReport, WorkspaceAttachReport,
    WorkspaceChangeReport, WorkspaceDeleteReport, WorkspaceListEntry, WorkspaceRemovalReport,
    WorkspaceRenameReport, WorkspaceRenameStatus, WorkspaceStatusEntry, WorkspaceUnarchiveReport,
    WorktreesCleanReport,
};
use crate::error::{AppError, Result};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";

/// Shown beside workspaces that have no saved symbol.
const DEFAULT_SYMBOL: &str = "🌲";

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

    fn stderr() -> Self {
        Self {
            enabled: io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
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

fn render_warnings(warnings: &[String]) -> Result<()> {
    if warnings.is_empty() {
        return Ok(());
    }
    let styles = Styles::stderr();
    let stderr = io::stderr();
    let mut writer = stderr.lock();
    for warning in warnings {
        writeln!(
            writer,
            "{}warning:{} {warning}",
            styles.yellow(),
            styles.reset()
        )
        .map_err(AppError::WriteOutput)?;
    }
    Ok(())
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
            CommandReport::ArchivesList(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspaceUnarchive(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspacesStatus(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspacePath(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspaceAttach(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspaceRename(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspaceArchive(report) => render_json(&mut writer, report)?,
            CommandReport::WorkspaceDelete(report) => render_json(&mut writer, report)?,
            CommandReport::WorktreesClean(report) => render_json(&mut writer, report)?,
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
            render_workspace_tree(&mut writer, &report.workspaces, styles)
        }
        CommandReport::ArchivesList(report) => {
            if report.archives.is_empty() {
                writeln!(writer, "No archives found.").map_err(AppError::WriteOutput)?;
            }
            for archive in &report.archives {
                writeln!(
                    writer,
                    "{}  {}  (ID: {})\n  {}",
                    archive.workspace,
                    crate::archives::display_date(&archive.archive_id),
                    archive.archive_id,
                    archive.archive_path.display()
                )
                .map_err(AppError::WriteOutput)?;
            }
            Ok(())
        }
        CommandReport::WorkspaceUnarchive(report) => {
            render_workspace_unarchive(&mut writer, report, styles)
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
            let order = tree_order(
                &report.workspaces,
                |workspace| &workspace.name,
                |workspace| workspace.parent.as_deref(),
            );
            for (index, (depth, workspace)) in order.into_iter().enumerate() {
                if index > 0 {
                    writeln!(writer).map_err(AppError::WriteOutput)?;
                }
                render_indented(&mut writer, depth, |writer| {
                    render_workspace_status(writer, workspace, styles)
                })?;
            }
            Ok(())
        }
        CommandReport::WorkspacePath(report) => {
            writeln!(writer, "{}", report.path.display()).map_err(AppError::WriteOutput)
        }
        CommandReport::WorkspaceAttach(report) => {
            render_warnings(&report.warnings)?;
            render_workspace_attach(&mut writer, report, styles)
        }
        CommandReport::WorkspaceRename(report) => {
            render_workspace_rename(&mut writer, report, styles)
        }
        CommandReport::WorkspaceArchive(report) => {
            render_workspace_archive(&mut writer, report, styles)
        }
        CommandReport::WorkspaceDelete(report) => {
            render_workspace_delete(&mut writer, report, styles)
        }
        CommandReport::WorktreesClean(report) => {
            render_worktrees_clean(&mut writer, report, styles)
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
        None,
        styles,
    )?;

    if report.repositories.is_empty() {
        writeln!(
            writer,
            "  {}No checkouts requested.{}",
            styles.dim(),
            styles.reset()
        )
        .map_err(AppError::WriteOutput)?;
    }

    let name_width = report
        .repositories
        .iter()
        .map(|repository| repository.checkout.chars().count())
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
            repository.checkout,
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

/// Renders workspace names as a tree, drawing each workspace under its
/// parent with its saved symbol.
fn render_workspace_tree(
    writer: &mut impl Write,
    workspaces: &[WorkspaceListEntry],
    styles: Styles,
) -> Result<()> {
    let order = tree_order(
        workspaces,
        |workspace| &workspace.name,
        |workspace| workspace.parent.as_deref(),
    );
    // Whether the entry currently open at each depth is its parent's last child.
    let mut last = Vec::new();
    for (index, (depth, workspace)) in order.iter().enumerate() {
        let is_last = order[index + 1..]
            .iter()
            .find(|(next, _)| next <= depth)
            .is_none_or(|(next, _)| next < depth);
        last.truncate(*depth);
        last.push(is_last);

        let mut branches = String::new();
        for &ended in last.iter().take(*depth).skip(1) {
            branches.push_str(if ended { "    " } else { "│   " });
        }
        if *depth > 0 {
            branches.push_str(if is_last { "└── " } else { "├── " });
        }
        let name_style = if *depth == 0 { styles.bold() } else { "" };
        writeln!(
            writer,
            "{}{branches}{}{} {name_style}{}{}",
            styles.dim(),
            styles.reset(),
            workspace.symbol.as_deref().unwrap_or(DEFAULT_SYMBOL),
            workspace.name,
            styles.reset(),
        )
        .map_err(AppError::WriteOutput)?;
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
        workspace.parent.as_deref(),
        styles,
    )?;

    if workspace.repositories.is_empty() {
        writeln!(writer, "  {}No worktrees.{}", styles.dim(), styles.reset())
            .map_err(AppError::WriteOutput)?;
    }
    let name_width = workspace
        .repositories
        .iter()
        .map(|repository| repository.checkout.chars().count())
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
            repository.checkout,
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

    render_workspace_entries(writer, &workspace.workspace_entries, styles)?;

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

fn render_workspace_entries(
    writer: &mut impl Write,
    entries: &[std::path::PathBuf],
    styles: Styles,
) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }

    writeln!(
        writer,
        "\n{}Workspace entries{}",
        styles.bold(),
        styles.reset()
    )
    .map_err(AppError::WriteOutput)?;
    for entry in entries {
        writeln!(writer, "  {}", entry.display()).map_err(AppError::WriteOutput)?;
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
    if let Some(parent) = &report.parent {
        render_header_field(writer, "Parent", parent, "", styles)?;
    }
    render_header_field(
        writer,
        report.multiplexer.title(),
        report.host_id(),
        styles.cyan(),
        styles,
    )?;
    writeln!(writer).map_err(AppError::WriteOutput)?;

    let rows = report
        .tabs
        .iter()
        .map(|tab| (tab, None))
        .chain(report.descendants.iter().flat_map(|descendant| {
            let elsewhere =
                (descendant.host_id() != report.host_id()).then(|| descendant.host_id());
            descendant.tabs.iter().map(move |tab| (tab, elsewhere))
        }))
        .collect::<Vec<_>>();
    let label_width = rows
        .iter()
        .map(|(tab, _)| tab.label.chars().count())
        .max()
        .unwrap_or(0);
    let status_width = rows
        .iter()
        .map(|(tab, _)| attach_status_name(tab.status).chars().count())
        .max()
        .unwrap_or(0);
    for (tab, elsewhere) in rows {
        let status = attach_status_name(tab.status);
        write!(
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
        if let Some(host_id) = elsewhere {
            write!(
                writer,
                "  {}({} {host_id}){}",
                styles.dim(),
                report.multiplexer.title(),
                styles.reset()
            )
            .map_err(AppError::WriteOutput)?;
        }
        writeln!(writer).map_err(AppError::WriteOutput)?;
    }
    Ok(())
}

fn render_workspace_rename(
    writer: &mut impl Write,
    report: &WorkspaceRenameReport,
    styles: Styles,
) -> Result<()> {
    render_header_field(writer, "From", &report.old_workspace, styles.bold(), styles)?;
    render_header_field(writer, "Old path", report.old_path.display(), "", styles)?;
    render_header_field(writer, "To", &report.workspace, styles.bold(), styles)?;
    render_header_field(writer, "Path", report.path.display(), "", styles)?;
    writeln!(writer).map_err(AppError::WriteOutput)?;

    let name_width = report
        .repositories
        .iter()
        .map(|repository| repository.checkout.chars().count())
        .max()
        .unwrap_or(0);
    for repository in &report.repositories {
        let (status, symbol, color) = match repository.status {
            RenameStatus::Repaired => ("repaired", "✓", styles.green()),
            RenameStatus::AlreadyRepaired => ("already repaired", "✓", styles.green()),
            RenameStatus::Failed => ("failed", "✗", styles.red()),
            RenameStatus::NotRun => ("not run", "–", styles.yellow()),
        };
        write!(
            writer,
            "  {color}{symbol}{} {}{:name_width$}{}  {color}{status}{}",
            styles.reset(),
            styles.bold(),
            repository.checkout,
            styles.reset(),
            styles.reset(),
        )
        .map_err(AppError::WriteOutput)?;
        if let Some(branch) = &repository.branch {
            write!(writer, "  {}{branch}{}", styles.cyan(), styles.reset())
                .map_err(AppError::WriteOutput)?;
        }
        writeln!(writer).map_err(AppError::WriteOutput)?;
        if let Some(message) = &repository.message {
            render_message(writer, message, styles)?;
        }
    }

    let (symbol, color, summary) = match report.status {
        WorkspaceRenameStatus::Renamed => ("✓", styles.green(), "Workspace renamed."),
        WorkspaceRenameStatus::Conflict => ("✗", styles.red(), "Workspace was not renamed."),
        WorkspaceRenameStatus::Failed => ("✗", styles.red(), "Workspace rename failed."),
    };
    writeln!(writer, "\n  {color}{symbol}{} {summary}", styles.reset())
        .map_err(AppError::WriteOutput)?;
    if let Some(message) = &report.message {
        render_message(writer, message, styles)?;
    }
    Ok(())
}

fn render_workspace_archive(
    writer: &mut impl Write,
    report: &WorkspaceArchiveReport,
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
    render_header_field(writer, "Archive", report.archive_path.display(), "", styles)?;
    render_header_field(writer, "Archive ID", &report.archive_id, "", styles)?;
    writeln!(writer).map_err(AppError::WriteOutput)?;

    render_repository_removals(writer, &report.repositories, styles)?;

    let (symbol, color, summary) = match report.status {
        ArchiveStatus::Archived => ("✓", styles.green(), "Workspace archived."),
        ArchiveStatus::AlreadyArchived => ("✓", styles.green(), "Workspace is already archived."),
        ArchiveStatus::Conflict => ("✗", styles.red(), "Workspace was not archived."),
        ArchiveStatus::Failed => ("✗", styles.red(), "Workspace archival failed."),
    };
    writeln!(writer, "\n  {color}{symbol}{} {summary}", styles.reset())
        .map_err(AppError::WriteOutput)?;
    if let Some(message) = &report.message {
        render_message(writer, message, styles)?;
    }
    if !report.preserved_entries.is_empty() {
        writeln!(writer, "\n{}Preserved{}", styles.bold(), styles.reset())
            .map_err(AppError::WriteOutput)?;
        for entry in &report.preserved_entries {
            writeln!(writer, "  {}", entry.display()).map_err(AppError::WriteOutput)?;
        }
    }
    Ok(())
}

fn render_workspace_unarchive(
    writer: &mut impl Write,
    report: &WorkspaceUnarchiveReport,
    styles: Styles,
) -> Result<()> {
    render_workspace_change(
        writer,
        &WorkspaceChangeReport {
            workspace: report.workspace.clone(),
            path: report.path.clone(),
            repositories: report.repositories.clone(),
        },
        styles,
    )?;
    if let Some(id) = &report.archive_id {
        render_header_field(writer, "Archive ID", id, "", styles)?;
    }
    let summary = match report.status {
        UnarchiveStatus::Unarchived => "Workspace unarchived.",
        UnarchiveStatus::AlreadyActive => "Workspace is already active.",
        UnarchiveStatus::Conflict => "Workspace was not unarchived.",
        UnarchiveStatus::Failed => "Workspace restoration failed.",
    };
    writeln!(writer, "\n{summary}").map_err(AppError::WriteOutput)?;
    if let Some(message) = &report.message {
        render_message(writer, message, styles)?;
    }
    Ok(())
}

fn render_workspace_delete(
    writer: &mut impl Write,
    report: &WorkspaceDeleteReport,
    styles: Styles,
) -> Result<()> {
    render_workspace_header(
        writer,
        &report.workspace,
        report.path.display(),
        None,
        None,
        styles,
    )?;
    render_repository_removals(writer, &report.repositories, styles)?;

    let (symbol, color, summary) = match report.status {
        DeleteStatus::Deleted => ("✓", styles.green(), "Workspace deleted."),
        DeleteStatus::AlreadyDeleted => ("✓", styles.green(), "Workspace is already deleted."),
        DeleteStatus::Conflict => ("✗", styles.red(), "Workspace was not deleted."),
        DeleteStatus::Failed => ("✗", styles.red(), "Workspace deletion failed."),
    };
    writeln!(writer, "\n  {color}{symbol}{} {summary}", styles.reset())
        .map_err(AppError::WriteOutput)?;
    if let Some(message) = &report.message {
        render_message(writer, message, styles)?;
    }
    if !report.deleted_entries.is_empty() {
        writeln!(writer, "\n{}Deleted{}", styles.bold(), styles.reset())
            .map_err(AppError::WriteOutput)?;
        for entry in &report.deleted_entries {
            writeln!(writer, "  {}", entry.display()).map_err(AppError::WriteOutput)?;
        }
    }
    Ok(())
}

fn render_worktrees_clean(
    writer: &mut impl Write,
    report: &WorktreesCleanReport,
    styles: Styles,
) -> Result<()> {
    writeln!(
        writer,
        "{}Clean stale worktrees{}",
        styles.bold(),
        styles.reset()
    )
    .map_err(AppError::WriteOutput)?;
    if report.worktrees.is_empty() {
        return writeln!(
            writer,
            "\n  {}No stale worktree registrations found.{}",
            styles.dim(),
            styles.reset()
        )
        .map_err(AppError::WriteOutput);
    }

    writeln!(writer).map_err(AppError::WriteOutput)?;
    let workspace_width = report
        .worktrees
        .iter()
        .map(|worktree| worktree.workspace.chars().count())
        .max()
        .unwrap_or(0);
    let checkout_width = report
        .worktrees
        .iter()
        .map(|worktree| worktree.checkout.chars().count())
        .max()
        .unwrap_or(0);
    for worktree in &report.worktrees {
        let (status, symbol, color) = match worktree.status {
            CleanStatus::Removed => ("removed", "✓", styles.green()),
            CleanStatus::Failed => ("failed", "✗", styles.red()),
        };
        writeln!(
            writer,
            "  {color}{symbol}{} {}{:workspace_width$}{}  {}{:checkout_width$}{}  {color}{status}{}",
            styles.reset(),
            styles.bold(),
            worktree.workspace,
            styles.reset(),
            styles.bold(),
            worktree.checkout,
            styles.reset(),
            styles.reset(),
        )
        .map_err(AppError::WriteOutput)?;
        if let Some(message) = &worktree.message {
            render_message(writer, message, styles)?;
        }
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
        None,
        styles,
    )?;

    render_repository_removals(writer, &report.repositories, styles)?;

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

fn render_repository_removals(
    writer: &mut impl Write,
    repositories: &[RepositoryRemoval],
    styles: Styles,
) -> Result<()> {
    let name_width = repositories
        .iter()
        .map(|repository| repository.checkout.chars().count())
        .max()
        .unwrap_or(0);
    for repository in repositories {
        let status = removal_status_name(repository.status);
        let (symbol, color) = removal_status_style(repository.status, styles);
        writeln!(
            writer,
            "  {color}{symbol}{} {}{:name_width$}{}  {color}{status}{}",
            styles.reset(),
            styles.bold(),
            repository.checkout,
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

fn render_workspace_header(
    writer: &mut impl Write,
    workspace: &str,
    path: impl std::fmt::Display,
    branch: Option<&str>,
    parent: Option<&str>,
    styles: Styles,
) -> Result<()> {
    render_header_field(writer, "Workspace", workspace, styles.bold(), styles)?;
    render_header_field(writer, "Path", path, "", styles)?;
    if let Some(branch) = branch {
        render_header_field(writer, "Branch", branch, styles.cyan(), styles)?;
    }
    if let Some(parent) = parent {
        render_header_field(writer, "Parent", parent, "", styles)?;
    }
    writeln!(writer).map_err(AppError::WriteOutput)
}

/// Orders entries depth-first under their parents. Entries whose parent is
/// not listed, or whose parents form a cycle, are treated as roots.
fn tree_order<T>(
    entries: &[T],
    name: impl Fn(&T) -> &str,
    parent: impl Fn(&T) -> Option<&str>,
) -> Vec<(usize, &T)> {
    fn visit<'a, T>(
        index: usize,
        depth: usize,
        entries: &'a [T],
        children: &[Vec<usize>],
        visited: &mut [bool],
        order: &mut Vec<(usize, &'a T)>,
    ) {
        if std::mem::replace(&mut visited[index], true) {
            return;
        }
        order.push((depth, &entries[index]));
        for &child in &children[index] {
            visit(child, depth + 1, entries, children, visited, order);
        }
    }

    let parent_index = entries
        .iter()
        .map(|entry| {
            parent(entry).and_then(|parent| {
                entries
                    .iter()
                    .position(|candidate| name(candidate) == parent)
            })
        })
        .collect::<Vec<_>>();
    let mut children = vec![Vec::new(); entries.len()];
    for (index, parent) in parent_index.iter().enumerate() {
        if let Some(parent) = parent {
            children[*parent].push(index);
        }
    }
    let mut visited = vec![false; entries.len()];
    let mut order = Vec::with_capacity(entries.len());
    for (index, parent) in parent_index.iter().enumerate() {
        if parent.is_none() {
            visit(index, 0, entries, &children, &mut visited, &mut order);
        }
    }
    for index in 0..entries.len() {
        visit(index, 0, entries, &children, &mut visited, &mut order);
    }
    order
}

fn render_indented(
    writer: &mut impl Write,
    depth: usize,
    render: impl FnOnce(&mut Vec<u8>) -> Result<()>,
) -> Result<()> {
    let mut buffer = Vec::new();
    render(&mut buffer)?;
    let indent = "    ".repeat(depth);
    for line in buffer.split_inclusive(|byte| *byte == b'\n') {
        if line != b"\n" {
            writer
                .write_all(indent.as_bytes())
                .map_err(AppError::WriteOutput)?;
        }
        writer.write_all(line).map_err(AppError::WriteOutput)?;
    }
    Ok(())
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
    use crate::config::Multiplexer;
    use crate::domain::{AttachedTabReport, RepositoryChangeReport};

    #[test]
    fn renders_workspace_attachment_as_a_compact_summary() {
        let tab = |label: &str, path: &str, id: &str, status| AttachedTabReport {
            label: label.to_owned(),
            path: PathBuf::from(path),
            herdr_tab_id: Some(id.to_owned()),
            rex_window_id: None,
            status,
        };
        let descendant = |name: &str, herdr: &str, tab| WorkspaceAttachReport {
            workspace: name.to_owned(),
            path: PathBuf::from(format!("/workspaces/{name}")),
            parent: Some("project".to_owned()),
            multiplexer: Multiplexer::Herdr,
            herdr_workspace_id: Some(herdr.to_owned()),
            rex_session_id: None,
            status: AttachStatus::Created,
            tabs: vec![tab],
            warnings: Vec::new(),
            descendants: Vec::new(),
        };
        let report = WorkspaceAttachReport {
            workspace: "project".to_owned(),
            path: PathBuf::from("/workspaces/project"),
            parent: None,
            multiplexer: Multiplexer::Herdr,
            herdr_workspace_id: Some("w1".to_owned()),
            rex_session_id: None,
            status: AttachStatus::Created,
            tabs: vec![tab(
                "main",
                "/workspaces/project",
                "w1:t1",
                AttachStatus::Created,
            )],
            warnings: Vec::new(),
            descendants: vec![
                descendant(
                    "topic",
                    "w1",
                    tab("topic", "/workspaces/topic", "w1:t2", AttachStatus::Created),
                ),
                descendant(
                    "other",
                    "w2",
                    tab("main", "/workspaces/other", "w2:t1", AttachStatus::Reused),
                ),
            ],
        };
        let mut output = Vec::new();

        render_workspace_attach(&mut output, &report, Styles { enabled: false }).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            concat!(
                "Workspace  project\n",
                "Path       /workspaces/project\n",
                "Herdr      w1\n",
                "\n",
                "  ✓ main   created  /workspaces/project\n",
                "  ✓ topic  created  /workspaces/topic\n",
                "  ✓ main   reused   /workspaces/other  (Herdr w2)\n",
            )
        );
    }

    #[test]
    fn renders_workspaces_as_a_tree_of_symbols_and_names() {
        let workspace =
            |name: &str, parent: Option<&str>, symbol: Option<&str>| WorkspaceListEntry {
                name: name.to_owned(),
                path: PathBuf::from(format!("/workspaces/{name}")),
                exists: true,
                parent: parent.map(str::to_owned),
                symbol: symbol.map(str::to_owned),
                repositories: Vec::new(),
                workspace_entries: vec![PathBuf::from("/workspaces/notes.md")],
                inconsistencies: Vec::new(),
            };
        let workspaces = [
            workspace("project", None, Some("🚦")),
            workspace("first", Some("project"), Some("🚦")),
            workspace("nested", Some("first"), None),
            workspace("second", Some("project"), None),
            workspace("deep", Some("second"), None),
            workspace("solo", None, None),
        ];
        let mut output = Vec::new();

        render_workspace_tree(&mut output, &workspaces, Styles { enabled: false }).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            concat!(
                "🚦 project\n",
                "├── 🚦 first\n",
                "│   └── 🌲 nested\n",
                "└── 🌲 second\n",
                "    └── 🌲 deep\n",
                "🌲 solo\n",
            )
        );
    }

    #[test]
    fn orders_workspaces_under_parents_and_survives_cycles() {
        let entries = [
            ("orphan", Some("missing")),
            ("child", Some("root")),
            ("loop-a", Some("loop-b")),
            ("root", None),
            ("grandchild", Some("child")),
            ("loop-b", Some("loop-a")),
        ];

        let order = tree_order(&entries, |entry| entry.0, |entry| entry.1)
            .into_iter()
            .map(|(depth, entry)| (depth, entry.0))
            .collect::<Vec<_>>();

        assert_eq!(
            order,
            [
                (0, "orphan"),
                (0, "root"),
                (1, "child"),
                (2, "grandchild"),
                (0, "loop-a"),
                (1, "loop-b"),
            ]
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
                    checkout: "alpha".to_owned(),
                    slot: None,
                    path: PathBuf::from("/workspaces/topic/alpha"),
                    branch: "user/topic".to_owned(),
                    base_ref: Some("origin/main".to_owned()),
                    action: Some(ChangeAction::CreateBranch),
                    status: ChangeStatus::Created,
                    message: None,
                },
                RepositoryChangeReport {
                    name: "beta".to_owned(),
                    checkout: "beta".to_owned(),
                    slot: None,
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
