set shell := ["bash", "-euo", "pipefail", "-c"]

# List available recipes.
default:
    @just --list

# Install git-forest into Cargo's binary directory.
install:
    cargo install --locked --force --path "{{ justfile_directory() }}"

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
