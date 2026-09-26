use std::fs;
use std::io;
use std::path::Path;

use crate::archives;
use crate::cli::{ArchiveArgs, DeleteArgs, OutputArgs};
use crate::config::{CheckoutId, Config};
use crate::domain::{
    ArchiveStatus, CommandOutcome, CommandReport, DeleteStatus, WorkspaceDeleteReport,
};
use crate::error::Result;
use crate::git::Git;
use crate::workspace;

use super::archive;

pub fn run(config: &Config, git: &Git, arguments: &DeleteArgs) -> Result<CommandOutcome> {
    run_with_cleanup(config, git, arguments, |path| fs::remove_dir_all(path))
}

fn run_with_cleanup(
    config: &Config,
    git: &Git,
    arguments: &DeleteArgs,
    cleanup: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<CommandOutcome> {
    let _lock = workspace::lock_mutations(config)?;
    let path = config.workspace_path(&arguments.workspace)?;
    let storage_root = config.archive_root().join(".deleting");
    archives::validate_storage_root(config, &storage_root)?;
    let pending = storage_root.join(&arguments.workspace);
    let pending_metadata = archive::path_metadata(&pending)?;
    let active_exists = archive::path_metadata(&path)?.is_some();
    let mut report = WorkspaceDeleteReport {
        workspace: arguments.workspace.clone(),
        path,
        repositories: Vec::new(),
        status: DeleteStatus::Deleted,
        deleted_entries: Vec::new(),
        message: None,
    };
    if let Some(metadata) = pending_metadata {
        if !metadata.is_dir() {
            return Ok(conflict(
                report,
                format!(
                    "pending deletion {} is not a directory (symlinks are not allowed)",
                    pending.display()
                ),
            ));
        }
        if active_exists
            || workspace::scan(config, git)?
                .iter()
                .any(|state| state.name == arguments.workspace)
        {
            return Ok(conflict(report, "pending deletion and active workspace state both exist; move the active workspace with `git forest rename` or resolve its stale registrations before retrying".to_owned()));
        }
        if !archive::entries(&storage_root)?
            .iter()
            .any(|entry| entry.file_name() == pending.file_name())
        {
            return Ok(conflict(
                report,
                "workspace name casing does not match the pending deletion directory".to_owned(),
            ));
        }
        report.deleted_entries = archive::entries(&pending)?;
        for entry in &report.deleted_entries {
            if entry
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.parse::<CheckoutId>().ok())
                .is_some_and(|checkout| config.repository(&checkout.repository).is_some())
            {
                let message = format!(
                    "checkout path {} appeared in pending deletion storage; resolve it before retrying",
                    entry.display()
                );
                return Ok(conflict(report, message));
            }
        }
        for repository in &config.repositories {
            if !repository.path.exists()
                || !git.inspect_repository(&repository.path)?.is_git_worktree
            {
                continue;
            }
            if let Some(worktree) = git
                .worktrees(&repository.path)?
                .iter()
                .find(|worktree| workspace::path_is_within(&worktree.path, &pending))
            {
                let message = format!(
                    "registered worktree {} conflicts with pending deletion; resolve it before retrying",
                    worktree.path.display()
                );
                return Ok(conflict(report, message));
            }
        }
    } else {
        if !active_exists
            && workspace::scan(config, git)?
                .iter()
                .all(|state| state.name != arguments.workspace)
        {
            report.status = DeleteStatus::AlreadyDeleted;
            return Ok(outcome(report));
        }
        let retirement = archive::retire_locked(
            config,
            git,
            &ArchiveArgs {
                workspace: arguments.workspace.clone(),
                force: arguments.force,
                output: OutputArgs { json: false },
            },
            &pending,
        )?;
        report.repositories = retirement.repositories;
        report.message = retirement.message;
        report.status = match retirement.status {
            ArchiveStatus::Archived => DeleteStatus::Deleted,
            ArchiveStatus::Conflict => DeleteStatus::Conflict,
            ArchiveStatus::Failed => DeleteStatus::Failed,
            ArchiveStatus::AlreadyArchived => {
                unreachable!("retirement never reuses an existing destination")
            }
        };
        if report.status != DeleteStatus::Deleted {
            return Ok(outcome(report));
        }
        report.deleted_entries = retirement.preserved_entries;
    }

    if let Err(source) = cleanup(&pending) {
        report.status = DeleteStatus::Failed;
        report.deleted_entries.clear();
        report.message = Some(format!(
            "deletion is incomplete at {}: {source}; repeat `git forest delete {}` to retry cleanup; pending files are not restorable archives",
            pending.display(),
            arguments.workspace
        ));
    }
    Ok(outcome(report))
}

