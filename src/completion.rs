use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::PathBuf;

use clap::CommandFactory;
use clap_complete::CompleteEnv;
use clap_complete::engine::CompletionCandidate;
use clap_complete::env::{Bash, Elvish, EnvCompleter, Fish, Powershell, Zsh};

use crate::cli::{Cli, CompletionShell};
use crate::config::{self, Config};
use crate::error::{AppError, Result};
use crate::git::Git;
use crate::workspace;

const COMPLETE_ENV: &str = "FOREST_COMPLETE";
const BINARY: &str = "git-forest";

pub fn handle_environment() {
    CompleteEnv::with_factory(Cli::command)
        .var(COMPLETE_ENV)
        .bin(BINARY)
        .completer(BINARY)
        .complete();
}

pub fn write_registration(shell: CompletionShell) -> Result<()> {
    let completer: &dyn EnvCompleter = match shell {
        CompletionShell::Bash => &Bash,
        CompletionShell::Elvish => &Elvish,
        CompletionShell::Fish => &Fish,
        CompletionShell::PowerShell => &Powershell,
        CompletionShell::Zsh => &Zsh,
    };
    let stdout = io::stdout();
    let mut output = stdout.lock();
    completer
        .write_registration(COMPLETE_ENV, BINARY, BINARY, BINARY, &mut output)
        .map_err(AppError::WriteOutput)?;
    output
        .write_all(git_adapter(shell).as_bytes())
        .map_err(AppError::WriteOutput)
}

pub fn workspaces(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(line) = CompletionLine::from_environment() else {
        return Vec::new();
    };
    if line.subcommand() == Some("create") {
        return Vec::new();
    }
    let Some(config) = line.load_config() else {
        return Vec::new();
    };
    let Some(current) = current.to_str() else {
        return Vec::new();
    };

    let mut names = workspace_directories(&config);
    if matches!(line.subcommand(), Some("remove" | "status"))
        && let Ok(states) = workspace::scan(&config, &Git)
    {
        names.extend(states.into_iter().map(|state| state.name));
    }
    names.retain(|name| config::validate_workspace_name(name).is_ok() && name.starts_with(current));
    names.sort();
    names.dedup();
    candidates(names)
}

pub fn repositories(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(line) = CompletionLine::from_environment() else {
        return Vec::new();
    };
    let Some(config) = line.load_config() else {
        return Vec::new();
    };
    let Some(current) = current.to_str() else {
        return Vec::new();
    };
    let selected = line.selected_values();

    config
        .repositories
        .iter()
        .filter(|repository| {
            repository.name.starts_with(current) && !selected.contains(&repository.name.as_str())
        })
        .map(|repository| CompletionCandidate::new(repository.name.clone()))
        .collect()
}

pub fn checkouts(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(line) = CompletionLine::from_environment() else {
        return Vec::new();
    };
    let Some(config) = line.load_config() else {
        return Vec::new();
    };
    let Some(current) = current.to_str() else {
        return Vec::new();
    };
    let selected = line.selected_values();

    let mut values = if line.subcommand() == Some("remove") {
        remove_checkouts(&line, &config)
    } else {
        config
            .repositories
            .iter()
            .map(|repository| repository.name.clone())
            .collect()
    };
    values.retain(|value| value.starts_with(current) && !selected.contains(&value.as_str()));
    values.sort();
    values.dedup();
    candidates(values)
}

fn workspace_directories(config: &Config) -> Vec<String> {
    config
        .workspaces_root
        .read_dir()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

fn remove_checkouts(line: &CompletionLine, config: &Config) -> Vec<String> {
    let Some(name) = line.workspace() else {
        return Vec::new();
    };
    if let Ok(states) = workspace::scan(config, &Git)
        && let Some(state) = states.into_iter().find(|state| state.name == name)
    {
        return state
            .members
            .into_iter()
            .map(|member| member.id.to_string())
            .collect();
    }

    config
        .workspace_path(name)
        .ok()
        .and_then(|path| path.read_dir().ok())
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            name.parse::<crate::config::CheckoutId>()
                .ok()
                .is_some_and(|checkout| config.repository(&checkout.repository).is_some())
        })
        .collect()
}

fn candidates(values: Vec<String>) -> Vec<CompletionCandidate> {
    values.into_iter().map(CompletionCandidate::new).collect()
}

struct CompletionLine {
    prior: Vec<OsString>,
}

