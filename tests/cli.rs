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

    fn archived_workspace(&self, workspace: &str) -> PathBuf {
        self.workspace(".archive").join(workspace)
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
fn setup_resolves_relative_remotes_from_the_configuration_directory() {
    let fixture = WorkspaceFixture::without_clones();
    write_config_with_remote(
        &fixture.root.join(".forest.toml"),
        &["alpha", "beta", "gamma"],
        Some("{name}-origin.git"),
    );
    let nested = fixture.root.join("src/nested");
    fs::create_dir_all(&nested).unwrap();

    let output = forest(&nested, &["setup", "--json"]);

    assert_success(&output);
    for name in ["alpha", "beta", "gamma"] {
        assert_eq!(
            git_stdout(&fixture.canonical(name), &["branch", "--show-current"]),
            "main"
        );
    }
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
remote=""
destination=""
after_separator=false
for argument in "$@"; do
  if [ "$after_separator" = true ] && [ -z "$remote" ]; then
    remote="$argument"
  fi
  if [ "$argument" = "--" ]; then
    after_separator=true
  fi
  destination="$argument"
done
if [ "$remote" = "$FAIL_REMOTE" ]; then
  mkdir "$destination/.git"
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
        .env("FAIL_REMOTE", fixture.root.join("beta-origin.git"))
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
    assert!(
        fs::read_dir(fixture.root.join("src"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".git-forest-clone-"))
    );

    let interrupted_staging = fixture.root.join("src/.git-forest-clone-interrupted/.git");
    fs::create_dir_all(interrupted_staging).unwrap();
    let resumed = forest(&fixture.root, &["setup", "--json"]);

    assert_success(&resumed);
    let report: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "reused");
    assert_eq!(report["repositories"][1]["status"], "cloned");
    assert_eq!(report["repositories"][2]["status"], "cloned");
}

