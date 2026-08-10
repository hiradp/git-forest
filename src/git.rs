use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::error::{AppError, Result};

pub(crate) const REPOSITORY_ENVIRONMENT: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_DIR",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_INTERNAL_SUPER_PREFIX",
    "GIT_NAMESPACE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "GIT_REPLACE_REF_BASE",
    "GIT_SHALLOW_FILE",
    "GIT_WORK_TREE",
];

#[derive(Debug, Default)]
pub struct Git;

#[derive(Debug)]
pub struct RepositoryInspection {
    pub is_git_worktree: bool,
    pub origin_url: Option<String>,
    pub default_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub detached: bool,
}

#[derive(Debug)]
pub struct WorktreeInspection {
    pub head: String,
    pub branch: Option<String>,
    pub dirty: bool,
    pub upstream: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
}

impl Git {
    pub fn inspect_repository(&self, path: &Path) -> Result<RepositoryInspection> {
        let is_git_worktree = self.is_worktree_root(path)?;

        if !is_git_worktree {
            return Ok(RepositoryInspection {
                is_git_worktree: false,
                origin_url: None,
                default_ref: None,
            });
        }

        let origin = self.run(path, ["remote", "get-url", "origin"])?;
        let origin_url = origin.status.success().then(|| text(&origin.stdout));
        let default_ref = self.default_ref(path)?;

        Ok(RepositoryInspection {
            is_git_worktree,
            origin_url,
            default_ref,
        })
    }

