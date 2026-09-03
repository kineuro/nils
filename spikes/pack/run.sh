#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# The measurement protocol of the pack spike.
#
#   run.sh DATA_DIR OUT_DIR V0_SRC
#
# DATA_DIR holds the two CSVs `export.sh` wrote, decompressed. V0_SRC is the
# `backend/src` of a v0 0.5.3 checkout, which lives on the private host and
# never in this repository. Each mode is run twice, the pack and then v0's own
# code, and diffed; the exit status is non-zero when anything disagrees.
set -eu

data=${1:?usage: run.sh DATA_DIR OUT_DIR V0_SRC}
out=${2:?}
v0=${3:?}
here=$(cd "$(dirname "$0")" && pwd)

mkdir -p "$out"
cargo build --release --manifest-path "$here/rust/Cargo.toml"
eval="$here/rust/target/release/packeval"

fp="$data/stack_fingerprint.csv"
scc="$data/series_classification_cache.csv"

for mode in flags branch vote; do
    printf '\n=== %s\n' "$mode" >&2
    case $mode in
        flags) args="" ;;
        *)     args="--scc $scc" ;;
    esac
    # shellcheck disable=SC2086
    "$eval" "$mode" --pack "$here/pack" --fp "$fp" $args > "$out/$mode-v1.tsv"
    # shellcheck disable=SC2086
    python3 "$here/referee.py" "$mode" --v0 "$v0" --fp "$fp" $args > "$out/$mode-v0.tsv"
    python3 "$here/compare.py" "$mode" "$out/$mode-v1.tsv" "$out/$mode-v0.tsv" | tee "$out/$mode.diff"
done
