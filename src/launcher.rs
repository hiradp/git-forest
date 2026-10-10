use std::collections::HashSet;
use std::fmt;
use std::io::{self, IsTerminal, Write};

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::Print;
use crossterm::terminal::{self, ClearType};
use crossterm::{execute, queue};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

use crate::config::{self, CheckoutId, Config};
use crate::error::{AppError, Result};
use crate::git::Git;
use crate::workspace::{self, Ancestry, WorkspaceState};

const PAGE_SIZE: usize = 10;

#[derive(Debug)]
pub enum Action {
    Attach {
        workspace: String,
        issue: Option<String>,
    },
    Create {
        workspace: String,
        checkouts: Vec<CheckoutId>,
    },
    Archive {
        workspaces: Vec<String>,
        force: bool,
    },
    Delete {
        workspaces: Vec<String>,
        force: bool,
    },
}

#[derive(Debug)]
pub enum Outcome {
    Action(Action),
    Cancelled,
    Interrupted,
}

#[derive(Clone, Debug)]
enum WorkspaceChoice {
    Create,
    Existing {
        name: String,
        name_width: usize,
        summary: String,
        issue: Option<String>,
        /// Workspaces nested under this one. The picker hides them because
        /// opening this workspace opens them too.
        descendants: Vec<String>,
    },
}

impl WorkspaceChoice {
    fn answer(&self) -> &str {
        match self {
            Self::Create => "Create a new workspace",
            Self::Existing { name, .. } => name,
        }
    }

    /// Hidden descendants stay searchable through the workspace that shows
    /// them.
    fn search_text(&self) -> String {
        match self {
            Self::Create => self.to_string(),
            Self::Existing { descendants, .. } => {
                format!("{self} {}", descendants.join(" "))
            }
        }
    }
}

impl fmt::Display for WorkspaceChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Create => formatter.write_str("+  Create a new workspace"),
            Self::Existing {
                name,
                name_width,
                summary,
                issue,
                descendants,
            } => {
                write!(formatter, "{name:<name_width$}  {summary}")?;
                match descendants.len() {
                    0 => {}
                    1 => formatter.write_str("  +1 child")?,
                    count => write!(formatter, "  +{count} children")?,
                }
                if issue.is_some() {
                    formatter.write_str("  ! needs attention")?;
                }
                Ok(())
            }
        }
    }
}

pub fn is_interactive_terminal() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

pub fn prompt(config: &Config, git: &Git) -> Result<Outcome> {
    if !is_interactive_terminal() {
        return Err(AppError::InvalidInput(
            "the workspace launcher requires an interactive terminal".to_owned(),
        ));
    }

    write_banner()?;

    let states = workspace::scan(config, git)?;
    let existing_names = states
        .iter()
        .map(|state| state.name.clone())
        .collect::<HashSet<_>>();
    let mut choices = vec![WorkspaceChoice::Create];
    choices.extend(workspace_choices(&states));

    let mut terminal = PromptTerminal::new().map_err(AppError::Prompt)?;
    let mut selected_workspaces = HashSet::new();
    loop {
        let choice = match select(
            &mut terminal,
            "Where do you want to work?",
            &choices,
            &mut selected_workspaces,
            "search · ↑↓ move · space select · enter open · ctrl+d/del actions · esc leave",
            |choice| choice.answer(),
            WorkspaceChoice::search_text,
        )? {
            PromptAnswer::Value(SelectionAction::Open(index)) => {
                return match choices[index].clone() {
                    WorkspaceChoice::Existing { name, issue, .. } => {
                        Ok(Outcome::Action(Action::Attach {
                            workspace: name,
                            issue,
                        }))
                    }
                    WorkspaceChoice::Create => {
                        prompt_for_workspace(config, existing_names, &mut terminal)
                    }
                };
            }
            PromptAnswer::Value(SelectionAction::Manage(indices)) => indices
                .into_iter()
                .flat_map(|index| retirement_names(&choices[index]))
                .collect::<Vec<_>>(),
            PromptAnswer::Cancelled => return Ok(Outcome::Cancelled),
            PromptAnswer::Interrupted => return Ok(Outcome::Interrupted),
        };

        if choice.is_empty() {
            continue;
        }
        match prompt_for_retirement(&mut terminal, &choice)? {
            PromptAnswer::Value(action) => return Ok(Outcome::Action(action)),
            PromptAnswer::Cancelled => {}
            PromptAnswer::Interrupted => return Ok(Outcome::Interrupted),
        }
    }
}

