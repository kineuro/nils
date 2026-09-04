# SPDX-License-Identifier: AGPL-3.0-only
"""v0's own sort, run again over the cohort from current v0 code.

    resort.py --v0 SRC --csv chain.csv --reference rules --out cache.csv

The gate compares v1 against `series_classification_cache`, which is a table v0
wrote when it last sorted each cohort. That table is older than v0's code, so
some of what the gate reports is v0 disagreeing with itself rather than v1
disagreeing with v0. This runs v0's classification over the same stacks with
the code as it stands today and writes the same columns, so the comparison is
against v0 as it is rather than against what it once said.

Two of v0's steps decide an axis, and both run here:

  step 3, `ClassificationPipeline.classify`, once per stack; and
  step 4 phase 3, `sort/gap_filling.py`, which fills a base and a technique
  the rules left empty from the physics of the stacks around it, followed by
  phase 4, which synthesizes the intent of a `misc` stack again once the fill
  has given it a base and a technique.

`--reference` says which stacks the fill is allowed to learn from, which is the
one thing v0 leaves to the history of the database rather than to its code:

  none      no fill at all, so the file holds what the rules alone decided
  rules     only stacks the rules decided, which is what a database sorted in
            one go would hold
  filled    `rules`, then the fill's own answers added to the reference and the
            fill run again until nothing changes
  stored    the reference read from an existing cache file, which is what a
            database sorted cohort by cohort holds by the time the last one
            arrives

v0 is private and is never copied into this repository: `--v0` is the
`backend/src` of a checkout on the host.
"""

from __future__ import annotations

import argparse
import csv
import importlib.util
import os
import sys
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

csv.field_size_limit(1 << 30)

#: the columns of `series_classification_cache`, in the order v0 declares them
COLUMNS = (
    "series_stack_id",
    "directory_type",
    "base",
    "technique",
    "modifier_csv",
    "construct_csv",
    "provenance",
    "acceleration_csv",
    "body_part",
    "post_contrast",
    "localizer",
    "spinal_cord",
    "manual_review_required",
    "manual_review_reasons_csv",
    "dicom_origin_cohort",
)

NUMERIC = {"mr_tr", "mr_te", "mr_ti", "mr_flip_angle", "fov_x", "fov_y", "aspect_ratio"}
INTEGER = {"mr_echo_train_length", "stack_n_instances"}

#: v0's step 4 leaves these alone: what they are is decided by their route, not
#: by the physics of their neighbours (`step4_completion.py`, phase 3)
NO_FILL = ("SyMRI", "SWIRecon", "EPIMix", "BOLDRecon", "STAGE")

#: fields the fill reads, so a worker returns only these
PHYSICS = ("mr_tr", "mr_te", "mr_ti", "mr_flip_angle", "stack_n_instances", "scanning_sequence")

#: {cohort: {(axis, bucket_path): {"added": [...], "removed": [...]}}}, read from
#: v0's `cohort_classification_overrides`. A cohort can add or remove keywords
#: for one bucket of one detector, and that table lives in v0's application
#: database rather than in its code, so a stack's classification cannot be
#: reproduced from the code and the stack alone.
_OVERRIDES: dict[str, dict] = {}
_PIPE: dict[str | None, object] = {}


def rows(path: str):
    """The fingerprint CSV, typed the way v0's context expects it."""
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


def module(src: str, relative: str, name: str):
    """One of v0's modules, loaded from the host checkout by path."""
    spec = importlib.util.spec_from_file_location(name, Path(src) / relative)
    mod = importlib.util.module_from_spec(spec)
    # `dataclasses` resolves a field's type through sys.modules, so the module
    # has to be registered before it is executed.
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


def _start(src: str, overrides: dict) -> None:
    """One pipeline per cohort per worker process, built on first use."""
    global _OVERRIDES, _PIPE
    sys.path.insert(0, src)
    _OVERRIDES = overrides
    _PIPE = {}


def _pipeline(cohort: str | None):
    from classification.overrides import merge_overrides
    from classification.pipeline import ClassificationPipeline

    if cohort not in _PIPE:
        delta = _OVERRIDES.get(cohort)
        _PIPE[cohort] = (
            ClassificationPipeline(merged_configs=merge_overrides(None, delta))
            if delta
            else ClassificationPipeline()
        )
    return _PIPE[cohort]


def _classify(batch: list[dict]) -> list[dict]:
    from classification.core.context import ClassificationContext

    out = []
    for fp in batch:
        pipe = _pipeline(fp.get("cohort"))
        r = pipe.classify(ClassificationContext.from_fingerprint(fp))
        d = r.to_dict()
        d["series_stack_id"] = fp["series_stack_id"]
        d["dicom_origin_cohort"] = fp.get("cohort")
        d["modality"] = fp.get("modality")
        for k in PHYSICS:
            d[k] = fp.get(k)
        out.append(d)
    return out


