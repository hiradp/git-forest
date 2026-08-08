mod create;
mod list;
mod path;
mod remove;
mod repos;
mod status;

use crate::cli::{Cli, Command};
use crate::config::Config;
use crate::domain::{CommandOutcome, CommandReport};
use crate::error::Result;
use crate::git::Git;

pub fn run(cli: &Cli, config: &Config, git: &Git) -> Result<CommandOutcome> {
    match &cli.command {
        Command::Repos(_) => repos::run(config, git)
            .map(CommandReport::Repositories)
            .map(CommandOutcome::success),
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
        Command::Remove(arguments) => remove::run(config, git, arguments),
    }
}