fn prompt_for_workspace(
    config: &Config,
    existing_names: HashSet<String>,
    terminal: &mut PromptTerminal,
) -> Result<Outcome> {
    let workspace_root = config.workspaces_root.clone();
    let workspace = match text(
        terminal,
        "Name your workspace",
        "feature-name",
        "letters, numbers, dots, dashes, and underscores",
        |input| match config::validate_workspace_name(input) {
            Ok(()) if existing_names.contains(input) || workspace_root.join(input).exists() => {
                Some("That workspace already exists — pick another name.".to_owned())
            }
            Ok(()) => None,
            Err(error) => Some(input_error_message(&error)),
        },
    )? {
        PromptAnswer::Value(workspace) => workspace,
        PromptAnswer::Cancelled => return Ok(Outcome::Cancelled),
        PromptAnswer::Interrupted => return Ok(Outcome::Interrupted),
    };

    let repositories = config
        .repositories
        .iter()
        .map(|repository| repository.name.clone())
        .collect::<Vec<_>>();
    let repositories = match multi_select(
        terminal,
        "Which repositories are coming along?",
        &repositories,
        "space to toggle · type to search · → all · enter to create",
        config.repositories.len() == 1,
    )? {
        PromptAnswer::Value(repositories) => repositories,
        PromptAnswer::Cancelled => return Ok(Outcome::Cancelled),
        PromptAnswer::Interrupted => return Ok(Outcome::Interrupted),
    };

    Ok(Outcome::Action(Action::Create {
        workspace,
        checkouts: repositories.into_iter().map(CheckoutId::primary).collect(),
    }))
}

/// Retiring a workspace from the picker retires the descendants it hides.
fn retirement_names(choice: &WorkspaceChoice) -> Vec<String> {
    match choice {
        WorkspaceChoice::Create => Vec::new(),
        WorkspaceChoice::Existing {
            name, descendants, ..
        } => std::iter::once(name).chain(descendants).cloned().collect(),
    }
}

/// One choice per workspace that is not nested under another active
/// workspace. A missing or cyclic parent leaves a workspace at the top level.
fn workspace_choices(states: &[WorkspaceState]) -> Vec<WorkspaceChoice> {
    let roots = states
        .iter()
        .map(|state| {
            if !state.exists {
                return state.name.as_str();
            }
            match workspace::ancestry(states, &state.name) {
                Ancestry::Linked(ancestors) | Ancestry::MissingParent { ancestors, .. } => {
                    ancestors.last().map_or(&state.name, |root| &root.name)
                }
                Ancestry::Cycle(_) => &state.name,
            }
        })
        .collect::<Vec<_>>();
    let name_width = states
        .iter()
        .zip(&roots)
        .filter(|(state, root)| state.name == **root)
        .map(|(state, _)| state.name.chars().count())
        .max()
        .unwrap_or(0);

    states
        .iter()
        .zip(&roots)
        .filter(|(state, root)| state.name == **root)
        .map(|(state, _)| {
            let descendants = states
                .iter()
                .zip(&roots)
                .filter(|(other, root)| **root == state.name && other.name != state.name)
                .map(|(other, _)| other)
                .collect::<Vec<_>>();
            workspace_choice(state, &descendants, name_width)
        })
        .collect()
}

fn workspace_choice(
    state: &WorkspaceState,
    descendants: &[&WorkspaceState],
    name_width: usize,
) -> WorkspaceChoice {
    let repositories = state
        .members
        .iter()
        .filter(|member| member.exists && member.registered)
        .map(|member| member.id.to_string())
        .collect::<Vec<_>>();
    let summary = if repositories.is_empty() {
        "workspace root only".to_owned()
    } else {
        repositories.join(" · ")
    };

    // Opening a workspace opens its descendants, so their issues block it too.
    let issue = workspace_issue(state).or_else(|| {
        descendants.iter().find_map(|descendant| {
            workspace_issue(descendant).map(|issue| format!("{}: {issue}", descendant.name))
        })
    });

    WorkspaceChoice::Existing {
        name: state.name.clone(),
        name_width,
        summary,
        issue,
        descendants: descendants
            .iter()
            .map(|descendant| descendant.name.clone())
            .collect(),
    }
}

fn workspace_issue(state: &WorkspaceState) -> Option<String> {
    if !state.exists {
        return Some(
            state
                .inconsistencies
                .first()
                .cloned()
                .unwrap_or_else(|| "workspace directory is missing".to_owned()),
        );
    }
    state.members.iter().find_map(|member| {
        member
            .inconsistencies
            .first()
            .map(|issue| format!("{}: {issue}", member.id))
    })
}

fn input_error_message(error: &AppError) -> String {
    let message = error.to_string();
    message
        .strip_prefix("invalid input: ")
        .unwrap_or(&message)
        .to_owned()
}

