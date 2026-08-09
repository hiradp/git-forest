mod attach;
mod create;
mod fetch;
mod list;
mod path;
mod remove;
mod repos;
mod setup;
mod status;

use crate::cli::Command;
use crate::config::Config;
use crate::domain::{CommandOutcome, CommandReport};
use crate::error::Result;
use crate::git::Git;
use crate::herdr::Herdr;

pub fn run(command: &Command, config: &Config, git: &Git, herdr: &Herdr) -> Result<CommandOutcome> {
    match command {
        Command::Open => unreachable!("the interactive launcher is dispatched by main"),
        Command::Setup(_) => setup::run(config, git),
        Command::Repos(_) => repos::run(config, git)
            .map(CommandReport::Repositories)
            .map(CommandOutcome::success),
        Command::Fetch(arguments) => fetch::run(config, git, arguments),
        Command::Create(arguments) => create::run(config, git, arguments, false),
        Command::Add(arguments) => create::run(config, git, arguments, true),
        Command::List(_) => list::run(config, git)
            .map(CommandReport::WorkspacesList)
            .map(CommandOutcome::success),
        Command::Status(arguments) => status::run(config, git, arguments)
            .map(CommandReport::WorkspacesStatus)
            .map(CommandOutcome::success),
        Command::Path(arguments) => path::run(config, &arguments.workspace)
            .map(CommandReport::WorkspacePath)
            .map(CommandOutcome::success),
        Command::Attach(arguments) => attach::run(config, git, herdr, arguments),
        Command::Remove(arguments) => remove::run(config, git, arguments),
    }
}
