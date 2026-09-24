#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# The engine's tests, every test binary beside the others. `cargo test` runs the
# binaries one after another, and a few of them take a minute each; here a few run
# at once, and each binary has a Postgres database of its own, since the tests of a
# crate keep one schema name and take turns on it only inside one binary. A binary
# that passes gets a line; one that fails shows everything it printed. Without
# NILS_TEST_POSTGRES_DSN every binary skips its Postgres half, as under `cargo test`.
# TEST_JOBS sets how many binaries run at once, the number of cores by default.
# TEST_BINARY_TIMEOUT (seconds, 900 by default) bounds each binary: one that hangs
# is stopped and fails with what it printed, where libtest names every test still
# running after a minute, so a hang fails in minutes and says which test it is.
# While binaries run, a line a minute says which, and the memory and disk left.
set -euo pipefail
cd "$(dirname "$0")/../engine"

jobs=${TEST_JOBS:-$(nproc)}
limit=${TEST_BINARY_TIMEOUT:-900}
dsn=${NILS_TEST_POSTGRES_DSN:-}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# every test binary the workspace builds, with the crate directory it runs in
cargo test --workspace --locked --no-run --message-format=json-render-diagnostics |
  jq -r 'select(.reason == "compiler-artifact" and .profile.test and .executable != null)
    | [.executable, (.manifest_path | rtrimstr("/Cargo.toml"))] | @tsv' > "$work/binaries"

one() {
  local n=$1 exe=$2 dir=$3 db="" start=$SECONDS
  if [ -n "$dsn" ]; then
    db="${dsn%/*}/nils_test_$n"
    psql "$dsn" -q -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS nils_test_$n" -c "CREATE DATABASE nils_test_$n" > "$work/$n.log" 2>&1 ||
      { echo "FAILED  no database for ${exe##*/}"; cat "$work/$n.log"; return 1; }
  fi
  echo "${exe##*/}" > "$work/$n.running"
  local status=0
  (cd "$dir" && CARGO_MANIFEST_DIR="$dir" NILS_TEST_POSTGRES_DSN="$db" \
    timeout --kill-after=30 "$limit" "$exe") > "$work/$n.log" 2>&1 || status=$?
  rm -f "$work/$n.running"
  if [ "$status" -eq 0 ]; then
    printf 'ok      %3ds  %-24s %s\n' $((SECONDS - start)) "${exe##*/}" "$(grep -m1 -oE '[0-9]+ passed' "$work/$n.log" || true)"
  else
    if [ "$status" -eq 124 ] || [ "$status" -eq 137 ]; then
      printf 'TIMEOUT %3ds  %s did not end within %ss\n' $((SECONDS - start)) "${exe##*/}" "$limit"
    fi
    printf 'FAILED  %3ds  %s\n' $((SECONDS - start)) "${exe##*/}"
    cat "$work/$n.log"
    return 1
  fi
}

# a line a minute while binaries run: which, and what memory and disk are left
(
  while sleep 60; do
    names=$(cat "$work"/*.running 2>/dev/null | tr '\n' ' ' || true)
    mem=$(awk '/^MemAvailable:/ {print int($2 / 1024)}' /proc/meminfo 2>/dev/null || true)
    disk=$(df -Pm "${TMPDIR:-/tmp}" 2>/dev/null | awk 'NR == 2 {print $4}' || true)
    printf 'running %4ds  %s(%s MiB memory and %s MiB disk free)\n' "$SECONDS" "$names" "$mem" "$disk"
  done
) &
beat=$!
trap 'kill "$beat" 2>/dev/null || true; rm -rf "$work"' EXIT

failed=0 running=0 n=0
while IFS=$'\t' read -r exe dir; do
  n=$((n + 1))
  one "$n" "$exe" "$dir" &
  running=$((running + 1))
  if [ "$running" -ge "$jobs" ]; then
    wait -n || failed=1
    running=$((running - 1))
  fi
done < "$work/binaries"
while [ "$running" -gt 0 ]; do
  wait -n || failed=1
  running=$((running - 1))
done
echo "$n test binaries"

# the examples in the documentation, which `cargo test` runs after the binaries
cargo test --workspace --locked --doc --quiet
exit $failed
