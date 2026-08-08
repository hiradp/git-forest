mod cli;
mod commands;
mod config;
mod domain;
mod error;
mod git;
mod herdr;
mod output;
mod workspace;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::Cli;
use crate::config::Config;
use crate::error::Result;
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
    let json = cli.command.json();
    let config = Config::load(cli.config.as_deref())?;
    let outcome = commands::run(cli, &config, &Git, &Herdr)?;
    output::render(&outcome.report, json)?;
    Ok(outcome.exit_code)
}
