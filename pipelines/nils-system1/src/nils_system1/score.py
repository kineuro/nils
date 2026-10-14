# SPDX-License-Identifier: AGPL-3.0-only
"""``nils-system1 score``: System 1's opinion on every stack of the run.

Reads ``/input/stacks.json`` with ``x-nils.input.header`` and
``x-nils.input.rows``: per stack its header facts, and the registry's rows
a header model reads (each stack's whole fingerprint row and votes, each
series's frame of reference as a digest and private elements, for the run's
stacks and the other MR stacks of their sessions, never scored here).

Each scored stack becomes a record the predictor reads::

    {"stack_id", "fingerprint", "classification", "votes", "series_id",
     "series": {"frame_of_reference", "private"}}

and the predictor answers, per record, ``{"axes": {axis: {"value", "p",
"probabilities"?, "rules_value"?}}, "reliability"}``; ``rules_value`` is the
rules' answer as the model reads it, in its own vocabulary, which the engine
compares its answer with. The fields that mattered are measured
by leaving each of a stated list out of every scored stack at once
(``--explain``) and predicting again: a field's effect on an axis is the
fall in the probability of the answer given; the largest come first.

Written: ``system1/<stack>.json`` per stack (the answers, the reliability
and the fields), and ``results.json`` with one unit per stack and the
``opinions`` (``contracts/job/v1/opinions.schema.json``) by the model the
run's manifest names. A stack of a sample sealed now is never answered.
"""

from __future__ import annotations

import copy
import hashlib
import json
import os
import time
from pathlib import Path

from . import __version__
from .bundle import BundleError, load_predictor, open_bundle

PIPELINE = "system1-score"

# The fields left out one at a time to measure what mattered: the text a
# person typed at the console and the physics a sequence is known by.
EXPLAIN = [
    "text_series_description",
    "text_protocol_name",
    "text_sequence_name",
    "image_type",
    "scanning_sequence",
    "sequence_variant",
    "echo_time",
    "repetition_time",
    "inversion_time",
    "flip_angle",
    "manufacturer",
    "mr_acquisition_type",
]


class InputError(ValueError):
    pass


def read_manifest(path: Path) -> tuple[list[dict], dict]:
    doc = json.loads(path.read_text())
    if doc.get("contract") != "job/v1":
        raise InputError("stacks.json is not of the job/v1 contract")
    rows = doc.get("rows")
    if not isinstance(rows, dict):
        raise InputError("stacks.json carries no rows: the descriptor sets x-nils.input.rows")
    return list(doc.get("stacks") or []), rows


def records_of(stacks: list[dict], rows: dict) -> tuple[list[dict], list[int]]:
    """The predictor's records of the stacks to score, and the stacks left
    out because they are of a sample sealed now (their geometry or rows say
    so) or have no fingerprint row."""
    out, left = [], []
    by_stack = rows.get("stacks") or {}
    series = rows.get("series") or {}
    for s in stacks:
        sid = int(s["stack_id"])
        r = by_stack.get(str(sid))
        sealed = (s.get("geometry") or {}).get("sealed") is True
        if r is None or sealed or not r.get("fingerprint"):
            left.append(sid)
            continue
        fp = r["fingerprint"]
        se = fp.get("series_id")
        out.append({
            "stack_id": sid,
            "unit": s.get("unit") or f"stack-{sid}",
            "fingerprint": fp,
            "classification": (s.get("header") or {}).get("classification") or {},
            "votes": r.get("votes") or [],
            "series_id": se,
            "series": series.get(str(se)) or {},
        })
    return out, left


def _p_of(said: dict, value) -> float:
    """The probability a prediction gives a value: its own p where it
    answers the same value, else the value's class probability."""
    if said.get("value") == value:
        return float(said.get("p", 0.0))
    probs = said.get("probabilities") or {}
    key = value if isinstance(value, str) else json.dumps(value)
    return float(probs.get(key, 0.0)) if key is not None else 0.0


