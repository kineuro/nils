#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# The engine's tests, every test binary beside the others. `cargo test` runs the
# binaries one after another, and a few of them take a minute each; here a few run
# at once, and each binary has a Postgres database of its own, since the tests of a
# crate keep one schema name and take turns on it only inside one binary. A binary
# that passes gets a line; one that fails shows everything it printed. Without
# NILS_TEST_POSTGRES_DSN every binary skips its Postgres half, as under `cargo test`.
# TEST_JOBS sets how many binaries run at once, the number of cores by default.
set -euo pipefail
cd "$(dirname "$0")/../engine"

jobs=${TEST_JOBS:-$(nproc)}
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
  if (cd "$dir" && CARGO_MANIFEST_DIR="$dir" NILS_TEST_POSTGRES_DSN="$db" "$exe") > "$work/$n.log" 2>&1; then
    printf 'ok      %3ds  %-24s %s\n' $((SECONDS - start)) "${exe##*/}" "$(grep -m1 -oE '[0-9]+ passed' "$work/$n.log" || true)"
  else
    printf 'FAILED  %3ds  %s\n' $((SECONDS - start)) "${exe##*/}"
    cat "$work/$n.log"
    return 1
  fi
}

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
