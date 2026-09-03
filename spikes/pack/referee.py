# SPDX-License-Identifier: AGPL-3.0-only
"""The referee: v0's own code, over the same stacks, writing the same rows.

    referee.py flags|branch|vote --v0 SRC --fp FILE.csv [--scc FILE.csv]

`SRC` is the `backend/src` of a v0 0.5.3 checkout. v0 is private and is never
copied into this repository: the spike imports it from wherever it is on the
host. Three of its modules import nothing but the standard library, which is
why the referee can run them without standing v0 up.
"""

from __future__ import annotations

import argparse
import csv
import importlib.util
import sys
from pathlib import Path

csv.field_size_limit(1 << 30)

NUMERIC = {
    "mr_tr", "mr_te", "mr_ti", "mr_flip_angle",
    "fov_x", "fov_y", "aspect_ratio",
}
INTEGER = {"mr_echo_train_length", "stack_n_instances"}


def rows(path: str):
    """The export as v0 would have read it from Postgres: numbers as numbers,
    an empty field as None."""
    with open(path, newline="", encoding="utf-8") as fh:
        for r in csv.DictReader(fh):
            for k, v in list(r.items()):
                if v == "":
                    r[k] = None
                elif k in NUMERIC:
                    try:
                        r[k] = float(v)
                    except ValueError:
                        r[k] = None
                elif k in INTEGER:
                    try:
                        r[k] = int(float(v))
                    except ValueError:
                        r[k] = None
            yield r


def load_gap_filling(src: Path):
    """`sort/__init__.py` pulls in pydantic, which the spike does not need;
    load the one module from its file instead."""
    spec = importlib.util.spec_from_file_location("v0_gap_filling", src / "sort" / "gap_filling.py")
    mod = importlib.util.module_from_spec(spec)
    # `dataclasses` resolves a field's type through sys.modules, so the module
    # has to be registered before it is executed.
    sys.modules[spec.name] = mod
    spec.loader.exec_module(mod)
    return mod


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["flags", "branch", "vote"])
    ap.add_argument("--v0", required=True)
    ap.add_argument("--fp", required=True)
    ap.add_argument("--scc")
    a = ap.parse_args()

    src = Path(a.v0)
    sys.path.insert(0, str(src))
    from classification.core.context import ClassificationContext  # noqa: E402

    out = sys.stdout

    if a.mode == "flags":
        for fp in rows(a.fp):
            ctx = ClassificationContext.from_fingerprint(fp)
            uf = ctx.unified_flags
            on = ",".join(sorted(k for k, v in uf.items() if v))
            out.write(f"{fp['series_stack_id']}\t{on}\n")
        return 0

    verdicts = {}
    if a.scc:
        with open(a.scc, newline="", encoding="utf-8") as fh:
            for r in csv.DictReader(fh):
                verdicts[r["series_stack_id"]] = r

    if a.mode == "branch":
        from classification.branches.swi import apply_swi_logic  # noqa: E402

        n = 0
        for fp in rows(a.fp):
            v = verdicts.get(str(fp["series_stack_id"]))
            if not v or v.get("provenance") != "SWIRecon":
                continue
            ctx = ClassificationContext.from_fingerprint(fp)
            r = apply_swi_logic(ctx)
            e = r.evidence[0]
            out.write(
                f"{fp['series_stack_id']}\t{r.base}\t{r.construct}\t{r.technique}"
                f"\t{r.confidence:.2f}\t{e.field}\t{e.value}\n"
            )
            n += 1
        print(f"{n} stacks entered the SWI branch", file=sys.stderr)
        return 0

    # --- vote -------------------------------------------------------------
    g = load_gap_filling(src)
    fps = list(rows(a.fp))

    ref_rows = []
    for fp in fps:
        v = verdicts.get(str(fp["series_stack_id"]))
        if not v or fp.get("modality") != "MR":
            continue
        base, tech, dt = v.get("base"), v.get("technique"), v.get("directory_type")
        if not base or base == "Unknown" or not tech or tech == "Unknown":
            continue
        if not dt or dt == "excluded":
            continue
        ref_rows.append(
            {
                "series_stack_id": fp["series_stack_id"],
                "base": base,
                "technique": tech,
                "directory_type": dt,
                "mr_tr": fp.get("mr_tr"),
                "mr_te": fp.get("mr_te"),
                "mr_ti": fp.get("mr_ti"),
                "mr_flip_angle": fp.get("mr_flip_angle"),
                "stack_n_instances": fp.get("stack_n_instances"),
            }
        )
    by_intent, global_db = g.build_intent_scoped_databases(ref_rows)
    print(
        f"reference: {len(ref_rows)} stacks, {len(by_intent)} pools, "
        f"{global_db.bin_count} bins in the global pool",
        file=sys.stderr,
    )

    for fp in fps:
        if fp.get("modality") != "MR":
            continue
        v = verdicts.get(str(fp["series_stack_id"]))
        dt = (v or {}).get("directory_type") or None
        db = by_intent.get(dt, global_db)
        which = "scoped" if dt in by_intent else "global"
        r = g.find_best_match(
            ref_db=db,
            tr=fp.get("mr_tr"),
            te=fp.get("mr_te"),
            ti=fp.get("mr_ti"),
            fa=fp.get("mr_flip_angle"),
            n_instances=fp.get("stack_n_instances"),
            scanning_sequence=fp.get("scanning_sequence"),
        )
        if r.method in ("no_match", "insufficient_matches", "no_compatible_match") and dt != "misc":
            r = g.find_best_match(
                ref_db=global_db,
                tr=fp.get("mr_tr"),
                te=fp.get("mr_te"),
                ti=fp.get("mr_ti"),
                fa=fp.get("mr_flip_angle"),
                n_instances=fp.get("stack_n_instances"),
                scanning_sequence=fp.get("scanning_sequence"),
            )
            which = "global"
        out.write(
            f"{fp['series_stack_id']}\t{r.method}\t{r.base or ''}\t{r.technique or ''}"
            f"\t{r.match_count}\t{r.total_in_bin}\t{which}\n"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