enum PromptAnswer<T> {
    Value(T),
    Cancelled,
    Interrupted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SelectionAction {
    Open(usize),
    Manage(Vec<usize>),
}

fn select<T, F, S>(
    terminal: &mut PromptTerminal,
    message: &str,
    choices: &[T],
    checked: &mut HashSet<usize>,
    help: &str,
    answer: F,
    search_text: S,
) -> Result<PromptAnswer<SelectionAction>>
where
    T: fmt::Display,
    F: for<'a> Fn(&'a T) -> &'a str,
    S: Fn(&T) -> String,
{
    let mut query = String::new();
    let mut selection = 0;

    loop {
        let filtered = filtered_indices(choices, &query, &search_text);
        if selection >= filtered.len() {
            selection = filtered.len().saturating_sub(1);
        }
        terminal
            .render(&selection_lines(
                message, choices, &filtered, checked, selection, &query, help,
            ))
            .map_err(AppError::Prompt)?;

        match read_event().map_err(AppError::Prompt)? {
            Event::Key(key) if interrupted(key) => {
                terminal
                    .finish(message, "<interrupted>", LineStyle::Dim)
                    .map_err(AppError::Prompt)?;
                return Ok(PromptAnswer::Interrupted);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Esc, ..
            }) => {
                terminal
                    .finish(message, "<stayed put>", LineStyle::Dim)
                    .map_err(AppError::Prompt)?;
                return Ok(PromptAnswer::Cancelled);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) if !filtered.is_empty() => {
                let index = filtered[selection];
                terminal
                    .finish(message, answer(&choices[index]), LineStyle::Green)
                    .map_err(AppError::Prompt)?;
                return Ok(PromptAnswer::Value(SelectionAction::Open(index)));
            }
            Event::Key(key) if workspace_action_requested(key) && !filtered.is_empty() => {
                let indices = selected_action_indices(checked, &filtered, selection);
                if !indices.is_empty() {
                    return Ok(PromptAnswer::Value(SelectionAction::Manage(indices)));
                }
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char(' '),
                ..
            }) if !filtered.is_empty() && filtered[selection] > 0 => {
                toggle_workspace_selection(
                    checked,
                    filtered[selection],
                    choices.len(),
                    &mut selection,
                    &mut query,
                );
            }
            Event::Key(KeyEvent {
                code: KeyCode::Up, ..
            }) if !filtered.is_empty() => {
                selection = selection.saturating_sub(1);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Down,
                ..
            }) if !filtered.is_empty() => {
                selection = (selection + 1).min(filtered.len() - 1);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Backspace,
                ..
            }) => {
                query.pop();
                selection = 0;
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char(character),
                modifiers,
                ..
            }) if text_modifiers(modifiers) => {
                query.push(character);
                selection = 0;
            }
            Event::Paste(value) => {
                append_printable(&mut query, &value);
                selection = 0;
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum RetirementChoice {
    Archive,
    ForceArchive,
    Delete,
    ForceDelete,
}

impl RetirementChoice {
    const ALL: [Self; 4] = [
        Self::Archive,
        Self::ForceArchive,
        Self::Delete,
        Self::ForceDelete,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Archive => "Archive",
            Self::ForceArchive => "Force archive",
            Self::Delete => "Delete",
            Self::ForceDelete => "Force delete",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Archive => "preserve local files; refuse dirty worktrees",
            Self::ForceArchive => "preserve local files; discard dirty worktree changes",
            Self::Delete => "permanently delete local files; refuse dirty worktrees",
            Self::ForceDelete => "permanently delete local files and dirty worktree changes",
        }
    }

    fn action(self, workspaces: Vec<String>) -> Action {
        match self {
            Self::Archive => Action::Archive {
                workspaces,
                force: false,
            },
            Self::ForceArchive => Action::Archive {
                workspaces,
                force: true,
            },
            Self::Delete => Action::Delete {
                workspaces,
                force: false,
            },
            Self::ForceDelete => Action::Delete {
                workspaces,
                force: true,
            },
        }
    }
}

fn prompt_for_retirement(
    terminal: &mut PromptTerminal,
    workspaces: &[String],
) -> Result<PromptAnswer<Action>> {
    let mut selection = 0;
    loop {
        let mut lines = vec![DisplayLine::new(
            format!(
                "◆ What should Forest do with {}?",
                workspace_summary(workspaces)
            ),
            LineStyle::Plain,
        )];
        lines.extend(
            RetirementChoice::ALL
                .iter()
                .enumerate()
                .map(|(index, choice)| {
                    DisplayLine::new(
                        format!(
                            "{} {:<13}  {}",
                            if index == selection { "›" } else { " " },
                            choice.label(),
                            choice.description()
                        ),
                        if index == selection {
                            LineStyle::CyanBold
                        } else {
                            LineStyle::Plain
                        },
                    )
                }),
        );
        lines.push(DisplayLine::new(
            "[↑↓ to move · enter to choose · esc to go back]",
            LineStyle::Dim,
        ));
        terminal.render(&lines).map_err(AppError::Prompt)?;

        match read_event().map_err(AppError::Prompt)? {
            Event::Key(key) if interrupted(key) => {
                terminal
                    .finish("Workspace action", "<interrupted>", LineStyle::Dim)
                    .map_err(AppError::Prompt)?;
                return Ok(PromptAnswer::Interrupted);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Esc, ..
            }) => return Ok(PromptAnswer::Cancelled),
            Event::Key(KeyEvent {
                code: KeyCode::Up, ..
            }) => selection = selection.saturating_sub(1),
            Event::Key(KeyEvent {
                code: KeyCode::Down,
                ..
            }) => selection = (selection + 1).min(RetirementChoice::ALL.len() - 1),
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                let choice = RetirementChoice::ALL[selection];
                return confirm_retirement(terminal, workspaces, choice);
            }
            _ => {}
        }
    }
}