#[test]
fn setup_does_not_publish_over_a_destination_created_during_clone() {
    let fixture = WorkspaceFixture::without_clones();
    let fake_bin = fixture.root.join("fake-publish-bin");
    fs::create_dir(&fake_bin).unwrap();
    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        r#"#!/bin/sh
remote=""
after_separator=false
for argument in "$@"; do
  if [ "$after_separator" = true ] && [ -z "$remote" ]; then
    remote="$argument"
  fi
  if [ "$argument" = "--" ]; then
    after_separator=true
  fi
done
"$REAL_GIT" "$@"
status=$?
if [ "$status" -eq 0 ] && [ "$remote" = "$RACE_REMOTE" ]; then
  mkdir "$PUBLISH_DESTINATION"
fi
exit "$status"
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
    let destination = fixture.canonical("alpha");

    let output = Command::new(binary())
        .current_dir(&fixture.root)
        .env("PATH", command_path)
        .env("REAL_GIT", find_executable("git"))
        .env("RACE_REMOTE", fixture.root.join("alpha-origin.git"))
        .env("PUBLISH_DESTINATION", &destination)
        .args(["setup", "--json"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "failed");
    assert_eq!(report["repositories"][1]["status"], "not_run");
    assert_eq!(report["repositories"][2]["status"], "not_run");
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    assert!(
        fs::read_dir(fixture.root.join("src"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".git-forest-clone-"))
    );
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
    git(&publisher, &["tag", "remote-only-tag"]);
    git(&publisher, &["push", "origin", "main"]);
    git(&publisher, &["push", "origin", "remote-only-tag"]);
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
    assert_eq!(
        git_stdout(&canonical, &["tag", "--list", "remote-only-tag"]),
        ""
    );

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
fn fetches_repositories_concurrently_and_preserves_report_order() {
    let fixture = WorkspaceFixture::new();
    let fake_bin = fixture.root.join("fake-fetch-bin");
    let markers = fixture.root.join("fetch-markers");
    fs::create_dir(&fake_bin).unwrap();
    fs::create_dir(&markers).unwrap();
    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        r#"#!/bin/sh
if [ "$1" = "-C" ] && [ "$3" = "fetch" ]; then
  marker=$(basename "$2")
  : > "$FETCH_MARKERS/$marker"
  attempts=0
  while [ "$(find "$FETCH_MARKERS" -type f | wc -l | tr -d ' ')" -lt 3 ]; do
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 100 ]; then
      echo "fetches did not run concurrently" >&2
      exit 1
    fi
    sleep 0.02
  done
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

    let output = Command::new(binary())
        .current_dir(&fixture.root)
        .env("PATH", command_path)
        .env("REAL_GIT", find_executable("git"))
        .env("FETCH_MARKERS", markers)
        .args(["fetch", "--jobs", "3", "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let names = report["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|repository| repository["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, ["alpha", "beta", "gamma"]);
}

#[test]
fn update_fetches_and_fast_forwards_discovered_default_branches() {
    let fixture = WorkspaceFixture::with_default("master");
    let canonical = fixture.canonical("alpha");
    let original = git_stdout(&canonical, &["rev-parse", "master"]);
    let publisher = clone_publisher(&fixture, "alpha");
    git(&publisher, &["switch", "-c", "topic"]);
    publish_file(&publisher, "topic.txt", "topic\n", "topic");
    git(&publisher, &["switch", "master"]);
    let published = publish_file(&publisher, "published.txt", "published\n", "published");

    let output = forest(&fixture.root, &["update", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["name"], "alpha");
    assert_eq!(report["repositories"][0]["branch"], "master");
    assert_eq!(report["repositories"][0]["status"], "updated");
    assert_eq!(report["repositories"][1]["status"], "up_to_date");
    assert_eq!(report["repositories"][2]["status"], "up_to_date");
    assert_ne!(published, original);
    assert_eq!(git_stdout(&canonical, &["rev-parse", "master"]), published);
    assert_eq!(
        fs::read_to_string(canonical.join("published.txt")).unwrap(),
        "published\n"
    );
    assert_eq!(
        git_stdout(
            &canonical,
            &["branch", "--remotes", "--list", "origin/topic"]
        ),
        ""
    );
}

#[test]
fn update_refuses_dirty_default_branches_after_fetching() {
    let fixture = WorkspaceFixture::new();
    let canonical = fixture.canonical("alpha");
    let original = git_stdout(&canonical, &["rev-parse", "main"]);
    let publisher = clone_publisher(&fixture, "alpha");
    let published = publish_file(&publisher, "published.txt", "published\n", "published");
    fs::write(canonical.join("local.txt"), "dirty\n").unwrap();

    let output = forest(&fixture.root, &["update", "alpha", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["branch"], "main");
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert!(
        report["repositories"][0]["message"]
            .as_str()
            .unwrap()
            .contains("uncommitted changes")
    );
    assert_eq!(git_stdout(&canonical, &["rev-parse", "main"]), original);
    assert_eq!(
        git_stdout(&canonical, &["rev-parse", "refs/remotes/origin/main"]),
        published
    );
    assert_eq!(
        fs::read_to_string(canonical.join("local.txt")).unwrap(),
        "dirty\n"
    );
}

#[test]
fn update_preserves_ignored_files_that_conflict_with_incoming_changes() {
    let fixture = WorkspaceFixture::new();
    let canonical = fixture.canonical("alpha");
    let original = git_stdout(&canonical, &["rev-parse", "main"]);
    fs::write(canonical.join(".git/info/exclude"), "local-generated.txt\n").unwrap();
    fs::write(canonical.join("local-generated.txt"), "local data\n").unwrap();
    let publisher = clone_publisher(&fixture, "alpha");
    let published = publish_file(
        &publisher,
        "local-generated.txt",
        "remote data\n",
        "publish generated file",
    );

    let output = forest(&fixture.root, &["update", "alpha", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert!(
        report["repositories"][0]["message"]
            .as_str()
            .unwrap()
            .contains("ignored path")
    );
    assert_eq!(git_stdout(&canonical, &["rev-parse", "main"]), original);
    assert_eq!(
        git_stdout(&canonical, &["rev-parse", "refs/remotes/origin/main"]),
        published
    );
    assert_eq!(
        fs::read_to_string(canonical.join("local-generated.txt")).unwrap(),
        "local data\n"
    );
}

#[test]
fn update_refuses_diverged_default_branches() {
    let fixture = WorkspaceFixture::new();
    let canonical = fixture.canonical("alpha");
    let publisher = clone_publisher(&fixture, "alpha");
    fs::write(canonical.join("local.txt"), "local\n").unwrap();
    git(&canonical, &["add", "local.txt"]);
    git(&canonical, &["commit", "-m", "local"]);
    let local = git_stdout(&canonical, &["rev-parse", "main"]);
    publish_file(&publisher, "remote.txt", "remote\n", "remote");

    let output = forest(&fixture.root, &["update", "alpha", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert!(
        report["repositories"][0]["message"]
            .as_str()
            .unwrap()
            .contains("local commit(s)")
    );
    assert_eq!(git_stdout(&canonical, &["rev-parse", "main"]), local);
}

#[test]
fn update_moves_a_default_branch_that_is_not_checked_out() {
    let fixture = WorkspaceFixture::new();
    let canonical = fixture.canonical("alpha");
    let original = git_stdout(&canonical, &["rev-parse", "main"]);
    git(&canonical, &["switch", "-c", "topic"]);
    let publisher = clone_publisher(&fixture, "alpha");
    let published = publish_file(&publisher, "published.txt", "published\n", "published");

    let output = forest(&fixture.root, &["update", "alpha", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "updated");
    assert_eq!(
        git_stdout(&canonical, &["branch", "--show-current"]),
        "topic"
    );
    assert_eq!(git_stdout(&canonical, &["rev-parse", "HEAD"]), original);
    assert_eq!(git_stdout(&canonical, &["rev-parse", "main"]), published);
    assert!(!canonical.join("published.txt").exists());
}

#[test]
fn update_does_not_replace_a_branch_changed_after_fast_forward_validation() {
    let fixture = WorkspaceFixture::new();
    let canonical = fixture.canonical("alpha");
    git(&canonical, &["switch", "-c", "topic"]);
    fs::write(canonical.join("concurrent.txt"), "concurrent\n").unwrap();
    git(&canonical, &["add", "concurrent.txt"]);
    git(&canonical, &["commit", "-m", "concurrent local commit"]);
    let concurrent = git_stdout(&canonical, &["rev-parse", "HEAD"]);
    let publisher = clone_publisher(&fixture, "alpha");
    let published = publish_file(&publisher, "published.txt", "published\n", "published");
    assert_ne!(concurrent, published);

    let fake_bin = fixture.root.join("ref-race-bin");
    let race_marker = fixture.root.join("ref-race-marker");
    fs::create_dir(&fake_bin).unwrap();
    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        r#"#!/bin/sh
if [ "$1" = "-C" ] && [ "$3" = "worktree" ] && [ "$4" = "list" ]; then
  : > "$RACE_MARKER"
  "$REAL_GIT" -C "$2" update-ref refs/heads/main "$CONCURRENT_OID" || exit $?
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

    let output = Command::new(binary())
        .current_dir(&fixture.root)
        .env("PATH", command_path)
        .env("REAL_GIT", find_executable("git"))
        .env("RACE_MARKER", &race_marker)
        .env("CONCURRENT_OID", &concurrent)
        .args(["update", "alpha", "--json"])
        .output()
        .unwrap();

    assert!(race_marker.exists(), "the fake Git race hook did not run");
    assert!(
        !output.status.success(),
        "update unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "failed");
    assert_eq!(git_stdout(&canonical, &["rev-parse", "main"]), concurrent);
    assert_eq!(
        git_stdout(&canonical, &["rev-parse", "refs/remotes/origin/main"]),
        published
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
fn creates_an_empty_workspace_and_adds_a_worktree_later() {
    let fixture = WorkspaceFixture::new();
    let workspace = fixture.workspace("scratch");

    let created = forest(&fixture.root, &["create", "scratch", "--json"]);

    assert_success(&created);
    let report: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(report["workspace"], "scratch");
    assert_eq!(report["path"], path(&workspace));
    assert_eq!(report["repositories"].as_array().unwrap().len(), 0);
    assert!(workspace.is_dir());
    assert_eq!(fs::read_dir(&workspace).unwrap().count(), 0);

    let notes = workspace.join("notes.txt");
    fs::write(&notes, "scratch notes\n").unwrap();
    let repeated = forest(&fixture.root, &["create", "scratch", "--json"]);
    assert_success(&repeated);
    assert_eq!(fs::read_to_string(&notes).unwrap(), "scratch notes\n");

    let added = forest(&fixture.root, &["add", "scratch", "alpha", "--json"]);

    assert_success(&added);
    let report: Value = serde_json::from_slice(&added.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "created");
    assert!(workspace.join("alpha").is_dir());
    assert_eq!(fs::read_to_string(notes).unwrap(), "scratch notes\n");

    let no_op = forest(&fixture.root, &["create", "scratch"]);
    assert_success(&no_op);
    assert!(
        String::from_utf8(no_op.stdout)
            .unwrap()
            .contains("No checkouts requested.")
    );
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
fn manages_multiple_named_checkouts_for_one_repository() {
    let fixture = WorkspaceFixture::new();

    let created = forest(
        &fixture.root,
        &[
            "create",
            "stacked",
            "alpha",
            "beta",
            "beta@part-2",
            "--json",
        ],
    );
    assert_success(&created);
    let report: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(report["repositories"][0]["checkout"], "alpha");
    assert_eq!(report["repositories"][0]["slot"], Value::Null);
    assert_eq!(report["repositories"][1]["checkout"], "beta");
    assert_eq!(report["repositories"][2]["name"], "beta");
    assert_eq!(report["repositories"][2]["checkout"], "beta@part-2");
    assert_eq!(report["repositories"][2]["slot"], "part-2");
    assert_eq!(report["repositories"][2]["branch"], "test/part-2");
    assert_eq!(
        git_stdout(
            &fixture.workspace("stacked").join("beta"),
            &["branch", "--show-current"]
        ),
        "test/stacked"
    );
    assert_eq!(
        git_stdout(
            &fixture.workspace("stacked").join("beta@part-2"),
            &["branch", "--show-current"]
        ),
        "test/part-2"
    );

    let repeated = forest(
        &fixture.root,
        &[
            "create",
            "stacked",
            "alpha",
            "beta",
            "beta@part-2",
            "--json",
        ],
    );
    assert_success(&repeated);
    let report: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert!(
        report["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|checkout| checkout["status"] == "reused")
    );

    let listed = forest(&fixture.root, &["list", "--json"]);
    assert_success(&listed);
    let report: Value = serde_json::from_slice(&listed.stdout).unwrap();
    let checkouts = report["workspaces"][0]["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|checkout| checkout["checkout"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(checkouts, ["alpha", "beta", "beta@part-2"]);

    let herdr = FakeHerdr::new(&fixture.root);
    let attached = herdr
        .command(&fixture.root)
        .args(["attach", "stacked", "--json"])
        .output()
        .unwrap();
    assert_success(&attached);
    let report: Value = serde_json::from_slice(&attached.stdout).unwrap();
    assert_eq!(report["tabs"][3]["label"], "4-beta@part-2");
    assert!(herdr.calls().contains(
        &"pane\treport-metadata\tw-new:p-beta@part-2\t--source\tgit-forest\t--token\tgit_forest_tab=repository:beta@part-2"
            .to_owned()
    ));

    let removed = forest(
        &fixture.root,
        &["remove", "stacked", "beta@part-2", "--json"],
    );
    assert_success(&removed);
    let report: Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(report["repositories"][0]["name"], "beta");
    assert_eq!(report["repositories"][0]["checkout"], "beta@part-2");
    assert!(!fixture.workspace("stacked").join("beta@part-2").exists());
    assert!(fixture.workspace("stacked").join("beta").exists());
}

#[test]
fn applies_branch_overrides_to_named_checkouts() {
    let fixture = WorkspaceFixture::new();
    for branch in ["contributor/part-1", "contributor/part-2"] {
        git(&fixture.canonical("alpha"), &["branch", branch, "main"]);
    }

    let output = forest(
        &fixture.root,
        &[
            "create",
            "stacked",
            "alpha@part-1",
            "alpha@part-2",
            "--branch",
            "alpha@part-1=contributor/part-1",
            "--branch",
            "alpha@part-2=contributor/part-2",
            "--json",
        ],
    );

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["checkout"], "alpha@part-1");
    assert_eq!(report["repositories"][0]["branch"], "contributor/part-1");
    assert_eq!(report["repositories"][1]["checkout"], "alpha@part-2");
    assert_eq!(report["repositories"][1]["branch"], "contributor/part-2");
}

#[test]
fn preflight_rejects_checkouts_that_select_the_same_branch() {
    let fixture = WorkspaceFixture::new();
    write_config_with_branch(
        &fixture.root.join(".forest.toml"),
        &["alpha", "beta", "gamma"],
        "test/{workspace}",
    );

    let output = forest(
        &fixture.root,
        &["create", "stacked", "beta", "beta@part-2", "--json"],
    );

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "not_run");
    assert_eq!(report["repositories"][1]["status"], "conflict");
    assert!(
        report["repositories"][1]["message"]
            .as_str()
            .unwrap()
            .contains("select the same branch")
    );
    assert!(!fixture.workspace("stacked").exists());
}

#[test]
fn rejects_case_folded_checkout_destination_aliases() {
    let fixture = WorkspaceFixture::new();

    let output = forest(
        &fixture.root,
        &["create", "aliases", "alpha@Foo", "alpha@foo", "--json"],
    );

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["exit_code"], 2);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("same destination")
    );
    assert!(!fixture.workspace("aliases").exists());
}

#[test]
fn rejects_case_folded_existing_branch_namespace_conflicts() {
    let fixture = WorkspaceFixture::new();
    let canonical = fixture.canonical("alpha");
    git(&canonical, &["branch", "Foo", "main"]);
    git(&canonical, &["config", "core.ignoreCase", "true"]);
    write_config_with_branch(
        &fixture.root.join(".forest.toml"),
        &["alpha", "beta", "gamma"],
        "foo/{checkout}",
    );

    let output = forest(
        &fixture.root,
        &["create", "case-refs", "alpha@child", "--json"],
    );

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert!(
        report["repositories"][0]["message"]
            .as_str()
            .unwrap()
            .contains("existing branch Foo")
    );
    assert!(!fixture.workspace("case-refs").exists());
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
fn remove_all_removes_primary_and_named_checkouts() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &[
            "create",
            "remove-named",
            "alpha",
            "alpha@part-2",
            "beta",
            "--json",
        ],
    ));

    let output = forest(&fixture.root, &["remove", "remove-named", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let checkouts = report["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|checkout| checkout["checkout"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(checkouts, ["alpha", "alpha@part-2", "beta"]);
    assert!(
        report["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|checkout| checkout["status"] == "removed")
    );
    assert_eq!(report["workspace_removed"], true);
    assert!(!fixture.workspace("remove-named").exists());
    git(
        &fixture.canonical("alpha"),
        &["show-ref", "--verify", "--quiet", "refs/heads/test/part-2"],
    );
}

#[test]
fn dirty_named_checkout_blocks_remove_all() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "dirty-named", "alpha", "alpha@part-2", "--json"],
    ));
    let named = fixture.workspace("dirty-named").join("alpha@part-2");
    fs::write(named.join("untracked.txt"), "dirty\n").unwrap();

    let output = forest(&fixture.root, &["remove", "dirty-named", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["checkout"], "alpha");
    assert_eq!(report["repositories"][0]["status"], "not_run");
    assert_eq!(report["repositories"][1]["checkout"], "alpha@part-2");
    assert_eq!(report["repositories"][1]["status"], "conflict");
    assert!(fixture.workspace("dirty-named").join("alpha").exists());
    assert!(named.exists());
}

#[test]
fn force_removes_dirty_worktrees() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "force-dirty", "alpha", "beta", "--json"],
    ));
    let dirty = fixture.workspace("force-dirty").join("beta");
    fs::write(dirty.join("untracked.txt"), "dirty\n").unwrap();

    let output = forest(
        &fixture.root,
        &["remove", "force-dirty", "--force", "--json"],
    );

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let repositories = report["repositories"].as_array().unwrap();
    let alpha = repositories
        .iter()
        .find(|repository| repository["checkout"] == "alpha")
        .unwrap();
    assert_eq!(alpha["status"], "removed");
    assert_eq!(alpha["message"], Value::Null);
    let beta = repositories
        .iter()
        .find(|repository| repository["checkout"] == "beta")
        .unwrap();
    assert_eq!(beta["status"], "removed");
    assert_eq!(
        beta["message"],
        "discarded modified, untracked, or ignored files (--force)"
    );
    assert_eq!(report["workspace_removed"], true);
    assert!(!fixture.workspace("force-dirty").exists());
}

#[test]
fn force_remove_still_rejects_unregistered_paths() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "force-unregistered", "alpha", "--json"],
    ));
    let stray = fixture.workspace("force-unregistered").join("beta");
    fs::create_dir(&stray).unwrap();

    let output = forest(
        &fixture.root,
        &["remove", "force-unregistered", "--force", "--json"],
    );

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let repositories = report["repositories"].as_array().unwrap();
    let beta = repositories
        .iter()
        .find(|repository| repository["checkout"] == "beta")
        .unwrap();
    assert_eq!(beta["status"], "conflict");
    assert!(beta["message"].as_str().unwrap().contains("not registered"));
    assert!(stray.exists());
    assert!(
        fixture
            .workspace("force-unregistered")
            .join("alpha")
            .exists()
    );
}

#[test]
fn renames_workspace_and_repairs_worktrees_without_renaming_branches() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &[
            "create",
            "old-topic",
            "alpha",
            "alpha@part-2",
            "beta",
            "--json",
        ],
    ));
    let old_workspace = fixture.workspace("old-topic");
    let new_workspace = fixture.workspace("review-123");
    fs::write(old_workspace.join("alpha/README.md"), "modified\n").unwrap();
    fs::write(old_workspace.join("alpha/untracked.txt"), "dirty\n").unwrap();
    fs::write(old_workspace.join("notes.md"), "keep\n").unwrap();

    let output = forest(
        &fixture.root,
        &["rename", "old-topic", "review-123", "--json"],
    );

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["old_workspace"], "old-topic");
    assert_eq!(report["old_path"], path(&old_workspace));
    assert_eq!(report["workspace"], "review-123");
    assert_eq!(report["path"], path(&new_workspace));
    assert_eq!(report["status"], "renamed");
    assert!(
        report["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|repository| repository["status"] == "repaired")
    );
    assert!(!old_workspace.exists());
    assert_eq!(
        fs::read_to_string(new_workspace.join("alpha/README.md")).unwrap(),
        "modified\n"
    );
    assert_eq!(
        fs::read_to_string(new_workspace.join("alpha/untracked.txt")).unwrap(),
        "dirty\n"
    );
    assert_eq!(
        fs::read_to_string(new_workspace.join("notes.md")).unwrap(),
        "keep\n"
    );
    assert_eq!(
        git_stdout(&new_workspace.join("alpha"), &["branch", "--show-current"]),
        "test/old-topic"
    );
    assert_eq!(
        git_stdout(
            &new_workspace.join("alpha@part-2"),
            &["branch", "--show-current"]
        ),
        "test/part-2"
    );
    for (repository, checkout) in [
        ("alpha", "alpha"),
        ("alpha", "alpha@part-2"),
        ("beta", "beta"),
    ] {
        let worktrees = git_stdout(
            &fixture.canonical(repository),
            &["worktree", "list", "--porcelain"],
        );
        assert!(worktrees.contains(path(&new_workspace.join(checkout))));
        assert!(!worktrees.contains(path(&old_workspace.join(checkout))));
    }

    let listed = forest(&fixture.root, &["list", "--json"]);
    assert_success(&listed);
    let listed: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed["workspaces"][0]["name"], "review-123");
    assert_eq!(listed["workspaces"].as_array().unwrap().len(), 1);

    assert_success(&forest(
        &fixture.root,
        &["add", "review-123", "gamma", "--json"],
    ));
    assert_eq!(
        git_stdout(&new_workspace.join("gamma"), &["branch", "--show-current"]),
        "test/review-123"
    );
    assert_eq!(
        git_stdout(&new_workspace.join("alpha"), &["branch", "--show-current"]),
        "test/old-topic"
    );
}

