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
use crate::workspace::{self, WorkspaceState};

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
    },
}

impl WorkspaceChoice {
    fn answer(&self) -> &str {
        match self {
            Self::Create => "Create a new workspace",
            Self::Existing { name, .. } => name,
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
            } => {
                write!(formatter, "{name:<name_width$}  {summary}")?;
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
    let name_width = states
        .iter()
        .map(|state| state.name.chars().count())
        .max()
        .unwrap_or(0);
    let mut choices = Vec::with_capacity(states.len() + 1);
    choices.push(WorkspaceChoice::Create);
    choices.extend(
        states
            .iter()
            .map(|state| workspace_choice(state, name_width)),
    );

    let mut terminal = PromptTerminal::new().map_err(AppError::Prompt)?;
    let choice = match select(
        &mut terminal,
        "Where do you want to work?",
        &choices,
        "type to search · ↑↓ to move · enter to open · esc to leave",
        |choice| choice.answer(),
    )? {
        PromptAnswer::Value(index) => choices[index].clone(),
        PromptAnswer::Cancelled => return Ok(Outcome::Cancelled),
        PromptAnswer::Interrupted => return Ok(Outcome::Interrupted),
    };

    match choice {
        WorkspaceChoice::Existing { name, issue, .. } => Ok(Outcome::Action(Action::Attach {
            workspace: name,
            issue,
        })),
        WorkspaceChoice::Create => prompt_for_workspace(config, existing_names, &mut terminal),
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

fn workspace_choice(state: &WorkspaceState, name_width: usize) -> WorkspaceChoice {
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

    let issue = if !state.exists {
        Some(
            state
                .inconsistencies
                .first()
                .cloned()
                .unwrap_or_else(|| "workspace directory is missing".to_owned()),
        )
    } else {
        state.members.iter().find_map(|member| {
            member
                .inconsistencies
                .first()
                .map(|issue| format!("{}: {issue}", member.id))
        })
    };

    WorkspaceChoice::Existing {
        name: state.name.clone(),
        name_width,
        summary,
        issue,
    }
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

fn select<T, F>(
    terminal: &mut PromptTerminal,
    message: &str,
    choices: &[T],
    help: &str,
    answer: F,
) -> Result<PromptAnswer<usize>>
where
    T: fmt::Display,
    F: for<'a> Fn(&'a T) -> &'a str,
{
    let mut query = String::new();
    let mut selection = 0;

    loop {
        let filtered = filtered_indices(choices, &query);
        if selection >= filtered.len() {
            selection = filtered.len().saturating_sub(1);
        }
        terminal
            .render(&selection_lines(
                message, choices, &filtered, selection, &query, help,
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
                return Ok(PromptAnswer::Value(index));
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
        let filtered = filtered_indices(choices, &query);
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
                format!("{} {}", if selected { "›" } else { " " }, choices[*index]),
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

fn filtered_indices<T: fmt::Display>(choices: &[T], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..choices.len()).collect();
    }

    let matcher = SkimMatcherV2::default();
    let mut matches = choices
        .iter()
        .enumerate()
        .filter_map(|(index, choice)| {
            matcher
                .fuzzy_match(&choice.to_string(), query)
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
        };
        let unhealthy = WorkspaceChoice::Existing {
            name: "broken".to_owned(),
            name_width: 8,
            summary: "api".to_owned(),
            issue: Some("api is missing".to_owned()),
        };

        assert_eq!(healthy.to_string(), "short     api · web");
        assert_eq!(unhealthy.to_string(), "broken    api  ! needs attention");
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
