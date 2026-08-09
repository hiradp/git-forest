use std::path::PathBuf;
use std::str::FromStr;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "git-forest", version, about)]
pub struct Cli {
    /// Use an explicit configuration file instead of discovery
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    pub fn json(&self) -> bool {
        self.command.as_ref().is_some_and(Command::json)
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Search, create, and open workspaces interactively
    Open,

    /// Ensure configured canonical repositories are cloned
    Setup(OutputArgs),

    /// List configured repositories
    Repos(OutputArgs),

    /// Fetch origin for configured repositories
    Fetch(FetchArgs),

    /// Create a workspace and its linked worktrees
    Create(CreateArgs),

    /// Add linked worktrees to an existing workspace
    Add(CreateArgs),

    /// List workspaces
    List(OutputArgs),

    /// Show workspace and worktree status
    Status(StatusArgs),

    /// Print a workspace path
    Path(PathArgs),

    /// Open a workspace in Herdr
    Attach(AttachArgs),

    /// Remove worktrees from a workspace
    Remove(RemoveArgs),
}

impl Command {
    pub fn json(&self) -> bool {
        match self {
            Self::Open => false,
            Self::Setup(args) | Self::Repos(args) | Self::List(args) => args.json,
            Self::Fetch(args) => args.output.json,
            Self::Create(args) | Self::Add(args) => args.output.json,
            Self::Status(args) => args.output.json,
            Self::Path(args) => args.output.json,
            Self::Attach(args) => args.output.json,
            Self::Remove(args) => args.output.json,
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct OutputArgs {
    /// Emit structured JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct FetchArgs {
    /// Configured repository names; fetch all when omitted
    pub repositories: Vec<String>,

    #[command(flatten)]
    pub output: OutputArgs,
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    /// Workspace name
    pub workspace: String,

    /// Configured repository names
    #[arg(required = true, num_args = 1..)]
    pub repositories: Vec<String>,

    /// Override a repository's creation base
    #[arg(long = "base", value_name = "REPOSITORY=REF")]
    pub bases: Vec<BaseOverride>,

    /// Use a local branch or create it tracking origin
    #[arg(long = "branch", value_name = "REPOSITORY=BRANCH")]
    pub branches: Vec<BranchOverride>,

    #[command(flatten)]
    pub output: OutputArgs,
}

#[derive(Debug, Clone)]
pub struct BaseOverride {
    pub repository: String,
    pub reference: String,
}

impl FromStr for BaseOverride {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (repository, reference) = value
            .split_once('=')
            .ok_or_else(|| "expected REPOSITORY=REF".to_owned())?;

        if repository.is_empty() {
            return Err("repository name cannot be empty".to_owned());
        }
        if reference.is_empty() {
            return Err("base ref cannot be empty".to_owned());
        }

        Ok(Self {
            repository: repository.to_owned(),
            reference: reference.to_owned(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct BranchOverride {
    pub repository: String,
    pub branch: String,
}

impl FromStr for BranchOverride {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (repository, branch) = value
            .split_once('=')
            .ok_or_else(|| "expected REPOSITORY=BRANCH".to_owned())?;

        if repository.is_empty() {
            return Err("repository name cannot be empty".to_owned());
        }
        if branch.is_empty() {
            return Err("branch name cannot be empty".to_owned());
        }

        Ok(Self {
            repository: repository.to_owned(),
            branch: branch.to_owned(),
        })
    }
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Limit status to one workspace
    pub workspace: Option<String>,

    #[command(flatten)]
    pub output: OutputArgs,
}

#[derive(Debug, Args)]
pub struct PathArgs {
    pub workspace: String,

    #[command(flatten)]
    pub output: OutputArgs,
}

#[derive(Debug, Args)]
pub struct AttachArgs {
    /// Workspace name
    pub workspace: String,

    #[command(flatten)]
    pub output: OutputArgs,
}

#[derive(Debug, Args)]
pub struct RemoveArgs {
    pub workspace: String,

    /// Remove only these configured repositories
    pub repositories: Vec<String>,

    #[command(flatten)]
    pub output: OutputArgs,
}
