#!/bin/sh
# Development aid: compare heap behaviour of the working tree against a
# baseline git worktree, scenario by scenario.
#
# Both trees are built with the `alloc_count` example (see examples/alloc_count.rs),
# which counts allocation calls, total bytes requested and peak live heap for one
# `ebook` conversion. Run the same input through both binaries with identical flags
# and diff the three numbers; peak live is the one that answers "did this change
# raise peak memory?".
#
# Usage: sh scripts/memory_bench.sh [baseline-worktree] [input.cbz]
set -eu
BASE="${1:-target/bench/base3}"
INPUT="${2:-target/bench/bench.cbz}"
BASE_BIN="$BASE/target/release/examples/alloc_count"
CUR_BIN="target/release/examples/alloc_count"

cargo build --release --example alloc_count
(cd "$BASE" && cargo build --release --example alloc_count)

run() {
  label="$1"
  bin="$2"
  out="$3"
  shift 3
  rm -rf "$out"
  printf '%-14s' "$label"
  "$bin" "$INPUT" "$out" "$@" 2>/dev/null | grep -E 'allocations|allocated MiB|peak live' | tr '\n' ' '
  echo
}

compare() {
  name="$1"
  shift
  echo "== $name =="
  run baseline "$BASE_BIN" target/bench/out_mem_base "$@"
  run current "$CUR_BIN" target/bench/out_mem_cur "$@"
}

compare "epub (default)"
compare "force-png" --force-png
compare "webtoon" --webtoon
compare "pdf" -f pdf
compare "light-novel" --light-novel
compare "no-processing" --no-processing
