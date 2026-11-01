# JSON reports

Every command except `completions` accepts `--json`.

Paths are absolute. Optional values are represented as `null` rather than
omitted.

## Setup

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

## Repositories

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

## Fetch

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

## Update

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

## Create and add

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

`name` is the configured repository name. `checkout` is its unique command
selector and `slot` is null for a primary checkout. `action` is `reuse`,
`add_existing_branch`, `create_branch`, or `null` for a conflict discovered
before an action could be selected. A branch created to track an explicit
`--branch` uses `create_branch`, with its remote-tracking ref in `base_ref`.
`status` is `reused`, `created`, `conflict`, `failed`, or `not_run`.

## List

```json
{
  "workspaces": [
    {
      "name": "logical-slots",
      "path": "/project/src/.workspaces/logical-slots",
      "exists": true,
      "parent": "q4-storage",
      "symbol": "🚦",
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

## List archived

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

## Status

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

## Path

```json
{
  "workspace": "logical-slots",
  "path": "/project/src/.workspaces/logical-slots"
}
```

## Rename

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

## Archive

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

## Unarchive

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

## Delete

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

## Clean

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

## Remove

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
- `1`: an operational conflict or Git or filesystem failure;
- `2`: usage, input, or configuration error.
