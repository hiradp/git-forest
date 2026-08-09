use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    config: PathBuf,
    canonical: PathBuf,
    origin: PathBuf,
}

struct WorkspaceFixture {
    _temp: TempDir,
    root: PathBuf,
}

struct FakeHerdr {
    bin: PathBuf,
    log: PathBuf,
}

impl FakeHerdr {
    fn new(root: &Path) -> Self {
        let bin = root.join("fake-herdr-bin");
        let log = root.join("herdr-calls.log");
        fs::create_dir(&bin).unwrap();
        let executable = bin.join("herdr");
        fs::write(
            &executable,
            r#"#!/bin/sh
if [ -n "${GIT_DIR:-}" ]; then
  printf 'inherited GIT_DIR\n' >&2
  exit 1
fi

{
  separator=""
  for argument in "$@"; do
    printf '%s%s' "$separator" "$argument"
    separator="$(printf '\t')"
  done
  printf '\n'
} >> "$HERDR_FAKE_LOG"

case "$1:$2" in
  workspace:list)
    if [ -n "${HERDR_WORKSPACES_RESPONSE:-}" ]; then
      printf '%s\n' "$HERDR_WORKSPACES_RESPONSE"
    else
      printf '{"result":{"workspaces":[]}}\n'
    fi
    ;;
  workspace:create)
    label=""
    previous=""
    for argument in "$@"; do
      if [ "$previous" = "--label" ]; then label="$argument"; fi
      previous="$argument"
    done
    printf '{"result":{"workspace":{"workspace_id":"w-new"},"tab":{"tab_id":"w-new:t-main","label":"%s","number":1,"pane_count":1},"root_pane":{"pane_id":"w-new:p-main","tab_id":"w-new:t-main"}}}\n' "$label"
    ;;
  tab:list)
    if [ -n "${HERDR_TABS_RESPONSE:-}" ]; then
      printf '%s\n' "$HERDR_TABS_RESPONSE"
    else
      printf '{"result":{"tabs":[]}}\n'
    fi
    ;;
  tab:create)
    label=""
    workspace="w-new"
    previous=""
    for argument in "$@"; do
      if [ "$previous" = "--label" ]; then label="$argument"; fi
      if [ "$previous" = "--workspace" ]; then workspace="$argument"; fi
      previous="$argument"
    done
    number=${label%%-*}
    suffix=${label#*-}
    printf '{"result":{"tab":{"tab_id":"%s:t-%s","label":"%s","number":%s,"pane_count":1},"root_pane":{"pane_id":"%s:p-%s","tab_id":"%s:t-%s"}}}\n' "$workspace" "$suffix" "$label" "$number" "$workspace" "$suffix" "$workspace" "$suffix"
    ;;
  pane:list)
    if [ -n "${HERDR_PANES_RESPONSE:-}" ]; then
      printf '%s\n' "$HERDR_PANES_RESPONSE"
    else
      printf '{"result":{"panes":[]}}\n'
    fi
    ;;
  *)
    ;;

esac
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();
        Self { bin, log }
    }

    fn command(&self, current_dir: &Path) -> Command {
        let existing_path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.bin.clone()];
        paths.extend(std::env::split_paths(&existing_path));
        let command_path = std::env::join_paths(paths).unwrap();
        let mut command = Command::new(binary());
        command
            .current_dir(current_dir)
            .env_remove("FOREST_CONFIG")
            .env("PATH", command_path)
            .env("HERDR_FAKE_LOG", &self.log);
        command
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

impl WorkspaceFixture {
    fn new() -> Self {
        Self::with_default("main")
    }

    fn with_default(default_branch: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace project");
        fs::create_dir_all(root.join("src")).unwrap();
        for name in ["alpha", "beta", "gamma"] {
            initialize_repository(&root, name, default_branch);
        }
        write_config(&root.join(".forest.toml"), &["alpha", "beta", "gamma"]);
        Self { _temp: temp, root }
    }

    fn without_clones() -> Self {
        let fixture = Self::new();
        for name in ["alpha", "beta", "gamma"] {
            fs::remove_dir_all(fixture.canonical(name)).unwrap();
        }
        let remote = format!("{}/{{name}}-origin.git", fixture.root.display());
        write_config_with_remote(
            &fixture.root.join(".forest.toml"),
            &["alpha", "beta", "gamma"],
            Some(&remote),
        );
        fixture
    }

    fn canonical(&self, name: &str) -> PathBuf {
        self.root.join("src").join(name)
    }

    fn workspace(&self, workspace: &str) -> PathBuf {
        self.root
            .canonicalize()
            .unwrap()
            .join("src/.workspaces")
            .join(workspace)
    }
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project with spaces");
        let repositories = root.join("src");
        let canonical = repositories.join("alpha");
        let origin = root.join("alpha-origin.git");
        fs::create_dir_all(&repositories).unwrap();

        git(
            &root,
            &["init", "--bare", "--initial-branch=main", path(&origin)],
        );
        git(&root, &["init", "--initial-branch=main", path(&canonical)]);
        git(&canonical, &["config", "user.name", "Forest Test"]);
        git(&canonical, &["config", "user.email", "forest@example.com"]);
        fs::write(canonical.join("README.md"), "alpha\n").unwrap();
        git(&canonical, &["add", "README.md"]);
        git(&canonical, &["commit", "-m", "initial"]);
        git(&canonical, &["remote", "add", "origin", path(&origin)]);
        git(&canonical, &["push", "-u", "origin", "main"]);
        git(&canonical, &["remote", "set-head", "origin", "main"]);

        let config = root.join(".forest.toml");
        write_config(&config, &["alpha", "missing"]);

        Self {
            _temp: temp,
            root,
            config,
            canonical,
            origin,
        }
    }
}

