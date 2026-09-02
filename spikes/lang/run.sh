#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# The measurement protocol of the language spike (docs/decisions/15, C1).
#
#   run.sh --root DIR --label NAME [--workers "8 16"] [--runs 2] [--out DIR] [--referee]
#
# For every worker count, the Rust harness runs, then the Go harness, RUNS times each,
# into OUT/<label>/<impl>-w<N>-run<k>/. The referee (pydicom) judges the last pair when
# asked. Everything printed is a count or a rate; the outputs stay on the host.
set -euo pipefail
cd "$(dirname "$0")"
root=""; label=""; workers="8"; runs=2; out="/scratch/nils/spike"; referee=0
while [ $# -gt 0 ]; do
  case "$1" in
    --root) root=$2; shift 2 ;;
    --label) label=$2; shift 2 ;;
    --workers) workers=$2; shift 2 ;;
    --runs) runs=$2; shift 2 ;;
    --out) out=$2; shift 2 ;;
    --referee) referee=1; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[ -n "$root" ] && [ -n "$label" ] || { echo "usage: run.sh --root DIR --label NAME [--workers \"8 16\"] [--runs 2] [--out DIR] [--referee]" >&2; exit 2; }
rust=rust/target/release/parse
go=go/parse
[ -x "$rust" ] || (cd rust && cargo build --release -p nils-spike-parse)
[ -x "$go" ] || (cd go && go build -o parse ./cmd/parse)
mkdir -p "$out/$label"

line() {  # one line of numbers from a summary.json
  python3 - "$1" <<'EOF'
import json, sys
s = json.load(open(sys.argv[1]))
c = " ".join(f"{k}={v}" for k, v in sorted(s["classes"].items()))
cpu = s["user_cpu_seconds"] + s["system_cpu_seconds"]
print(f"{s['implementation']:4s} w{s['workers']:<3d} files={s['files']} parsed={s['parsed']} failed={s['failed']} "
      f"wall={s['wall_seconds']:.1f}s files/s={s['files_per_second']:.0f} MB/s={s['megabytes_per_second']:.0f} "
      f"cpu={cpu:.0f}s rss={s['peak_rss_megabytes']:.0f}MB  {c}")
EOF
}

echo "host: $(hostname), $(nproc) cpus, $(free -g | awk '/Mem:/{print $2}') GB; corpus: $label; workers: $workers; runs: $runs"
for w in $workers; do
  for k in $(seq 1 "$runs"); do
    for impl in rust go; do
      d="$out/$label/$impl-w$w-run$k"
      bin=$rust; [ "$impl" = go ] && bin=$go
      rm -rf "$d"
      "$bin" --root "$root" --out "$d" --workers "$w" --label "$label" > /dev/null
      line "$d/summary.json"
    done
  done
done

# The results file: every summary, counts and rates only.
python3 - "$out/$label" <<'EOF'
import json, sys
from pathlib import Path
d = Path(sys.argv[1])
runs = []
for s in sorted(d.glob("*-w*-run*/summary.json")):
    j = json.load(s.open())
    runs.append({k: j[k] for k in ("implementation", "library", "workers", "host_cpus", "files", "bytes", "parsed", "failed",
                                  "classes", "transfer_syntaxes", "wall_seconds", "files_per_second", "megabytes_per_second",
                                  "user_cpu_seconds", "system_cpu_seconds", "peak_rss_megabytes")} | {"run": s.parent.name})
(d / "results.json").write_text(json.dumps({"label": d.name, "runs": runs}, indent=2) + "\n")
print(f"results: {d / 'results.json'} ({len(runs)} runs)")
EOF

if [ "$referee" = 1 ]; then
  last=$(echo $workers | awk '{print $NF}')
  echo "referee on w$last run$runs"
  python3 referee.py "$out/$label/rust-w$last-run$runs" "$out/$label/go-w$last-run$runs"
fi
