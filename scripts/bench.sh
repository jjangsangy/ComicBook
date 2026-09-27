#!/bin/sh
# Development aid: time `comic-book ebook` on a synthetic manga archive.
#
# The input is generated deterministically (see examples/gen_bench.rs) so
# repeated runs are comparable, and it is kept deliberately short so the CPU
# does not thermally throttle before the measurement finishes. `user` is the
# most stable signal: it is the total CPU work, independent of turbo/scheduling.
#
# Usage: sh scripts/bench.sh [pages] [runs]
set -eu
PAGES="${1:-40}"
RUNS="${2:-3}"
INPUT="target/bench/bench.cbz"
OUT="target/bench/out_time"

cargo build --release

if [ ! -f "$INPUT" ]; then
  cargo run --release --example gen_bench -- "$INPUT" "$PAGES" 1600 2400
fi

rm -rf "$OUT"
mkdir -p "$OUT"
i=1
while [ "$i" -le "$RUNS" ]; do
  /usr/bin/time -p ./target/release/comic-book ebook "$INPUT" -f epub -o "$OUT" 2>&1 \
    | grep -E '^(real|user|sys)' | tr '\n' ' '
  echo " (run $i/$RUNS)"
  i=$((i + 1))
done
