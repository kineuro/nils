# SPDX-License-Identifier: AGPL-3.0-only
"""Diff two runs of the same mode, and say what disagrees rather than how much.

    compare.py flags  v1.tsv v0.tsv
    compare.py branch v1.tsv v0.tsv
    compare.py vote   v1.tsv v0.tsv

Prints counts and, for a flag, its name: never a description, a path or an
identifier, because the corpus is private and this output is not.
"""

from __future__ import annotations

import sys
from collections import Counter


def load(path: str) -> dict[str, list[str]]:
    out = {}
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            parts = line.rstrip("\n").split("\t")
            out[parts[0]] = parts[1:]
    return out


def main() -> int:
    mode, a_path, b_path = sys.argv[1], sys.argv[2], sys.argv[3]
    a, b = load(a_path), load(b_path)
    only_a, only_b = set(a) - set(b), set(b) - set(a)
    both = set(a) & set(b)
    print(f"{len(a)} rows in v1, {len(b)} in v0, {len(both)} shared")
    if only_a or only_b:
        print(f"  only in v1: {len(only_a)}   only in v0: {len(only_b)}")

    if mode == "flags":
        # v1 also computes the pack's own helper flags; v0 has no such names,
        # so they are left out of the comparison rather than counted as extra.
        disagree = Counter()
        rows_bad = 0
        for k in both:
            sa = {f for f in (a[k][0].split(",") if a[k] and a[k][0] else []) if f not in HELPERS}
            sb = set(b[k][0].split(",")) if b[k] and b[k][0] else set()
            d = sa ^ sb
            if d:
                rows_bad += 1
                for f in d:
                    disagree[f] += 1
        print(f"stacks whose flag set differs: {rows_bad}")
        for f, n in disagree.most_common():
            print(f"  {f}: {n}")
        return 1 if rows_bad else 0

    fields = {
        "branch": ["base", "construct", "technique", "confidence", "source", "cite"],
        "vote": ["method", "base", "technique", "matches", "total_in_bin", "pool"],
    }[mode]
    # v1's branch row carries the rule id first; v0 has no such column.
    offset = 1 if mode == "branch" else 0
    bad = Counter()
    rows_bad = 0
    examples: dict[str, Counter] = {f: Counter() for f in fields}
    for k in both:
        ra, rb = a[k][offset:], b[k]
        differing = [f for i, f in enumerate(fields) if ra[i : i + 1] != rb[i : i + 1]]
        if differing:
            rows_bad += 1
            for f in differing:
                bad[f] += 1
                i = fields.index(f)
                examples[f][(ra[i] if i < len(ra) else "", rb[i] if i < len(rb) else "")] += 1
    print(f"stacks that differ: {rows_bad}")
    for f, n in bad.most_common():
        print(f"  {f}: {n}")
        for (x, y), c in examples[f].most_common(8):
            print(f"      v1={x!r} v0={y!r}  {c}")
    return 1 if rows_bad else 0


HELPERS = {
    "has_positive_b_value",
    "has_mp2rage_context",
    "has_mpr_in_text",
    "has_inv1_in_text",
    "has_inv2_in_text",
    "is_mp2rage_inv1_by_ti",
    "is_mp2rage_inv2_by_ti",
}

if __name__ == "__main__":
    raise SystemExit(main())