#[test]
fn renames_empty_workspace_and_preserves_local_entries() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(&fixture.root, &["create", "scratch", "--json"]));
    fs::write(fixture.workspace("scratch").join("notes.md"), "keep\n").unwrap();

    let output = forest(&fixture.root, &["rename", "scratch", "notes", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "renamed");
    assert_eq!(report["repositories"], serde_json::json!([]));
    assert!(!fixture.workspace("scratch").exists());
    assert_eq!(
        fs::read_to_string(fixture.workspace("notes").join("notes.md")).unwrap(),
        "keep\n"
    );
}

#[test]
fn rename_rejects_active_and_archived_destinations_before_mutation() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "rename-source", "alpha", "--json"],
    ));
    fs::create_dir_all(fixture.workspace("occupied")).unwrap();

    let occupied = forest(
        &fixture.root,
        &["rename", "rename-source", "occupied", "--json"],
    );

    assert!(!occupied.status.success());
    let report: Value = serde_json::from_slice(&occupied.stdout).unwrap();
    assert_eq!(report["status"], "conflict");
    assert!(fixture.workspace("rename-source").join("alpha").exists());

    fs::create_dir_all(fixture.archived_workspace("reserved")).unwrap();
    let archived = forest(
        &fixture.root,
        &["rename", "rename-source", "reserved", "--json"],
    );
    assert!(!archived.status.success());
    let report: Value = serde_json::from_slice(&archived.stdout).unwrap();
    assert_eq!(report["status"], "conflict");
    assert!(report["message"].as_str().unwrap().contains("reserved"));
    assert!(fixture.workspace("rename-source").join("alpha").exists());
}

