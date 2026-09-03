# SPDX-License-Identifier: AGPL-3.0-only
"""The benchmark gate (Wave 1 spec, §12.6): read a digest report and hold it
against the baseline recorded for this runner class.

    gate.py REPORT.json BASELINE.json RUNNER [--record]

Fails when the rate is below the baseline's floor or the resident memory is
above the cap. `--record` writes the measured numbers back into the baseline
instead of judging them, which is how the first run on a runner class fills it
in; that write is a commit someone makes on purpose.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path


def main(argv: list[str]) -> int:
    if len(argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    report = json.loads(Path(argv[0]).read_text(encoding="utf-8"))
    baseline_path = Path(argv[1])
    baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
    runner = argv[2]
    record = "--record" in argv[3:]

    rate = float(report["files_per_s"])
    rss_bytes = report.get("peak_rss_bytes") or 0
    rss = rss_bytes / 2**30
    seen = report["seen"]
    print(f"{seen:,} files, {rate:,.0f} files/s, peak RSS {rss:.2f} GiB, on {runner}")

    entry = baseline["runners"].get(runner)
    if entry is None:
        print(f"no baseline for the runner class {runner}; run with --record to write one", file=sys.stderr)
        return 1 if not record else write(baseline_path, baseline, runner, rate, rss, report)
    if record or not entry.get("files_per_s"):
        return write(baseline_path, baseline, runner, rate, rss, report)

    floor = float(baseline["floor"]) * float(entry["files_per_s"])
    cap = float(entry["rss_gib"])
    ok = True
    if rate < floor:
        print(f"FAIL: {rate:,.0f} files/s is below the floor of {floor:,.0f} "
              f"({baseline['floor']:.0%} of {entry['files_per_s']:,.0f})", file=sys.stderr)
        ok = False
    else:
        print(f"pass: {rate:,.0f} files/s against a floor of {floor:,.0f}")
    if rss > cap:
        print(f"FAIL: peak RSS {rss:.2f} GiB is above the cap of {cap:.2f} GiB", file=sys.stderr)
        ok = False
    else:
        print(f"pass: peak RSS {rss:.2f} GiB against a cap of {cap:.2f} GiB")
    return 0 if ok else 1


def write(path: Path, baseline: dict, runner: str, rate: float, rss: float, report: dict) -> int:
    """Record what this run measured, rounded to what is worth comparing."""
    entry = baseline["runners"].setdefault(runner, {})
    entry["files_per_s"] = round(rate)
    entry.setdefault("rss_gib", 4.0)
    entry["measured"] = {
        "version": report.get("version") or report.get("nils_version"),
        "seen": report["seen"],
        "elapsed_s": round(float(report["elapsed_s"]), 1),
        "peak_rss_gib": round(rss, 2),
    }
    entry.pop("note", None)
    path.write_text(json.dumps(baseline, indent=2) + "\n", encoding="utf-8")
    print(f"recorded {round(rate):,} files/s for {runner} in {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
