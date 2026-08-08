# Development guidance

`git-forest` is a deterministic Rust CLI for managing linked worktrees across
configured canonical repositories.

## Invariants

- Git worktree metadata and the filesystem are authoritative. Do not add a
  database, global registry, or required generated manifest.
- Invoke the installed `git` executable with argument arrays. Do not use shell
  command strings or add a Git library.
- Clear inherited repository-routing Git environment variables before nested
  Git commands.
- Do not add implicit `fetch`, `pull`, clone, or other network operations.
- Creation must preflight every requested repository before mutation and remain
  safe to rerun after partial execution.
- Removal must use `git worktree remove`, preserve branches, reject dirty or
  unregistered paths, and never offer force behavior.
- Keep human output on stdout, diagnostics on stderr, and JSON streams free of
  non-JSON text.
- Keep command report field names stable. Update README examples and integration
  tests when intentionally changing the JSON contract.

## Structure

- `src/cli.rs`: command-line syntax only.
- `src/config.rs`: discovery, TOML parsing, templates, and path policy.
- `src/git.rs`: all Git subprocess invocation and porcelain parsing.
- `src/workspace.rs`: reconciliation of filesystem and Git worktree state.
- `src/domain.rs`: serializable reports.
- `src/output.rs`: human and JSON rendering.
- `src/commands/`: command behavior.
- `tests/cli.rs`: temporary-repository integration coverage.

Command implementations should inspect state and return typed reports rather
than printing directly.

## Quality gates

Run all gates before considering a change complete:

```sh
just check
just test
```

`just check` applies formatting and Clippy fixes before validating the tree, so
review its diff. Tests must use temporary repositories and local bare origins.
Do not depend on network access, global Git identity, or the developer's real
repositories.

The equivalent non-mutating CI gates are:

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
```

## Automation and supply chain

- Keep `rust-toolchain.toml` and `Cargo.lock` committed. Use `--locked` for all
  dependency-resolving Cargo commands in CI.
- Pin every external GitHub Actions `uses:` reference to a full 40-character
  commit SHA and retain a release comment beside it. Never use a floating tag,
  branch, or mutable Docker tag.
- Use fixed hosted-runner labels rather than `*-latest`, least-privilege job
  permissions, explicit timeouts, and checkout with credential persistence
  disabled.
- Update action pins and dependencies through reviewed Dependabot pull requests.