    pub fn default_ref(&self, repository: &Path) -> Result<Option<String>> {
        let symbolic = self.run(
            repository,
            ["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
        )?;
        if !symbolic.status.success() {
            return Ok(None);
        }

        let reference = text(&symbolic.stdout);
        let valid_name = reference
            .strip_prefix("refs/remotes/origin/")
            .is_some_and(|name| !name.is_empty());
        let exists = self
            .run(repository, ["show-ref", "--verify", "--quiet", &reference])?
            .status
            .success();
        Ok((valid_name && exists).then_some(reference))
    }

    pub fn check_branch_name(&self, repository: &Path, branch: &str) -> Result<bool> {
        Ok(self
            .run(repository, ["check-ref-format", "--branch", branch])?
            .status
            .success())
    }

    pub fn branch_exists(&self, repository: &Path, branch: &str) -> Result<bool> {
        let reference = format!("refs/heads/{branch}");
        Ok(self
            .run(repository, ["show-ref", "--verify", "--quiet", &reference])?
            .status
            .success())
    }

    pub fn resolve_reference(&self, repository: &Path, reference: &str) -> Result<String> {
        let output = self.run(repository, ["rev-parse", "--verify", reference])?;
        if !output.status.success() {
            return Err(AppError::Git {
                context: format!("could not resolve {reference} in {}", repository.display()),
                message: failure_message(&output),
            });
        }
        Ok(text(&output.stdout))
    }

    pub fn reference_ahead_behind(
        &self,
        repository: &Path,
        local: &str,
        remote: &str,
    ) -> Result<(u64, u64)> {
        let range = format!("{local}...{remote}");
        let output = self.run(repository, ["rev-list", "--left-right", "--count", &range])?;
        if !output.status.success() {
            return Err(AppError::Git {
                context: format!(
                    "could not compare {local} with {remote} in {}",
                    repository.display()
                ),
                message: failure_message(&output),
            });
        }
        let counts = text(&output.stdout);
        let mut fields = counts.split_whitespace();
        let ahead = fields.next().and_then(|count| count.parse::<u64>().ok());
        let behind = fields.next().and_then(|count| count.parse::<u64>().ok());
        if fields.next().is_some() || ahead.is_none() || behind.is_none() {
            return Err(AppError::Git {
                context: format!(
                    "could not compare {local} with {remote} in {}",
                    repository.display()
                ),
                message: format!("could not parse Git ahead/behind counts {counts:?}"),
            });
        }
        Ok((ahead.unwrap(), behind.unwrap()))
    }

    pub fn branch_namespace_conflict(
        &self,
        repository: &Path,
        branch: &str,
    ) -> Result<Option<String>> {
        let output = self.run(
            repository,
            ["for-each-ref", "--format=%(refname:strip=2)", "refs/heads"],
        )?;
        if !output.status.success() {
            return Err(AppError::Git {
                context: format!("could not inspect branches in {}", repository.display()),
                message: failure_message(&output),
            });
        }

        Ok(text(&output.stdout)
            .lines()
            .find(|existing| {
                *existing != branch
                    && (branch
                        .strip_prefix(*existing)
                        .is_some_and(|suffix| suffix.starts_with('/'))
                        || existing
                            .strip_prefix(branch)
                            .is_some_and(|suffix| suffix.starts_with('/')))
            })
            .map(str::to_owned))
    }

    pub fn resolves_to_commit(&self, repository: &Path, reference: &str) -> Result<bool> {
        let commit = format!("{reference}^{{commit}}");
        Ok(self
            .run(
                repository,
                [
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    "--end-of-options",
                    &commit,
                ],
            )?
            .status
            .success())
    }

    pub fn worktrees(&self, repository: &Path) -> Result<Vec<Worktree>> {
        let output = self.run(repository, ["worktree", "list", "--porcelain", "-z"])?;
        if !output.status.success() {
            return Err(AppError::Git {
                context: format!("could not list worktrees for {}", repository.display()),
                message: failure_message(&output),
            });
        }
        parse_worktrees(&output.stdout)
    }

    pub fn worktree_belongs_to(&self, repository: &Path, worktree: &Path) -> Result<bool> {
        let repository_common_dir = self.git_common_dir(repository)?;
        let worktree_common_dir = self.git_common_dir(worktree)?;
        Ok(matches!(
            (repository_common_dir, worktree_common_dir),
            (Some(repository), Some(worktree)) if paths_match(&repository, &worktree)
        ))
    }

    pub fn worktree_dirty(&self, worktree: &Path) -> Result<bool> {
        let status = self.run(
            worktree,
            ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        if !status.status.success() {
            return Err(AppError::Git {
                context: format!("could not inspect changes in {}", worktree.display()),
                message: failure_message(&status),
            });
        }
        Ok(!status.stdout.is_empty())
    }

    pub fn ignored_path_changed_between(
        &self,
        worktree: &Path,
        old_oid: &str,
        new_oid: &str,
    ) -> Result<Option<String>> {
        let changed = self.run(
            worktree,
            [
                "diff",
                "--name-only",
                "--no-renames",
                "-z",
                old_oid,
                new_oid,
                "--",
            ],
        )?;
        if !changed.status.success() {
            return Err(AppError::Git {
                context: format!(
                    "could not inspect incoming changes in {}",
                    worktree.display()
                ),
                message: failure_message(&changed),
            });
        }

        let ignored = self.run(
            worktree,
            [
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignored=matching",
            ],
        )?;
        if !ignored.status.success() {
            return Err(AppError::Git {
                context: format!("could not inspect ignored files in {}", worktree.display()),
                message: failure_message(&ignored),
            });
        }

        let ignore_case = self.repository_path_ignore_case(worktree)?;
        let changed_paths: Vec<&[u8]> = nul_fields(&changed.stdout).collect();
        Ok(nul_fields(&ignored.stdout)
            .filter_map(porcelain_ignored_path)
            .find(|ignored_path| {
                changed_paths.iter().any(|changed_path| {
                    repository_paths_overlap(ignored_path, changed_path, ignore_case)
                })
            })
            .map(|path| String::from_utf8_lossy(path).into_owned()))
    }

    fn repository_path_ignore_case(&self, repository: &Path) -> Result<bool> {
        let output = self.run(
            repository,
            ["config", "--type=bool", "--get", "core.ignoreCase"],
        )?;
        if !output.status.success() {
            if output.stderr.is_empty() {
                return Ok(false);
            }
            return Err(AppError::Git {
                context: format!(
                    "could not inspect path handling in {}",
                    repository.display()
                ),
                message: failure_message(&output),
            });
        }

        match text(&output.stdout).as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            value => Err(AppError::Git {
                context: format!(
                    "could not inspect path handling in {}",
                    repository.display()
                ),
                message: format!("unexpected core.ignoreCase value {value:?}"),
            }),
        }
    }

    pub fn current_branch_ref(&self, worktree: &Path) -> Result<Option<String>> {
        let output = self.run(worktree, ["symbolic-ref", "--quiet", "HEAD"])?;
        if output.status.success() {
            return Ok(Some(text(&output.stdout)));
        }
        if output.stderr.is_empty() {
            return Ok(None);
        }
        Err(AppError::Git {
            context: format!("could not inspect branch in {}", worktree.display()),
            message: failure_message(&output),
        })
    }

    pub fn inspect_worktree(
        &self,
        worktree: &Path,
    ) -> Result<std::result::Result<WorktreeInspection, String>> {
        if !self.is_worktree_root(worktree)? {
            return Ok(Err(format!(
                "{} is not a Git worktree root",
                worktree.display()
            )));
        }

        let head = self.run(worktree, ["rev-parse", "--verify", "HEAD"])?;
        if !head.status.success() {
            return Ok(Err(failure_message(&head)));
        }

        let status = self.run(
            worktree,
            ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        if !status.status.success() {
            return Ok(Err(failure_message(&status)));
        }

        let branch = self.run(worktree, ["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        let branch = branch.status.success().then(|| text(&branch.stdout));
        let upstream = self.run(
            worktree,
            [
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
        )?;
        let upstream = upstream.status.success().then(|| text(&upstream.stdout));
        let (ahead, behind) = if upstream.is_some() {
            let counts = self.run(
                worktree,
                ["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
            )?;
            if !counts.status.success() {
                return Ok(Err(failure_message(&counts)));
            }
            let counts = text(&counts.stdout);
            let Some((ahead, behind)) = counts.split_once(char::is_whitespace) else {
                return Ok(Err(format!(
                    "could not parse Git ahead/behind counts {counts:?}"
                )));
            };
            let ahead = ahead.parse::<u64>().map_err(|_| AppError::Git {
                context: "could not parse Git ahead count".to_owned(),
                message: counts.clone(),
            })?;
            let behind = behind.trim().parse::<u64>().map_err(|_| AppError::Git {
                context: "could not parse Git behind count".to_owned(),
                message: counts,
            })?;
            (Some(ahead), Some(behind))
        } else {
            (None, None)
        };

        Ok(Ok(WorktreeInspection {
            head: text(&head.stdout),
            branch,
            dirty: !status.stdout.is_empty(),
            upstream,
            ahead,
            behind,
        }))
    }

    pub fn is_clean_for_removal(
        &self,
        worktree: &Path,
    ) -> Result<std::result::Result<bool, String>> {
        if !self.is_worktree_root(worktree)? {
            return Ok(Err(format!(
                "{} is not a Git worktree root",
                worktree.display()
            )));
        }
        let status = self.run(
            worktree,
            [
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignored=matching",
            ],
        )?;
        if !status.status.success() {
            return Ok(Err(failure_message(&status)));
        }
        Ok(Ok(status.stdout.is_empty()))
    }

    pub fn clone_repository(
        &self,
        working_directory: &Path,
        remote: &str,
        destination: &Path,
    ) -> Result<Output> {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(working_directory)
            .arg("clone")
            .arg("--quiet")
            .arg("--no-recurse-submodules")
            .arg("--")
            .arg(remote)
            .arg(destination);
        for variable in REPOSITORY_ENVIRONMENT {
            command.env_remove(variable);
        }
        command.output().map_err(AppError::StartGit)
    }

    pub fn fetch_origin(&self, repository: &Path) -> Result<Output> {
        self.run(
            repository,
            [
                "fetch",
                "--quiet",
                "--no-recurse-submodules",
                "--no-tags",
                "origin",
            ],
        )
    }

    pub fn fetch_origin_branch(&self, repository: &Path, branch: &str) -> Result<Output> {
        let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
        self.run(
            repository,
            [
                "fetch",
                "--quiet",
                "--no-recurse-submodules",
                "--no-tags",
                "--no-write-fetch-head",
                "origin",
                &refspec,
            ],
        )
    }

    pub fn fast_forward(&self, worktree: &Path, remote_ref: &str) -> Result<Output> {
        self.run(worktree, ["merge", "--quiet", "--ff-only", remote_ref])
    }

    pub fn update_ref(
        &self,
        repository: &Path,
        local_ref: &str,
        new_oid: &str,
        old_oid: &str,
    ) -> Result<Output> {
        self.run(repository, ["update-ref", local_ref, new_oid, old_oid])
    }

    pub fn remove_worktree(&self, repository: &Path, worktree: &Path) -> Result<Output> {
        self.run(
            repository,
            [
                OsStr::new("worktree"),
                OsStr::new("remove"),
                worktree.as_os_str(),
            ],
        )
    }

    pub fn add_existing_branch(
        &self,
        repository: &Path,
        destination: &Path,
        branch: &str,
    ) -> Result<Output> {
        self.run(
            repository,
            [
                OsStr::new("worktree"),
                OsStr::new("add"),
                destination.as_os_str(),
                OsStr::new(branch),
            ],
        )
    }

    pub fn add_new_branch(
        &self,
        repository: &Path,
        destination: &Path,
        branch: &str,
        base: &str,
    ) -> Result<Output> {
        self.run(
            repository,
            [
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("-b"),
                OsStr::new(branch),
                destination.as_os_str(),
                OsStr::new(base),
            ],
        )
    }

    pub fn add_tracking_branch(
        &self,
        repository: &Path,
        destination: &Path,
        branch: &str,
        remote_ref: &str,
    ) -> Result<Output> {
        self.run(
            repository,
            [
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("--track"),
                OsStr::new("-b"),
                OsStr::new(branch),
                destination.as_os_str(),
                OsStr::new(remote_ref),
            ],
        )
    }

    pub fn is_worktree_root(&self, path: &Path) -> Result<bool> {
        let inside = self.run(path, ["rev-parse", "--is-inside-work-tree"])?;
        if !inside.status.success() || text(&inside.stdout) != "true" {
            return Ok(false);
        }
        let top_level = self.run(path, ["rev-parse", "--show-toplevel"])?;
        if !top_level.status.success() {
            return Ok(false);
        }
        let top_level = PathBuf::from(text(&top_level.stdout));
        Ok(paths_match(&top_level, path))
    }

    fn git_common_dir(&self, path: &Path) -> Result<Option<PathBuf>> {
        if !self.is_worktree_root(path)? {
            return Ok(None);
        }
        let output = self.run(path, ["rev-parse", "--git-common-dir"])?;
        if !output.status.success() {
            return Err(AppError::Git {
                context: format!(
                    "could not inspect Git common directory in {}",
                    path.display()
                ),
                message: failure_message(&output),
            });
        }
        let common_dir = PathBuf::from(text(&output.stdout));
        let common_dir = if common_dir.is_absolute() {
            common_dir
        } else {
            path.join(common_dir)
        };
        let common_dir = common_dir
            .canonicalize()
            .map_err(|source| AppError::Filesystem {
                context: format!(
                    "could not resolve Git common directory {}",
                    common_dir.display()
                ),
                source,
            })?;
        Ok(Some(common_dir))
    }

    pub fn run<I, S>(&self, repository: &Path, arguments: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let arguments: Vec<OsString> = arguments
            .into_iter()
            .map(|argument| argument.as_ref().to_os_string())
            .collect();
        let mut command = Command::new("git");
        command.arg("-C").arg(repository).args(&arguments);
        for variable in REPOSITORY_ENVIRONMENT {
            command.env_remove(variable);
        }
        command.output().map_err(AppError::StartGit)
    }
}

pub fn failure_message(output: &Output) -> String {
    let stderr = text(&output.stderr);
    if stderr.is_empty() {
        format!("git exited with {}", output.status)
    } else {
        stderr
    }
}

fn parse_worktrees(bytes: &[u8]) -> Result<Vec<Worktree>> {
    #[derive(Default)]
    struct PartialWorktree {
        path: Option<PathBuf>,
        head: Option<String>,
        branch: Option<String>,
        detached: bool,
    }

    fn finish(partial: &mut PartialWorktree, worktrees: &mut Vec<Worktree>) -> Result<()> {
        if partial.path.is_none() && partial.head.is_none() {
            return Ok(());
        }
        let path = partial.path.take().ok_or_else(|| AppError::Git {
            context: "could not parse git worktree metadata".to_owned(),
            message: "worktree record has no path".to_owned(),
        })?;
        let head = partial.head.take().ok_or_else(|| AppError::Git {
            context: "could not parse git worktree metadata".to_owned(),
            message: format!("worktree {} has no HEAD", path.display()),
        })?;
        worktrees.push(Worktree {
            path,
            head,
            branch: partial.branch.take(),
            detached: partial.detached,
        });
        partial.detached = false;
        Ok(())
    }

    let mut worktrees = Vec::new();
    let mut partial = PartialWorktree::default();
    for field in bytes.split(|byte| *byte == 0) {
        if field.is_empty() {
            finish(&mut partial, &mut worktrees)?;
            continue;
        }

        let field = String::from_utf8_lossy(field);
        if let Some(path) = field.strip_prefix("worktree ") {
            if partial.path.is_some() {
                finish(&mut partial, &mut worktrees)?;
            }
            partial.path = Some(PathBuf::from(path));
        } else if let Some(head) = field.strip_prefix("HEAD ") {
            partial.head = Some(head.to_owned());
        } else if let Some(branch) = field.strip_prefix("branch ") {
            partial.branch = Some(branch.to_owned());
        } else if field == "detached" {
            partial.detached = true;
        }
    }
    finish(&mut partial, &mut worktrees)?;
    Ok(worktrees)
}

fn nul_fields(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
}

fn porcelain_ignored_path(record: &[u8]) -> Option<&[u8]> {
    record
        .strip_prefix(b"!! ")
        .map(|path| path.strip_suffix(b"/").unwrap_or(path))
}

fn repository_paths_overlap(left: &[u8], right: &[u8], ignore_case: bool) -> bool {
    path_is_same_or_descendant(left, right, ignore_case)
        || path_is_same_or_descendant(right, left, ignore_case)
}

fn path_is_same_or_descendant(path: &[u8], ancestor: &[u8], ignore_case: bool) -> bool {
    let Some((prefix, suffix)) = path.split_at_checked(ancestor.len()) else {
        return false;
    };
    let same_prefix = if ignore_case {
        prefix.eq_ignore_ascii_case(ancestor)
    } else {
        prefix == ancestor
    };
    same_prefix && (suffix.is_empty() || suffix.first() == Some(&b'/'))
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left == right
        || match (left.canonicalize(), right.canonicalize()) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nul_terminated_worktree_porcelain() {
        let worktrees = parse_worktrees(
            b"worktree /project/repo\0HEAD abc123\0branch refs/heads/main\0\0worktree /project/space path\0HEAD def456\0detached\0\0",
        )
        .unwrap();

        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].path, Path::new("/project/repo"));
        assert_eq!(worktrees[0].branch.as_deref(), Some("refs/heads/main"));
        assert_eq!(worktrees[1].path, Path::new("/project/space path"));
        assert!(worktrees[1].detached);
    }

    #[test]
    fn detects_overlapping_repository_paths() {
        assert_eq!(
            porcelain_ignored_path(b"!! generated/"),
            Some(b"generated".as_slice())
        );
        assert!(repository_paths_overlap(b"generated", b"generated", false));
        assert!(repository_paths_overlap(
            b"generated/nested/file",
            b"generated",
            false
        ));
        assert!(repository_paths_overlap(
            b"generated",
            b"generated/nested/file",
            false
        ));
        assert!(repository_paths_overlap(b"Generated", b"generated", true));
        assert!(!repository_paths_overlap(b"Generated", b"generated", false));
        assert!(!repository_paths_overlap(
            b"generated",
            b"generated-old",
            true
        ));
    }
}
