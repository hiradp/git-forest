set shell := ["bash", "-euo", "pipefail", "-c"]

# List available recipes.
default:
    @just --list

# Install git-forest and its manual into Cargo's installation prefix.
install:
    root="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}"; \
        cargo install --locked --force --root "$root" --path "{{ justfile_directory() }}"; \
        mkdir -p "$root/share/man/man1"; \
        install -m 0644 "{{ justfile_directory() }}/docs/git-forest.1" "$root/share/man/man1/git-forest.1"

# Apply formatting and Clippy fixes, then verify the tree.
check:
    cargo fmt --all
    cargo clippy --fix --allow-dirty --allow-staged --locked --all-targets --all-features -- -D warnings
    cargo fmt --all
    cargo fmt --all --check
    cargo clippy --locked --all-targets --all-features -- -D warnings

# Run the test suite.
test:
    cargo test --locked --all-features