#[test]
fn discovers_config_from_a_canonical_repository_and_reports_repositories() {
    let fixture = Fixture::new();
    let nested = fixture.canonical.join("nested/directory");
    fs::create_dir_all(&nested).unwrap();

    let output = forest(&nested, &["repos", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let repositories = report["repositories"].as_array().unwrap();
    assert_eq!(repositories.len(), 2);
    assert_eq!(repositories[0]["name"], "alpha");
    assert_eq!(
        repositories[0]["path"],
        path(&fixture.canonical.canonicalize().unwrap())
    );
    assert_eq!(repositories[0]["exists"], true);
    assert_eq!(repositories[0]["is_git_worktree"], true);
    assert_eq!(repositories[0]["origin_url"], path(&fixture.origin));
    assert_eq!(repositories[0]["default_ref"], "refs/remotes/origin/main");
    assert_eq!(repositories[1]["name"], "missing");
    assert_eq!(repositories[1]["exists"], false);
    assert!(output.stderr.is_empty());
}

#[test]
fn explicit_config_takes_precedence_over_environment() {
    let fixture = Fixture::new();
    let output = Command::new(binary())
        .current_dir(&fixture.root)
        .env("FOREST_CONFIG", fixture.root.join("does-not-exist.toml"))
        .args(["--config", path(&fixture.config), "repos", "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["name"], "alpha");
}

#[test]
fn environment_config_works_outside_the_project_tree() {
    let fixture = Fixture::new();
    let outside = fixture._temp.path().join("outside");
    fs::create_dir(&outside).unwrap();

    let output = Command::new(binary())
        .current_dir(outside)
        .env("FOREST_CONFIG", &fixture.config)
        .args(["repos", "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["name"], "alpha");
}

#[test]
fn setup_clones_missing_canonical_repositories_and_is_idempotent() {
    let fixture = WorkspaceFixture::without_clones();

    let output = Command::new(binary())
        .current_dir(&fixture.root)
        .env("GIT_DIR", fixture.root.join("unrelated.git"))
        .args(["setup", "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let repositories = report["repositories"].as_array().unwrap();
    assert_eq!(repositories.len(), 3);
    assert!(
        repositories
            .iter()
            .all(|repository| repository["status"] == "cloned")
    );
    for name in ["alpha", "beta", "gamma"] {
        let canonical = fixture.canonical(name);
        assert_eq!(
            git_stdout(&canonical, &["branch", "--show-current"]),
            "main"
        );
        assert_eq!(
            git_stdout(&canonical, &["remote", "get-url", "origin"]),
            path(&fixture.root.join(format!("{name}-origin.git")))
        );
    }

    let repeated = forest(&fixture.root, &["setup", "--json"]);

    assert_success(&repeated);
    let report: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert!(
        report["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|repository| repository["status"] == "reused")
    );
}

#[test]
fn setup_preflights_every_repository_before_cloning() {
    let fixture = WorkspaceFixture::without_clones();
    let occupied = fixture.canonical("beta");
    fs::create_dir_all(&occupied).unwrap();
    fs::write(occupied.join("keep.txt"), "keep\n").unwrap();

    let output = forest(&fixture.root, &["setup", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "not_run");
    assert_eq!(report["repositories"][1]["status"], "conflict");
    assert_eq!(report["repositories"][2]["status"], "not_run");
    assert!(
        report["repositories"][1]["message"]
            .as_str()
            .unwrap()
            .contains("not a Git worktree")
    );
    assert!(!fixture.canonical("alpha").exists());
    assert!(occupied.join("keep.txt").exists());
    assert!(!fixture.canonical("gamma").exists());
}

#[test]
fn setup_requires_a_remote_template_for_missing_repositories() {
    let fixture = WorkspaceFixture::without_clones();
    write_config_with_remote(
        &fixture.root.join(".forest.toml"),
        &["alpha", "beta", "gamma"],
        None,
    );

    let output = forest(&fixture.root, &["setup", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|repository| repository["status"] == "conflict")
    );
    assert!(!fixture.canonical("alpha").exists());
}

#[test]
fn setup_reports_partial_clone_failure_and_resumes() {
    let fixture = WorkspaceFixture::without_clones();
    let fake_bin = fixture.root.join("fake-setup-bin");
    fs::create_dir(&fake_bin).unwrap();
    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        r#"#!/bin/sh
destination=""
for argument in "$@"; do destination="$argument"; done
if [ "$1" = "clone" ] && [ "$destination" = "$FAIL_REPO" ]; then
  echo "simulated clone failure" >&2
  exit 1
fi
exec "$REAL_GIT" "$@"
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake_git).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_git, permissions).unwrap();
    let existing_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![fake_bin];
    paths.extend(std::env::split_paths(&existing_path));
    let command_path = std::env::join_paths(paths).unwrap();

    let partial = Command::new(binary())
        .current_dir(&fixture.root)
        .env("PATH", command_path)
        .env("REAL_GIT", find_executable("git"))
        .env(
            "FAIL_REPO",
            fixture.root.canonicalize().unwrap().join("src/beta"),
        )
        .args(["setup", "--json"])
        .output()
        .unwrap();

    assert!(!partial.status.success());
    let report: Value = serde_json::from_slice(&partial.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "cloned");
    assert_eq!(report["repositories"][1]["status"], "failed");
    assert_eq!(report["repositories"][2]["status"], "not_run");
    assert!(fixture.canonical("alpha").exists());
    assert!(!fixture.canonical("beta").exists());
    assert!(!fixture.canonical("gamma").exists());

    let resumed = forest(&fixture.root, &["setup", "--json"]);

    assert_success(&resumed);
    let report: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "reused");
    assert_eq!(report["repositories"][1]["status"], "cloned");
    assert_eq!(report["repositories"][2]["status"], "cloned");
}

#[test]
fn nested_git_commands_ignore_inherited_repository_environment() {
    let fixture = Fixture::new();
    let unrelated = fixture.root.join("unrelated");
    git(&fixture.root, &["init", path(&unrelated)]);

    let output = Command::new(binary())
        .current_dir(&fixture.canonical)
        .env("GIT_DIR", unrelated.join(".git"))
        .args(["repos", "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["is_git_worktree"], true);
    assert_eq!(
        report["repositories"][0]["default_ref"],
        "refs/remotes/origin/main"
    );
}

#[test]
fn fetches_all_origins_before_creating_a_workspace() {
    let fixture = WorkspaceFixture::new();
    let canonical = fixture.canonical("alpha");
    let original = git_stdout(&canonical, &["rev-parse", "main"]);
    let publisher = fixture.root.join("alpha-publisher");
    let origin = fixture.root.join("alpha-origin.git");
    git(&fixture.root, &["clone", path(&origin), path(&publisher)]);
    git(&publisher, &["config", "user.name", "Forest Test"]);
    git(&publisher, &["config", "user.email", "forest@example.com"]);
    fs::write(publisher.join("published.txt"), "published\n").unwrap();
    git(&publisher, &["add", "published.txt"]);
    git(&publisher, &["commit", "-m", "published"]);
    git(&publisher, &["push", "origin", "main"]);
    let published = git_stdout(&publisher, &["rev-parse", "HEAD"]);

    assert_eq!(
        git_stdout(&canonical, &["rev-parse", "refs/remotes/origin/main"]),
        original
    );

    let fetched = forest(&fixture.root, &["fetch", "--json"]);

    assert_success(&fetched);
    let report: Value = serde_json::from_slice(&fetched.stdout).unwrap();
    let repositories = report["repositories"].as_array().unwrap();
    assert_eq!(repositories.len(), 3);
    assert!(
        repositories
            .iter()
            .all(|repository| repository["status"] == "fetched")
    );
    assert_eq!(
        git_stdout(&canonical, &["rev-parse", "refs/remotes/origin/main"]),
        published
    );
    assert_eq!(git_stdout(&canonical, &["rev-parse", "main"]), original);

    let created = forest(&fixture.root, &["create", "fresh", "alpha", "--json"]);
    assert_success(&created);
    assert_eq!(
        git_stdout(
            &fixture.workspace("fresh").join("alpha"),
            &["rev-parse", "HEAD"]
        ),
        published
    );
}

#[test]
fn reports_fetch_failures_without_skipping_other_repositories() {
    let fixture = Fixture::new();

    let output = forest(&fixture.root, &["fetch", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["name"], "alpha");
    assert_eq!(report["repositories"][0]["status"], "fetched");
    assert_eq!(report["repositories"][1]["name"], "missing");
    assert_eq!(report["repositories"][1]["status"], "failed");
    assert!(
        report["repositories"][1]["message"]
            .as_str()
            .unwrap()
            .contains("does not exist")
    );
}

#[test]
fn discovers_master_as_a_default_without_guessing() {
    let fixture = WorkspaceFixture::with_default("master");

    let repositories = forest(&fixture.root, &["repos", "--json"]);
    assert_success(&repositories);
    let report: Value = serde_json::from_slice(&repositories.stdout).unwrap();
    assert_eq!(
        report["repositories"][0]["default_ref"],
        "refs/remotes/origin/master"
    );

    let created = forest(&fixture.root, &["create", "legacy", "alpha", "--json"]);
    assert_success(&created);
    let expected = git_stdout(
        &fixture.canonical("alpha"),
        &["rev-parse", "refs/remotes/origin/master"],
    );
    assert_eq!(
        git_stdout(
            &fixture.workspace("legacy").join("alpha"),
            &["rev-parse", "HEAD"]
        ),
        expected
    );
}

#[test]
fn requires_an_explicit_base_when_origin_head_is_missing() {
    let fixture = WorkspaceFixture::new();
    git(
        &fixture.canonical("alpha"),
        &["remote", "set-head", "origin", "-d"],
    );

    let missing = forest(&fixture.root, &["create", "base", "alpha", "--json"]);
    assert!(!missing.status.success());
    let report: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert!(
        report["repositories"][0]["message"]
            .as_str()
            .unwrap()
            .contains("--base alpha=<ref>")
    );

    let explicit = forest(
        &fixture.root,
        &[
            "create",
            "base",
            "alpha",
            "--base",
            "alpha=refs/remotes/origin/main",
            "--json",
        ],
    );
    assert_success(&explicit);
    let report: Value = serde_json::from_slice(&explicit.stdout).unwrap();
    assert_eq!(
        report["repositories"][0]["base_ref"],
        "refs/remotes/origin/main"
    );
}

#[test]
fn rejects_branch_namespace_conflicts_during_preflight() {
    let fixture = WorkspaceFixture::new();
    git(&fixture.canonical("alpha"), &["branch", "test", "main"]);

    let output = forest(&fixture.root, &["create", "namespace", "alpha", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert!(
        report["repositories"][0]["message"]
            .as_str()
            .unwrap()
            .contains("conflicts with existing branch test")
    );
    assert!(!fixture.workspace("namespace").exists());
}

#[test]
fn creates_multiple_worktrees_and_is_idempotent() {
    let fixture = WorkspaceFixture::new();

    let created = forest(
        &fixture.root,
        &["create", "topic", "alpha", "beta", "--json"],
    );
    assert_success(&created);
    let report: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "created");
    assert_eq!(report["repositories"][1]["status"], "created");
    assert_eq!(
        git_stdout(
            &fixture.workspace("topic").join("alpha"),
            &["branch", "--show-current"]
        ),
        "test/topic"
    );
    assert_eq!(
        git_stdout(
            &fixture.workspace("topic").join("beta"),
            &["branch", "--show-current"]
        ),
        "test/topic"
    );

    let repeated = forest(
        &fixture.root,
        &["create", "topic", "alpha", "beta", "--json"],
    );
    assert_success(&repeated);
    let report: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "reused");
    assert_eq!(report["repositories"][1]["status"], "reused");
}

#[test]
fn rejects_stale_registration_pointing_at_a_foreign_repository() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "foreign", "alpha", "--json"],
    ));
    let destination = fixture.workspace("foreign").join("alpha");
    fs::rename(&destination, fixture.root.join("displaced-alpha")).unwrap();
    git(
        &fixture.root,
        &[
            "init",
            "--initial-branch=totally-different",
            path(&destination),
        ],
    );
    git(&destination, &["config", "user.name", "Forest Test"]);
    git(
        &destination,
        &["config", "user.email", "forest@example.com"],
    );
    fs::write(destination.join("foreign.txt"), "foreign\n").unwrap();
    git(&destination, &["add", "foreign.txt"]);
    git(&destination, &["commit", "-m", "foreign"]);

    let output = forest(&fixture.root, &["create", "foreign", "alpha", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert!(
        report["repositories"][0]["message"]
            .as_str()
            .unwrap()
            .contains("does not belong to canonical repository")
    );
    assert_eq!(
        git_stdout(&destination, &["branch", "--show-current"]),
        "totally-different"
    );
}

#[test]
fn reports_partial_failure_and_safely_resumes() {
    let fixture = WorkspaceFixture::new();
    let fake_bin = fixture.root.join("fake-bin");
    fs::create_dir(&fake_bin).unwrap();
    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        r#"#!/bin/sh
if [ "$1" = "-C" ] && [ "$2" = "$FAIL_REPO" ] && [ "$3" = "worktree" ] && [ "$4" = "add" ]; then
  echo "simulated worktree creation failure" >&2
  exit 1
fi
exec "$REAL_GIT" "$@"
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake_git).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_git, permissions).unwrap();
    let existing_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![fake_bin];
    paths.extend(std::env::split_paths(&existing_path));
    let command_path = std::env::join_paths(paths).unwrap();

    let partial = Command::new(binary())
        .current_dir(&fixture.root)
        .env("PATH", command_path)
        .env("REAL_GIT", find_executable("git"))
        .env(
            "FAIL_REPO",
            fixture.canonical("beta").canonicalize().unwrap(),
        )
        .args(["create", "partial", "alpha", "beta", "--json"])
        .output()
        .unwrap();

    assert!(!partial.status.success());
    let report: Value = serde_json::from_slice(&partial.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "created");
    assert_eq!(report["repositories"][1]["status"], "failed");
    assert!(fixture.workspace("partial").join("alpha").exists());
    assert!(!fixture.workspace("partial").join("beta").exists());

    let resumed = forest(
        &fixture.root,
        &["create", "partial", "alpha", "beta", "--json"],
    );
    assert_success(&resumed);
    let report: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "reused");
    assert_eq!(report["repositories"][1]["status"], "created");
}

#[test]
fn adds_a_repository_to_an_existing_workspace() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "topic", "alpha", "--json"],
    ));

    let output = forest(&fixture.root, &["add", "topic", "gamma", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["name"], "gamma");
    assert_eq!(report["repositories"][0]["status"], "created");
    assert_eq!(
        git_stdout(
            &fixture.workspace("topic").join("gamma"),
            &["branch", "--show-current"]
        ),
        "test/topic"
    );
}

#[test]
fn adds_an_explicit_branch_to_an_existing_workspace() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "topic", "alpha", "--json"],
    ));
    git(
        &fixture.canonical("beta"),
        &["branch", "contributor/operator", "main"],
    );

    let output = forest(
        &fixture.root,
        &[
            "add",
            "topic",
            "beta",
            "--branch",
            "beta=contributor/operator",
            "--json",
        ],
    );

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["branch"], "contributor/operator");
    assert_eq!(report["repositories"][0]["action"], "add_existing_branch");
    assert_eq!(
        git_stdout(
            &fixture.workspace("topic").join("beta"),
            &["branch", "--show-current"]
        ),
        "contributor/operator"
    );
}

