# SPDX-License-Identifier: AGPL-3.0-only
"""``nils-postcontrast cascade``: the certified post-contrast model (model
r1) served through its three-slice student (record 55 E3, spec 12.2).
``nils-postcontrast full`` is model r1's full pipeline on the stacks the
cascade defers (``full.py``).

What a container meets (``contracts/job/v1``): ``/input/stacks.json``, the
source places read-only under ``/source/<n>``, the typed inputs read-only
under ``/inputs/<id>``, and ``/output``, the only place it writes. The run
ends with ``results.json``; exit 0 when the run completed whatever its
units' statuses, 1 when it could not be done.

Per stack, in order:

1. a stack sealed now, or not MR, is not read (``sealed``, ``not-mr``);
2. only T1-type brain is a candidate (the certificate's scope); every other
   stack is "not sure" with ``not-t1-brain``; a site the fusion has no
   calibration for is ``no-calibration`` (a run given a calibration file,
   ``--calibration`` or the site file the descriptor names through
   ``NILS_PC_CALIBRATION``, adds the file's site pairs beside the fusion's
   first; a site the frozen fusion has a pair for is refused);
3. stage 1: three native slices chosen from the manifest's geometry, the
   student's three seeds, each fused and their mean fused with the stack's
   header evidence; accepted where the three seeds give the mean's answer
   and its log-odds margin from the nearer threshold is at least m (4);
4. a stack stage 1 does not accept (or cannot read: no geometry, a
   multi-frame file, no readable slice) is ``deferred``: no proposal here,
   and the engine runs the full pipeline (``postcontrast-full``) over it.

Each stack in the run gets ``postcontrast/<stack>.json``; each answered one
a ``post_contrast`` proposal by the cascade's digest.

With ``--raw-scores`` (0.3.0) a T1-type brain stack at a site with no
calibration is still read and scored, and its table carries the fused
log-odds before any calibration (``score_raw``, with each seed's in
``score_raw_seeds``) beside the ``no-calibration`` reason, with its
``site``, ``make`` and ``cohort_candidates`` (a JSON list); its ``p_given``
and ``value`` stay empty, so it is never a proposal; such a stack the
student could not read keeps ``no-calibration`` with no score (``stage1``
says why), so the run's deferral list keeps its meaning. The site fits its
Platt pair on those scores with ``nils-postcontrast calibrate``
(``calibration.py``). Every table and every unit's metrics carry
``score_raw`` (the table the five columns); without the flag they are null
and the rest of the output is as before.
"""

from __future__ import annotations

import argparse
import functools
import json
import logging
import os
import sys
import time
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np

from . import AXIS, GIVEN, NOT_GIVEN, __version__
from . import calibration, evidence, manifest, parts, slices
from .fusion import Fusion, FusionError, stage1
from .results import FAILED, SKIPPED, Run, Unit

logger = logging.getLogger("nils_postcontrast")
K = 3
VALUE = {"given": GIVEN, "not given": NOT_GIVEN}


def _read_one(task):
    """A worker: one stack's student input, from its files."""
    sid, iop, files, mf, acq3d, thick = task
    G = slices.stack_geometry(iop, files, mf)
    if G is None:
        return sid, ("multi-frame" if mf else "no-geometry"), None, 0
    x, opened = slices.stack_input(G, K, acq3d, thick)
    if x is None:
        return sid, "no-slice", None, opened
    return sid, None, x, opened


def _device(asked: str) -> str:
    if asked == "cpu":
        return "cpu"
    try:
        import torch

        if torch.cuda.is_available():
            return "cuda"
    except Exception:  # noqa: BLE001
        pass
    if asked == "cuda":
        raise RuntimeError("a card was asked for and none is usable")
    return "cpu"


def _table(sid: int, cas, **kw) -> dict:
    # the raw-score mode's five columns are on every row of every run, null
    # without the mode, so the table's shape is constant
    doc = {"p_given": None, "value": None, "confidence": None, "answers": 0, "stage": None, "stage1": None, "reason": None, "margin": None,
           "seeds_agree": None, "files_read": 0, "score_raw": None, "score_raw_seeds": None, "site": None, "make": None, "cohort_candidates": None}
    doc.update(kw)
    doc.update(cascade_digest=cas.digest, student_digest=cas.student.digest, fusion_digest=cas.fusion.digest, stack_id=sid)
    return doc