fn confirm_retirement(
    terminal: &mut PromptTerminal,
    workspaces: &[String],
    choice: RetirementChoice,
) -> Result<PromptAnswer<Action>> {
    let message = format!(
        "{} {}?",
        choice.label(),
        if workspaces.len() == 1 {
            format!("workspace {:?}", workspaces[0])
        } else {
            format!("{} workspaces", workspaces.len())
        }
    );
    loop {
        terminal
            .render(&[
                DisplayLine::new(format!("◆ {message}"), LineStyle::Plain),
                DisplayLine::new(format!("  {}", workspaces.join(" · ")), LineStyle::Plain),
                DisplayLine::new(format!("  {}.", choice.description()), LineStyle::Dim),
                DisplayLine::new("  Git branches are always preserved.", LineStyle::Dim),
                DisplayLine::new("[y to confirm · n/esc to go back]", LineStyle::Dim),
            ])
            .map_err(AppError::Prompt)?;

        match read_event().map_err(AppError::Prompt)? {
            Event::Key(key) if interrupted(key) => {
                terminal
                    .finish(&message, "<interrupted>", LineStyle::Dim)
                    .map_err(AppError::Prompt)?;
                return Ok(PromptAnswer::Interrupted);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('y' | 'Y'),
                modifiers,
                ..
            }) if text_modifiers(modifiers) => {
                terminal
                    .finish(
                        choice.label(),
                        &workspace_summary(workspaces),
                        LineStyle::Green,
                    )
                    .map_err(AppError::Prompt)?;
                return Ok(PromptAnswer::Value(choice.action(workspaces.to_vec())));
            }
            Event::Key(KeyEvent {
                code: KeyCode::Esc | KeyCode::Enter | KeyCode::Char('n' | 'N'),
                ..
            }) => return Ok(PromptAnswer::Cancelled),
            _ => {}
        }
    }
}

fn workspace_summary(workspaces: &[String]) -> String {
    if workspaces.len() == 1 {
        format!("workspace {:?}", workspaces[0])
    } else {
        format!("{} workspaces", workspaces.len())
    }
}

fn text<F>(
    terminal: &mut PromptTerminal,
    message: &str,
    placeholder: &str,
    help: &str,
    validate: F,
) -> Result<PromptAnswer<String>>
where
    F: Fn(&str) -> Option<String>,
{
    let mut value = String::new();
    let mut error = None;

    loop {
        let shown = if value.is_empty() {
            format!("◆ {message}  {placeholder}")
        } else {
            format!("◆ {message}  {value}")
        };
        let mut lines = vec![DisplayLine::new(
            shown,
            if value.is_empty() {
                LineStyle::Dim
            } else {
                LineStyle::Plain
            },
        )];
        if let Some(error) = &error {
            lines.push(DisplayLine::new(format!("! {error}"), LineStyle::Red));
        }
        lines.push(DisplayLine::new(format!("[{help}]"), LineStyle::Dim));
        terminal.render(&lines).map_err(AppError::Prompt)?;

        match read_event().map_err(AppError::Prompt)? {
            Event::Key(key) if interrupted(key) => {
                terminal
                    .finish(message, "<interrupted>", LineStyle::Dim)
                    .map_err(AppError::Prompt)?;
                return Ok(PromptAnswer::Interrupted);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Esc, ..
            }) => {
                terminal
                    .finish(message, "<stayed put>", LineStyle::Dim)
                    .map_err(AppError::Prompt)?;
                return Ok(PromptAnswer::Cancelled);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => match validate(&value) {
                Some(message) => error = Some(message),
                None => {
                    terminal
                        .finish(message, &value, LineStyle::Green)
                        .map_err(AppError::Prompt)?;
                    return Ok(PromptAnswer::Value(value));
                }
            },
            Event::Key(KeyEvent {
                code: KeyCode::Backspace,
                ..
            }) => {
                value.pop();
                error = None;
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char(character),
                modifiers,
                ..
            }) if text_modifiers(modifiers) => {
                value.push(character);
                error = None;
            }
            Event::Paste(pasted) => {
                append_printable(&mut value, &pasted);
                error = None;
            }
            _ => {}
        }
    }
}