#[test]
fn reuses_a_preexisting_branch_that_is_not_checked_out() {
    let fixture = WorkspaceFixture::new();
    git(
        &fixture.canonical("alpha"),
        &["branch", "test/existing", "main"],
    );

    let output = forest(&fixture.root, &["create", "existing", "alpha", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["action"], "add_existing_branch");
    assert_eq!(report["repositories"][0]["base_ref"], Value::Null);
}

#[test]
fn reattaches_an_explicit_local_branch_after_removing_its_workspace() {
    let fixture = WorkspaceFixture::new();
    git(
        &fixture.canonical("alpha"),
        &["branch", "contributor/retained", "main"],
    );
    assert_success(&forest(
        &fixture.root,
        &[
            "create",
            "old-review",
            "alpha",
            "--branch",
            "alpha=contributor/retained",
            "--json",
        ],
    ));
    assert_success(&forest(&fixture.root, &["remove", "old-review", "--json"]));

    let output = forest(
        &fixture.root,
        &[
            "create",
            "recovered",
            "alpha",
            "--branch",
            "alpha=contributor/retained",
            "--json",
        ],
    );

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["branch"], "contributor/retained");
    assert_eq!(report["repositories"][0]["action"], "add_existing_branch");
    assert_eq!(report["repositories"][0]["base_ref"], Value::Null);
    assert_eq!(
        git_stdout(
            &fixture.workspace("recovered").join("alpha"),
            &["branch", "--show-current"]
        ),
        "contributor/retained"
    );
}

