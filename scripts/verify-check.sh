#!/bin/sh
# Scheduled check: the project's own verify command.
#
# The daemon runs this script in a throwaway worktree under PULSE_HOME/work
# that it removes when the run ends, and its environment carries no
# CARGO_TARGET_DIR. A target directory inside that worktree is therefore cold
# on every run: the whole workspace compiles from scratch (several GB written
# and then unlinked each day, on the bulk disk when one is attached). Build
# into a directory that outlives the worktree instead, one per schedule so it
# never overlaps the job clones' verify runs or the daemon's own
# `cargo build --release` (self-update): sharing a target with a concurrent
# build rewrites artifacts under both, doctests fail with E0460 "found
# possibly newer version of crate" and test binaries exit abnormally for
# reasons the default branch does not contain.
#
# A CARGO_TARGET_DIR set by the caller still wins; it gets its own
# subdirectory for the same reason.
set -e
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR/verify-check"
elif [ -n "${PULSE_HOME:-}" ]; then
  CARGO_TARGET_DIR="$PULSE_HOME/cache/cargo-target-check/${PULSE_PROJECT:-amistio-template}/${PULSE_SCHEDULE:-verify-check}"
else
  CARGO_TARGET_DIR="target/verify-check"
fi
export CARGO_TARGET_DIR
# Incremental compilation keeps per-crate session caches that are rewritten on
# every change; a once-a-day check gains little from them and writes far less
# without. A caller's explicit choice still wins.
: "${CARGO_INCREMENTAL:=0}"
export CARGO_INCREMENTAL
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
