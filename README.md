# git-forest

`git-forest` manages one named workspace across multiple Git repositories using
linked worktrees. Git exposes the installed binary as both:

```sh
git-forest <command>
git forest <command>
```

Running `git forest` without a subcommand opens an interactive workspace
launcher. The existing management commands remain non-interactive; `open`
explicitly starts the same launcher. Forest contacts remotes only for explicit
`setup`, `fetch`, and `update` commands. It does not reset or delete branches,
start runtime services, or maintain a separate worktree registry. Git worktree
metadata and the filesystem are authoritative. The launcher and the explicit
`attach` command can create or focus a workspace in a running
[Herdr](https://herdr.dev) session.

## Installation

The project currently tests Linux and macOS. Building from source requires Git
and [Rust](https://rustup.rs/); the repository pins its Rust toolchain in
`rust-toolchain.toml`.

Clone the repository and use the [`just`](https://just.systems/) install
recipe:

```sh
git clone https://github.com/hiradp/git-forest.git
cd git-forest
just install
```

This installs the executable into Cargo's binary directory and the
`git-forest(1)` manual into the same prefix. The manual is required because Git
interprets `git forest --help` as a request for a manual page before it invokes
an external Git command. The executable directory (normally `$CARGO_HOME/bin`)
must be on `PATH` so both the command and its manual can be discovered.

Set `CARGO_INSTALL_ROOT` to choose another installation prefix:

```sh
CARGO_INSTALL_ROOT="$HOME/.local" just install
```

Cargo itself installs only executables. If you instead run
`cargo install --locked --path .`, use `git forest -h` or `git-forest --help`
for command-line help; `git forest --help` requires the manual installed by the
`just` recipe.

## Shell completion

Forest generates shell setup that calls back into the executable for dynamic
completion. Add the command for your shell to its startup file:

For Bash, add this to `~/.bashrc`:

```sh
source <(git-forest completions bash)
```

For Zsh, add this to `~/.zshrc`:

```sh
source <(git-forest completions zsh)
```

For Fish, add this to `~/.config/fish/config.fish`:

```fish
git-forest completions fish | source
```

For Elvish, use `eval (git-forest completions elvish | slurp)`. For
PowerShell, use
`git-forest completions powershell | Out-String | Invoke-Expression`.
Keep the generator invocation in the startup file rather than saving its
output because the completion protocol is version-dependent.

Bash, Zsh, and Fish setup completes both `git-forest` and `git forest`.
Completion includes commands and options, configured repository names, existing
workspace names for commands such as `attach`, and existing primary and named
checkouts for `remove`. It honors normal configuration discovery,
`FOREST_CONFIG`, and `--config`. Completion is read-only: it does not contact
remotes, mutate worktrees, or invoke Herdr. Invalid or missing configuration
simply produces no configuration-dependent candidates.

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
branch = "user/{checkout}"
```

All paths are relative to the directory containing `.forest.toml`.
`repositories.root` contains the canonical clones. Each member is both its CLI
name and its directory beneath that root. `repositories.remote` is optional for
projects that provision repositories separately. It is required by `setup`
when a canonical clone is missing. Relative local remotes are resolved from the
directory containing `.forest.toml`.

The only supported placeholders are:

- `{name}` in `repositories.remote`;
- `{workspace}` and `{checkout}` in `workspaces.branch`.

`{workspace}` is always the workspace name. `{checkout}` is the explicit slot
for a named checkout and the workspace name for a primary checkout. Thus the
template `user/{checkout}` renders `user/stacked` for `api` in workspace
`stacked` and `user/part-2` for `api@part-2`. A branch template must contain at
least one supported placeholder. When present, the remote template must contain
`{name}`. Unknown placeholders, duplicate members, absolute roots, and
unsupported configuration versions are rejected.

Configuration precedence is:

1. `--config <path>`;
2. `FOREST_CONFIG`;
3. `.forest.toml` found by walking from the current directory to the filesystem
   root.

Workspace names and checkout slots must match
`[A-Za-z0-9][A-Za-z0-9._-]*`. `.` and `..` are not valid names. Forest reserves
`.archive` directly beneath `workspaces.root` for archived workspaces. A checkout is
selected as `repository` for its primary worktree or `repository@slot` for an
additional worktree from the same canonical repository. A single request may
not contain checkout identifiers that differ only by ASCII case because they
alias on case-insensitive filesystems.

## Commands

```text
git forest
git forest open
git forest setup [--json]
git forest repos [--json]
git forest fetch [<repository>...] [--jobs <N>] [--json]
git forest update [<repository>...] [--jobs <N>] [--json]
git forest create <workspace> [<checkout>...] [--symbol <symbol>] [--parent <workspace>] [--base <checkout>=<ref>]... [--branch <checkout>=<branch>]... [--json]
git forest add <workspace> <checkout>... [--base <checkout>=<ref>]... [--branch <checkout>=<branch>]... [--json]
git forest list [--archived] [--json]
git forest status [<workspace>] [--json]
git forest path <workspace> [--json]
git forest attach <workspace> [--json]
git forest rename <workspace> <new-workspace> [--json]
git forest archive <workspace> [--force] [--json]
git forest unarchive <workspace> [<checkout>...] [--archive <ID>] [--as <workspace>] [--symbol <symbol>] [--base <checkout>=<ref>]... [--branch <checkout>=<branch>]... [--json]
git forest delete <workspace> [--force] [--json]
git forest clean [--json]
git forest remove <workspace> [<checkout>...] [--force] [--json]
git forest completions <shell>
```

Global options:

```text
--config <path>
-h, --help
-V, --version
```

### Interactive launcher

Run `git forest` in a terminal to open the workspace launcher. `git forest open`
is the explicit equivalent. Start typing to fuzzy-search workspaces, then press
enter to attach the selected workspace in Herdr.

```text
  🌲 Forest
  Pick a workspace. We’ll get it ready.

◆ Where do you want to work?
›   +  Create a new workspace
  ○ logical-slots  api · operator
  ● review-123     api
[search · ↑↓ move · space select · enter open · ctrl+d/del actions · esc leave]
```

The first picker also offers **Create a new workspace**. Forest prompts for a
valid name and presents the configured repositories as a searchable
multi-select. After a successful preflight it creates the linked worktrees and
attaches the new workspace. The launcher creates one primary checkout per
selected repository using the configured branch template and each repository's
local `origin/HEAD`; use the
non-interactive `create` and `add` commands for named checkouts or branch and
base overrides.

Press Space to select any number of existing workspaces, then Ctrl+D or Delete
to choose Archive, Force archive, Delete, or Force delete for the selection. If
nothing is selected, the action applies only to the highlighted workspace. A
single confirmation covers the full selection. Archive moves workspace-local
files beneath `.archive`; Delete permanently removes them.
Non-force actions refuse dirty worktrees, while force actions explicitly discard
dirty worktree changes. All four actions preserve Git branches. Press escape at
any prompt to go back or leave without making changes. The launcher honors
`NO_COLOR`. Without an interactive terminal, invoking Forest without a
subcommand prints help instead of waiting for input.

### `setup`

Ensures every configured canonical repository exists. Existing Git worktrees
are reused without fetching or changing their remotes. Missing repositories are
cloned from the rendered `repositories.remote` template in configuration order.

Before cloning, Forest checks every configured destination. An existing path
that is not a Git worktree or a missing repository without a configured remote
prevents all cloning. If a clone fails after earlier repositories succeeded,
the successful clones are preserved and later repositories are not run.
Repeating the command safely reuses completed clones and resumes the rest.
Clones are completed in adjacent staging directories before being moved into
the configured canonical paths, so an interrupted clone is never reused as a
completed repository. Forest never overwrites an existing path.

### `repos`

Lists configured repositories in configuration order. Missing canonical clones
are reported rather than making the whole command fail. For present clones it
reports the origin URL and the default ref when available.

The default base is discovered exclusively through the symbolic ref
`refs/remotes/origin/HEAD`. The command never guesses `main` or `master` and
never contacts a remote to repair a missing default.

### `fetch`

Fetches `origin` for every configured canonical repository. Pass repository
names to fetch only a subset. Fetches run concurrently, with at most 16 in
flight by default; use `--jobs <N>` to change that bound. The report remains in
configuration order. The command attempts every selected repository and exits
unsuccessfully if any fetch fails. Forest does not fetch tags because its
workspace operations consume remote-tracking branch refs only.

`fetch` updates remote-tracking refs, including the `origin/HEAD` target used as
the default creation base, but does not merge, reset, or otherwise update local
branches or worktrees. To create a workspace from the latest fetched defaults:

```sh
git forest fetch
git forest create logical-slots api operator
```

### `update`

Fetches the remote default branch, then fast-forwards its local branch in every
configured canonical repository. Pass repository names to update only a subset.
Fetches use the same bounded concurrency and `--jobs <N>` option as `fetch`, but
transfer only the branch identified by `refs/remotes/origin/HEAD` rather than
negotiating every remote branch and tag.

Forest does not hard-code `main` or `master`. If the branch is checked out, Forest updates its
worktree with a fast-forward-only merge. If it is not checked out, Forest moves
the local ref only after verifying a fast-forward. Dirty checked-out branches,
ignored paths that overlap incoming changes, missing local default branches,
locally ahead branches, and diverged branches are reported as conflicts and are
never reset or forced. A fetch or Git failure
in one repository does not prevent the other selected repositories from being
processed, but conflicts and failures make the command exit unsuccessfully.

```sh
git forest update
```

### `create` and `add`

`create` permits the workspace directory to be absent. When no checkouts are
provided, it creates an empty workspace directory; repositories can be added
later with `add`. Repeating empty creation is safe. `add` requires an existing
workspace and at least one checkout. Both use the same idempotent creation
engine. Archived names remain available: `create` starts a fresh active workspace
without restoring or changing saved files. Existing branches may still be reused;
use `unarchive` to restore saved workspace files instead.

```sh
git forest create scratch
git forest add scratch api
```

Assign an emoji or symbol for the Herdr workspace name with `--symbol`:

```sh
git forest create logical-slots api operator --symbol "🌲"
git forest attach logical-slots
# Herdr workspace name: 🌲 logical-slots
```

Record a parent workspace with `--parent` to group related workspaces, such as
a coordination workspace and the workspaces that implement its parts:

```sh
git forest create q4-storage --symbol "🗺"
git forest create logical-slots api --parent q4-storage
git forest create slot-tests api@tests --parent logical-slots
```

The parent must be an active workspace, and Forest rejects a parent that would
form a cycle. Links can be any depth. Directories stay side by side under the
workspaces root; the link is metadata only.

Symbols and parents are stored in the optional workspace-local
`.forest-workspace.toml` file:

```toml
symbol = "🌲"
parent = "q4-storage"
```

Both fields are optional. The file is authoritative and can be edited by hand;
Forest reads it on every command. Directory names, branch names, and JSON
workspace identifiers stay unchanged. Symbols may include composed emoji, but
must not contain whitespace or control characters. To set or change a symbol or
parent on an existing workspace, repeat `create` with `--symbol` or `--parent`;
omitting an option preserves the saved value. When Forest writes the file, it
keeps other keys but not comments. Metadata survives rename, archive, and
unarchive. `unarchive --symbol` and `unarchive --parent` override the saved
values. An invalid file is reported with its path. The file appears in
workspace entries like other workspace-local files.

Workspaces created by earlier versions may have a `.forest-symbol` file instead.
Forest still reads it, ignoring trailing whitespace, and replaces it with
`.forest-workspace.toml` the next time it writes metadata for that workspace.

Before mutation, every requested checkout is checked for:

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

A primary checkout uses the repository name and retains the existing layout,
such as `stacked/api`. A named checkout uses `repository@slot`, such as
`stacked/api@part-2`. Its default branch is rendered with the slot as
`{checkout}`, allowing multiple branches from one repository in a workspace:

```sh
git forest create stacked api operator operator@part-2
```

With `branch = "user/{checkout}"`, this creates `user/stacked` at `api` and
`operator`, plus `user/part-2` at `operator@part-2`. Checkout identifiers must
be unique within a request. Two checkouts from the same repository may not
select the same branch because Git permits a branch to be checked out only
once.

Use `--branch <checkout>=<branch>` to select a branch independently of the
configured template. Forest first uses an existing local branch. If it is
absent, Forest creates a local branch from `refs/remotes/origin/<branch>` and
configures the remote branch as its upstream. The remote-tracking ref must
already exist locally; creation never fetches. For example, to review a branch
after fetching:

```sh
git forest fetch api
git forest create review-123 api@fix --branch api@fix=contributor/fix
```

Checkouts without a branch override continue to use the configured branch
template. A branch override and a base override cannot both target the same
checkout. After later fetches, Forest reports whether a tracking branch is
behind but never merges, resets, or otherwise updates it implicitly.

Preflight conflicts prevent all mutation. If Git fails after earlier checkouts
have been created, successful worktrees are preserved and later checkouts are
marked as not run. Repeating the command resumes safely.

Human output keeps shared workspace details in one header and summarizes each
checkout on a compact result line:

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
worktree metadata. Primary and named checkouts are reported separately. It
reports unregistered paths, missing registered paths, workspace-local entries,
and layout mismatches. Workspace-local entries are direct children that are not
configured checkout paths; they are supported and do not make the workspace
inconsistent. The JSON field remains `unexpected_entries` for compatibility.

Human output nests each workspace under its parent. JSON keeps a flat list and
reports each workspace's saved `parent`, or `null`.

`list --archived` shows saved archives instead of active workspaces, with each
archive's workspace, human-readable date, ID, and path.
Legacy archives show `legacy` instead of a date. Use the ID with
`unarchive --archive <ID>` when a workspace has several archives.

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

### `attach`

Opens an existing Forest workspace in Herdr. The `herdr` executable must be on
`PATH`, and a Herdr server for the current session must already be running.
Forest never starts or stops the server.

A workspace opens as a single tab with one shell pane rooted in the workspace
directory. Forest does not start commands. Where the tab goes depends on where
`attach` runs:

1. If the workspace is already open anywhere, as its own Herdr workspace or as
   a tab, Forest focuses it and never opens a second copy.
2. Otherwise, if the Herdr workspace that `attach` runs in has the workspace's
   parent open, either as that Herdr workspace or as one of its tabs, Forest
   adds a tab there. This applies from any tab in that Herdr workspace.
3. Otherwise, Forest creates a Herdr workspace whose only tab is `1-main`.

Forest identifies the Herdr workspace that `attach` runs in from
`HERDR_WORKSPACE_ID`, which Herdr sets in its panes. Outside Herdr, the third
rule always applies.

```text
Herdr: 🗺 q4-storage
  1-main
  2-🌲 logical-slots
  3-slot-tests
```

A child tab is labeled with its position, the workspace's symbol if it has one,
and its name, such as `2-🌲 logical-slots`. A workspace opened as its own Herdr
workspace is named with its symbol and a space before its name. Without a
symbol, new Herdr workspaces use the plain workspace name and existing Herdr
names are left alone. Each attachment repairs the tab's numeric prefix from its
current Herdr position.

If a saved parent is not an active workspace, Forest prints a warning and
ignores it; JSON reports carry the warning in `warnings`. Parents that form a
cycle are rejected before Forest contacts Herdr.

Attaching a workspace also opens all of its descendants. The workspace attaches
by the rules above. Each descendant then opens as a tab in that same Herdr
workspace, depth-first in name order. A descendant that is already open stays
where it is, even in another Herdr workspace, because Herdr cannot move tabs
between workspaces. Only the named workspace is focused. Forest checks every
workspace in the tree before contacting Herdr, so a descendant with invalid
metadata or inconsistent worktrees blocks attaching its ancestors until it is
fixed. If Herdr fails partway, rerunning the command opens only what is
missing.

Forest records a fixed-length identifier derived from the canonical workspace
path in Herdr's runtime metadata: on the Herdr workspace for a workspace opened
on its own, and on the tab's pane for a child tab. Herdr truncates metadata
values to 80 bytes, so Forest does not store the path itself. Herdr workspaces
tagged with the full path by earlier versions are still recognized when the
path fit, and Forest adds the new identifier the first time it sees one. Tabs
and panes Forest did not create keep their positions. Herdr workspaces created
by earlier versions with one tab per checkout are reused;
their checkout tabs are left open and are no longer managed. Multiple matches
are rejected rather than guessed.

Attachment does not create worktrees or otherwise change Git state. A workspace
with inconsistent configured worktrees or invalid metadata is rejected.
Removing a Forest workspace does not close its Herdr workspace, tabs, or
processes.

Human output summarizes the attached tab:

```text
Workspace  logical-slots
Path       /project/src/.workspaces/logical-slots
Parent     q4-storage
Herdr      w1

  ✓ 2-🌲 logical-slots  created  /project/src/.workspaces/logical-slots
```

### `rename`

Renames an active workspace without changing its branches, commits, upstreams,
or working tree contents:

```sh
git forest rename logical-slots review-123
```

Forest preflights the complete workspace, atomically moves its directory, and
uses `git worktree repair` to update each canonical repository's worktree
metadata. Dirty, untracked, and ignored files are preserved, as are
workspace-local entries. An inconsistent source or an active destination
prevents all mutation. Archives do not reserve names and remain untouched.
Rename never fetches and has no `--force` option.

Existing branch names are authoritative and remain unchanged. If the branch
template uses `{workspace}`, a checkout created after the rename uses the new
workspace name while existing checkouts retain branches rendered with the old
name. Forest cannot safely infer which branches came from the template rather
than an explicit `--branch` override.

A Git repair failure can leave the directory at the new path with one or more
registrations still pointing to the old path. Repeating the same rename command
recognizes that partial state and resumes repair. Forest does not update Herdr
runtime metadata, so a later `attach` under the new name may create a new Herdr
workspace rather than reuse one attached before the rename.

Before moving anything, Forest updates `parent` in every active workspace that
names the old workspace as its parent, and restores those links if the
directory move fails. A resumed rename updates any children that still name the
old workspace.

### `archive`

Archives an entire workspace while preserving workspace-local files and
directories. Forest first preflights every configured checkout, then removes all
clean registered worktrees with `git worktree remove`. Branches are preserved.
After every removal succeeds, Forest atomically moves the remaining workspace
directory to a new generation beneath `<workspaces.root>/.archive/.generations`
without replacing an existing destination.

```sh
git forest archive nkdb-azure
# Saved beneath .archive/.generations/nkdb-azure--2026-09-26-09-46-11-0700
```

Generation names use `<workspace>--<archive_id>`. IDs use local time and its UTC
offset (`YYYY-MM-DD-HH-MM-SS±HHMM`), adding `-2`, `-3`, etc. for same-second
collisions. Older generations remain untouched. Legacy `.archive/<workspace>`
directories use ID `legacy`; `.generations` keeps timestamp-looking legacy names
unambiguous without a manifest or database. Within each workspace, archives sort
by actual instant and numeric suffix, with `legacy` first.

Dirty worktrees, unregistered checkout paths, layout mismatches, a missing
workspace directory, or active child workspaces prevent archival. Archive or
delete the children first, or point them at another parent. When the interactive
launcher retires several selected workspaces, it retires children before their
parents. Passing `--force` removes dirty worktrees
anyway, discarding their modified, untracked, and ignored files; all other
conflicts still prevent archival. Workspace-local entries do not. A Git failure
can leave a partially removed active workspace, and repeating the command safely
resumes. Forest serializes its `create`, `add`, `rename`, `archive`, `unarchive`,
`delete`, `clean`, and `remove` mutations for a configuration so they cannot race
one another. Repeating a successful archive reports `already_archived` and the
newest generation (or the legacy archive if it is the only one) while no new
active workspace exists. Archives are excluded from ordinary `list`, the
interactive launcher, active workspace completion, and `path`; use
`list --archived` to see them. Forest does not close matching Herdr processes.
The archive is local dormant storage, not a compressed archive or backup.

### `unarchive`

Restores saved workspace-local files, optionally creating only the checkouts you
request. Previous checkouts are not recreated automatically: Forest keeps no
manifest of them. Branch and base overrides work as for `create`.

```sh
git forest list --archived
git forest unarchive nkdb-azure api --archive 2026-09-26-09-46-11-0700 --as review-123
```

A single archive is selected automatically; multiple archives require
`--archive <ID>` (including `legacy` for an older archive). `--as <workspace>`
chooses a different active name. Forest preflights every requested checkout
before moving saved files, refuses an existing active destination, and never
overwrites files. Restoration consumes the selected archive, leaving other
generations untouched.

If checkout creation fails after the move, repeat the command. When no matching
archive remains and the destination is active, Forest reconciles the requested
checkouts and reports `already_active`. It never merges an archive into an active
workspace. Without a manifest, it cannot verify the workspace's origin or
distinguish a well-formed nonexistent ID from a consumed one. Malformed IDs are
rejected before reconciliation. Invalid IDs and ambiguous selections return
`conflict` reports.

### `delete`

Permanently deletes a workspace. Forest first applies the same complete-workspace
preflight and worktree removal as `archive`, preserving branches and refusing
dirty or unregistered worktrees and workspaces with active children. It stages
the remaining workspace-local files at `.archive/.deleting/<workspace>`, then
permanently removes them. Pending deletions
are excluded from archive discovery and cannot be restored with `unarchive`.
Pass `--force` to discard modified, untracked, or ignored files in registered
worktrees as well.
Force never deletes branches and does not bypass unregistered-path checks.
Deleting an active workspace preserves all older archives of that name.

If file removal fails after staging, `delete` reports `failed`; repeat the
command to remove the pending files. If a new active workspace has reused the
name, retry refuses both paths until you rename the active workspace.

With no active workspace, remaining registrations, or pending deletion, `delete`
reports `already_deleted`. It never falls back to retained archives. To delete
an archive, including a legacy archive, restore it with `unarchive --archive <ID>`
(optionally `--as <workspace>`), then delete the restored workspace.

### `clean`

Removes stale Git registrations left behind when Forest worktree directories
have already been deleted outside Forest:

```sh
git forest clean
```

Use `remove` or `archive` for normal workspace retirement so Forest can reject
dirty worktrees; `clean` only reconciles registrations after files are already
missing and cannot recover deleted contents.

Forest scans every configured canonical repository and runs
`git worktree remove` for registered worktrees beneath the active workspace
root whose paths no longer exist. It preserves branches, ignores present
worktrees, and does not touch stale registrations outside the configured
workspace root or beneath `.archive`. The command attempts every stale
registration and exits unsuccessfully if any removal fails. Repeating it is
safe; when nothing needs cleaning, it succeeds with an empty report.

### `remove`

Removal is deliberately conservative:

- modified, untracked, and ignored files prevent removal unless `--force` is
  passed, in which case they are discarded via `git worktree remove --force`;
- every selected path must be registered with its configured canonical
  repository, even with `--force`;
- removal always uses `git worktree remove`, including to clean up registered
  worktrees whose paths are already missing;
- branches are never deleted;
- the workspace directory is removed only when it is empty and no active
  workspace names it as a parent;
- workspace-local entries are reported and preserved.

A saved `.forest-workspace.toml` is preserved too, so `remove` leaves the
workspace directory in place even after its last checkout is removed. Use
`delete` to remove the workspace and its local files.

Removing only named checkout identifiers leaves other worktrees in place.
`remove stacked api@part-2` removes only that named checkout; `api` continues
to mean only the primary checkout. Repeating a partially completed removal is
safe.

## JSON contract

Paths are absolute. Optional values are represented as `null` rather than
omitted.

### Setup

```json
{
  "repositories": [
    {
      "name": "api",
      "path": "/project/src/api",
      "remote": "git@github.com:example/api.git",
      "status": "cloned",
      "message": null
    }
  ]
}
```

Setup status is `cloned`, `reused`, `conflict`, `failed`, or `not_run`.

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

### Update

```json
{
  "repositories": [
    {
      "name": "api",
      "path": "/project/src/api",
      "branch": "main",
      "status": "updated",
      "message": null
    }
  ]
}
```

Update status is `updated`, `up_to_date`, `conflict`, or `failed`. `branch` is
null when `origin/HEAD` or the fetch result does not identify a default branch.

### Create and add

```json
{
  "workspace": "logical-slots",
  "path": "/project/src/.workspaces/logical-slots",
  "repositories": [
    {
      "name": "api",
      "checkout": "api",
      "slot": null,
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

`name` remains the configured repository name. `checkout` is its unique command
selector and `slot` is null for a primary checkout. `action` is `reuse`,
`add_existing_branch`, `create_branch`, or `null` for a conflict discovered
before an action could be selected. A branch created to track an explicit
`--branch` uses `create_branch`, with its remote-tracking ref in `base_ref`.
`status` is `reused`, `created`, `conflict`, `failed`, or `not_run`.

### List

```json
{
  "workspaces": [
    {
      "name": "logical-slots",
      "path": "/project/src/.workspaces/logical-slots",
      "exists": true,
      "parent": "q4-storage",
      "repositories": [
        {
          "name": "api",
          "checkout": "api",
          "slot": null,
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

### List archived

`list --archived --json` returns archives instead of active workspaces:

```json
{
  "archives": [
    {
      "workspace": "nkdb-azure",
      "archive_id": "2026-09-26-09-46-11-0700",
      "archive_path": "/project/src/.workspaces/.archive/.generations/nkdb-azure--2026-09-26-09-46-11-0700"
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
      "parent": "q4-storage",
      "repositories": [
        {
          "name": "api",
          "checkout": "api",
          "slot": null,
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

### Attach

```json
{
  "workspace": "logical-slots",
  "path": "/project/src/.workspaces/logical-slots",
  "parent": "q4-storage",
  "herdr_workspace_id": "w1",
  "status": "created",
  "tabs": [
    {
      "label": "2-🌲 logical-slots",
      "path": "/project/src/.workspaces/logical-slots",
      "herdr_tab_id": "w1:t2",
      "status": "created"
    }
  ],
  "warnings": [],
  "descendants": []
}
```

`tabs` always holds the single attached tab. `parent` is the saved parent, or
`null`. `descendants` lists a report of the same shape for each descendant in
attach order, and is empty for a workspace without children; each carries its
own `herdr_workspace_id`.
Workspace and tab status is one of `created`, `reused`, or `reconciled`.

### Rename

```json
{
  "old_workspace": "logical-slots",
  "old_path": "/project/src/.workspaces/logical-slots",
  "workspace": "review-123",
  "path": "/project/src/.workspaces/review-123",
  "repositories": [
    {
      "name": "api",
      "checkout": "api",
      "slot": null,
      "old_path": "/project/src/.workspaces/logical-slots/api",
      "path": "/project/src/.workspaces/review-123/api",
      "branch": "user/logical-slots",
      "status": "repaired",
      "message": null
    }
  ],
  "status": "renamed",
  "message": null
}
```

Workspace rename status is `renamed`, `conflict`, or `failed`. Repository
status is `repaired`, `already_repaired`, `failed`, or `not_run`.

### Archive

```json
{
  "workspace": "logical-slots",
  "path": "/project/src/.workspaces/logical-slots",
  "archive_id": "2026-09-26-09-46-11-0700",
  "archive_path": "/project/src/.workspaces/.archive/.generations/logical-slots--2026-09-26-09-46-11-0700",
  "repositories": [
    {
      "name": "api",
      "checkout": "api",
      "slot": null,
      "path": "/project/src/.workspaces/logical-slots/api",
      "status": "removed",
      "message": null
    }
  ],
  "status": "archived",
  "preserved_entries": [
    "/project/src/.workspaces/.archive/.generations/logical-slots--2026-09-26-09-46-11-0700/notes.md"
  ],
  "message": null
}
```

Archive status is `archived`, `already_archived`, `conflict`, or `failed`.
Repository removal statuses have the same meanings as for `remove`.

### Unarchive

Restoring saved files without requesting checkouts:

```json
{
  "source_workspace": "nkdb-azure",
  "workspace": "review-123",
  "path": "/project/src/.workspaces/review-123",
  "archive_id": "2026-09-26-09-46-11-0700",
  "archive_path": "/project/src/.workspaces/.archive/.generations/nkdb-azure--2026-09-26-09-46-11-0700",
  "repositories": [],
  "status": "unarchived",
  "message": null
}
```

Unarchive status is `unarchived`, `already_active`, `conflict`, or `failed`.
Requested checkout reports in `repositories` use the same fields and statuses
as `create`. `archive_path` records the selected source, even after its move;
it is `null` on a retry when that archive has already been consumed.
`archive_id` is `null` when no archive is selected and no ID was supplied.

### Delete

```json
{
  "workspace": "logical-slots",
  "path": "/project/src/.workspaces/logical-slots",
  "repositories": [
    {
      "name": "api",
      "checkout": "api",
      "slot": null,
      "path": "/project/src/.workspaces/logical-slots/api",
      "status": "removed",
      "message": null
    }
  ],
  "status": "deleted",
  "deleted_entries": [
    "/project/src/.workspaces/.archive/.deleting/logical-slots/notes.md"
  ],
  "message": null
}
```

Delete status is `deleted`, `already_deleted`, `conflict`, or `failed`.
`deleted_entries` lists the deletion staging paths of removed workspace-local
entries, excluding retained archives. Repository removal statuses have the same
meanings as for `remove`.

### Clean

```json
{
  "worktrees": [
    {
      "workspace": "logical-slots",
      "name": "api",
      "checkout": "api",
      "slot": null,
      "path": "/project/src/.workspaces/logical-slots/api",
      "status": "removed",
      "message": null
    }
  ]
}
```

Clean status is `removed` or `failed`.

### Remove

```json
{
  "workspace": "logical-slots",
  "path": "/project/src/.workspaces/logical-slots",
  "repositories": [
    {
      "name": "api",
      "checkout": "api",
      "slot": null,
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
- `1`: an operational conflict or Git, Herdr, or filesystem failure;
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