#[test]
fn creates_an_explicit_branch_tracking_origin() {
    let fixture = WorkspaceFixture::new();
    let canonical = fixture.canonical("alpha");
    let publisher = fixture.root.join("alpha-review-publisher");
    git(
        &fixture.root,
        &[
            "clone",
            path(&fixture.root.join("alpha-origin.git")),
            path(&publisher),
        ],
    );
    git(&publisher, &["config", "user.name", "Forest Test"]);
    git(&publisher, &["config", "user.email", "forest@example.com"]);
    git(&publisher, &["checkout", "-b", "contributor/review"]);
    fs::write(publisher.join("review.txt"), "review\n").unwrap();
    git(&publisher, &["add", "review.txt"]);
    git(&publisher, &["commit", "-m", "review change"]);
    let review_head = git_stdout(&publisher, &["rev-parse", "HEAD"]);
    git(&publisher, &["push", "origin", "contributor/review"]);
    assert_eq!(
        git_stdout(&canonical, &["branch", "--list", "contributor/review"]),
        ""
    );
    assert_success(&forest(&fixture.root, &["fetch", "alpha", "--json"]));
    git(
        &canonical,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/remotes/origin/contributor/review",
        ],
    );

    let output = forest(
        &fixture.root,
        &[
            "create",
            "pr-123",
            "alpha",
            "--branch",
            "alpha=contributor/review",
            "--json",
        ],
    );

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["branch"], "contributor/review");
    assert_eq!(report["repositories"][0]["action"], "create_branch");
    assert_eq!(
        report["repositories"][0]["base_ref"],
        "refs/remotes/origin/contributor/review"
    );
    let worktree = fixture.workspace("pr-123").join("alpha");
    assert_eq!(git_stdout(&worktree, &["rev-parse", "HEAD"]), review_head);
    assert_eq!(
        git_stdout(
            &worktree,
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ]
        ),
        "origin/contributor/review"
    );

    let repeated = forest(
        &fixture.root,
        &[
            "create",
            "pr-123",
            "alpha",
            "--branch",
            "alpha=contributor/review",
            "--json",
        ],
    );
    assert_success(&repeated);
    let report: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "reused");
}

#[test]
fn explicit_branch_requires_a_local_or_origin_branch_before_mutation() {
    let fixture = WorkspaceFixture::new();

    let output = forest(
        &fixture.root,
        &[
            "create",
            "missing-branch",
            "alpha",
            "beta",
            "--branch",
            "alpha=contributor/missing",
            "--json",
        ],
    );

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert!(
        report["repositories"][0]["message"]
            .as_str()
            .unwrap()
            .contains("git forest fetch alpha")
    );
    assert_eq!(report["repositories"][1]["status"], "not_run");
    assert!(!fixture.workspace("missing-branch").exists());
}

#[test]
fn rejects_branch_and_base_overrides_for_the_same_repository() {
    let fixture = WorkspaceFixture::new();

    let output = forest(
        &fixture.root,
        &[
            "create",
            "ambiguous",
            "alpha",
            "--branch",
            "alpha=contributor/review",
            "--base",
            "alpha=main",
            "--json",
        ],
    );

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["exit_code"], 2);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("branch and base overrides")
    );
    assert!(!fixture.workspace("ambiguous").exists());
}

#[test]
fn preflight_rejects_a_branch_checked_out_elsewhere_without_mutation() {
    let fixture = WorkspaceFixture::new();
    let other = fixture.root.join("other-alpha");
    git(
        &fixture.canonical("alpha"),
        &[
            "worktree",
            "add",
            "-b",
            "test/conflict",
            path(&other),
            "main",
        ],
    );

    let output = forest(
        &fixture.root,
        &["create", "conflict", "alpha", "beta", "--json"],
    );

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert_eq!(report["repositories"][1]["status"], "not_run");
    assert!(!fixture.workspace("conflict").exists());
}

#[test]
fn rejects_unknown_repositories_before_mutation() {
    let fixture = WorkspaceFixture::new();

    let output = forest(
        &fixture.root,
        &["create", "unknown", "alpha", "nope", "--json"],
    );

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown repository")
    );
    assert_eq!(error["error"]["exit_code"], 2);
    assert!(!fixture.workspace("unknown").exists());
}