#[test]
fn rename_rejects_inconsistent_workspaces_before_mutation() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "inconsistent-rename", "--json"],
    ));
    fs::create_dir(fixture.workspace("inconsistent-rename").join("alpha")).unwrap();

    let output = forest(
        &fixture.root,
        &["rename", "inconsistent-rename", "renamed", "--json"],
    );

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "conflict");
    assert!(report["message"].as_str().unwrap().contains("inconsistent"));
    assert!(fixture.workspace("inconsistent-rename").exists());
    assert!(!fixture.workspace("renamed").exists());
}

#[test]
fn rename_resumes_after_a_worktree_repair_failure() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "partial-rename", "alpha", "beta", "--json"],
    ));
    let fake_bin = fixture.root.join("rename-fake-bin");
    fs::create_dir(&fake_bin).unwrap();
    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        r#"#!/bin/sh
if [ "$1" = "-C" ] && [ "$2" = "$FAIL_REPO" ] && [ "$3" = "worktree" ] && [ "$4" = "repair" ]; then
  echo "simulated worktree repair failure" >&2
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
        .args(["rename", "partial-rename", "resumed-rename", "--json"])
        .output()
        .unwrap();

    assert!(!partial.status.success());
    let report: Value = serde_json::from_slice(&partial.stdout).unwrap();
    assert_eq!(report["status"], "failed");
    assert_eq!(report["repositories"][0]["status"], "repaired");
    assert_eq!(report["repositories"][1]["status"], "failed");
    assert!(!fixture.workspace("partial-rename").exists());
    assert!(fixture.workspace("resumed-rename").join("alpha").exists());
    assert!(fixture.workspace("resumed-rename").join("beta").exists());

    let resumed = forest(
        &fixture.root,
        &["rename", "partial-rename", "resumed-rename", "--json"],
    );

    assert_success(&resumed);
    let report: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(report["status"], "renamed");
    assert_eq!(report["repositories"][0]["status"], "already_repaired");
    assert_eq!(report["repositories"][1]["status"], "repaired");
    for name in ["alpha", "beta"] {
        let worktrees = git_stdout(
            &fixture.canonical(name),
            &["worktree", "list", "--porcelain"],
        );
        assert!(worktrees.contains(path(&fixture.workspace("resumed-rename").join(name))));
        assert!(!worktrees.contains(path(&fixture.workspace("partial-rename").join(name))));
    }
}