fn multi_select(
    terminal: &mut PromptTerminal,
    message: &str,
    choices: &[String],
    help: &str,
    all_selected: bool,
) -> Result<PromptAnswer<Vec<String>>> {
    let mut checked = if all_selected {
        (0..choices.len()).collect::<HashSet<_>>()
    } else {
        HashSet::new()
    };
    let mut query = String::new();
    let mut selection = 0;
    let mut error = None;

    loop {
        let filtered = filtered_indices(choices, &query, ToString::to_string);
        if selection >= filtered.len() {
            selection = filtered.len().saturating_sub(1);
        }
        terminal
            .render(&multi_select_lines(
                message, choices, &filtered, &checked, selection, &query, help, error,
            ))
            .map_err(AppError::Prompt)?;

        match read_event().map_err(AppError::Prompt)? {
            Event::Key(key) if interrupted(key) => {
                terminal
                    .finish(message, "<interrupted>", LineStyle::Dim)
                    .map_err(AppError::Prompt)?;
                return Ok(PromptAnswer::Interrupted);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Esc, ..
            }) => {
                terminal
                    .finish(message, "<stayed put>", LineStyle::Dim)
                    .map_err(AppError::Prompt)?;
                return Ok(PromptAnswer::Cancelled);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) if checked.is_empty() => {
                error = Some("Pick at least one repository.");
            }
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                let selected = choices
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| checked.contains(index))
                    .map(|(_, choice)| choice.clone())
                    .collect::<Vec<_>>();
                terminal
                    .finish(message, &selected.join(" · "), LineStyle::Green)
                    .map_err(AppError::Prompt)?;
                return Ok(PromptAnswer::Value(selected));
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char(' '),
                ..
            }) if !filtered.is_empty() => {
                toggle_current_selection(&mut checked, &filtered, &mut selection, &mut query);
                error = None;
            }
            Event::Key(KeyEvent {
                code: KeyCode::Right,
                ..
            }) => {
                select_filtered_choices(&mut checked, &filtered, &mut selection, &mut query);
                error = None;
            }
            Event::Key(KeyEvent {
                code: KeyCode::Left,
                ..
            }) => {
                checked.clear();
                reset_filter(&mut query, &mut selection);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Up, ..
            }) if !filtered.is_empty() => {
                selection = selection.saturating_sub(1);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Down,
                ..
            }) if !filtered.is_empty() => {
                selection = (selection + 1).min(filtered.len() - 1);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Backspace,
                ..
            }) => {
                query.pop();
                selection = 0;
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char(character),
                modifiers,
                ..
            }) if text_modifiers(modifiers) => {
                query.push(character);
                selection = 0;
            }
            Event::Paste(value) => {
                append_printable(&mut query, &value);
                selection = 0;
            }
            _ => {}
        }
    }
}

fn toggle_current_selection(
    checked: &mut HashSet<usize>,
    filtered: &[usize],
    selection: &mut usize,
    query: &mut String,
) {
    let index = filtered[*selection];
    if !checked.insert(index) {
        checked.remove(&index);
    }
    reset_filter(query, selection);
}

fn select_filtered_choices(
    checked: &mut HashSet<usize>,
    filtered: &[usize],
    selection: &mut usize,
    query: &mut String,
) {
    checked.clear();
    checked.extend(filtered.iter().copied());
    reset_filter(query, selection);
}

fn reset_filter(query: &mut String, selection: &mut usize) {
    query.clear();
    *selection = 0;
}

fn selection_lines<T: fmt::Display>(
    message: &str,
    choices: &[T],
    filtered: &[usize],
    checked: &HashSet<usize>,
    selection: usize,
    query: &str,
    help: &str,
) -> Vec<DisplayLine> {
    let mut lines = vec![DisplayLine::new(
        prompt_line(message, query),
        LineStyle::Plain,
    )];
    if filtered.is_empty() {
        lines.push(DisplayLine::new("  No matches", LineStyle::Red));
    } else {
        let start = page_start(selection);
        for (position, index) in filtered.iter().enumerate().skip(start).take(PAGE_SIZE) {
            let selected = position == selection;
            lines.push(DisplayLine::new(
                format!(
                    "{} {} {}",
                    if selected { "›" } else { " " },
                    if *index == 0 {
                        " "
                    } else if checked.contains(index) {
                        "●"
                    } else {
                        "○"
                    },
                    choices[*index]
                ),
                if selected {
                    LineStyle::CyanBold
                } else {
                    LineStyle::Plain
                },
            ));
        }
    }
    lines.push(DisplayLine::new(format!("[{help}]"), LineStyle::Dim));
    lines
}

