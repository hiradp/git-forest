mod archive;
mod attach;
mod clean;
mod create;
mod delete;
mod fetch;
mod list;
mod path;
mod remove;
mod rename;
mod repos;
mod setup;
mod status;
mod unarchive;
mod update;

use crate::cli::Command;
use crate::config::Config;
use crate::domain::{CommandOutcome, CommandReport};
use crate::error::Result;
use crate::git::Git;
use crate::herdr::Herdr;

pub fn run(command: &Command, config: &Config, git: &Git, herdr: &Herdr) -> Result<CommandOutcome> {
    match command {
        Command::Open | Command::Completions(_) => {
            unreachable!("interactive and completion commands are dispatched by main")
        }
        Command::Setup(_) => setup::run(config, git),
        Command::Repos(_) => repos::run(config, git)
            .map(CommandReport::Repositories)
            .map(CommandOutcome::success),
        Command::Fetch(arguments) => fetch::run(config, git, arguments),
        Command::Update(arguments) => update::run(config, git, arguments),
        Command::Create(arguments) => create::run_create(config, git, arguments),
        Command::Add(arguments) => create::run_add(config, git, arguments),
        Command::List(arguments) if arguments.archived => list::archived(config)
            .map(CommandReport::ArchivesList)
            .map(CommandOutcome::success),
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
        Command::Rename(arguments) => rename::run(config, git, arguments),
        Command::Archive(arguments) => archive::run(config, git, arguments),
        Command::Unarchive(arguments) => unarchive::run(config, git, arguments),
        Command::Delete(arguments) => delete::run(config, git, arguments),
        Command::Clean(_) => clean::run(config, git),
        Command::Remove(arguments) => remove::run(config, git, arguments),
    }
}
