# SPDX-License-Identifier: AGPL-3.0-only
"""Diff two runs of the same axis, and say what disagrees.

    compare.py v1.tsv v0.tsv [--samples N]

Prints counts and value names. Never a description, a path or an identifier.
"""

from __future__ import annotations

import sys
from collections import Counter


def load(path: str) -> dict[str, tuple[str, str, str]]:
    out = {}
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            p = line.rstrip("\n").split("\t")
            if len(p) < 3:
                continue
            out[(p[0], p[1])] = (p[2], p[3] if len(p) > 3 else "", p[4] if len(p) > 4 else "")
    return out


def main() -> int:
    a, b = load(sys.argv[1]), load(sys.argv[2])
    both = set(a) & set(b)
    print(f"{len(a)} rows in v1, {len(b)} in v0, {len(both)} shared")
    if set(a) - set(b) or set(b) - set(a):
        print(f"  only in v1: {len(set(a) - set(b))}   only in v0: {len(set(b) - set(a))}")

    value_diff = Counter()
    tier_diff = Counter()
    agree = 0
    for k in both:
        av, at, _ = a[k]
        bv, bt, _ = b[k]
        if av == bv:
            agree += 1
            if at != bt:
                tier_diff[(at, bt)] += 1
        else:
            value_diff[(av, bv)] += 1
    print(f"values agree {agree}, differ {len(both) - agree}")
    for (x, y), n in value_diff.most_common(30):
        print(f"  v1={x!r:22s} v0={y!r:22s} {n}")
    if tier_diff:
        print("same value, different tier:")
        for (x, y), n in tier_diff.most_common(10):
            print(f"  v1={x!r:14s} v0={y!r:14s} {n}")
    return 1 if len(both) - agree else 0


if __name__ == "__main__":
    raise SystemExit(main())
