#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The partition test of §14 item 5: do v1's stacks split each series the way
v0's signature does? Reads v1's SQLite registry and v0's partition lines,
restricts both to the instances they both hold, and compares series by series.
Prints counts only; never a UID, a value or a path.

    compare.py REGISTRY_DB V0.jsonl

Two partitions of a series are equal when every v0 group is exactly one v1
stack and the other way round. A series that differs is classified by what
happened: `split:<field>` when v1 put one v0 group into several stacks and the
stacks differ on that field once v0's rounding is applied to v1's stored
values, `join:<field>` when v1 put several v0 groups into one stack and the
groups' signatures differ at that field. A series may carry several labels.
"""
import json
import sqlite3
import sys
from collections import Counter, defaultdict

# v0's signature after the series UID, in order (stack_utils.py)
V0_FIELDS = [
    ("echo_time", 2),
    ("inversion_time", 1),
    ("echo_numbers", None),
    ("echo_train_length", None),
    ("repetition_time", 1),
    ("flip_angle", 1),
    ("receive_coil_name", None),
    ("xray_exposure", None),
    ("kvp", 0),
    ("tube_current", 0),
    ("pet_bed_index", None),
    ("pet_frame_type", None),
    ("orientation", None),
    ("image_type", None),
]
V1_COLUMNS = [name for name, _ in V0_FIELDS]


def rounded(value, decimals):
    if value is None or decimals is None:
        return value
    try:
        return round(float(value), decimals)
    except (TypeError, ValueError):
        return None


def load_v1(db):
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    stacks = {}
    cols = ", ".join(V1_COLUMNS)
    for row in con.execute(f"SELECT id, stack_key, {cols} FROM stack"):
        values = tuple(rounded(v, d) for v, (_, d) in zip(row[2:], V0_FIELDS))
        stacks[row[0]] = (row[1], values)
    instances = {}
    for sop, series, stack_id in con.execute(
        "SELECT i.sop_instance_uid, s.series_instance_uid, i.stack_id "
        "FROM instance i JOIN series s ON s.id = i.series_id"
    ):
        instances[sop] = (series, stack_id)
    con.close()
    return stacks, instances


def load_v0(path):
    instances = {}
    kept = 0
    with open(path) as f:
        for line in f:
            d = json.loads(line)
            instances[d["sop"]] = (d["series"], tuple(d["sig"]), d["kept"])
            kept += d["kept"]
    return instances, kept


def main() -> int:
    db, v0_path = sys.argv[1], sys.argv[2]
    v1_stacks, v1 = load_v1(db)
    v0, v0_kept = load_v0(v0_path)

    common = v1.keys() & v0.keys()
    out = Counter()
    out["v1_instances"] = len(v1)
    out["v0_readable_instances"] = len(v0)
    out["v0_kept_instances"] = v0_kept
    out["common_instances"] = len(common)
    out["only_v0_kept"] = sum(1 for s in v0.keys() - v1.keys() if v0[s][2])
    out["only_v0_not_kept"] = sum(1 for s in v0.keys() - v1.keys() if not v0[s][2])
    out["only_v1"] = len(v1.keys() - v0.keys())
    out["series_uid_disagreements"] = sum(1 for s in common if v1[s][0] != v0[s][0])

    # per series, over the common instances
    by_series = defaultdict(list)
    for sop in common:
        by_series[v1[sop][0]].append(sop)
    out["series_compared"] = len(by_series)
    labels = Counter()
    patterns = Counter()
    for series, sops in by_series.items():
        v0_group = {}
        pairs = set()
        for sop in sops:
            g = v0[sop][1]
            k = v1[sop][1]
            v0_group[g] = g
            pairs.add((g, k))
        groups = {g for g, _ in pairs}
        keys = {k for _, k in pairs}
        out["v0_groups"] += len(groups)
        out["v1_stacks"] += len(keys)
        if len(pairs) == len(groups) == len(keys):
            out["series_equal"] += 1
            if len(keys) > 1:
                out["series_equal_multi_stack"] += 1
            continue
        out["series_differing"] += 1
        found = set()
        # one v0 group, several v1 stacks
        by_group = defaultdict(set)
        by_key = defaultdict(set)
        for g, k in pairs:
            by_group[g].add(k)
            by_key[k].add(g)
        for g, ks in by_group.items():
            if len(ks) < 2:
                continue
            rows = [v1_stacks[k][1] for k in ks if k in v1_stacks]
            split_on = [
                name
                for i, (name, _) in enumerate(V0_FIELDS)
                if len({r[i] for r in rows}) > 1
            ]
            found.update(f"split:{n}" for n in (split_on or ["?"]))
        for k, gs in by_key.items():
            if len(gs) < 2:
                continue
            gs = list(gs)
            join_on = [
                name
                for i, (name, _) in enumerate(V0_FIELDS)
                if len({g[i] for g in gs}) > 1
            ]
            found.update(f"join:{n}" for n in (join_on or ["?"]))
        for label in found:
            labels[label] += 1
        patterns[",".join(sorted(found))] += 1

    print(json.dumps(out, indent=1, sort_keys=True))
    print(json.dumps({"labels": labels, "patterns": patterns}, indent=1, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
