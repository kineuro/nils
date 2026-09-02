#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The referee: pydicom judges the two harnesses' failures, and their indexes are
compared field by field. Prints counts only; runs on the host next to the outputs.

    referee.py RUST_OUT GO_OUT [--sample N]

For every file that either harness could not parse, pydicom (force=True,
stop_before_pixels=True) is asked whether it can read a SOP Instance UID out of it.
A file pydicom reads but a harness rejected is that library's miss; a file nobody
reads is bad on disk (counted, never named). Then the rows both harnesses produced
are joined on the path and every kept tag is compared; disagreements are counted per
tag, with a normalisation for the two libraries' number formatting.
"""
import argparse
import csv
import json
import random
import re
import sys
from collections import Counter
from pathlib import Path

import pydicom

FAILURE_CLASSES = ("not_dicom", "parse_error", "truncated", "unsupported_ts", "missing_sop", "io_error")


def read_tsv(path: Path):
    with path.open(newline="") as f:
        return list(csv.DictReader(f, delimiter="\t", quoting=csv.QUOTE_NONE))


def load(out: Path):
    paths = {r["seq"]: r["path"] for r in read_tsv(out / "paths.tsv")}
    index = {paths[r["seq"]]: r for r in read_tsv(out / "index.tsv")}
    failures = {paths[r["seq"]]: r for r in read_tsv(out / "failures.tsv")}
    summary = json.loads((out / "summary.json").read_text())
    return index, failures, summary


def pydicom_reads(path: str) -> str:
    """'reads' when pydicom finds a SOP Instance UID, else the exception class."""
    try:
        ds = pydicom.dcmread(path, force=True, stop_before_pixels=True)
        return "reads" if ds.get("SOPInstanceUID") else "no_sop"
    except Exception as e:  # noqa: BLE001, the class name is the finding
        return type(e).__name__


NUM = re.compile(r"^[-+]?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?$")


def norm(v: str) -> str:
    """Numbers compare as numbers; multi-values element-wise; text stripped."""
    parts = [p.strip() for p in v.split("\\")]
    out = []
    for p in parts:
        if NUM.match(p):
            out.append(repr(float(p)))
        else:
            out.append(p)
    return "\\".join(out)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("rust", type=Path)
    ap.add_argument("go", type=Path)
    ap.add_argument("--sample", type=int, default=20000, help="rows compared field by field (0 = all)")
    ap.add_argument("--seed", type=int, default=1)
    args = ap.parse_args()

    r_index, r_fail, r_sum = load(args.rust)
    g_index, g_fail, g_sum = load(args.go)
    print(f"files: rust {r_sum['files']}, go {g_sum['files']}")
    print(f"parsed: rust {r_sum['parsed']}, go {g_sum['parsed']}")
    print(f"classes rust: {r_sum['classes']}")
    print(f"classes go:   {g_sum['classes']}")

    # 1. The referee on every failure of either side.
    union = set(r_fail) | set(g_fail)
    verdict = {p: pydicom_reads(p) for p in union}
    print(f"\nfailures, union of both sides: {len(union)}")
    for side, fail in (("rust", r_fail), ("go", g_fail)):
        c = Counter()
        for p, row in fail.items():
            c[(row["class"], verdict[p])] += 1
        print(f"\n{side}: failure class x pydicom verdict")
        for (cls, v), n in sorted(c.items()):
            print(f"  {cls:15s} {v:25s} {n}")
    only_r = {p for p in r_fail if p not in g_fail}
    only_g = {p for p in g_fail if p not in r_fail}
    both = set(r_fail) & set(g_fail)
    print(f"\nfailed in rust only: {len(only_r)}, of which pydicom reads {sum(verdict[p] == 'reads' for p in only_r)}")
    print(f"failed in go only:   {len(only_g)}, of which pydicom reads {sum(verdict[p] == 'reads' for p in only_g)}")
    print(f"failed in both:      {len(both)}, of which pydicom reads {sum(verdict[p] == 'reads' for p in both)}")
    misses = {
        "rust": sum(verdict[p] == "reads" for p in r_fail),
        "go": sum(verdict[p] == "reads" for p in g_fail),
        "unreadable_by_all": sum(verdict[p] != "reads" for p in both),
    }
    print(f"misses (pydicom reads, library did not): {misses}")

    # 2. Field-by-field agreement on the rows both produced.
    common = sorted(set(r_index) & set(g_index))
    rows = common
    if args.sample and len(common) > args.sample:
        random.Random(args.seed).shuffle(rows)
        rows = rows[: args.sample]
    tags = [k for k in next(iter(r_index.values())).keys() if k not in ("seq", "size", "class", "ts", "path")]
    disagree = Counter()
    ts_disagree = 0
    for p in rows:
        a, b = r_index[p], g_index[p]
        if a["ts"] != b["ts"]:
            ts_disagree += 1
        for t in tags:
            if norm(a.get(t, "")) != norm(b.get(t, "")):
                disagree[t] += 1
    print(f"\nrows parsed by both: {len(common)}; compared: {len(rows)}")
    print(f"transfer syntax disagreements: {ts_disagree}")
    if disagree:
        print("value disagreements per tag:")
        for t, n in disagree.most_common():
            print(f"  {t:26s} {n}")
    else:
        print("value disagreements per tag: none")

    result = {
        "files": {"rust": r_sum["files"], "go": g_sum["files"]},
        "parsed": {"rust": r_sum["parsed"], "go": g_sum["parsed"]},
        "failures_union": len(union),
        "pydicom_verdicts": dict(Counter(verdict.values())),
        "misses": misses,
        "failed_only_rust": len(only_r),
        "failed_only_go": len(only_g),
        "failed_both": len(both),
        "compared_rows": len(rows),
        "ts_disagreements": ts_disagree,
        "value_disagreements": dict(disagree),
    }
    (args.rust.parent / "referee.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"\nwritten: {args.rust.parent / 'referee.json'}")


if __name__ == "__main__":
    sys.exit(main())
