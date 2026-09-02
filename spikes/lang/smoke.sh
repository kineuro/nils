#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# The smoke test: both parsers over the synthetic corpus, checked against the counts
# each library is known to produce (the difference on the no-preamble file is a
# finding of the spike, recorded in the README, not a bug in the harness).
set -euo pipefail
cd "$(dirname "$0")"
work=$(mktemp -d)
python3 ../../tools/synth/synthetic.py "$work/corpus" >/dev/null
rust/target/release/parse --root "$work/corpus" --out "$work/rust" --workers 2 --label smoke >/dev/null
go/parse --root "$work/corpus" --out "$work/go" --workers 2 --label smoke >/dev/null

check() {
  local side=$1 expect=$2
  local got
  got=$(python3 -c "import json,sys; c=json.load(open(sys.argv[1]))['classes']; print(' '.join(f'{k}={v}' for k,v in sorted(c.items())))" "$work/$side/summary.json")
  if [ "$got" != "$expect" ]; then
    echo "$side: got '$got', want '$expect'"; return 1
  fi
  echo "$side: $got"
}
check rust "missing_sop=1 not_dicom=3 ok=3 ok_raw=1 truncated=1"
check go "missing_sop=2 not_dicom=3 ok=2 ok_raw=1 truncated=1"

# The rows both sides produced carry the same values.
python3 - "$work" <<'EOF'
import csv, sys
from pathlib import Path
w = Path(sys.argv[1])
def rows(side):
    paths = {r["seq"]: r["path"] for r in csv.DictReader((w / side / "paths.tsv").open(), delimiter="\t", quoting=csv.QUOTE_NONE)}
    out = {}
    for r in csv.DictReader((w / side / "index.tsv").open(), delimiter="\t", quoting=csv.QUOTE_NONE):
        p = paths[r.pop("seq")]; r.pop("size"); out[p] = r
    return out
a, b = rows("rust"), rows("go")
common = sorted(set(a) & set(b))
assert len(common) == 3, common
for p in common:
    assert a[p] == b[p], (a[p], b[p])
print(f"{len(common)} rows identical on both sides")
EOF
rm -rf "$work"