def explain(predictor, records: list[dict], rows: dict, base: list[dict], fields: list[str]) -> list[dict]:
    """Per record and axis, each field's effect: the fall in the answer's
    probability when the field is left out of every scored stack."""
    effects: list[dict] = [{} for _ in records]
    for field in fields:
        if not any(r["fingerprint"].get(field) not in (None, "") for r in records):
            continue
        recs = copy.deepcopy(records)
        rws = copy.deepcopy(rows)
        for r in recs:
            r["fingerprint"][field] = None
            row = (rws.get("stacks") or {}).get(str(r["stack_id"]))
            if row is not None:
                row["fingerprint"][field] = None
        again = predictor.predict(recs, rws)
        for i, (b, a) in enumerate(zip(base, again)):
            if records[i]["fingerprint"].get(field) in (None, ""):
                continue
            for axis, said in (b.get("axes") or {}).items():
                now = (a.get("axes") or {}).get(axis) or {}
                effect = float(said.get("p", 0.0)) - _p_of(now, said.get("value"))
                if abs(effect) >= 1e-6:
                    effects[i].setdefault(axis, []).append({"field": field, "effect": round(effect, 6)})
    for e in effects:
        for axis in e:
            e[axis].sort(key=lambda x: -abs(x["effect"]))
            e[axis] = e[axis][:10]
    return effects


def model_of(inputs: Path) -> dict:
    """The model the run was given, from its inputs' manifest."""
    man = json.loads((inputs / "manifest.json").read_text())
    models = [m for m in man.get("models") or [] if m.get("input") == "model"]
    if len(models) != 1:
        raise InputError("the run's manifest names no one model for the input model")
    return models[0]


def run(stacks_json: Path, inputs: Path, output: Path, trust_code: bool, explain_fields: list[str] | None,
        bundle_path: Path | None = None) -> dict:
    t0 = time.time()
    output.mkdir(parents=True, exist_ok=True)
    stacks, rows = read_manifest(stacks_json)
    model = model_of(inputs)
    archive = bundle_path or Path(model.get("artifact") or "")
    if not archive or not Path(archive).is_file():
        raise InputError("the model's artifact, its bundle, is not mounted under the run's inputs")
    scratch = Path(os.environ.get("TMPDIR") or "/tmp") / f"nils-system1-{os.getpid()}"
    manifest = open_bundle(Path(archive), scratch)
    predictor = load_predictor(scratch, manifest, trust_code)
    records, left = records_of(stacks, rows)
    base = predictor.predict(records, rows) if records else []
    if len(base) != len(records):
        raise BundleError("the predictor answered another number of stacks than it was given")
    fields = explain(predictor, records, rows, base, explain_fields if explain_fields is not None else EXPLAIN) \
        if records else []
    units, opinions = [], []
    for i, r in enumerate(records):
        said = base[i]
        rel = said.get("reliability")
        per_axis = {}
        for axis in manifest["axes"]:
            a = (said.get("axes") or {}).get(axis)
            if a is None:
                continue
            op = {
                "stack_id": r["stack_id"], "axis": axis, "value": a.get("value"),
                "p": float(a.get("p", 0.0)), "reliability": None if rel is None else float(rel),
                "fields": fields[i].get(axis, []) if fields else [], "model_id": int(model["model_id"]),
            }
            # the rules' answer as the model read it, in its own words: what
            # the engine compares the model's answer with
            if "rules_value" in a:
                op["rules_read"] = {"value": a["rules_value"]}
            if a.get("probabilities"):
                op["probabilities"] = {str(k): float(v) for k, v in a["probabilities"].items()}
            opinions.append(op)
            per_axis[axis] = {k: v for k, v in op.items() if k not in ("stack_id", "axis", "model_id")}
        doc = {"stack": r["stack_id"], "model_id": model["model_id"], "reliability": rel, "axes": per_axis}
        data = (json.dumps(doc, sort_keys=True) + "\n").encode()
        relpath = f"system1/{r['stack_id']}.json"
        p = output / relpath
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(data)
        units.append({
            "unit_id": r["unit"], "work_unit": "stack", "status": "succeeded",
            "derivatives": [relpath],
            "outputs": [{"kind": "output", "path": relpath, "media_type": "application/json",
                         "bytes": len(data), "sha256": "sha256:" + hashlib.sha256(data).hexdigest()}],
            "metrics": {}, "error": None,
        })
    for sid in left:
        units.append({"unit_id": f"stack-{sid}", "work_unit": "stack", "status": "skipped", "derivatives": [],
                      "outputs": [], "metrics": {},
                      "error": "sealed now, or no fingerprint row: System 1 gives no opinion on it"})
    results = {
        "schema_version": "1",
        "pipeline": PIPELINE,
        "tool_version": __version__,
        "units": units,
        "opinions": opinions,
        "models": [],
        "metrics": {"stacks": len(records), "left_out": len(left), "seconds": round(time.time() - t0, 3),
                    "bundle": {"name": manifest.get("name"), "version": manifest.get("version")}},
    }
    (output / "results.json").write_text(json.dumps(results) + "\n")
    return results