#[allow(clippy::too_many_arguments)]
fn multi_select_lines(
    message: &str,
    choices: &[String],
    filtered: &[usize],
    checked: &HashSet<usize>,
    selection: usize,
    query: &str,
    help: &str,
    error: Option<&str>,
) -> Vec<DisplayLine> {
    let mut lines = vec![DisplayLine::new(
        prompt_line(message, query),
        LineStyle::Plain,
    )];
    if filtered.is_empty() {
        lines.push(DisplayLine::new("  No matches", LineStyle::Red));
    } else {
        let start = page_start(selection);
        for (position, index) in filtered.iter().enumerate().skip(start).take(PAGE_SIZE) {
            let selected = position == selection;
            lines.push(DisplayLine::new(
                format!(
                    "{} {} {}",
                    if selected { "›" } else { " " },
                    if checked.contains(index) {
                        "●"
                    } else {
                        "○"
                    },
                    choices[*index]
                ),
                if selected {
                    LineStyle::CyanBold
                } else {
                    LineStyle::Plain
                },
            ));
        }
    }
    if let Some(error) = error {
        lines.push(DisplayLine::new(format!("! {error}"), LineStyle::Red));
    }
    lines.push(DisplayLine::new(format!("[{help}]"), LineStyle::Dim));
    lines
}

fn filtered_indices<T>(
    choices: &[T],
    query: &str,
    search_text: impl Fn(&T) -> String,
) -> Vec<usize> {
    if query.is_empty() {
        return (0..choices.len()).collect();
    }

    let matcher = SkimMatcherV2::default();
    let mut matches = choices
        .iter()
        .enumerate()
        .filter_map(|(index, choice)| {
            matcher
                .fuzzy_match(&search_text(choice), query)
                .map(|score| (index, score))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    matches.into_iter().map(|(index, _)| index).collect()
}

fn prompt_line(message: &str, query: &str) -> String {
    if query.is_empty() {
        format!("◆ {message}")
    } else {
        format!("◆ {message}  {query}")
    }
}

fn page_start(selection: usize) -> usize {
    selection / PAGE_SIZE * PAGE_SIZE
}

fn interrupted(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn workspace_action_requested(key: KeyEvent) -> bool {
    key.code == KeyCode::Delete
        || (key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL))
}

fn toggle_workspace_selection(
    checked: &mut HashSet<usize>,
    index: usize,
    choice_count: usize,
    selection: &mut usize,
    query: &mut String,
) {
    if !checked.insert(index) {
        checked.remove(&index);
    }
    query.clear();
    *selection = (index + 1).min(choice_count.saturating_sub(1));
}

fn selected_action_indices(
    checked: &HashSet<usize>,
    filtered: &[usize],
    selection: usize,
) -> Vec<usize> {
    if checked.is_empty() {
        let index = filtered[selection];
        return (index > 0).then_some(vec![index]).unwrap_or_default();
    }

    let mut indices = checked.iter().copied().collect::<Vec<_>>();
    indices.sort_unstable();
    indices
}

fn text_modifiers(modifiers: KeyModifiers) -> bool {
    !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

fn append_printable(destination: &mut String, value: &str) {
    destination.extend(value.chars().filter(|character| !character.is_control()));
}

fn read_event() -> io::Result<Event> {
    loop {
        let event = event::read()?;
        match event {
            Event::Key(key)
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat =>
            {
                return Ok(Event::Key(key));
            }
            Event::Paste(_) | Event::Resize(_, _) => return Ok(event),
            _ => {}
        }
    }
}

fn write_banner() -> Result<()> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    if std::env::var_os("NO_COLOR").is_some() {
        writeln!(writer, "\n  🌲 Forest").map_err(AppError::WriteOutput)?;
        writeln!(writer, "  Pick a workspace. We’ll get it ready.\n")
            .map_err(AppError::WriteOutput)?;
    } else {
        writeln!(writer, "\n  \x1b[1;32m🌲 Forest\x1b[0m").map_err(AppError::WriteOutput)?;
        writeln!(
            writer,
            "  \x1b[2mPick a workspace. We’ll get it ready.\x1b[0m\n"
        )
        .map_err(AppError::WriteOutput)?;
    }
    writer.flush().map_err(AppError::WriteOutput)
}

#[derive(Clone, Copy)]
enum LineStyle {
    Plain,
    Green,
    CyanBold,
    Dim,
    Red,
}

struct DisplayLine {
    text: String,
    style: LineStyle,
}

impl DisplayLine {
    fn new(text: impl Into<String>, style: LineStyle) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

struct PromptTerminal {
    stdout: io::Stdout,
    rendered_lines: u16,
    colored: bool,
}

impl PromptTerminal {
    fn new() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, event::EnableBracketedPaste, cursor::Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error);
        }
        Ok(Self {
            stdout,
            rendered_lines: 0,
            colored: std::env::var_os("NO_COLOR").is_none(),
        })
    }

    fn render(&mut self, lines: &[DisplayLine]) -> io::Result<()> {
        self.clear_frame()?;
        let width = terminal::size()
            .ok()
            .map(|(width, _)| width)
            .filter(|width| *width > 0)
            .unwrap_or(80);
        let max_chars = usize::from(width.saturating_sub(1));
        for line in lines {
            let text = truncate(&line.text, max_chars);
            let text = self.style(text, line.style);
            queue!(self.stdout, Print(text), Print("\r\n"))?;
        }
        self.stdout.flush()?;
        self.rendered_lines = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        Ok(())
    }

    fn finish(&mut self, message: &str, answer: &str, style: LineStyle) -> io::Result<()> {
        self.clear_frame()?;
        let text = self.style(format!("✓ {message}  {answer}"), style);
        queue!(self.stdout, Print(text), Print("\r\n"))?;
        self.stdout.flush()
    }

    fn clear_frame(&mut self) -> io::Result<()> {
        if self.rendered_lines > 0 {
            queue!(
                self.stdout,
                cursor::MoveUp(self.rendered_lines),
                cursor::MoveToColumn(0),
                terminal::Clear(ClearType::FromCursorDown)
            )?;
            self.rendered_lines = 0;
        }
        Ok(())
    }

    fn style(&self, text: String, style: LineStyle) -> String {
        if !self.colored || matches!(style, LineStyle::Plain) {
            return text;
        }
        let code = match style {
            LineStyle::Plain => unreachable!(),
            LineStyle::Green => "32",
            LineStyle::CyanBold => "1;36",
            LineStyle::Dim => "2",
            LineStyle::Red => "31",
        };
        format!("\x1b[{code}m{text}\x1b[0m")
    }
}