def batched(seq, size: int):
    for i in range(0, len(seq), size):
        yield seq[i : i + size]


def load_overrides(path: str | None) -> dict[str, dict]:
    """v0's `cohort_classification_overrides`, exported as a JSON array of
    `{cohort, axis, bucket_path, added, removed}`, in the shape
    `classification.overrides.merge_overrides` takes."""
    if not path:
        return {}
    import json

    out: dict[str, dict] = {}
    for r in json.load(open(path, encoding="utf-8")):
        out.setdefault(r["cohort"], {})[(r["axis"], r["bucket_path"])] = {
            "added": r.get("added") or [],
            "removed": r.get("removed") or [],
        }
    for cohort, delta in sorted(out.items()):
        print(f"overrides: {cohort} has {len(delta)} bucket(s)", file=sys.stderr)
    return out


def load_cohorts(path: str | None) -> dict[str, str]:
    """Which cohort each stack came from, which decides whose keywords apply."""
    if not path:
        return {}
    with open(path, newline="", encoding="utf-8") as fh:
        return {
            str(r["series_stack_id"]): r["dicom_origin_cohort"]
            for r in csv.DictReader(fh)
            if r.get("dicom_origin_cohort")
        }


def step3(src: str, csv_path: str, workers: int, batch: int, overrides: dict, cohorts: dict) -> list[dict]:
    """v0's per-stack classification over every stack in the CSV."""
    stacks = list(rows(csv_path))
    for fp in stacks:
        fp["cohort"] = cohorts.get(str(fp["series_stack_id"]))
    print(f"step 3: {len(stacks)} stacks on {workers} workers", file=sys.stderr)
    done: list[dict] = []
    with ProcessPoolExecutor(workers, initializer=_start, initargs=(src, overrides)) as pool:
        for part in pool.map(_classify, batched(stacks, batch)):
            done.extend(part)
            if len(done) % (batch * workers) == 0:
                print(f"  {len(done)}", file=sys.stderr)
    print(f"step 3: {len(done)} classified", file=sys.stderr)
    return done


def reference_of(verdicts: list[dict]) -> list[dict]:
    """The rows v0's step 4 selects as its reference, in its own SQL's terms:
    an MR stack with a base, a technique and an intent that is not `excluded`."""
    ref = []
    for s in verdicts:
        if s.get("modality") != "MR":
            continue
        base, tech, dt = s.get("base"), s.get("technique"), s.get("directory_type")
        if not base or base == "Unknown" or not tech or tech == "Unknown":
            continue
        if not dt or dt == "excluded":
            continue
        ref.append(
            {
                "series_stack_id": s["series_stack_id"],
                "base": base,
                "technique": tech,
                "directory_type": dt,
                "mr_tr": s.get("mr_tr"),
                "mr_te": s.get("mr_te"),
                "mr_ti": s.get("mr_ti"),
                "mr_flip_angle": s.get("mr_flip_angle"),
                "stack_n_instances": s.get("stack_n_instances"),
            }
        )
    return ref


def needs_fill(s: dict) -> bool:
    """v0's own condition, from `step4_completion.py` phase 3."""
    if s.get("modality") != "MR" or s.get("directory_type") == "excluded":
        return False
    if s.get("provenance") in NO_FILL:
        return False
    base, tech = s.get("base"), s.get("technique")
    return not base or base == "Unknown" or not tech or tech == "Unknown"


def fill(gap, verdicts: list[dict], reference: list[dict]) -> int:
    """v0's step 4 phase 3 over one frozen reference. Returns how many stacks
    took an answer from it."""
    by_intent, global_db = gap.build_intent_scoped_databases(reference)
    print(
        f"  reference: {len(reference)} stacks, {len(by_intent)} pools, "
        f"{global_db.total_count} in the global pool",
        file=sys.stderr,
    )
    filled = 0
    for s in verdicts:
        if not needs_fill(s):
            continue
        dt = s.get("directory_type")
        db = by_intent.get(dt, global_db)
        ask = lambda d: gap.find_best_match(  # noqa: E731
            ref_db=d,
            tr=s.get("mr_tr"),
            te=s.get("mr_te"),
            ti=s.get("mr_ti"),
            fa=s.get("mr_flip_angle"),
            n_instances=s.get("stack_n_instances"),
            scanning_sequence=s.get("scanning_sequence"),
        )
        r = ask(db)
        if r.method in ("no_match", "insufficient_matches", "no_compatible_match") and dt != "misc":
            r = ask(global_db)
        # v0 marks every stack it *offered* to the fill, not only the ones that
        # took an answer, and phase 4 reads that mark.
        s["fill_attempted"] = True
        took = False
        # v0 fills an empty base but a technique that is empty *or* Unknown.
        if r.base and not s.get("base"):
            s["base"] = r.base
            s["filled_base"] = r.method
            took = True
        if r.technique and (not s.get("technique") or s.get("technique") == "Unknown"):
            s["technique"] = r.technique
            s["filled_technique"] = r.method
            took = True
        if took:
            filled += 1
    return filled