#[test]
fn lists_workspaces_and_prints_a_composable_path() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "listed", "alpha", "beta", "--json"],
    ));

    let listed = forest(&fixture.root, &["list", "--json"]);
    assert_success(&listed);
    let report: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(report["workspaces"][0]["name"], "listed");
    assert_eq!(report["workspaces"][0]["repositories"][0]["name"], "alpha");
    assert_eq!(
        report["workspaces"][0]["repositories"][0]["branch"],
        "test/listed"
    );
    assert_eq!(report["workspaces"][0]["repositories"][1]["name"], "beta");

    let path_output = forest(&fixture.root, &["path", "listed"]);
    assert_success(&path_output);
    assert_eq!(
        String::from_utf8(path_output.stdout).unwrap(),
        format!("{}\n", fixture.workspace("listed").display())
    );
    assert!(path_output.stderr.is_empty());

    let json_path = forest(&fixture.root, &["path", "listed", "--json"]);
    assert_success(&json_path);
    let report: Value = serde_json::from_slice(&json_path.stdout).unwrap();
    assert_eq!(report["workspace"], "listed");
    assert_eq!(report["path"], path(&fixture.workspace("listed")));

    let from_worktree = forest(
        &fixture.workspace("listed").join("alpha"),
        &["status", "listed", "--json"],
    );
    assert_success(&from_worktree);
    let report: Value = serde_json::from_slice(&from_worktree.stdout).unwrap();
    assert_eq!(report["workspaces"][0]["name"], "listed");
}

#[test]
fn attaches_a_workspace_with_single_pane_herdr_tabs() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "topic", "alpha", "gamma", "--json"],
    ));
    write_config(
        &fixture.root.join(".forest.toml"),
        &["gamma", "beta", "alpha"],
    );
    let herdr = FakeHerdr::new(&fixture.root);

    let output = herdr
        .command(&fixture.root)
        .env("GIT_DIR", fixture.root.join("unrelated.git"))
        .args(["attach", "topic", "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["workspace"], "topic");
    assert_eq!(report["path"], path(&fixture.workspace("topic")));
    assert_eq!(report["herdr_workspace_id"], "w-new");
    assert_eq!(report["status"], "created");
    let tabs = report["tabs"].as_array().unwrap();
    assert_eq!(tabs.len(), 3);
    assert_eq!(tabs[0]["label"], "1-main");
    assert_eq!(tabs[0]["path"], path(&fixture.workspace("topic")));
    assert_eq!(tabs[0]["status"], "created");
    assert_eq!(tabs[1]["label"], "2-gamma");
    assert_eq!(
        tabs[1]["path"],
        path(&fixture.workspace("topic").join("gamma"))
    );
    assert_eq!(tabs[2]["label"], "3-alpha");
    assert_eq!(
        tabs[2]["path"],
        path(&fixture.workspace("topic").join("alpha"))
    );

    let calls = herdr.calls();
    assert!(calls.contains(&"workspace\tlist".to_owned()));
    assert!(calls.contains(&format!(
        "workspace\tcreate\t--cwd\t{}\t--label\ttopic\t--no-focus",
        fixture.workspace("topic").display()
    )));
    assert!(calls.contains(&format!(
        "workspace\treport-metadata\tw-new\t--source\tgit-forest\t--token\tgit_forest_path={}",
        fixture.workspace("topic").display()
    )));
    assert!(
        calls.contains(
            &"pane\treport-metadata\tw-new:p-main\t--source\tgit-forest\t--token\tgit_forest_tab=main"
                .to_owned()
        )
    );
    assert!(calls.contains(&"tab\trename\tw-new:t-main\t1-main".to_owned()));
    assert!(calls.contains(&format!(
        "tab\tcreate\t--workspace\tw-new\t--cwd\t{}\t--label\t2-gamma\t--no-focus",
        fixture.workspace("topic").join("gamma").display()
    )));
    assert!(calls.contains(&format!(
        "tab\tcreate\t--workspace\tw-new\t--cwd\t{}\t--label\t3-alpha\t--no-focus",
        fixture.workspace("topic").join("alpha").display()
    )));
    assert!(!calls.iter().any(|call| call.contains("beta")));
    assert_eq!(
        &calls[calls.len() - 2..],
        ["workspace\tfocus\tw-new", "tab\tfocus\tw-new:t-main"]
    );
}

#[test]
fn reconciles_and_focuses_an_existing_herdr_workspace() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "topic", "alpha", "beta", "--json"],
    ));
    let herdr = FakeHerdr::new(&fixture.root);
    let forest_workspace = fixture.workspace("topic");
    let workspace_path = path(&forest_workspace);
    let workspaces = serde_json::json!({
        "result": {
            "workspaces": [{
                "workspace_id": "w-existing",
                "tokens": {"git_forest_path": workspace_path}
            }]
        }
    });
    let tabs = serde_json::json!({
        "result": {
            "tabs": [
                {"tab_id": "w-existing:t-main", "label": "1-main", "number": 1, "pane_count": 1},
                {"tab_id": "w-existing:t-alpha", "label": "alpha-old", "number": 2, "pane_count": 1}
            ]
        }
    });
    let panes = serde_json::json!({
        "result": {
            "panes": [
                {
                    "pane_id": "w-existing:p-main",
                    "tab_id": "w-existing:t-main",
                    "tokens": {"git_forest_tab": "main"}
                },
                {
                    "pane_id": "w-existing:p-alpha",
                    "tab_id": "w-existing:t-alpha",
                    "tokens": {"git_forest_tab": "repository:alpha"}
                }
            ]
        }
    });

    let output = herdr
        .command(&fixture.root)
        .env("HERDR_WORKSPACES_RESPONSE", workspaces.to_string())
        .env("HERDR_TABS_RESPONSE", tabs.to_string())
        .env("HERDR_PANES_RESPONSE", panes.to_string())
        .args(["attach", "topic", "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["herdr_workspace_id"], "w-existing");
    assert_eq!(report["status"], "reconciled");
    assert_eq!(report["tabs"][0]["status"], "reused");
    assert_eq!(report["tabs"][1]["status"], "reconciled");
    assert_eq!(report["tabs"][2]["status"], "created");

    let calls = herdr.calls();
    assert!(!calls.iter().any(|call| call == "workspace\tcreate"));
    assert!(calls.contains(&"tab\trename\tw-existing:t-alpha\t2-alpha".to_owned()));
    assert!(calls.iter().any(
        |call| call.starts_with("tab\tcreate\t--workspace\tw-existing\t")
            && call.contains("\t3-beta\t")
    ));
    assert_eq!(
        &calls[calls.len() - 2..],
        [
            "workspace\tfocus\tw-existing",
            "tab\tfocus\tw-existing:t-main"
        ]
    );
}

#[test]
fn recovers_a_managed_tab_after_its_tagged_root_pane_is_closed() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "topic", "alpha", "--json"],
    ));
    let herdr = FakeHerdr::new(&fixture.root);
    let workspaces = serde_json::json!({
        "result": {
            "workspaces": [{
                "workspace_id": "w-existing",
                "tokens": {"git_forest_path": path(&fixture.workspace("topic"))}
            }]
        }
    });
    let tabs = serde_json::json!({
        "result": {
            "tabs": [
                {"tab_id": "w-existing:t-main", "label": "1-main", "number": 1, "pane_count": 1},
                {"tab_id": "w-existing:t-alpha", "label": "2-alpha", "number": 2, "pane_count": 2}
            ]
        }
    });
    let panes = serde_json::json!({
        "result": {
            "panes": [
                {
                    "pane_id": "w-existing:p-main",
                    "tab_id": "w-existing:t-main",
                    "tokens": {"git_forest_tab": "main"}
                },
                {
                    "pane_id": "w-existing:p-alpha-first",
                    "tab_id": "w-existing:t-alpha",
                    "cwd": path(&fixture.workspace("topic").join("alpha"))
                },
                {
                    "pane_id": "w-existing:p-alpha-second",
                    "tab_id": "w-existing:t-alpha",
                    "cwd": path(&fixture.workspace("topic").join("alpha"))
                }
            ]
        }
    });

    let output = herdr
        .command(&fixture.root)
        .env("HERDR_WORKSPACES_RESPONSE", workspaces.to_string())
        .env("HERDR_TABS_RESPONSE", tabs.to_string())
        .env("HERDR_PANES_RESPONSE", panes.to_string())
        .args(["attach", "topic", "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "reconciled");
    assert_eq!(report["tabs"][0]["status"], "reused");
    assert_eq!(report["tabs"][1]["herdr_tab_id"], "w-existing:t-alpha");
    assert_eq!(report["tabs"][1]["status"], "reconciled");

    let calls = herdr.calls();
    assert!(!calls.iter().any(|call| call.starts_with("tab\tcreate")));
    assert!(calls.contains(&"pane\treport-metadata\tw-existing:p-alpha-first\t--source\tgit-forest\t--token\tgit_forest_tab=repository:alpha".to_owned()));
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.contains("git_forest_tab=repository:alpha"))
            .count(),
        1
    );
}

