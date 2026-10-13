#!/bin/sh
# Scheduled check: the project's own verify command.
#
# The daemon hands this script its own environment, so CARGO_TARGET_DIR points
# at the target directory the job clones' verify runs and the daemon's own
# `cargo build --release` (self-update) share. Running `cargo test` in that
# directory while one of them builds rewrites artifacts under both: doctests
# fail with E0460 "found possibly newer version of crate" and test binaries
# exit abnormally for reasons the default branch does not contain. Give this
# check its own target subdirectory instead: it stays warm across runs and
# never overlaps another build.
set -e
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target}/verify-check"
export CARGO_TARGET_DIR
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
