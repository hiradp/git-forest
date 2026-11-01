# git-forest

One named workspace across many Git repositories, built from linked worktrees.

Forest keeps a canonical clone of each repository and gives every workspace a
directory with a worktree per repository. Git worktree metadata and the
filesystem are the only state. Forest contacts remotes only for `setup`,
`fetch`, and `update`, and never resets or deletes branches.

## Install

Requires Git, [Rust](https://rustup.rs/), and [`just`](https://just.systems/).
Tested on Linux and macOS.

```sh
git clone https://github.com/hiradp/git-forest.git
cd git-forest
just install
```

This installs `git-forest` and its manual into Cargo's prefix, or the one named
by `CARGO_INSTALL_ROOT`. Its `bin` directory must be on `PATH`. Git answers
`git forest --help` with the manual, which a plain `cargo install` does not
install; `git forest -h` works either way.

For completion of commands, repositories, workspaces, and checkouts, source the
setup from your shell's startup file. Bash, Zsh, Fish, Elvish, and PowerShell
are supported.

```sh
git-forest completions fish | source
```

## Use

```
git forest setup                           clone missing canonical repositories
git forest repos                           list configured repositories
git forest fetch [<repository>...]         fetch origin
git forest update [<repository>...]        fetch and fast-forward default branches
git forest create <workspace> [<checkout>...]     create a workspace; safe to rerun
git forest add <workspace> <checkout>...          add worktrees to a workspace
git forest list [--archived]               list workspaces as a tree, or archives
git forest status [<workspace>]            show branch, dirty state, and upstream
git forest path <workspace>                print the workspace path
git forest rename <workspace> <new>        rename without renaming branches
git forest archive <workspace>             remove worktrees and save local files
git forest unarchive <workspace> [<checkout>...]  restore saved local files
git forest delete <workspace>              remove worktrees and local files
git forest remove <workspace> [<checkout>...]     remove worktrees
git forest clean                           drop registrations of deleted worktrees
git forest completions <shell>             print shell completion setup
```

```console
$ git forest fetch
$ git forest create logical-slots api operator --symbol 🌲
Workspace  logical-slots
Path       /project/src/.workspaces/logical-slots
Branch     user/logical-slots

  ✓ api       created  new branch
  ✓ operator  created  new branch
$ git forest create slot-tests api@tests --parent logical-slots
$ git forest list
🌲 logical-slots
└── 🌲 slot-tests
$ cd "$(git forest path logical-slots)"
$ git forest archive logical-slots
```

- A checkout is `repository` for its primary worktree or `repository@slot` for
  another worktree of the same repository, such as `api@tests`.
- New branches start from the local `origin/HEAD`. `--base <checkout>=<ref>`
  and `--branch <checkout>=<branch>` override the base or the branch.
- `create` and `add` check every requested checkout before changing anything.
  Every command is safe to rerun after a partial failure.
- Dirty worktrees block `remove`, `archive`, and `delete` unless you pass
  `--force`. Branches are always preserved.
- `--parent` nests a workspace under another.
- Every command except `completions` takes `--json`.

## Configure

`.forest.toml`, found by walking up from the current directory, or the file
named by `--config` or `FOREST_CONFIG`.

```toml
version = 1

[repositories]
root = "src"
remote = "git@github.com:example/{name}.git"
members = ["api", "operator"]

[workspaces]
root = "src/.workspaces"
branch = "user/{checkout}"
```

| Field                 | Meaning                                                        |
|-----------------------|----------------------------------------------------------------|
| `repositories.root`   | Directory of canonical clones, one per member.                 |
| `repositories.remote` | Clone URL template for `setup`. Optional.                      |
| `workspaces.root`     | Directory of workspaces.                                       |
| `workspaces.branch`   | Branch template using `{workspace}` or `{checkout}`.           |

Paths are relative to the directory containing the file. `{checkout}` is the
slot for a named checkout and the workspace name otherwise, so
`user/{checkout}` gives `api` in `stacked` the branch `user/stacked` and
`api@part-2` the branch `user/part-2`.

## Reference

- `git forest --help` opens the manual, [`docs/git-forest.1`](docs/git-forest.1),
  which covers every command, option, and configuration rule.
- [`docs/json.md`](docs/json.md) documents the JSON reports and exit statuses.

## Development

```sh
just check  # apply formatting and Clippy fixes, then validate the tree
just test   # run all tests
```

Tests use temporary repositories and local bare origins, with no network access
or Git identity. See [SECURITY.md](SECURITY.md) to report a vulnerability.

## License

[MIT](LICENSE)