def score(stacks, context, cas, F: Fusion, device: str = "cpu", workers: int = 8, batch: int = 64, threads: int | None = None,
          raw: bool = False) -> tuple:
    """(every stack's table by stack id, the evidence rows the fusion read
    them from, ``evidence.rows_for``): the run's logic without its files.
    With ``raw`` (the raw-score mode) a T1-type brain stack at a site with
    no calibration is read and scored too, for its log-odds alone."""
    rows = evidence.rows_for(stacks, context, F)
    out: dict = {}
    tasks = []
    uncal: set = set()

    def table(sid, **kw):
        return evidence.table_row(rows, sid, raw, functools.partial(_table, cas=cas), **kw)

    for e in stacks:
        r = rows[e.stack_id]
        mod = e.modality or (e.header.get("fingerprint") or {}).get("modality")
        if e.sealed:
            out[e.stack_id] = table(e.stack_id, reason="sealed")
        elif str(mod or "").upper() != "MR":
            out[e.stack_id] = table(e.stack_id, reason="not-mr")
        elif not (r["acquired"] and r["family"] == "T1-type" and r["brain"]):
            out[e.stack_id] = table(e.stack_id, reason="not-t1-brain")
        elif (r["site"] is None or r["site"] not in F.cal) and not raw:
            out[e.stack_id] = table(e.stack_id, reason="no-calibration")
        else:
            if r["site"] is None or r["site"] not in F.cal:
                uncal.add(e.stack_id)
            iop = (e.geometry or {}).get("image_orientation_patient")
            tasks.append((e.stack_id, iop, manifest.ordered_files(e), manifest.multi_frame(e), r["acq3d"], r["thick"] or 0.0))
    if not tasks:
        return out, rows
    from .student import Student

    stu = Student(cas.student.artifact, device=device, threads=threads)
    buf: list = []

    def flush():
        if not buf:
            return
        tiles = np.stack([b[1][0] for b in buf])
        valid = np.stack([b[1][1] for b in buf])
        sg = np.stack([b[1][2] for b in buf])
        tok = np.stack([b[1][3] for b in buf])
        o = stu.predict(tiles, valid, sg, tok)
        res = stage1(F, [rows[b[0]] for b in buf], o, cas.m)
        for (sid, _x, opened), s in zip(buf, res):
            if sid in uncal:
                # the raw-score mode: the log-odds alone, no probability, no
                # answer, so never a proposal
                out[sid] = table(sid, stage1="raw", reason="no-calibration", files_read=opened, score_raw=round(s.z, 6),
                                 score_raw_seeds=" ".join(f"{z:.6f}" for z in s.seeds_z))
            elif s.accepted:
                out[sid] = table(sid, p_given=round(s.p, 6), value=VALUE[s.answer], confidence=round(max(s.p, 1 - s.p), 6), answers=1, stage="fs3",
                                 stage1="answered", margin=round(s.margin, 4), seeds_agree=1, files_read=opened)
            else:
                out[sid] = table(sid, p_given=None if s.p != s.p else round(s.p, 6), stage1="deferred", reason="deferred",
                                 margin=round(s.margin, 4), seeds_agree=int(s.agree), files_read=opened)
        buf.clear()

    import multiprocessing

    with ProcessPoolExecutor(max(1, workers), mp_context=multiprocessing.get_context("spawn")) as ex:
        for sid, why, x, opened in ex.map(_read_one, tasks, chunksize=2):
            if x is None:
                if sid in uncal:
                    # the raw-score mode: an uncalibrated stack the student could
                    # not read keeps no-calibration with no score (stage1 says
                    # why), so the engine's deferral list keeps its meaning
                    out[sid] = table(sid, stage1=why, reason="no-calibration", files_read=opened)
                else:
                    out[sid] = table(sid, stage1=why, reason="deferred", files_read=opened)
                continue
            buf.append((sid, x, opened))
            if len(buf) >= batch:
                flush()
        flush()
    return out, rows


