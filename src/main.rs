mod cli;
mod commands;
mod config;
mod domain;
mod error;
mod git;
mod herdr;
mod launcher;
mod output;
mod workspace;

use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use crate::cli::{AttachArgs, Cli, Command, CreateArgs, OutputArgs};
use crate::config::Config;
use crate::error::{AppError, Result};
use crate::git::Git;
use crate::herdr::Herdr;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let exit_code = error.exit_code() as u8;
            if exit_code != 0 && std::env::args_os().any(|argument| argument == "--json") {
                let message = error.to_string();
                let _ = output::render_error_message(message.trim(), exit_code);
            } else {
                let _ = error.print();
            }
            return ExitCode::from(exit_code);
        }
    };
    if cli.command.is_none() && !launcher::is_interactive_terminal() {
        let mut command = Cli::command();
        if let Err(error) = command.print_help() {
            eprintln!("error: could not write help: {error}");
            return ExitCode::from(1);
        }
        return ExitCode::from(2);
    }

    match run(&cli) {
        Ok(exit_code) => ExitCode::from(exit_code),
        Err(error) => {
            if cli.json() {
                if output::render_error(&error).is_err() {
                    return ExitCode::from(error.exit_code());
                }
            } else {
                eprintln!("error: {error}");
            }
            ExitCode::from(error.exit_code())
        }
    }
}

fn run(cli: &Cli) -> Result<u8> {
    let interactive = cli.command.is_none() || matches!(cli.command, Some(Command::Open));
    if interactive && !launcher::is_interactive_terminal() {
        return Err(AppError::InvalidInput(
            "the workspace launcher requires an interactive terminal".to_owned(),
        ));
    }

    let config = Config::load(cli.config.as_deref())?;
    match cli.command.as_ref() {
        None | Some(Command::Open) => run_launcher(&config),
        Some(command) => execute(command, &config),
    }
}

fn run_launcher(config: &Config) -> Result<u8> {
    match launcher::prompt(config, &Git)? {
        launcher::Outcome::Cancelled => Ok(0),
        launcher::Outcome::Interrupted => Ok(130),
        launcher::Outcome::Action(launcher::Action::Attach { workspace, issue }) => {
            if let Some(issue) = issue {
                return Err(AppError::Operational(format!(
                    "workspace {workspace:?} needs attention before it can be opened: {issue}; run `git forest list` for details"
                )));
            }
            execute(
                &Command::Attach(AttachArgs {
                    workspace,
                    output: OutputArgs { json: false },
                }),
                config,
            )
        }
        launcher::Outcome::Action(launcher::Action::Create {
            workspace,
            repositories,
        }) => {
            let created = execute(
                &Command::Create(CreateArgs {
                    workspace: workspace.clone(),
                    repositories,
                    bases: Vec::new(),
                    branches: Vec::new(),
                    output: OutputArgs { json: false },
                }),
                config,
            )?;
            if created != 0 {
                return Ok(created);
            }

            output::render_blank_line()?;
            execute(
                &Command::Attach(AttachArgs {
                    workspace: workspace.clone(),
                    output: OutputArgs { json: false },
                }),
                config,
            )
            .map_err(|error| {
                AppError::Operational(format!(
                    "workspace {workspace:?} was created, but it could not be opened in Herdr: {error}; retry with `git forest attach {workspace}`"
                ))
            })
        }
    }
}

fn execute(command: &Command, config: &Config) -> Result<u8> {
    let outcome = commands::run(command, config, &Git, &Herdr)?;
    output::render(&outcome.report, command.json())?;
    Ok(outcome.exit_code)
}