#[test]
fn numbers_a_repository_inserted_in_config_order_by_its_herdr_tab_position() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "topic", "alpha", "gamma", "--json"],
    ));
    assert_success(&forest(&fixture.root, &["add", "topic", "beta", "--json"]));
    let herdr = FakeHerdr::new(&fixture.root);
    let workspaces = serde_json::json!({
        "result": {
            "workspaces": [{
                "workspace_id": "w-existing",
                "tokens": {"git_forest_path": path(&fixture.workspace("topic"))}
            }]
        }
    });
    let tabs = serde_json::json!({
        "result": {
            "tabs": [
                {"tab_id": "w-existing:t-main", "label": "1-main", "number": 1, "pane_count": 1},
                {"tab_id": "w-existing:t-alpha", "label": "2-alpha", "number": 2, "pane_count": 1},
                {"tab_id": "w-existing:t-gamma", "label": "3-gamma", "number": 3, "pane_count": 1}
            ]
        }
    });
    let panes = serde_json::json!({
        "result": {
            "panes": [
                {"pane_id": "w-existing:p-main", "tab_id": "w-existing:t-main", "tokens": {"git_forest_tab": "main"}},
                {"pane_id": "w-existing:p-alpha", "tab_id": "w-existing:t-alpha", "tokens": {"git_forest_tab": "repository:alpha"}},
                {"pane_id": "w-existing:p-gamma", "tab_id": "w-existing:t-gamma", "tokens": {"git_forest_tab": "repository:gamma"}}
            ]
        }
    });

    let output = herdr
        .command(&fixture.root)
        .env("HERDR_WORKSPACES_RESPONSE", workspaces.to_string())
        .env("HERDR_TABS_RESPONSE", tabs.to_string())
        .env("HERDR_PANES_RESPONSE", panes.to_string())
        .args(["attach", "topic", "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "reconciled");
    let tabs = report["tabs"].as_array().unwrap();
    assert_eq!(tabs[0]["label"], "1-main");
    assert_eq!(tabs[1]["label"], "2-alpha");
    assert_eq!(tabs[2]["label"], "4-beta");
    assert_eq!(tabs[2]["status"], "created");
    assert_eq!(tabs[3]["label"], "3-gamma");
    assert_eq!(tabs[3]["status"], "reused");

    let calls = herdr.calls();
    assert!(calls.iter().any(
        |call| call.starts_with("tab\tcreate\t--workspace\tw-existing\t")
            && call.contains("\t4-beta\t")
    ));
    assert!(!calls.iter().any(|call| call.starts_with("tab\trename")));
}

#[test]
fn recovers_an_untagged_partial_herdr_workspace() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "topic", "alpha", "beta", "--json"],
    ));
    let herdr = FakeHerdr::new(&fixture.root);
    let workspaces = serde_json::json!({
        "result": {
            "workspaces": [{
                "workspace_id": "w-partial",
                "label": "topic"
            }]
        }
    });
    let tabs = serde_json::json!({
        "result": {
            "tabs": [
                {"tab_id": "w-partial:t-main", "label": "topic", "number": 1, "pane_count": 1}
            ]
        }
    });
    let panes = serde_json::json!({
        "result": {
            "panes": [{
                "pane_id": "w-partial:p-main",
                "tab_id": "w-partial:t-main",
                "cwd": path(&fixture.workspace("topic"))
            }]
        }
    });

    let output = herdr
        .command(&fixture.root)
        .env("HERDR_WORKSPACES_RESPONSE", workspaces.to_string())
        .env("HERDR_TABS_RESPONSE", tabs.to_string())
        .env("HERDR_PANES_RESPONSE", panes.to_string())
        .args(["attach", "topic", "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["herdr_workspace_id"], "w-partial");
    assert_eq!(report["status"], "reconciled");
    assert_eq!(report["tabs"][0]["status"], "reconciled");
    assert_eq!(report["tabs"][1]["status"], "created");
    assert_eq!(report["tabs"][2]["status"], "created");

    let calls = herdr.calls();
    assert!(
        !calls
            .iter()
            .any(|call| call.starts_with("workspace\tcreate"))
    );
    assert!(calls.contains(&format!(
        "workspace\treport-metadata\tw-partial\t--source\tgit-forest\t--token\tgit_forest_path={}",
        fixture.workspace("topic").display()
    )));
    assert!(calls.contains(&"pane\treport-metadata\tw-partial:p-main\t--source\tgit-forest\t--token\tgit_forest_tab=main".to_owned()));
    assert!(calls.contains(&"tab\trename\tw-partial:t-main\t1-main".to_owned()));
}

#[test]
fn rejects_multiple_matching_herdr_workspaces_before_mutation() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "topic", "alpha", "--json"],
    ));
    let herdr = FakeHerdr::new(&fixture.root);
    let workspaces = serde_json::json!({
        "result": {
            "workspaces": [
                {
                    "workspace_id": "w-first",
                    "tokens": {"git_forest_path": path(&fixture.workspace("topic"))}
                },
                {
                    "workspace_id": "w-second",
                    "tokens": {"git_forest_path": path(&fixture.workspace("topic"))}
                }
            ]
        }
    });

    let output = herdr
        .command(&fixture.root)
        .env("HERDR_WORKSPACES_RESPONSE", workspaces.to_string())
        .args(["attach", "topic", "--json"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["exit_code"], 1);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("multiple Herdr workspaces match")
    );
    assert_eq!(herdr.calls(), ["workspace\tlist"]);
}

