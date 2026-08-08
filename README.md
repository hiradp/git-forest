# git-forest

`git-forest` manages one named workspace across multiple Git repositories using
linked worktrees. Git exposes the installed binary as both:

```sh
git-forest <command>
git forest <command>
```

The tool is non-interactive. It contacts remotes only for an explicit `fetch`
command. It does not clone, delete branches, manage runtime resources, start
tmux, or maintain a separate worktree registry. Git worktree metadata and the
filesystem are authoritative.

## Installation

The project currently tests Linux and macOS. Building from source requires Git
and [Rust](https://rustup.rs/); the repository pins its Rust toolchain in
`rust-toolchain.toml`.

Clone the repository and install the binary with Cargo:

```sh
git clone https://github.com/hiradp/git-forest.git
cd git-forest
cargo install --locked --path .
```

The [`just`](https://just.systems/) development recipes provide an equivalent
install command that forces replacement of an existing installation:

```sh
just install
```

Both commands install into Cargo's binary directory (normally
`$CARGO_HOME/bin`, which must be on `PATH`). Set `CARGO_INSTALL_ROOT` to choose
another installation prefix:

```sh
CARGO_INSTALL_ROOT="$HOME/.local" just install
```

## Configuration

`git-forest` reads a `.forest.toml`:

```toml
version = 1

[repositories]
root = "src"
remote = "git@github.com:example/{name}.git"
members = [
  "api",
  "operator",
]

[workspaces]
root = "src/.workspaces"
branch = "user/{workspace}"
```

All paths are relative to the directory containing `.forest.toml`.
`repositories.root` contains the canonical clones. Each member is both its CLI
name and its directory beneath that root. `repositories.remote` is optional and
reserved for future setup support; v1 never clones a missing repository.

The only supported placeholders are:

- `{name}` in `repositories.remote`;
- `{workspace}` in `workspaces.branch`.

When present, the remote template must contain `{name}`; the branch template
must contain `{workspace}`. Unknown placeholders, duplicate members, absolute
roots, and unsupported configuration versions are rejected.

Configuration precedence is:

1. `--config <path>`;
2. `FOREST_CONFIG`;
3. `.forest.toml` found by walking from the current directory to the filesystem
   root.

Workspace names must match `[A-Za-z0-9][A-Za-z0-9._-]*`. `.` and `..` are not
valid names.

## Commands

```text
git forest repos [--json]
git forest fetch [<repository>...] [--json]
git forest create <workspace> <repository>... [--base <repository>=<ref>]... [--json]
git forest add <workspace> <repository>... [--base <repository>=<ref>]... [--json]
git forest list [--json]
git forest status [<workspace>] [--json]
git forest path <workspace> [--json]
git forest remove <workspace> [<repository>...] [--json]
```

Global options:

```text
--config <path>
--help
--version
```

### `repos`

Lists configured repositories in configuration order. Missing canonical clones
are reported rather than making the whole command fail. For present clones it
reports the origin URL and the default ref when available.

The default base is discovered exclusively through the symbolic ref
`refs/remotes/origin/HEAD`. The command never guesses `main` or `master` and
never contacts a remote to repair a missing default.

### `fetch`

Fetches `origin` for every configured canonical repository. Pass repository
names to fetch only a subset. The command attempts every selected repository
and exits unsuccessfully if any fetch fails.

This is the only command that contacts remotes. It updates remote-tracking refs,
including the `origin/HEAD` target used as the default creation base, but does
not merge, reset, or otherwise update local branches or worktrees. To create a
workspace from the latest fetched defaults:

```sh
git forest fetch
git forest create logical-slots api operator
```

### `create` and `add`

`create` permits the workspace directory to be absent. `add` requires an
existing workspace. Both use the same idempotent creation engine.

Before mutation, every requested repository is checked for:

- a present canonical Git worktree;
- a valid rendered branch name;
- a non-conflicting destination path;
- existing worktree registration;
- branches checked out elsewhere;
- branch namespace conflicts;
- a resolvable base when a new branch is needed.

An existing worktree is reused only when its path, canonical repository, and
branch all match. An existing branch that is not checked out elsewhere is added
without being recreated. New branches use `origin/HEAD` unless `--base` is
provided.

Preflight conflicts prevent all mutation. If Git fails after earlier
repositories have been created, successful worktrees are preserved and later
repositories are marked as not run. Repeating the command resumes safely.

Human output keeps shared workspace details in one header and summarizes each
repository on a compact result line:

```text
Workspace  logical-slots
Path       /project/src/.workspaces/logical-slots
Branch     user/logical-slots

  ✓ api       created  new branch
  ✓ operator  reused
```

Colors are enabled only when stdout is a terminal and can be disabled with
`NO_COLOR`. Use `--json` when every report field is needed.

### `list` and `status`

`list` reconciles workspace directories with every canonical repository's Git
worktree metadata. It reports unregistered paths, missing registered paths,
unexpected entries, and layout mismatches.

`status` additionally reports:

- current branch or detached state;
- HEAD commit;
- tracked and untracked dirty state;
- canonical worktree registration;
- upstream;
- ahead and behind counts.

### `path`

Human output is exactly the absolute workspace path followed by a newline:

```sh
workspace=$(git forest path logical-slots)
tmux-sessionizer "$workspace" logical-slots
```

The workspace must exist.

### `remove`

Removal is deliberately conservative:

- modified, untracked, and ignored files prevent removal;
- every selected path must be registered with its configured canonical
  repository;
- removal always uses `git worktree remove`;
- branches are never deleted;
- there is no force option;
- the workspace directory is removed only when it is empty;
- unexpected files are reported and preserved.

Removing only named repositories leaves other members in place. Repeating a
partially completed removal is safe.

## JSON contract

Paths are absolute. Optional values are represented as `null` rather than
omitted.

### Repositories

```json
{
  "repositories": [
    {
      "name": "api",
      "path": "/project/src/api",
      "exists": true,
      "is_git_worktree": true,
      "origin_url": "git@github.com:example/api.git",
      "default_ref": "refs/remotes/origin/main"
    }
  ]
}
```

### Fetch

```json
{
  "repositories": [
    {
      "name": "api",
      "path": "/project/src/api",
      "status": "fetched",
      "message": null
    }
  ]
}
```

Fetch status is `fetched` or `failed`.

### Create and add

```json
{
  "workspace": "logical-slots",
  "path": "/project/src/.workspaces/logical-slots",
  "repositories": [
    {
      "name": "api",
      "path": "/project/src/.workspaces/logical-slots/api",
      "branch": "user/logical-slots",
      "base_ref": "refs/remotes/origin/main",
      "action": "create_branch",
      "status": "created",
      "message": null
    }
  ]
}
```

`action` is `reuse`, `add_existing_branch`, `create_branch`, or `null` for a
conflict discovered before an action could be selected. `status` is `reused`,
`created`, `conflict`, `failed`, or `not_run`.

### List

```json
{
  "workspaces": [
    {
      "name": "logical-slots",
      "path": "/project/src/.workspaces/logical-slots",
      "exists": true,
      "repositories": [
        {
          "name": "api",
          "path": "/project/src/.workspaces/logical-slots/api",
          "exists": true,
          "registered": true,
          "branch": "user/logical-slots",
          "head": "0123456789abcdef",
          "inconsistencies": []
        }
      ],
      "unexpected_entries": [],
      "inconsistencies": []
    }
  ]
}
```

### Status

```json
{
  "workspaces": [
    {
      "name": "logical-slots",
      "path": "/project/src/.workspaces/logical-slots",
      "exists": true,
      "repositories": [
        {
          "name": "api",
          "path": "/project/src/.workspaces/logical-slots/api",
          "exists": true,
          "registered": true,
          "branch": "user/logical-slots",
          "detached": false,
          "head": "0123456789abcdef",
          "dirty": false,
          "upstream": "origin/user/logical-slots",
          "ahead": 1,
          "behind": 0,
          "inconsistencies": []
        }
      ],
      "unexpected_entries": [],
      "inconsistencies": []
    }
  ]
}
```

### Path

```json
{
  "workspace": "logical-slots",
  "path": "/project/src/.workspaces/logical-slots"
}
```

### Remove

```json
{
  "workspace": "logical-slots",
  "path": "/project/src/.workspaces/logical-slots",
  "repositories": [
    {
      "name": "api",
      "path": "/project/src/.workspaces/logical-slots/api",
      "status": "removed",
      "message": null
    }
  ],
  "workspace_removed": true,
  "remaining_entries": []
}
```

Removal status is `removed`, `already_absent`, `conflict`, `failed`, or
`not_run`.

Application errors in JSON mode are emitted as JSON to stderr:

```json
{
  "error": {
    "message": "invalid input: unknown repository \"unknown\"",
    "exit_code": 2
  }
}
```

Operational conflict reports remain on stdout because they contain the result
for every requested repository.

## Exit status

- `0`: successful, including fully idempotent operations;
- `1`: an operational conflict or Git/filesystem failure;
- `2`: usage, input, or configuration error.

## Development

The test suite creates temporary repositories and local bare origins. It does
not require network access or the developer's Git identity.

```sh
just check  # apply formatting and Clippy fixes, then validate the tree
just test   # run all tests
```

`just check` is intentionally allowed to update tracked files. CI runs the
non-mutating formatting and Clippy checks plus tests on fixed Linux and macOS
runner images. All Cargo commands in automation use the committed lockfile.

## Security

The security workflow audits `Cargo.lock` with RustSec, reviews dependency
changes on pull requests, and rejects workflow actions that are not pinned to a
full commit SHA. Dependabot proposes weekly Cargo and GitHub Actions updates.
See [SECURITY.md](SECURITY.md) for private vulnerability reporting.

## License

Copyright (c) 2026 Hirad Pourtahmasbi. Licensed under the [MIT License](LICENSE).