def resynthesize(gap, verdicts: list[dict]) -> int:
    """v0's step 4 phase 4: a stack the fill has just given a base and a
    technique may no longer be `misc`.

    v0 offers every stack the fill *touched* to this phase, whether or not it
    took an answer, and the synthesizer here is not the one the pipeline used
    but a shorter one, so a stack can leave `misc` on this phase alone."""
    changed = 0
    for s in verdicts:
        if s.get("directory_type") != "misc" or not s.get("fill_attempted"):
            continue
        dt = gap.synthesize_directory_type(
            base=s.get("base"),
            technique=s.get("technique"),
            construct_csv=s.get("construct_csv") or "",
            provenance=s.get("provenance"),
            localizer=int(s.get("localizer") or 0),
        )
        if dt and dt != "misc":
            s["directory_type"] = dt
            changed += 1
    return changed


def read_cache(path: str) -> list[dict]:
    with open(path, newline="", encoding="utf-8") as fh:
        return list(csv.DictReader(fh))


def write_cache(path: str, verdicts: list[dict]) -> None:
    with open(path, "w", newline="", encoding="utf-8") as fh:
        w = csv.DictWriter(fh, fieldnames=COLUMNS, extrasaction="ignore")
        w.writeheader()
        for s in sorted(verdicts, key=lambda r: int(r["series_stack_id"])):
            w.writerow(s)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--v0", required=True, help="the backend/src of a v0 checkout on this host")
    ap.add_argument("--csv", required=True, help="the fingerprint CSV, one row per stack")
    ap.add_argument("--out", required=True)
    ap.add_argument("--reference", default="rules", choices=("none", "rules", "filled", "stored"))
    ap.add_argument("--stored", help="for --reference stored: the cache to take the reference from")
    ap.add_argument("--verdicts", help="reuse a step 3 output instead of classifying again")
    ap.add_argument("--save-verdicts", help="write step 3's output before the fill")
    ap.add_argument("--workers", type=int, default=min(32, os.cpu_count() or 8))
    ap.add_argument("--batch", type=int, default=2000)
    ap.add_argument("--rounds", type=int, default=10, help="for --reference filled: the cap")
    ap.add_argument("--overrides", help="cohort_classification_overrides, exported as JSON")
    ap.add_argument("--cohorts", help="a CSV with series_stack_id and dicom_origin_cohort")
    a = ap.parse_args()

    if a.reference == "stored" and not a.stored:
        raise SystemExit("--reference stored needs --stored")

    if a.verdicts:
        verdicts = read_cache(a.verdicts)
        physics = {}
        for fp in rows(a.csv):
            physics[str(fp["series_stack_id"])] = fp
        for s in verdicts:
            fp = physics.get(str(s["series_stack_id"]), {})
            s["modality"] = fp.get("modality")
            for k in PHYSICS:
                s[k] = fp.get(k)
        print(f"step 3: {len(verdicts)} read from {a.verdicts}", file=sys.stderr)
    else:
        verdicts = step3(
            a.v0, a.csv, a.workers, a.batch, load_overrides(a.overrides), load_cohorts(a.cohorts)
        )
        if a.save_verdicts:
            write_cache(a.save_verdicts, verdicts)

    if a.reference == "none":
        write_cache(a.out, verdicts)
        return 0

    sys.path.insert(0, a.v0)
    gap = module(a.v0, "sort/gap_filling.py", "v0_gap_filling")

    if a.reference == "stored":
        stored = read_cache(a.stored)
        physics = {str(s["series_stack_id"]): s for s in verdicts}
        rows_ = []
        for s in stored:
            p = physics.get(str(s["series_stack_id"]))
            if not p:
                continue
            rows_.append({**s, **{k: p.get(k) for k in ("modality", *PHYSICS)}})
        n = fill(gap, verdicts, reference_of(rows_))
        print(f"fill: {n} stacks took an answer", file=sys.stderr)
    elif a.reference == "rules":
        n = fill(gap, verdicts, reference_of(verdicts))
        print(f"fill: {n} stacks took an answer", file=sys.stderr)
    else:
        total = 0
        for round_ in range(1, a.rounds + 1):
            n = fill(gap, verdicts, reference_of(verdicts))
            total += n
            print(f"fill round {round_}: {n} stacks took an answer", file=sys.stderr)
            if n == 0:
                break
        print(f"fill: {total} over {round_} rounds", file=sys.stderr)

    changed = resynthesize(gap, verdicts)
    print(f"resynthesis: {changed} misc stacks took an intent", file=sys.stderr)
    write_cache(a.out, verdicts)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
