#!/bin/sh
# Scheduled check: the project's own verify command.
set -e
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
