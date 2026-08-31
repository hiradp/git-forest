use std::collections::HashSet;

use crate::config::Config;
use crate::domain::{
    CleanStatus, CommandOutcome, CommandReport, StaleWorktreeReport, WorktreesCleanReport,
};
use crate::error::Result;
use crate::git::{Git, failure_message};
use crate::workspace;

pub fn run(config: &Config, git: &Git) -> Result<CommandOutcome> {
    let _lock = workspace::lock_mutations(config)?;
    let states = workspace::scan(config, git)?;
    let mut seen = HashSet::new();
    let mut stale = Vec::new();
    for state in states {
        for member in state.members {
            let Some(metadata) = member.metadata else {
                continue;
            };
            if member.exists || !seen.insert((member.canonical_path.clone(), metadata.path.clone()))
            {
                continue;
            }
            stale.push((
                state.name.clone(),
                member.id,
                member.canonical_path,
                metadata.path,
            ));
        }
    }

    let mut failed = false;
    let mut worktrees = Vec::new();
    for (workspace, checkout, canonical_path, path) in stale {
        let output = git.remove_worktree(&canonical_path, &path, false)?;
        let (status, message) = if output.status.success() {
            (CleanStatus::Removed, None)
        } else {
            failed = true;
            (CleanStatus::Failed, Some(failure_message(&output)))
        };
        worktrees.push(StaleWorktreeReport {
            workspace,
            name: checkout.repository.clone(),
            checkout: checkout.to_string(),
            slot: checkout.slot,
            path,
            status,
            message,
        });
    }

    Ok(CommandOutcome {
        report: CommandReport::WorktreesClean(WorktreesCleanReport { worktrees }),
        exit_code: u8::from(failed),
    })
}