impl Drop for PromptTerminal {
    fn drop(&mut self) {
        let _ = self.clear_frame();
        let _ = execute!(self.stdout, event::DisableBracketedPaste, cursor::Show);
        let _ = self.stdout.flush();
        let _ = terminal::disable_raw_mode();
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut characters = value.chars();
    let truncated = characters.by_ref().take(max_chars).collect::<String>();
    if characters.next().is_some() && max_chars > 0 {
        let mut truncated = truncated
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect::<String>();
        truncated.push('…');
        truncated
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_choice_has_a_clear_call_to_action() {
        assert_eq!(
            WorkspaceChoice::Create.to_string(),
            "+  Create a new workspace"
        );
        assert_eq!(WorkspaceChoice::Create.answer(), "Create a new workspace");
    }

    #[test]
    fn workspace_choices_align_details_and_mark_issues() {
        let healthy = WorkspaceChoice::Existing {
            name: "short".to_owned(),
            name_width: 8,
            summary: "api · web".to_owned(),
            issue: None,
            descendants: Vec::new(),
        };
        let unhealthy = WorkspaceChoice::Existing {
            name: "broken".to_owned(),
            name_width: 8,
            summary: "api".to_owned(),
            issue: Some("api is missing".to_owned()),
            descendants: Vec::new(),
        };

        assert_eq!(healthy.to_string(), "short     api · web");
        assert_eq!(unhealthy.to_string(), "broken    api  ! needs attention");
    }

    fn workspace(name: &str, parent: Option<&str>, exists: bool) -> WorkspaceState {
        WorkspaceState {
            name: name.to_owned(),
            path: std::path::PathBuf::from(name),
            exists,
            metadata: workspace::WorkspaceMetadata {
                symbol: None,
                parent: parent.map(str::to_owned),
            },
            members: Vec::new(),
            workspace_entries: Vec::new(),
            inconsistencies: Vec::new(),
        }
    }

    #[test]
    fn picker_lists_only_top_level_workspaces() {
        let choices = workspace_choices(&[
            workspace("billing", None, true),
            workspace("billing-db", Some("billing-ui"), true),
            workspace("billing-ui", Some("billing"), true),
            workspace("looped", Some("looped-too"), true),
            workspace("looped-too", Some("looped"), true),
            workspace("orphan", Some("retired"), true),
            workspace("search", None, true),
        ]);

        assert_eq!(
            choices.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [
                "billing     workspace root only  +2 children",
                "looped      workspace root only",
                "looped-too  workspace root only",
                "orphan      workspace root only",
                "search      workspace root only",
            ]
        );
    }

    #[test]
    fn hidden_descendants_are_searchable_and_retired_with_their_root() {
        let choices = workspace_choices(&[
            workspace("billing", None, true),
            workspace("checkout-flow", Some("billing"), true),
            workspace("search", None, true),
        ]);

        assert_eq!(
            filtered_indices(&choices, "checkout", WorkspaceChoice::search_text),
            [0]
        );
        assert_eq!(retirement_names(&choices[0]), ["billing", "checkout-flow"]);
        assert_eq!(retirement_names(&choices[1]), ["search"]);
    }

    #[test]
    fn a_broken_descendant_marks_its_root() {
        let mut broken = workspace("billing-ui", Some("billing"), true);
        broken.members.push(workspace::MemberState {
            id: CheckoutId::primary("web"),
            canonical_path: std::path::PathBuf::from("web"),
            path: std::path::PathBuf::from("billing-ui/web"),
            exists: true,
            registered: true,
            metadata: None,
            unexpected_worktree_paths: Vec::new(),
            inconsistencies: vec!["branch is checked out elsewhere".to_owned()],
        });
        let choices = workspace_choices(&[
            workspace("billing", None, true),
            broken,
            workspace("gone", Some("billing"), false),
        ]);

        // A missing workspace is not an active child, so it keeps its own row.
        assert_eq!(choices.len(), 2);
        assert!(matches!(
            &choices[0],
            WorkspaceChoice::Existing { issue: Some(issue), descendants, .. }
                if issue == "billing-ui: web: branch is checked out elsewhere"
                    && descendants == &["billing-ui"]
        ));
        assert!(matches!(
            &choices[1],
            WorkspaceChoice::Existing { name, .. } if name == "gone"
        ));
    }

    #[test]
    fn workspace_action_shortcuts_require_an_explicit_action_key() {
        assert!(workspace_action_requested(KeyEvent::new(
            KeyCode::Delete,
            KeyModifiers::NONE
        )));
        assert!(workspace_action_requested(KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::CONTROL
        )));
        assert!(!workspace_action_requested(KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE
        )));
        assert!(!workspace_action_requested(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn workspace_picker_marks_checked_existing_workspaces() {
        let lines = selection_lines(
            "Pick",
            &["Create", "one", "two"],
            &[0, 1, 2],
            &HashSet::from([2]),
            0,
            "",
            "help",
        );

        assert_eq!(lines[1].text, "›   Create");
        assert_eq!(lines[2].text, "  ○ one");
        assert_eq!(lines[3].text, "  ● two");
    }

    #[test]
    fn selecting_a_workspace_clears_search_and_advances() {
        let mut checked = HashSet::new();
        let mut selection = 0;
        let mut query = "one".to_owned();

        toggle_workspace_selection(&mut checked, 1, 4, &mut selection, &mut query);

        assert_eq!(checked, HashSet::from([1]));
        assert_eq!(selection, 2);
        assert!(query.is_empty());
    }

    #[test]
    fn workspace_actions_use_all_checked_workspaces_or_the_highlighted_fallback() {
        assert_eq!(
            selected_action_indices(&HashSet::from([3, 1]), &[0, 1, 2, 3], 0),
            [1, 3]
        );
        assert_eq!(selected_action_indices(&HashSet::new(), &[2], 0), [2]);
        assert!(selected_action_indices(&HashSet::new(), &[0], 0).is_empty());
    }

    #[test]
    fn retirement_choices_keep_force_explicit() {
        for (choice, archive, force) in [
            (RetirementChoice::Archive, true, false),
            (RetirementChoice::ForceArchive, true, true),
            (RetirementChoice::Delete, false, false),
            (RetirementChoice::ForceDelete, false, true),
        ] {
            match choice.action(vec!["topic".to_owned(), "other".to_owned()]) {
                Action::Archive {
                    workspaces,
                    force: actual_force,
                } => {
                    assert!(archive);
                    assert_eq!(workspaces, ["topic", "other"]);
                    assert_eq!(actual_force, force);
                }
                Action::Delete {
                    workspaces,
                    force: actual_force,
                } => {
                    assert!(!archive);
                    assert_eq!(workspaces, ["topic", "other"]);
                    assert_eq!(actual_force, force);
                }
                Action::Attach { .. } | Action::Create { .. } => {
                    panic!("retirement choice produced a non-retirement action")
                }
            }
        }
    }

    #[test]
    fn strips_application_prefix_from_validation_errors() {
        let error = AppError::InvalidInput("workspace name must not be empty".to_owned());
        assert_eq!(
            input_error_message(&error),
            "workspace name must not be empty"
        );
    }

    #[test]
    fn selecting_all_only_checks_filtered_choices() {
        let mut checked = HashSet::from([0]);
        let mut selection = 0;
        let mut query = "beta".to_owned();

        select_filtered_choices(&mut checked, &[1], &mut selection, &mut query);

        assert_eq!(checked, HashSet::from([1]));
        assert_eq!(selection, 0);
        assert!(query.is_empty());
    }

    #[test]
    fn toggling_a_filtered_choice_resets_the_filter() {
        let mut checked = HashSet::new();
        let mut selection = 0;
        let mut query = "beta".to_owned();

        toggle_current_selection(&mut checked, &[1], &mut selection, &mut query);

        assert_eq!(checked, HashSet::from([1]));
        assert_eq!(selection, 0);
        assert!(query.is_empty());
    }
}