def cmd_cascade(a: argparse.Namespace) -> int:
    params = {"workers": a.threads, "batch": a.batch, "device": a.device}
    if a.raw_scores:
        params["raw_scores"] = True
    run = Run("postcontrast-cascade", a.output, params, "cpu")
    try:
        cas = parts.cascade(a.inputs)
        F = calibration.for_run(Fusion.load(cas.fusion.artifact), a.calibration, run.extra, cas.fusion.digest)
        ct = cas.part.card.get("threshold")
        if isinstance(ct, bool) or not isinstance(ct, (int, float)) or abs(float(ct) - F.t) > 1e-9:
            raise parts.PartsError("the cascade's card names no threshold, or not the fusion's")
        stacks, context = manifest.read(a.stacks, a.source_root)
        device = _device(a.device)
        run.device = device
    except (parts.PartsError, FusionError, calibration.CalibrationError, manifest.ManifestError, RuntimeError) as e:
        run.extra["error"] = str(e)
        run.write()
        print(f"nils-postcontrast: {e}", file=sys.stderr)
        return 1
    t0 = time.time()
    tables, rows = score(stacks, context, cas, F, device=device, workers=a.threads, batch=a.batch, threads=a.student_threads, raw=bool(a.raw_scores))
    counts: dict = {}
    for e in stacks:
        doc = tables[e.stack_id]
        counts[doc["reason"] or "answered"] = counts.get(doc["reason"] or "answered", 0) + 1
        body = (json.dumps(doc, indent=2) + "\n").encode()
        status = SKIPPED if doc["reason"] in ("sealed", "not-mr") else ("succeeded" if doc["answers"] or doc["reason"] else FAILED)
        # score_raw on every unit, null without the raw-score mode, as the table's column is
        metrics = {k: doc[k] for k in ("value", "confidence", "stage", "stage1", "reason", "files_read", "score_raw")}
        u = Unit(e.unit or f"stack-{e.stack_id}", status=status, metrics=metrics)
        u.outputs.append(run.output_file(f"postcontrast/{e.stack_id}.json", body, "table", "application/json"))
        run.units.append(u)
        if doc["answers"]:
            p = doc["p_given"]
            run.proposals.append({"stack_id": e.stack_id, "axis": AXIS, "value": doc["value"],
                                  "probabilities": {GIVEN: round(p, 6), NOT_GIVEN: round(1 - p, 6)}, "model_digest": cas.digest})
    run.metrics = {"stacks": len(stacks), "by_outcome": counts, "seconds": round(time.time() - t0, 3),
                   "deferred": [e.stack_id for e in stacks if tables[e.stack_id]["reason"] == "deferred"]}
    if a.raw_scores:
        run.metrics["raw_scored"] = sum(1 for d in tables.values() if d["score_raw"] is not None)
    calibration.record(run.extra, {sid: r["site"] for sid, r in rows.items()}, tables)
    run.models = [cas.part.ref(), cas.student.ref(), cas.fusion.ref()]
    run.write()
    return 0


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="nils-postcontrast", description=__doc__.split("\n\n")[0])
    p.add_argument("--version", action="version", version=__version__)
    sub = p.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("cascade", help="score post-contrast on T1-type brain through the three-slice student")
    c.add_argument("--stacks", type=Path, default=Path("/input/stacks.json"))
    c.add_argument("--inputs", type=Path, default=Path("/inputs"))
    c.add_argument("--output", type=Path, default=Path("/output"))
    c.add_argument("--source-root", type=Path, default=None, help="where the manifest's source mounts are, outside a container")
    c.add_argument("--threads", type=int, default=8, help="worker processes reading slices")
    c.add_argument("--batch", type=int, default=64, help="stacks the student scores at once")
    c.add_argument("--device", choices=["auto", "cpu", "cuda"], default="auto")
    c.add_argument("--student-threads", type=int, default=None, help="torch's threads for the student on a CPU")
    calibration.add_run_options(c)
    c.set_defaults(fn=cmd_cascade)
    from .full import add_parser

    add_parser(sub)
    calibration.add_parser(sub)
    return p


def main(argv: list | None = None) -> int:
    logging.basicConfig(level=os.environ.get("NILS_LOG", "WARNING"))
    a = parser().parse_args(argv)
    if getattr(a, "output", None) is not None:
        a.output.mkdir(parents=True, exist_ok=True)
    return a.fn(a)


if __name__ == "__main__":
    sys.exit(main())
