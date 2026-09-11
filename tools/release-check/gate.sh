#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
#
# The release gate (`docs/specs/wave3-anonymize-and-bids.md`, §12).
#
# Builds a registry from the reference selections, releases it in both layouts
# under three date policies, hands one over, and asserts every bar. Nothing here
# touches a real corpus: the reference tree is synthetic and its right answers
# are checked in beside it, which is what lets the gate assert rather than
# eyeball.
#
#     tools/release-check/gate.sh /data/working/release-gate
#
# The first bar, the repairs, is `tools/repair-check/gate.sh` and runs
# separately: it is about digest and this is about release.
set -euo pipefail

work="${1:-}"
if [[ -z "$work" ]]; then
  echo "usage: gate.sh WORKDIR" >&2
  exit 2
fi
if [[ -e "$work" ]]; then
  echo "gate: $work already exists; a gate run must never write into another run's directory" >&2
  exit 2
fi

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
engine="$root/engine"
packs="$root/packs"

nils="${NILS:-$engine/target/release/nils}"
export NILS="$nils"
if [[ ! -x "$nils" ]]; then
  echo "gate: building nils" >&2
  (cd "$engine" && cargo build --release -p nils --example-free 2>/dev/null || cargo build --release -p nils)
fi
if [[ ! -x "$engine/target/release/examples/reference" ]]; then
  (cd "$engine" && cargo build --release -p nils-dicom --example reference)
fi

mkdir -p "$work"
export NILS_REGISTRY="$work/home"
export NILS_PACK_DIR="$packs"
mkdir -p "$NILS_REGISTRY"

echo "gate: the reference selections"
"$engine/target/release/examples/reference" --out "$work/source" >/dev/null

head -c 32 /dev/urandom > "$work/key.bin"
"$nils" key add gate --from-file "$work/key.bin" >/dev/null
"$nils" init --key gate --backend sqlite >/dev/null

echo "gate: digest, fingerprint, classify"
"$nils" digest "$work/source" --name reference --json > "$work/digest.json"
"$nils" fingerprint --json > "$work/fingerprint.json"
"$nils" classify --json > "$work/classify.json"
"$nils" pick run --json > "$work/pick.json" 2>/dev/null || true

echo "gate: the clinical layer (Wave 4a section 7)"
# The vocabulary is pack data, and the EDSS file names the subject by the
# code the registry gave it, which is what the clinic's export would carry
# after the linkage step. Two scores, five days before the first session and
# five days after the second, so that each session has one nearest value and
# the two are told apart.
"$nils" clinical vocabulary load --json > "$work/vocabulary.json"
code="$(python3 -c 'import sqlite3, sys; print(sqlite3.connect(sys.argv[1]).execute("SELECT code FROM subject ORDER BY id LIMIT 1").fetchone()[0])' "$NILS_REGISTRY/registry.db")"
cat > "$work/edss.csv" <<CSV
code,visit_date,edss
$code,2022-01-10,3.5
$code,2022-07-20,4.0
CSV
cat > "$work/edss.yml" <<'YML'
import:
  target: event
  subject: {column: code, by: code}
  observation_type: EDSS
  source: the gate's EDSS file
  columns:
    event_date: {column: visit_date, parser: date, format: "%Y-%m-%d"}
    value: {column: edss, parser: float}
  key: [event_date]
  on_existing: skip
YML
"$nils" clinical import --mapping "$work/edss.yml" --file "$work/edss.csv" --apply --json \
  > "$work/edss-import.json"

echo "gate: the descriptive layout"
/usr/bin/time -v "$nils" release --out "$work/descriptive" --name gate-descriptive \
  --layout descriptive --json > "$work/descriptive.json" 2> "$work/descriptive.time"

echo "gate: the same release again, which must write nothing"
"$nils" release --out "$work/descriptive" --name gate-descriptive \
  --layout descriptive --json > "$work/descriptive-again.json"

