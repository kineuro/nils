#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
#
# The budget half of the Wave 4a gate (spec section 12, bar 11), on the
# baseline host: a registry from raw, its classification and a descriptive
# release, each step timed, and the digest's rate and peak memory held to a
# floor and a cap. No clinical file is needed, so it runs where the raw tree
# is and nothing else is.
#
#     tools/wave4a-gate/budget.sh --nils BIN --source DIR --packs DIR --work DIR [--workers N]
set -euo pipefail

nils=""; source=""; packs=""; work=""; workers=8
while [[ $# -gt 0 ]]; do
  case "$1" in
    --nils) nils="$2"; shift 2 ;;
    --source) source="$2"; shift 2 ;;
    --packs) packs="$2"; shift 2 ;;
    --work) work="$2"; shift 2 ;;
    --workers) workers="$2"; shift 2 ;;
    *) echo "budget.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
for v in nils source packs work; do
  if [[ -z "${!v}" ]]; then echo "budget.sh: --$v is required" >&2; exit 2; fi
done
if [[ -e "$work" ]]; then
  echo "budget.sh: $work already exists" >&2
  exit 2
fi
mkdir -p "$work"
export NILS_REGISTRY="$work/home"
export NILS_PACK_DIR="$packs"
mkdir -p "$NILS_REGISTRY"
budget="$work/budget.tsv"
printf 'step\tseconds\n' > "$budget"
step() {
  local name="$1"; shift
  local t0=$SECONDS
  echo "budget: $name" >&2
  "$@"
  printf '%s\t%s\n' "$name" "$((SECONDS - t0))" >> "$budget"
}
head -c 32 /dev/urandom > "$work/key.bin"
"$nils" key add gate --from-file "$work/key.bin" >/dev/null
"$nils" init --key gate --backend sqlite >/dev/null
step digest "$nils" digest "$source" --name budget --workers "$workers" --json > "$work/digest.json"
step digest-again "$nils" digest "$source" --name budget --workers "$workers" --json > "$work/digest-again.json"
step fingerprint "$nils" fingerprint --json > "$work/fingerprint.json"
step classify "$nils" classify --json > "$work/classify.json"
step release-descriptive "$nils" release --out "$work/desc" --name budget-desc --layout descriptive \
  --on-unknown write --json > "$work/desc.json"
step release-descriptive-again "$nils" release --out "$work/desc" --name budget-desc --layout descriptive \
  --on-unknown write --json > "$work/desc-again.json"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 "$here/check.py" "$work" --budget