impl CompletionLine {
    fn from_environment() -> Option<Self> {
        let arguments = env::args_os().collect::<Vec<_>>();
        let marker = arguments.iter().position(|argument| argument == "--")?;
        let words = &arguments[marker + 1..];
        if words.is_empty() {
            return None;
        }
        let cursor = env::var("_CLAP_COMPLETE_INDEX")
            .ok()
            .and_then(|index| index.parse::<usize>().ok())
            .unwrap_or(words.len().saturating_sub(1))
            .min(words.len());
        Some(Self {
            prior: words[..cursor].to_vec(),
        })
    }

    fn load_config(&self) -> Option<Config> {
        Config::load(self.explicit_config().as_deref()).ok()
    }

    fn explicit_config(&self) -> Option<PathBuf> {
        let mut arguments = self.prior.iter();
        while let Some(argument) = arguments.next() {
            if argument == "--config" {
                return arguments.next().map(PathBuf::from);
            }
            if let Some(argument) = argument.to_str()
                && let Some(path) = argument.strip_prefix("--config=")
            {
                return Some(PathBuf::from(path));
            }
        }
        None
    }

    fn subcommand(&self) -> Option<&str> {
        self.subcommand_index()
            .and_then(|index| self.prior[index].to_str())
    }

    fn subcommand_index(&self) -> Option<usize> {
        let mut index = 1;
        while index < self.prior.len() {
            let argument = self.prior[index].to_str()?;
            if argument == "--config" {
                index += 2;
                continue;
            }
            if argument.starts_with('-') {
                index += 1;
                continue;
            }
            return matches!(
                argument,
                "open"
                    | "setup"
                    | "repos"
                    | "fetch"
                    | "update"
                    | "create"
                    | "add"
                    | "list"
                    | "status"
                    | "path"
                    | "attach"
                    | "remove"
                    | "completions"
            )
            .then_some(index);
        }
        None
    }

    fn workspace(&self) -> Option<&str> {
        let index = self.subcommand_index()?;
        positional_values(&self.prior[index + 1..])
            .into_iter()
            .next()
    }

    fn selected_values(&self) -> Vec<&str> {
        let Some(index) = self.subcommand_index() else {
            return Vec::new();
        };
        let Some(subcommand) = self.prior[index].to_str() else {
            return Vec::new();
        };
        let positional = positional_values(&self.prior[index + 1..]);
        match subcommand {
            "create" | "add" | "remove" => positional.into_iter().skip(1).collect(),
            "fetch" | "update" => positional,
            _ => Vec::new(),
        }
    }
}

fn positional_values(arguments: &[OsString]) -> Vec<&str> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        let Some(argument) = arguments[index].to_str() else {
            index += 1;
            continue;
        };
        if matches!(argument, "--config" | "--jobs" | "--base" | "--branch") {
            index += 2;
            continue;
        }
        if argument.starts_with('-') {
            index += 1;
            continue;
        }
        values.push(argument);
        index += 1;
    }
    values
}

fn git_adapter(shell: CompletionShell) -> &'static str {
    match shell {
        CompletionShell::Bash => {
            r#"
_git_forest() {
    local command_index=${__git_cmd_idx:-1}
    local original_cword=$COMP_CWORD
    local -a original_words=("${COMP_WORDS[@]}")
    local -a forest_words=("git-forest")
    forest_words+=("${original_words[@]:$((command_index + 1))}")
    local COMP_CWORD=$((original_cword - command_index))
    local -a COMP_WORDS=("${forest_words[@]}")
    _clap_complete_git_forest git-forest "${COMP_WORDS[COMP_CWORD]}"
}
"#
        }
        CompletionShell::Fish => {
            r#"
function __fish_git_forest_active
    set --local tokens (commandline --current-process --tokens-expanded)
    test (count $tokens) -ge 2; and test $tokens[1] = git; and test $tokens[2] = forest
end

function __fish_git_forest_complete
    set --local tokens (commandline --current-process --tokenize --cut-at-cursor) (commandline --current-token)
    set --local words git-forest $tokens[3..-1]
    FOREST_COMPLETE=fish git-forest -- $words
end

complete --keep-order --exclusive --command git --condition __fish_git_forest_active --arguments '(__fish_git_forest_complete)'
"#
        }
        CompletionShell::Zsh => {
            r#"
function __git_forest_complete() {
    local skip=$1
    local -a original_words=("${words[@]}")
    local original_current=$CURRENT
    words=("git-forest" "${original_words[@]:$skip}")
    CURRENT=$((original_current - skip + 1))
    _clap_dynamic_completer_git_forest
    local completion_status=$?
    words=("${original_words[@]}")
    CURRENT=$original_current
    return $completion_status
}

function _git_forest() {
    __git_forest_complete 2
}

function _git-forest() {
    __git_forest_complete 1
}
"#
        }
        CompletionShell::Elvish | CompletionShell::PowerShell => "",
    }
}
