mod archives;
mod cli;
mod commands;
mod completion;
mod config;
mod domain;
mod error;
mod git;
mod output;
mod workspace;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::config::Config;
use crate::error::Result;
use crate::git::Git;

fn main() -> ExitCode {
    completion::handle_environment();

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
    match run(&cli) {
        Ok(exit_code) => ExitCode::from(exit_code),
        Err(error) => {
            if cli.command.json() {
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
    if let Command::Completions(arguments) = &cli.command {
        completion::write_registration(arguments.shell)?;
        return Ok(0);
    }

    let config = Config::load(cli.config.as_deref())?;
    execute(&cli.command, &config)
}

fn execute(command: &Command, config: &Config) -> Result<u8> {
    let outcome = commands::run(command, config, &Git)?;
    output::render(&outcome.report, command.json())?;
    Ok(outcome.exit_code)
}
