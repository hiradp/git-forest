use std::collections::HashSet;
use std::fmt;
use std::io::{self, IsTerminal, Write};

use inquire::error::{InquireError, InquireResult};
use inquire::list_option::ListOption;
use inquire::ui::{Attributes, Color, RenderConfig, StyleSheet, Styled};
use inquire::validator::Validation;
use inquire::{MultiSelect, Select, Text};

use crate::config::{self, Config};
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
        repositories: Vec<String>,
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
    io::stdin().is_terminal() && io::stdout().is_terminal() && io::stderr().is_terminal()
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

    let choice = match prompt_answer(
        Select::new("Where do you want to work?", choices)
            .with_help_message("type to search · ↑↓ to move · enter to open · esc to leave")
            .with_page_size(PAGE_SIZE)
            .with_formatter(&|answer| answer.value.answer().to_owned())
            .with_render_config(theme())
            .prompt_skippable(),
    )? {
        PromptAnswer::Value(choice) => choice,
        PromptAnswer::Cancelled => return Ok(Outcome::Cancelled),
        PromptAnswer::Interrupted => return Ok(Outcome::Interrupted),
    };

    match choice {
        WorkspaceChoice::Existing { name, issue, .. } => Ok(Outcome::Action(Action::Attach {
            workspace: name,
            issue,
        })),
        WorkspaceChoice::Create => prompt_for_workspace(config, existing_names),
    }
}

fn prompt_for_workspace(config: &Config, existing_names: HashSet<String>) -> Result<Outcome> {
    let workspace_root = config.workspaces_root.clone();
    let workspace = match prompt_answer(
        Text::new("Name your workspace")
            .with_placeholder("feature-name")
            .with_help_message("letters, numbers, dots, dashes, and underscores")
            .with_validator(move |input: &str| {
                let validation = match config::validate_workspace_name(input) {
                    Ok(())
                        if existing_names.contains(input)
                            || workspace_root.join(input).exists() =>
                    {
                        Validation::Invalid(
                            "That workspace already exists — pick another name.".into(),
                        )
                    }
                    Ok(()) => Validation::Valid,
                    Err(error) => Validation::Invalid(input_error_message(&error).into()),
                };
                Ok(validation)
            })
            .with_render_config(theme())
            .prompt_skippable(),
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
    let mut repository_prompt =
        MultiSelect::new("Which repositories are coming along?", repositories)
            .with_help_message("space to toggle · type to search · → all · enter to create")
            .with_page_size(PAGE_SIZE)
            .with_keep_filter(false)
            .with_validator(|selected: &[ListOption<&String>]| {
                Ok(if selected.is_empty() {
                    Validation::Invalid("Pick at least one repository.".into())
                } else {
                    Validation::Valid
                })
            })
            .with_formatter(&|selected| {
                selected
                    .iter()
                    .map(|repository| repository.value.as_str())
                    .collect::<Vec<_>>()
                    .join(" · ")
            })
            .with_render_config(theme());
    if config.repositories.len() == 1 {
        repository_prompt = repository_prompt.with_all_selected_by_default();
    }

    let repositories = match prompt_answer(repository_prompt.prompt_skippable())? {
        PromptAnswer::Value(repositories) => repositories,
        PromptAnswer::Cancelled => return Ok(Outcome::Cancelled),
        PromptAnswer::Interrupted => return Ok(Outcome::Interrupted),
    };

    Ok(Outcome::Action(Action::Create {
        workspace,
        repositories,
    }))
}

fn workspace_choice(state: &WorkspaceState, name_width: usize) -> WorkspaceChoice {
    let repositories = state
        .members
        .iter()
        .filter(|member| member.exists && member.registered)
        .map(|member| member.name.as_str())
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
                .map(|issue| format!("{}: {issue}", member.name))
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

fn prompt_answer<T>(result: InquireResult<Option<T>>) -> Result<PromptAnswer<T>> {
    match result {
        Ok(Some(value)) => Ok(PromptAnswer::Value(value)),
        Ok(None) | Err(InquireError::OperationCanceled) => Ok(PromptAnswer::Cancelled),
        Err(InquireError::OperationInterrupted) => Ok(PromptAnswer::Interrupted),
        Err(error) => Err(AppError::Prompt(error)),
    }
}

fn write_banner() -> Result<()> {
    let stderr = io::stderr();
    let mut writer = stderr.lock();
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

fn theme() -> RenderConfig<'static> {
    let colored = std::env::var_os("NO_COLOR").is_none();
    let mut theme = if colored {
        RenderConfig::default_colored()
    } else {
        RenderConfig::empty()
    };

    theme.prompt_prefix = Styled::new("◆");
    theme.answered_prompt_prefix = Styled::new("✓");
    theme.highlighted_option_prefix = Styled::new("›");
    theme.unhighlighted_option_prefix = Styled::new(" ");
    theme.scroll_up_prefix = Styled::new("↑");
    theme.scroll_down_prefix = Styled::new("↓");
    theme.selected_checkbox = Styled::new("●");
    theme.unselected_checkbox = Styled::new("○");
    theme.canceled_prompt_indicator = Styled::new("<stayed put>");
    theme.error_message.prefix = Styled::new("!");

    if colored {
        theme.prompt_prefix = theme.prompt_prefix.with_fg(Color::LightGreen);
        theme.answered_prompt_prefix = theme.answered_prompt_prefix.with_fg(Color::LightGreen);
        theme.highlighted_option_prefix = theme.highlighted_option_prefix.with_fg(Color::LightCyan);
        theme.selected_checkbox = theme.selected_checkbox.with_fg(Color::LightGreen);
        theme.unselected_checkbox = theme.unselected_checkbox.with_fg(Color::DarkGrey);
        theme.canceled_prompt_indicator = theme.canceled_prompt_indicator.with_fg(Color::DarkGrey);
        theme.error_message.prefix = theme.error_message.prefix.with_fg(Color::LightRed);
        theme.selected_option = Some(
            StyleSheet::new()
                .with_fg(Color::LightCyan)
                .with_attr(Attributes::BOLD),
        );
    }

    theme
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
}