#[test]
fn force_archives_workspace_with_dirty_worktrees() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "force-archive", "alpha", "--json"],
    ));
    let workspace = fixture.workspace("force-archive");
    fs::write(workspace.join("alpha/untracked.txt"), "dirty\n").unwrap();
    fs::write(workspace.join("notes.md"), "keep\n").unwrap();

    let output = forest(
        &fixture.root,
        &["archive", "force-archive", "--force", "--json"],
    );

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "archived");
    assert_eq!(report["repositories"][0]["status"], "removed");
    assert_eq!(
        report["repositories"][0]["message"],
        "discarded modified, untracked, or ignored files (--force)"
    );
    assert!(!workspace.exists());
    assert!(
        fixture
            .archived_workspace("force-archive")
            .join("notes.md")
            .exists()
    );
    assert!(
        !fixture
            .archived_workspace("force-archive")
            .join("alpha")
            .exists()
    );
}

#[test]
fn removes_stale_named_checkout_registration() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "stale-named", "alpha@part-2", "--json"],
    ));
    let missing = fixture.workspace("stale-named").join("alpha@part-2");
    fs::remove_dir_all(&missing).unwrap();

    let output = forest(&fixture.root, &["remove", "stale-named", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["checkout"], "alpha@part-2");
    assert_eq!(report["repositories"][0]["status"], "removed");
    assert_eq!(report["workspace_removed"], true);
    assert!(
        !git_stdout(
            &fixture.canonical("alpha"),
            &["worktree", "list", "--porcelain"]
        )
        .contains(path(&missing))
    );
}

#[test]
fn removes_stale_registration_when_a_worktree_is_missing() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "stale", "alpha", "beta", "--json"],
    ));
    let missing = fixture.workspace("stale").join("alpha");
    fs::remove_dir_all(&missing).unwrap();

    let output = forest(&fixture.root, &["remove", "stale", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["repositories"][0]["status"], "removed");
    assert_eq!(report["repositories"][1]["status"], "removed");
    assert_eq!(report["workspace_removed"], true);
    assert!(!fixture.workspace("stale").exists());
    assert!(
        !git_stdout(
            &fixture.canonical("alpha"),
            &["worktree", "list", "--porcelain"]
        )
        .contains(path(&missing))
    );
    for name in ["alpha", "beta"] {
        git(
            &fixture.canonical(name),
            &["show-ref", "--verify", "--quiet", "refs/heads/test/stale"],
        );
    }
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
fn archives_all_worktrees_and_preserves_workspace_entries() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &[
            "create",
            "archived",
            "alpha",
            "alpha@part-2",
            "beta",
            "--json",
        ],
    ));
    let workspace = fixture.workspace("archived");
    fs::write(workspace.join("notes.md"), "keep\n").unwrap();
    fs::create_dir(workspace.join("fixtures")).unwrap();
    fs::write(workspace.join("fixtures/input.txt"), "input\n").unwrap();

    let output = forest(&fixture.root, &["archive", "archived", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "archived");
    assert_eq!(report["path"], path(&workspace));
    assert_eq!(
        report["archive_path"],
        path(&fixture.archived_workspace("archived"))
    );
    assert!(
        report["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|repository| repository["status"] == "removed")
    );
    assert_eq!(
        report["preserved_entries"],
        serde_json::json!([
            path(&fixture.archived_workspace("archived").join("fixtures")),
            path(&fixture.archived_workspace("archived").join("notes.md")),
        ])
    );
    assert!(!workspace.exists());
    assert_eq!(
        fs::read_to_string(
            fixture
                .archived_workspace("archived")
                .join("fixtures/input.txt")
        )
        .unwrap(),
        "input\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.archived_workspace("archived").join("notes.md")).unwrap(),
        "keep\n"
    );
    for (repository, branch) in [
        ("alpha", "refs/heads/test/archived"),
        ("alpha", "refs/heads/test/part-2"),
        ("beta", "refs/heads/test/archived"),
    ] {
        git(
            &fixture.canonical(repository),
            &["show-ref", "--verify", "--quiet", branch],
        );
    }

    let listed = forest(&fixture.root, &["list", "--json"]);
    assert_success(&listed);
    let listed: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed["workspaces"], serde_json::json!([]));

    let repeated = forest(&fixture.root, &["archive", "archived", "--json"]);
    assert_success(&repeated);
    let repeated: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(repeated["status"], "already_archived");

    let create = forest(&fixture.root, &["create", "archived", "--json"]);
    assert!(!create.status.success());
    assert!(create.stdout.is_empty());
    assert!(
        serde_json::from_slice::<Value>(&create.stderr).unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("is archived")
    );
}

