#!/bin/sh
# Build wrapper: puts the project-local C toolchain + cargo on PATH.
ROOT="$(cd "$(dirname "$0")" && pwd)"
export PATH="$ROOT/toolchain/w64devkit/bin:$HOME/.cargo/bin:$PATH"
# Incremental artifacts have corrupted twice on this project and grow the
# target dir by gigabytes; every documented workflow runs without them.
export CARGO_INCREMENTAL=0
cd "$ROOT"
case "$1" in
  --release) shift; exec cargo build --release "$@" ;;
  --run)     shift; exec cargo run --release "$@" ;;
  # The app suite builds a HEADLESS GPU RENDERER PER TEST, and a document
  # can be a 6000x8600 page. Enough of those alive at once and the process
  # dies — observed 2026-08-19 as `memory allocation of 3342336 bytes
  # failed` on a 16 GB machine and as an outright STATUS_ACCESS_VIOLATION
  # under the software adapter. Two at a time is the default so nobody has
  # to remember a flag; export RUST_TEST_THREADS yourself to override.
  --test)    shift; export RUST_TEST_THREADS="${RUST_TEST_THREADS:-2}"
             exec cargo test "$@" ;;
  *)         exec cargo build "$@" ;;
esac