fn conflict(mut report: WorkspaceDeleteReport, message: String) -> CommandOutcome {
    report.status = DeleteStatus::Conflict;
    report.deleted_entries.clear();
    report.message = Some(message);
    outcome(report)
}

fn outcome(report: WorkspaceDeleteReport) -> CommandOutcome {
    let exit_code = u8::from(matches!(
        report.status,
        DeleteStatus::Conflict | DeleteStatus::Failed
    ));
    CommandOutcome {
        report: CommandReport::WorkspaceDelete(report),
        exit_code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(root: &Path) -> Config {
        let file = root.join(".forest.toml");
        fs::write(&file, "version = 1\n[repositories]\nroot = \"repos\"\nmembers = [\"alpha\"]\n[workspaces]\nroot = \"workspaces\"\nbranch = \"{workspace}\"\n").unwrap();
        Config::load(Some(&file)).unwrap()
    }

    fn arguments() -> DeleteArgs {
        DeleteArgs {
            workspace: "topic".to_owned(),
            force: false,
            output: OutputArgs { json: true },
        }
    }

    fn report(outcome: CommandOutcome) -> WorkspaceDeleteReport {
        let CommandReport::WorkspaceDelete(report) = outcome.report else {
            panic!("expected deletion report")
        };
        report
    }

    #[test]
    fn partial_deletion_is_not_an_archive_and_retries_only_pending_files() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let path = config.workspace_path("topic").unwrap();
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("removed.txt"), "delete").unwrap();
        fs::write(path.join("survivor.txt"), "delete too").unwrap();
        let legacy = config.archive_path("topic").unwrap();
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("notes.md"), "preserve old archive").unwrap();

        let failed = run_with_cleanup(&config, &Git, &arguments(), |pending| {
            fs::remove_file(pending.join("removed.txt"))?;
            Err(io::Error::other(
                "simulated filesystem failure after partial deletion",
            ))
        })
        .unwrap();
        assert_eq!(failed.exit_code, 1);
        let failed = report(failed);
        assert_eq!(failed.status, DeleteStatus::Failed);
        assert!(
            failed
                .message
                .unwrap()
                .contains("repeat `git forest delete topic`")
        );
        let pending = config.archive_root().join(".deleting/topic");
        assert!(!path.exists());
        assert!(!pending.join("removed.txt").exists());
        assert_eq!(
            fs::read_to_string(pending.join("survivor.txt")).unwrap(),
            "delete too"
        );
        let saved = archives::for_workspace(&config, "topic").unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].path, legacy);

        fs::create_dir(&path).unwrap();
        fs::write(path.join("new.txt"), "new workspace").unwrap();
        let conflicting = run(&config, &Git, &arguments()).unwrap();
        assert_eq!(conflicting.exit_code, 1);
        assert_eq!(report(conflicting).status, DeleteStatus::Conflict);
        assert_eq!(
            fs::read_to_string(path.join("new.txt")).unwrap(),
            "new workspace"
        );
        assert!(pending.join("survivor.txt").exists());
        fs::remove_dir_all(&path).unwrap();

        let retried = run(&config, &Git, &arguments()).unwrap();
        assert_eq!(retried.exit_code, 0);
        let retried = report(retried);
        assert_eq!(retried.status, DeleteStatus::Deleted);
        assert_eq!(retried.deleted_entries, vec![pending.join("survivor.txt")]);
        assert!(!pending.exists());
        assert_eq!(
            report(run(&config, &Git, &arguments()).unwrap()).status,
            DeleteStatus::AlreadyDeleted
        );
        assert_eq!(
            fs::read_to_string(legacy.join("notes.md")).unwrap(),
            "preserve old archive"
        );
    }

    #[cfg(unix)]
    #[test]
    fn deletion_never_follows_symlinks_in_pending_storage() {
        use std::os::unix::fs::symlink;
        for linked in [".deleting", ".deleting/topic"] {
            let temp = tempfile::tempdir().unwrap();
            let config = config(temp.path());
            let external = temp.path().join("external");
            fs::create_dir(&external).unwrap();
            fs::write(external.join("notes.md"), "keep").unwrap();
            let link = config.archive_root().join(linked);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(&external, link).unwrap();
            let outcome = run(&config, &Git, &arguments());
            assert!(outcome.is_err() || outcome.unwrap().exit_code != 0);
            assert_eq!(
                fs::read_to_string(external.join("notes.md")).unwrap(),
                "keep"
            );
        }
    }
}
