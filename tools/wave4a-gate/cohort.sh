#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
#
# The Wave 4a gate on a real cohort (`docs/specs/wave4a-engine-completes.md`,
# section 12), run on a private host against a raw tree, its clinical files
# and their mappings. Nothing here names a path: every location is an
# argument, and the outputs are counts and timings, never a value.
#
#     tools/wave4a-gate/cohort.sh --nils BIN --source DIR --csv DIR --maps DIR \
#         --packs DIR --work DIR [--v0-counts FILE] [--workers N]
#
# What it builds, in order: a registry from raw (bar 1), the clinical layer
# through the one importer, twice (bar 7), the classification and its review
# queue (bar 8), both release layouts with the clinical export and a re-run
# that writes nothing (bars 2, 6), a handover, a second process through the
# door doing what the command line did (bar 9), the custody table (bar 10),
# and the budget of each step (bar 11). `check.py` reads the work directory
# and asserts the bars.
set -euo pipefail

nils=""; source=""; csv=""; maps=""; packs=""; work=""; v0counts=""; workers=16
while [[ $# -gt 0 ]]; do
  case "$1" in
    --nils) nils="$2"; shift 2 ;;
    --source) source="$2"; shift 2 ;;
    --csv) csv="$2"; shift 2 ;;
    --maps) maps="$2"; shift 2 ;;
    --packs) packs="$2"; shift 2 ;;
    --work) work="$2"; shift 2 ;;
    --v0-counts) v0counts="$2"; shift 2 ;;
    --workers) workers="$2"; shift 2 ;;
    *) echo "cohort.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
for v in nils source csv maps packs work; do
  if [[ -z "${!v}" ]]; then echo "cohort.sh: --$v is required" >&2; exit 2; fi
done
if [[ -e "$work" ]]; then
  echo "cohort.sh: $work already exists; a gate run never writes into another run's directory" >&2
  exit 2
fi
mkdir -p "$work"
export NILS_REGISTRY="$work/home"
export NILS_PACK_DIR="$packs"
mkdir -p "$NILS_REGISTRY"
budget="$work/budget.tsv"
printf 'step\tseconds\n' > "$budget"
step() {
  # step NAME command...: run it, time it, record it.
  local name="$1"; shift
  local t0=$SECONDS
  echo "gate: $name" >&2
  "$@"
  printf '%s\t%s\n' "$name" "$((SECONDS - t0))" >> "$budget"
}
[[ -n "$v0counts" ]] && cp "$v0counts" "$work/v0-counts.json"

head -c 32 /dev/urandom > "$work/key.bin"
"$nils" key add gate --from-file "$work/key.bin" >/dev/null
"$nils" init --key gate --backend sqlite >/dev/null

# --- bar 1: the registry from raw
step digest "$nils" digest "$source" --name cohort --workers "$workers" --json > "$work/digest.json"
step fingerprint "$nils" fingerprint --json > "$work/fingerprint.json"
step classify "$nils" classify --json > "$work/classify.json"
step pick "$nils" pick run --json > "$work/pick.json"

# --- bar 7: the clinical layer through the one importer, twice
step vocabulary "$nils" clinical vocabulary load --json > "$work/vocabulary.json"
import() {
  # import NAME MAPPING CSV: preview, apply, apply again.
  local name="$1" mapping="$2" file="$3"
  "$nils" clinical import --mapping "$mapping" --file "$file" --json > "$work/import-$name-preview.json"
  "$nils" clinical import --mapping "$mapping" --file "$file" --apply --json > "$work/import-$name-apply.json"
  "$nils" clinical import --mapping "$mapping" --file "$file" --apply --json > "$work/import-$name-again.json"
}
step import-cohort import cohort "$maps/cohort.yml" "$maps/cohort.csv"
step import-membership import membership "$maps/membership.yml" "$csv/membership.csv"
step import-demographics import demographics "$maps/demographics.yml" "$csv/demographics.csv"
step import-diseases import diseases "$maps/diseases.yml" "$csv/diseases.csv"
step import-events import events "$maps/events.yml" "$csv/events.csv"

# --- bar 8: the queue a person reads
"$nils" review list --status open --json > "$work/review-open.json"

# --- bars 2 and 6: both layouts, the clinical export, a re-run that writes nothing
step release-descriptive "$nils" release --out "$work/desc" --name cohort-desc --layout descriptive \
  --on-unknown write --json > "$work/desc.json"
step release-descriptive-again "$nils" release --out "$work/desc" --name cohort-desc --layout descriptive \
  --on-unknown write --json > "$work/desc-again.json"
if command -v dcm2niix >/dev/null; then
  step release-bids "$nils" release --out "$work/bids" --name cohort-bids --layout bids \
    --on-unknown write --observation EDSS --json > "$work/bids.json"
  step release-bids-again "$nils" release --out "$work/bids" --name cohort-bids --layout bids \
    --on-unknown write --observation EDSS --json > "$work/bids-again.json"
else
  echo "gate: dcm2niix is not installed here; the BIDS half is skipped" >&2
fi
"$nils" release --history --json > "$work/releases.json"

# --- the handover
if command -v 7z >/dev/null; then
  step handover "$nils" handover run --release cohort-desc --out "$work/ship" --key gate \
    --chunk 1GB --json > "$work/handover.json"
else
  echo "gate: 7z is not installed here; the handover is skipped" >&2
fi

# --- bar 9: a second process through the door does what the command line did
"$nils" status --json > "$work/status.json"
"$nils" custody --json > "$work/custody.json"
"$nils" select --cohort nmosd --json > "$work/select.json"
door() {
  # door N: serve N requests on a free port, in the background; prints the port.
  "$nils" serve --bind 127.0.0.1:0 --requests "$1" > "$work/serve.out" 2> "$work/serve.err" &
  serve_pid=$!
  for _ in $(seq 1 50); do
    if [[ -s "$work/serve.out" ]]; then break; fi
    sleep 0.2
  done
  awk 'NR == 1 {print $3}' "$work/serve.out" | cut -d: -f2
}
port="$(door 7)"
url="http://127.0.0.1:$port/api"
curl -s "$url/capabilities" > "$work/door-capabilities.json"
curl -s "$url/status" > "$work/door-status.json"
curl -s "$url/custody" > "$work/door-custody.json"
curl -s -X POST -H 'Content-Type: application/json' -d '{"cohorts": ["nmosd"]}' "$url/select" > "$work/door-select.json"
curl -s "$url/review?status=open" > "$work/door-review.json"
curl -s "$url/releases" > "$work/door-releases.json"
curl -s -X POST -H 'Content-Type: application/json' \
  -d "{\"name\": \"cohort-door\", \"out\": \"$work/door-desc\", \"layout\": \"descriptive\", \"on_unknown\": \"write\", \"cohorts\": [\"nmosd\"]}" \
  "$url/releases" > "$work/door-release-queued.json"
wait "$serve_pid" || true
step door-release "$nils" jobs work --once
"$nils" jobs list --all --json > "$work/jobs.json"

# --- bar 10: the custody table; bar 11: the budget is in budget.tsv
"$nils" audit list --json --limit 200 > "$work/audit.json"
echo "gate: the bars"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 "$here/check.py" "$work"