#[test]
fn reuses_a_complete_herdr_workspace_without_duplicating_tabs() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "topic", "alpha", "beta", "--json"],
    ));
    let herdr = FakeHerdr::new(&fixture.root);
    let workspaces = serde_json::json!({
        "result": {
            "workspaces": [{
                "workspace_id": "w-existing",
                "tokens": {"git_forest_path": path(&fixture.workspace("topic"))}
            }]
        }
    });
    let tabs = serde_json::json!({
        "result": {
            "tabs": [
                {"tab_id": "w-existing:t-main", "label": "1-main", "number": 1, "pane_count": 1},
                {"tab_id": "w-existing:t-alpha", "label": "2-alpha", "number": 2, "pane_count": 1},
                {"tab_id": "w-existing:t-beta", "label": "3-beta", "number": 3, "pane_count": 1}
            ]
        }
    });
    let panes = serde_json::json!({
        "result": {
            "panes": [
                {"pane_id": "w-existing:p-main", "tab_id": "w-existing:t-main", "tokens": {"git_forest_tab": "main"}},
                {"pane_id": "w-existing:p-alpha", "tab_id": "w-existing:t-alpha", "tokens": {"git_forest_tab": "repository:alpha"}},
                {"pane_id": "w-existing:p-beta", "tab_id": "w-existing:t-beta", "tokens": {"git_forest_tab": "repository:beta"}}
            ]
        }
    });

    let output = herdr
        .command(&fixture.root)
        .env("HERDR_WORKSPACES_RESPONSE", workspaces.to_string())
        .env("HERDR_TABS_RESPONSE", tabs.to_string())
        .env("HERDR_PANES_RESPONSE", panes.to_string())
        .args(["attach", "topic", "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "reused");
    assert!(
        report["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tab| tab["status"] == "reused")
    );
    let calls = herdr.calls();
    assert!(!calls.iter().any(|call| {
        call.starts_with("workspace\tcreate")
            || call.starts_with("tab\tcreate")
            || call.starts_with("tab\trename")
            || call.starts_with("pane\treport-metadata")
    }));
}

#[test]
fn reports_dirty_ahead_behind_and_detached_status() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "state", "alpha", "beta", "--json"],
    ));
    let alpha = fixture.workspace("state").join("alpha");
    let beta = fixture.workspace("state").join("beta");

    let clean = forest(&fixture.root, &["status", "state", "--json"]);
    assert_success(&clean);
    let clean: Value = serde_json::from_slice(&clean.stdout).unwrap();
    assert_eq!(clean["workspaces"][0]["repositories"][0]["dirty"], false);

    git(&alpha, &["branch", "--set-upstream-to=origin/main"]);
    fs::write(alpha.join("feature.txt"), "feature\n").unwrap();
    git(&alpha, &["add", "feature.txt"]);
    git(&alpha, &["commit", "-m", "feature"]);
    fs::write(alpha.join("untracked.txt"), "untracked\n").unwrap();

    let canonical = fixture.canonical("alpha");
    fs::write(canonical.join("main.txt"), "main\n").unwrap();
    git(&canonical, &["add", "main.txt"]);
    git(&canonical, &["commit", "-m", "advance main"]);
    git(&canonical, &["push", "origin", "main"]);
    git(&beta, &["checkout", "--detach"]);

    let output = forest(&fixture.root, &["status", "state", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let repositories = report["workspaces"][0]["repositories"].as_array().unwrap();
    assert_eq!(repositories[0]["name"], "alpha");
    assert_eq!(repositories[0]["branch"], "test/state");
    assert_eq!(repositories[0]["dirty"], true);
    assert_eq!(repositories[0]["upstream"], "origin/main");
    assert_eq!(repositories[0]["ahead"], 1);
    assert_eq!(repositories[0]["behind"], 1);
    assert_eq!(repositories[1]["name"], "beta");
    assert_eq!(repositories[1]["branch"], Value::Null);
    assert_eq!(repositories[1]["detached"], true);
}

#[test]
fn does_not_treat_a_nested_plain_directory_as_a_git_worktree() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("parent-repository");
    fs::create_dir_all(root.join("src/plain")).unwrap();
    git(&root, &["init"]);
    write_config(&root.join(".forest.toml"), &["plain"]);

    let output = forest(&root, &["repos", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["exists"], true);
    assert_eq!(report["repositories"][0]["is_git_worktree"], false);
}

#[test]
fn refuses_dirty_removal_before_removing_any_member() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "dirty", "alpha", "beta", "--json"],
    ));
    fs::write(
        fixture.workspace("dirty").join("alpha/untracked.txt"),
        "dirty\n",
    )
    .unwrap();

    let output = forest(
        &fixture.root,
        &["remove", "dirty", "alpha", "beta", "--json"],
    );

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert_eq!(report["repositories"][1]["status"], "not_run");
    assert!(fixture.workspace("dirty").join("alpha").exists());
    assert!(fixture.workspace("dirty").join("beta").exists());
}