#[test]
fn dirty_worktree_blocks_archival_before_any_removal() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "dirty-archive", "alpha", "beta", "--json"],
    ));
    let workspace = fixture.workspace("dirty-archive");
    fs::write(workspace.join("alpha/untracked.txt"), "dirty\n").unwrap();
    fs::write(workspace.join("notes.md"), "keep\n").unwrap();

    let output = forest(&fixture.root, &["archive", "dirty-archive", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "conflict");
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert_eq!(report["repositories"][1]["status"], "not_run");
    assert!(workspace.join("alpha").exists());
    assert!(workspace.join("beta").exists());
    assert!(workspace.join("notes.md").exists());
    assert!(!fixture.archived_workspace("dirty-archive").exists());
}

#[test]
fn archive_serializes_concurrent_workspace_mutations() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "serialized-archive", "alpha", "--json"],
    ));

    let bin = fixture.root.join("blocking-git-bin");
    let signal = fixture.root.join("remove-started");
    let release = fixture.root.join("release-remove");
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(
        &wrapper,
        r#"#!/bin/sh
if [ "$3" = "worktree" ] && [ "$4" = "remove" ]; then
  : > "$FOREST_REMOVE_SIGNAL"
  while [ ! -e "$FOREST_REMOVE_RELEASE" ]; do sleep 0.02; done
fi
exec "$FOREST_REAL_GIT" "$@"
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&wrapper).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&wrapper, permissions).unwrap();

    let existing_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&existing_path));
    let command_path = std::env::join_paths(paths).unwrap();
    let real_git = find_executable("git");

    let mut archive = Command::new(binary())
        .current_dir(&fixture.root)
        .env_remove("FOREST_CONFIG")
        .env("PATH", &command_path)
        .env("FOREST_REAL_GIT", real_git)
        .env("FOREST_REMOVE_SIGNAL", &signal)
        .env("FOREST_REMOVE_RELEASE", &release)
        .args(["archive", "serialized-archive", "--json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    for _ in 0..250 {
        if signal.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if !signal.exists() {
        archive.kill().unwrap();
        archive.wait().unwrap();
        panic!("archive did not reach worktree removal");
    }

    let mut add = Command::new(binary())
        .current_dir(&fixture.root)
        .env_remove("FOREST_CONFIG")
        .args(["add", "serialized-archive", "beta", "--json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(150));
    let add_was_blocked = add.try_wait().unwrap().is_none();

    fs::write(&release, "continue\n").unwrap();
    let archive = archive.wait_with_output().unwrap();
    let add = add.wait_with_output().unwrap();

    assert_success(&archive);
    assert!(add_was_blocked, "concurrent add was not serialized");
    assert!(!add.status.success());
    assert!(fixture.archived_workspace("serialized-archive").is_dir());
    assert!(!fixture.workspace("serialized-archive").exists());
    assert!(
        String::from_utf8(add.stderr)
            .unwrap()
            .contains("is archived")
    );
}

#[test]
fn existing_archive_destination_blocks_removal() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "archive-conflict", "alpha", "--json"],
    ));
    let archive = fixture.archived_workspace("archive-conflict");
    fs::create_dir_all(&archive).unwrap();
    fs::write(archive.join("keep.txt"), "existing\n").unwrap();

    let output = forest(&fixture.root, &["archive", "archive-conflict", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "conflict");
    assert!(
        report["message"]
            .as_str()
            .unwrap()
            .contains("already exists")
    );
    assert!(fixture.workspace("archive-conflict").join("alpha").exists());
    assert_eq!(
        fs::read_to_string(archive.join("keep.txt")).unwrap(),
        "existing\n"
    );
}

#[test]
fn dangling_archive_destination_blocks_removal_and_name_reuse() {
    use std::os::unix::fs::symlink;

    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "dangling-archive", "alpha", "--json"],
    ));
    let archive = fixture.archived_workspace("dangling-archive");
    fs::create_dir_all(archive.parent().unwrap()).unwrap();
    symlink("missing-target", &archive).unwrap();

    let output = forest(&fixture.root, &["archive", "dangling-archive", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "conflict");
    assert!(fixture.workspace("dangling-archive").join("alpha").exists());
    assert!(
        fs::symlink_metadata(&archive)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let reserved = fixture.archived_workspace("reserved-name");
    symlink("missing-target", &reserved).unwrap();
    let create = forest(
        &fixture.root,
        &["create", "reserved-name", "alpha", "--json"],
    );
    assert!(!create.status.success());
    assert!(!fixture.workspace("reserved-name").exists());
}

#[test]
fn dangling_checkout_symlink_blocks_archival_before_removal() {
    use std::os::unix::fs::symlink;

    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "dangling-checkout", "beta", "--json"],
    ));
    let workspace = fixture.workspace("dangling-checkout");
    symlink("missing-target", workspace.join("alpha")).unwrap();

    let output = forest(&fixture.root, &["archive", "dangling-checkout", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "conflict");
    assert_eq!(report["repositories"][0]["checkout"], "alpha");
    assert_eq!(report["repositories"][0]["status"], "conflict");
    assert_eq!(report["repositories"][1]["checkout"], "beta");
    assert_eq!(report["repositories"][1]["status"], "not_run");
    assert!(workspace.join("beta").exists());
    assert!(
        fs::symlink_metadata(workspace.join("alpha"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn archive_rejects_workspace_name_casing_mismatches() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "ArchiveCase", "alpha", "--json"],
    ));

    let output = forest(&fixture.root, &["archive", "archivecase", "--json"]);

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "conflict");
    assert!(
        report["message"]
            .as_str()
            .unwrap()
            .contains("use \"ArchiveCase\"")
    );
    assert!(fixture.workspace("ArchiveCase").join("alpha").exists());
    assert!(!fixture.archived_workspace("ArchiveCase").exists());
}

#[cfg(target_os = "macos")]
#[test]
fn workspace_scan_excludes_case_aliased_archive_storage() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "archive-source", "alpha", "--json"],
    ));
    let active = fixture.workspace("archive-source");
    let archived = fixture.workspace(".Archive").join("old/alpha");
    fs::create_dir_all(archived.parent().unwrap()).unwrap();
    git(
        &fixture.canonical("alpha"),
        &[
            "worktree",
            "move",
            path(&active.join("alpha")),
            path(&archived),
        ],
    );
    fs::remove_dir(&active).unwrap();

    let output = forest(&fixture.root, &["list", "--json"]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["workspaces"], serde_json::json!([]));
}