converter="$(command -v dcm2niix || true)"
if [[ -n "$converter" ]]; then
  echo "gate: the BIDS layout"
  /usr/bin/time -v "$nils" release --out "$work/bids" --name gate-bids --layout bids \
    --json > "$work/bids.json" 2> "$work/bids.time"
  "$nils" release --out "$work/bids" --name gate-bids --layout bids \
    --json > "$work/bids-again.json"
  echo "gate: a shifted BIDS tree, for the clinical join under the shift"
  cat > "$work/ordinal.yml" <<'YML'
session:
  window_days: 0
  naming: ordinal
YML
  "$nils" release --out "$work/bids-shifted" --name gate-bids-shifted --layout bids \
    --dates shift --scheme "$work/ordinal.yml" --json > "$work/bids-shifted.json"
else
  echo "gate: dcm2niix is not installed; the BIDS bars are skipped" >&2
fi

echo "gate: a shifted release, for the date rules"
# 4.3's other half: a scheme that names a session by its date would put the
# date back in the path, and the release refuses it. So a shifted release runs
# under an ordinal one, which is the combination that works.
cat > "$work/ordinal.yml" <<'YML'
session:
  window_days: 0
  naming: ordinal
YML
"$nils" release --out "$work/shifted" --name gate-shifted --layout descriptive \
  --dates shift --scheme "$work/ordinal.yml" --json > "$work/shifted.json"

echo "gate: 4.3 is refused rather than warned about"
if "$nils" release --out "$work/refused" --name gate-refused --layout descriptive \
     --dates shift --json > /dev/null 2> "$work/refused.txt"; then
  echo "gate: a shifted release with a date-named scheme was accepted (4.3)" >&2
  exit 1
fi
if "$nils" release --out "$work/refused" --name gate-refused --layout descriptive \
     --dates shift --uids preserve --scheme "$work/ordinal.yml" \
     --json > /dev/null 2>> "$work/refused.txt"; then
  echo "gate: a shifted release with preserved UIDs was accepted (4.3)" >&2
  exit 1
fi
if [[ -e "$work/refused" ]]; then
  echo "gate: a refused release wrote a tree" >&2
  exit 1
fi

echo "gate: a second process through the door (Wave 4a section 12, bar 9)"
# The same registry, driven over HTTP as a notebook or a service would: the
# door's answers are compared with the command line's, and a release queued
# through the door and run by a worker writes the tree the command line wrote.
"$nils" status --json > "$work/status.json"
"$nils" custody --json > "$work/custody.json"
"$nils" select --json > "$work/select.json"
"$nils" serve --bind 127.0.0.1:0 --requests 6 > "$work/serve.out" 2> "$work/serve.err" &
serve_pid=$!
for _ in $(seq 1 50); do
  if [[ -s "$work/serve.out" ]]; then break; fi
  sleep 0.2
done
port="$(awk 'NR == 1 {print $3}' "$work/serve.out" | cut -d: -f2)"
url="http://127.0.0.1:$port/api"
curl -s "$url/capabilities" > "$work/door-capabilities.json"
curl -s "$url/status" > "$work/door-status.json"
curl -s "$url/custody" > "$work/door-custody.json"
curl -s -X POST -H 'Content-Type: application/json' -d '{}' "$url/select" > "$work/door-select.json"
curl -s "$url/review?status=open" > "$work/door-review.json"
curl -s -X POST -H 'Content-Type: application/json' \
  -d "{\"name\": \"gate-door\", \"out\": \"$work/door-desc\", \"layout\": \"descriptive\"}" \
  "$url/releases" > "$work/door-release-queued.json"
wait "$serve_pid" || true
"$nils" jobs work --once > "$work/door-work.out"
"$nils" jobs list --all --json > "$work/jobs.json"

archiver="$(command -v 7z || true)"
if [[ -n "$archiver" ]]; then
  echo "gate: the handover"
  "$nils" handover run --release gate-descriptive --out "$work/ship" --key gate \
    --chunk 1MB --json > "$work/handover.json"
else
  echo "gate: 7z is not installed; the handover bar is skipped" >&2
fi

echo "gate: the bars"
python3 "$here/check.py" "$work"