#[test]
fn refuses_removal_when_a_tracked_file_is_modified() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "modified", "alpha", "--json"],
    ));
    let worktree = fixture.workspace("modified").join("alpha");
    fs::write(worktree.join("README.md"), "modified\n").unwrap();

    let output = forest(&fixture.root, &["remove", "modified", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert!(
        report["repositories"][0]["message"]
            .as_str()
            .unwrap()
            .contains("modified")
    );
    assert_eq!(
        fs::read_to_string(worktree.join("README.md")).unwrap(),
        "modified\n"
    );
}

#[test]
fn refuses_removal_of_a_worktree_moved_to_an_unexpected_path() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "moved", "alpha", "--json"],
    ));
    let expected = fixture.workspace("moved").join("alpha");
    let actual = fixture.workspace("moved").join("renamed");
    git(
        &fixture.canonical("alpha"),
        &["worktree", "move", path(&expected), path(&actual)],
    );

    let listed = forest(&fixture.root, &["list", "--json"]);
    assert_success(&listed);
    let listed: Value = serde_json::from_slice(&listed.stdout).unwrap();
    let repository = &listed["workspaces"][0]["repositories"][0];
    assert_eq!(repository["name"], "alpha");
    assert_eq!(repository["path"], path(&actual));
    assert_eq!(repository["registered"], true);

    let output = forest(&fixture.root, &["remove", "moved", "alpha", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert_eq!(report["repositories"][0]["path"], path(&actual));
    assert!(
        report["repositories"][0]["message"]
            .as_str()
            .unwrap()
            .contains("registered worktree layout does not match")
    );
    assert!(actual.exists());
    assert!(
        git_stdout(
            &fixture.canonical("alpha"),
            &["worktree", "list", "--porcelain"]
        )
        .contains(path(&actual))
    );
}

#[test]
fn refuses_removal_when_only_ignored_files_are_present() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "ignored", "alpha", "--json"],
    ));
    fs::write(
        fixture.canonical("alpha").join(".git/info/exclude"),
        "ignored.txt\n",
    )
    .unwrap();
    fs::write(
        fixture.workspace("ignored").join("alpha/ignored.txt"),
        "keep\n",
    )
    .unwrap();

    let output = forest(&fixture.root, &["remove", "ignored", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert!(
        fixture
            .workspace("ignored")
            .join("alpha/ignored.txt")
            .exists()
    );
}

#[test]
fn removes_clean_worktrees_preserves_branches_and_is_rerunnable() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "remove", "alpha", "beta", "--json"],
    ));

    let output = forest(&fixture.root, &["remove", "remove", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "removed");
    assert_eq!(report["repositories"][1]["status"], "removed");
    assert_eq!(report["workspace_removed"], true);
    assert!(!fixture.workspace("remove").exists());
    for name in ["alpha", "beta"] {
        git(
            &fixture.canonical(name),
            &["show-ref", "--verify", "--quiet", "refs/heads/test/remove"],
        );
    }

    let repeated = forest(&fixture.root, &["remove", "remove", "alpha", "--json"]);
    assert_success(&repeated);
    let report: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "already_absent");
}

#[test]
fn removes_only_explicitly_selected_members() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "subset", "alpha", "beta", "--json"],
    ));

    let output = forest(&fixture.root, &["remove", "subset", "alpha", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "removed");
    assert_eq!(report["workspace_removed"], false);
    assert!(!fixture.workspace("subset").join("alpha").exists());
    assert!(fixture.workspace("subset").join("beta").exists());
}

#[test]
fn preserves_unexpected_workspace_files_on_removal() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "preserve", "alpha", "--json"],
    ));
    let note = fixture.workspace("preserve").join("notes.txt");
    fs::write(&note, "keep\n").unwrap();

    let output = forest(&fixture.root, &["remove", "preserve", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "removed");
    assert_eq!(report["workspace_removed"], false);
    assert_eq!(report["remaining_entries"][0], path(&note));
    assert!(note.exists());
}

#[test]
fn rejects_a_conflicting_destination_path() {
    let fixture = WorkspaceFixture::new();
    let destination = fixture.workspace("occupied").join("alpha");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("keep.txt"), "keep\n").unwrap();

    let output = forest(&fixture.root, &["create", "occupied", "alpha", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert!(destination.join("keep.txt").exists());
}

#[test]
fn emits_json_for_usage_errors_when_requested() {
    let output = Command::new(binary())
        .args(["create", "missing-repositories", "--json"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["exit_code"], 2);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("required arguments were not provided")
    );
}

#[test]
fn direct_and_git_subcommand_invocations_match() {
    let fixture = Fixture::new();
    let direct = forest(&fixture.canonical, &["repos", "--json"]);
    assert_success(&direct);

    let bin_dir = fixture.root.join("bin");
    fs::create_dir(&bin_dir).unwrap();
    fs::copy(binary(), bin_dir.join("git-forest")).unwrap();
    let existing_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin_dir];
    paths.extend(std::env::split_paths(&existing_path));
    let command_path = std::env::join_paths(paths).unwrap();
    let through_git = Command::new("git")
        .current_dir(&fixture.canonical)
        .env("PATH", command_path)
        .args(["forest", "repos", "--json"])
        .output()
        .unwrap();

    assert_success(&through_git);
    assert_eq!(through_git.stdout, direct.stdout);
    assert_eq!(through_git.stderr, direct.stderr);
}

fn initialize_repository(root: &Path, name: &str, default_branch: &str) {
    let canonical = root.join("src").join(name);
    let origin = root.join(format!("{name}-origin.git"));
    git(
        root,
        &[
            "init",
            "--bare",
            &format!("--initial-branch={default_branch}"),
            path(&origin),
        ],
    );
    git(
        root,
        &[
            "init",
            &format!("--initial-branch={default_branch}"),
            path(&canonical),
        ],
    );
    git(&canonical, &["config", "user.name", "Forest Test"]);
    git(&canonical, &["config", "user.email", "forest@example.com"]);
    fs::write(canonical.join("README.md"), format!("{name}\n")).unwrap();
    git(&canonical, &["add", "README.md"]);
    git(&canonical, &["commit", "-m", "initial"]);
    git(&canonical, &["remote", "add", "origin", path(&origin)]);
    git(&canonical, &["push", "-u", "origin", default_branch]);
    git(
        &canonical,
        &["remote", "set-head", "origin", default_branch],
    );
}

fn write_config(path: &Path, members: &[&str]) {
    write_config_with_remote(path, members, Some("git@example.com:{name}.git"));
}

fn write_config_with_remote(path: &Path, members: &[&str], remote: Option<&str>) {
    let members = members
        .iter()
        .map(|member| format!("  {member:?},"))
        .collect::<Vec<_>>()
        .join("\n");
    let remote = remote
        .map(|remote| format!("remote = {remote:?}\n"))
        .unwrap_or_default();
    fs::write(
        path,
        format!(
            r#"version = 1

[repositories]
root = "src"
{remote}members = [
{members}
]

[workspaces]
root = "src/.workspaces"
branch = "test/{{workspace}}"
"#,
        ),
    )
    .unwrap();
}

fn forest(current_dir: &Path, arguments: &[&str]) -> Output {
    Command::new(binary())
        .current_dir(current_dir)
        .env_remove("FOREST_CONFIG")
        .args(arguments)
        .output()
        .unwrap()
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_git-forest")
}

fn git(current_dir: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(current_dir)
        .args(arguments)
        .output()
        .unwrap();
    assert_success(&output);
}

fn git_stdout(current_dir: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(current_dir)
        .args(arguments)
        .output()
        .unwrap();
    assert_success(&output);
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn find_executable(name: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("could not find {name} on PATH"))
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}