#[test]
fn treats_non_checkout_siblings_as_workspace_entries() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "workspace-files", "--json"],
    ));
    let note = fixture.workspace("workspace-files").join("notes.md");
    fs::write(&note, "keep\n").unwrap();

    let json = forest(&fixture.root, &["list", "--json"]);
    assert_success(&json);
    let report: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        report["workspaces"][0]["unexpected_entries"][0],
        path(&note)
    );
    assert_eq!(
        report["workspaces"][0]["inconsistencies"],
        serde_json::json!([])
    );

    let human = forest(&fixture.root, &["list"]);
    assert_success(&human);
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(human.contains("Workspace entries"));
    assert!(human.contains(path(&note)));
    assert!(!human.contains("unexpected"));
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
        .args(["add", "missing-repositories", "--json"])
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
fn generates_dynamic_completion_setup_without_configuration() {
    let temp = tempfile::tempdir().unwrap();

    for shell in ["bash", "elvish", "fish", "powershell", "zsh"] {
        let output = forest(temp.path(), &["completions", shell]);

        assert_success(&output);
        let script = String::from_utf8(output.stdout).unwrap();
        assert!(script.contains("git-forest"), "{shell}: {script}");
        assert!(script.contains("FOREST_COMPLETE"), "{shell}: {script}");
        match shell {
            "bash" => assert!(script.contains("_git_forest"), "{shell}: {script}"),
            "zsh" => {
                assert!(
                    script.contains("function _git_forest()"),
                    "{shell}: {script}"
                );
                assert!(
                    script.contains("function _git-forest()"),
                    "{shell}: {script}"
                );
            }
            "fish" => assert!(
                script.contains("__fish_git_forest_complete"),
                "{shell}: {script}"
            ),
            _ => {}
        }
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn bash_setup_completes_the_git_style_invocation() {
    let fixture = WorkspaceFixture::new();
    fs::create_dir_all(fixture.workspace("logical-slots")).unwrap();
    fs::create_dir_all(fixture.workspace("review-123")).unwrap();
    let setup = forest(&fixture.root, &["completions", "bash"]);
    assert_success(&setup);
    let setup_path = fixture.root.join("forest-completion.bash");
    fs::write(&setup_path, setup.stdout).unwrap();

    let existing_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![Path::new(binary()).parent().unwrap().to_path_buf()];
    paths.extend(std::env::split_paths(&existing_path));
    let command_path = std::env::join_paths(paths).unwrap();
    let output = Command::new("bash")
        .current_dir(&fixture.root)
        .env("PATH", command_path)
        .env("FOREST_TEST_COMPLETION_SETUP", setup_path)
        .args([
            "--noprofile",
            "--norc",
            "-c",
            r#"
source "$FOREST_TEST_COMPLETION_SETUP"
COMP_WORDS=(git forest attach log)
COMP_CWORD=3
COMP_TYPE=9
__git_cmd_idx=1
_git_forest
printf '%s\n' "${COMPREPLY[@]}"
"#,
        ])
        .output()
        .unwrap();

    assert_success(&output);
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "logical-slots\n");
    assert!(output.stderr.is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn zsh_setup_completes_both_git_dispatch_conventions() {
    let fixture = WorkspaceFixture::new();
    fs::create_dir_all(fixture.workspace("stacked").join("alpha")).unwrap();
    fs::create_dir_all(fixture.workspace("stacked").join("alpha@part-2")).unwrap();
    let setup = forest(&fixture.root, &["completions", "zsh"]);
    assert_success(&setup);
    let setup_path = fixture.root.join("forest-completion.zsh");
    fs::write(&setup_path, setup.stdout).unwrap();

    let existing_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![Path::new(binary()).parent().unwrap().to_path_buf()];
    paths.extend(std::env::split_paths(&existing_path));
    let command_path = std::env::join_paths(paths).unwrap();
    let output = Command::new("/bin/zsh")
        .current_dir(&fixture.root)
        .env("PATH", command_path)
        .env("FOREST_TEST_COMPLETION_SETUP", setup_path)
        .args([
            "-f",
            "-c",
            r#"
compdef() { :; }
_describe() {
    local values_name=$3
    print -l -- ${(P)values_name}
}
source "$FOREST_TEST_COMPLETION_SETUP"

print underscore
words=(git forest remove stacked a)
CURRENT=5
_git_forest

print hyphen
words=(forest remove stacked a)
CURRENT=4
_git-forest
"#,
        ])
        .output()
        .unwrap();

    assert_success(&output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "underscore\nalpha\nalpha@part-2\nhyphen\nalpha\nalpha@part-2\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn dynamically_completes_workspaces_for_attach() {
    let fixture = WorkspaceFixture::new();
    fs::create_dir_all(fixture.workspace("logical-slots")).unwrap();
    fs::create_dir_all(fixture.workspace("review-123")).unwrap();

    let candidates = completions(&fixture.root, &["git-forest", "attach", "log"]);

    assert!(candidates.contains(&"logical-slots".to_owned()));
    assert!(!candidates.contains(&"review-123".to_owned()));
}

#[test]
fn dynamically_completes_the_source_workspace_for_rename() {
    let fixture = WorkspaceFixture::new();
    fs::create_dir_all(fixture.workspace("rename-source")).unwrap();
    fs::create_dir_all(fixture.workspace("other-topic")).unwrap();

    let candidates = completions(&fixture.root, &["git-forest", "rename", "rename"]);

    assert!(candidates.contains(&"rename-source".to_owned()));
    assert!(!candidates.contains(&"other-topic".to_owned()));
}

#[test]
fn dynamically_completes_only_active_workspaces_for_archive() {
    let fixture = WorkspaceFixture::new();
    fs::create_dir_all(fixture.workspace("active-topic")).unwrap();
    fs::create_dir_all(fixture.archived_workspace("archived-topic")).unwrap();

    let active = completions(&fixture.root, &["git-forest", "archive", "active"]);
    let archived = completions(&fixture.root, &["git-forest", "archive", "archived"]);

    assert!(active.contains(&"active-topic".to_owned()));
    assert!(!archived.contains(&"archived-topic".to_owned()));
}

#[test]
fn dynamic_completion_honors_an_explicit_config() {
    let fixture = WorkspaceFixture::new();
    fs::create_dir_all(fixture.workspace("logical-slots")).unwrap();
    let outside = fixture._temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let config = fixture.root.join(".forest.toml");

    let candidates = completions(
        &outside,
        &["git-forest", "--config", path(&config), "attach", "log"],
    );

    assert!(candidates.contains(&"logical-slots".to_owned()));
}

#[test]
fn dynamically_completes_configured_repositories_without_repeating_selections() {
    let fixture = WorkspaceFixture::new();

    let first = completions(&fixture.root, &["git-forest", "create", "topic", "a"]);
    assert!(first.contains(&"alpha".to_owned()));

    let remaining = completions(
        &fixture.root,
        &["git-forest", "create", "topic", "alpha", ""],
    );
    assert!(!remaining.contains(&"alpha".to_owned()));
    assert!(remaining.contains(&"beta".to_owned()));
    assert!(remaining.contains(&"gamma".to_owned()));
}

#[test]
fn dynamically_completes_named_checkouts_for_remove() {
    let fixture = WorkspaceFixture::new();
    fs::create_dir_all(fixture.workspace("stacked").join("alpha")).unwrap();
    fs::create_dir_all(fixture.workspace("stacked").join("alpha@part-2")).unwrap();
    fs::create_dir_all(fixture.workspace("stacked").join("unexpected")).unwrap();

    let candidates = completions(&fixture.root, &["git-forest", "remove", "stacked", "a"]);

    assert!(candidates.contains(&"alpha".to_owned()));
    assert!(candidates.contains(&"alpha@part-2".to_owned()));
    assert!(!candidates.contains(&"unexpected".to_owned()));
}

#[test]
fn dynamically_completes_stale_registered_worktrees_for_remove() {
    let fixture = WorkspaceFixture::new();
    assert_success(&forest(
        &fixture.root,
        &["create", "stale-completion", "alpha@part-2", "--json"],
    ));
    let workspace = fixture.workspace("stale-completion");
    fs::remove_dir_all(workspace.join("alpha@part-2")).unwrap();

    let checkouts = completions(
        &fixture.root,
        &["git-forest", "remove", "stale-completion", "a"],
    );
    assert!(checkouts.contains(&"alpha@part-2".to_owned()));

    fs::remove_dir(&workspace).unwrap();
    let workspaces = completions(&fixture.root, &["git-forest", "remove", "sta"]);
    assert!(workspaces.contains(&"stale-completion".to_owned()));
    let rename_sources = completions(&fixture.root, &["git-forest", "rename", "sta"]);
    assert!(rename_sources.contains(&"stale-completion".to_owned()));
}

#[test]
fn missing_configuration_produces_no_dynamic_value_candidates() {
    let temp = tempfile::tempdir().unwrap();

    let candidates = completions(temp.path(), &["git-forest", "attach", "log"]);

    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.starts_with('-'))
    );
}

#[test]
fn no_subcommand_prints_help_when_not_attached_to_a_terminal() {
    let output = Command::new(binary()).output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Usage: git-forest [OPTIONS] [COMMAND]"));
    assert!(stdout.contains("open"));
    assert!(output.stderr.is_empty());
}

#[test]
fn explicit_launcher_rejects_non_interactive_input() {
    let fixture = WorkspaceFixture::new();
    let output = forest(&fixture.root, &["open"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("workspace launcher requires an interactive terminal")
    );
}

#[test]
fn git_style_long_help_uses_installed_manual() {
    let temp = tempfile::tempdir().unwrap();
    let prefix = temp.path();
    let bin_dir = prefix.join("bin");
    let man_dir = prefix.join("share/man/man1");
    fs::create_dir(&bin_dir).unwrap();
    fs::create_dir_all(&man_dir).unwrap();
    fs::copy(binary(), bin_dir.join("git-forest")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/git-forest.1"),
        man_dir.join("git-forest.1"),
    )
    .unwrap();

    let man = bin_dir.join("man");
    fs::write(
        &man,
        "#!/bin/sh\nexec /bin/cat \"$FOREST_TEST_MAN_ROOT/share/man/man1/$1.1\"\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&man).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&man, permissions).unwrap();

    let existing_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin_dir];
    paths.extend(std::env::split_paths(&existing_path));
    let command_path = std::env::join_paths(paths).unwrap();
    let output = Command::new("git")
        .env("PATH", command_path)
        .env("HOME", prefix)
        .env("XDG_CONFIG_HOME", prefix)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_MAN_VIEWER", "man")
        .env("FOREST_TEST_MAN_ROOT", prefix)
        .args(["-c", "help.format=man", "forest", "--help"])
        .output()
        .unwrap();

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(".TH \"GIT-FOREST\" \"1\""));
    assert!(stdout.contains(".SH COMMANDS"));
    assert!(output.stderr.is_empty());
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

fn clone_publisher(fixture: &WorkspaceFixture, name: &str) -> PathBuf {
    let publisher = fixture.root.join(format!("{name}-publisher"));
    let origin = fixture.root.join(format!("{name}-origin.git"));
    git(&fixture.root, &["clone", path(&origin), path(&publisher)]);
    git(&publisher, &["config", "user.name", "Forest Test"]);
    git(&publisher, &["config", "user.email", "forest@example.com"]);
    publisher
}

fn publish_file(publisher: &Path, file: &str, contents: &str, message: &str) -> String {
    fs::write(publisher.join(file), contents).unwrap();
    git(publisher, &["add", file]);
    git(publisher, &["commit", "-m", message]);
    let branch = git_stdout(publisher, &["branch", "--show-current"]);
    let destination = format!("HEAD:{branch}");
    git(publisher, &["push", "origin", &destination]);
    git_stdout(publisher, &["rev-parse", "HEAD"])
}

fn write_config(path: &Path, members: &[&str]) {
    write_config_with_remote(path, members, Some("git@example.com:{name}.git"));
}

fn write_config_with_branch(path: &Path, members: &[&str], branch: &str) {
    write_config_contents(path, members, Some("git@example.com:{name}.git"), branch);
}

fn write_config_with_remote(path: &Path, members: &[&str], remote: Option<&str>) {
    write_config_contents(path, members, remote, "test/{checkout}");
}

fn write_config_contents(path: &Path, members: &[&str], remote: Option<&str>, branch: &str) {
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
branch = {branch:?}
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

fn completions(current_dir: &Path, arguments: &[&str]) -> Vec<String> {
    let output = Command::new(binary())
        .current_dir(current_dir)
        .env_remove("FOREST_CONFIG")
        .env("FOREST_COMPLETE", "fish")
        .arg("--")
        .args(arguments)
        .output()
        .unwrap();
    assert_success(&output);
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| line.split_once('\t').map_or(line, |(value, _)| value))
        .map(str::to_owned)
        .collect()
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
