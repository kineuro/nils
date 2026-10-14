# SPDX-License-Identifier: AGPL-3.0-only
"""Body part's cascade (record 55 E2, P1b): the certified model of record 50
answered in stages, each reading less than the next, as nils-design study
``2026-10-05-image-model-speed`` froze it in amendment 5
(``tools/cascade2_bp.py``, ``cascade3_bp.py``, ``freeze_bp.py``).

The cascade is one registered model (ruling R1 (a)): its artifact, the
cascade file, names every part by digest, the stages, and the thresholds.
A run is given each part as a model input of its own (``encoder``,
``head``, ``coarse``, ``student``, ``deferral``) and the cascade file as
``cascade``. The stages, in order:

1. ``s1``: the student over the stack's slot-0 native slice
   (``bodypart-slice-192:v1``). Each seed's six probabilities, and their
   mean, go through the certified head and calibration with the header
   features and cohort the registry gives; the deferral model D3 scores the
   stage's 62 features (``d3_features``) and the stage answers when D3 is at
   or above its threshold (0.99).
2. ``b24``: the certified encoder over the planes of at most 24 frames
   (``bodypart-planes-b24:v1``); it answers when the smaller of the two
   modes' log-odds distances from the threshold is at least m3 (0.5).
3. ``touched``: the certified encoder over the frames the planes touch
   (``bodypart-planes-touched:v1``), reusing the frames ``b24`` read; it
   always answers.

A stack the student cannot read (no registry geometry, a multi-frame file,
a slot-0 file that does not decode) starts at ``b24``: the research scored
a blank slice there, and deferring it can only move an answer to the
certified path.
"""

from __future__ import annotations

import json
import math
from dataclasses import dataclass
from pathlib import Path

import numpy as np

from . import fusion

PARTS = ("encoder", "head", "coarse", "student", "deferral")
MODES = ("fine", "coarse")
D3_FEATURES = 62
PLANE_CODE = {"axial": 0, "coronal": 1, "sagittal": 2}


class CascadeError(ValueError):
    pass


@dataclass
class Cascade:
    part: fusion.Part  # the cascade file itself
    doc: dict
    student: fusion.Part
    deferral: fusion.Part
    booster: object
    d3_threshold: float
    m3: float
    b24: str  # the reduced reader's policy of the middle stage
    thresholds: dict

    @property
    def digest(self) -> str:
        return self.part.digest

    def refs(self) -> dict:
        return {"cascade": self.part.ref(), "student": self.student.ref(), "deferral": self.deferral.ref()}


def _checked(p: fusion.Part) -> fusion.Part:
    if not p.artifact.is_file():
        raise CascadeError(f"the {p.input} model's artifact is not in the inputs")
    if not p.digest.startswith("sha256:") or fusion.sha256_file(p.artifact) != p.digest:
        raise CascadeError(f"the {p.input} model's artifact is not the one its digest names")
    if p.card.get("digest") not in (None, p.digest):
        raise CascadeError(f"the {p.input} model's card names another artifact")
    return p


def load(inputs: Path, model: fusion.Model) -> Cascade | None:
    """The cascade the run was given, each part checked against the cascade
    file; None for a run of the plain head."""
    parts = fusion._parts(Path(inputs))
    if "cascade" not in parts:
        return None
    for need in ("student", "deferral"):
        if need not in parts:
            raise CascadeError(f"the run was given a cascade and no {need} model")
    cas, stu, dfr = (_checked(parts[k]) for k in ("cascade", "student", "deferral"))
    try:
        doc = json.loads(cas.artifact.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as e:
        raise CascadeError("the cascade is not JSON") from e
    if not isinstance(doc, dict) or not isinstance(doc.get("parts"), dict):
        raise CascadeError("the cascade file names no parts")
    if model.coarse_part is None:
        raise CascadeError("the cascade answers both modes, and the run was given no coarse model")
    given = {"encoder": model.enc.digest, "head": model.head_part.digest, "coarse": model.coarse_part.digest, "student": stu.digest,
             "deferral": dfr.digest}
    for k in PARTS:
        if doc["parts"].get(k) != given[k]:
            raise CascadeError(f"the cascade names another {k} than the one the run was given")
    card_parts = cas.card.get("parts")
    if isinstance(card_parts, dict):
        for k, v in card_parts.items():
            if k in given and v != given[k]:
                raise CascadeError(f"the cascade's card names another {k} than its file")
    # the engine stages each body_part decision at the threshold of the
    # card that names it, the cascade's: it must be the head's, or the
    # engine would stage what the run said abstains, or the reverse
    ct = cas.card.get("threshold")
    if isinstance(ct, bool) or not isinstance(ct, (int, float)) or abs(float(ct) - float(model.fine_threshold)) > 1e-9:
        raise CascadeError("the cascade's card names no threshold, or not the head's")
    th = doc.get("thresholds") or {}
    for mode, have in (("fine", model.fine_threshold), ("coarse", model.coarse_threshold)):
        t = th.get(mode)
        if not isinstance(t, (int, float)) or abs(float(t) - float(have)) > 1e-9:
            raise CascadeError(f"the cascade's {mode} threshold is not the model's")
    stages = doc.get("stages") or []
    names = [s.get("stage") for s in stages if isinstance(s, dict)]
    if names != ["s1", "b24", "touched"]:
        raise CascadeError("the cascade's stages are not s1, b24 and touched")
    s1, b24, touched = stages
    d3 = (s1.get("accepts") or {}).get("deferral_at_least")
    m3 = (b24.get("accepts") or {}).get("margin_logit_at_least")
    if not isinstance(d3, (int, float)) or not isinstance(m3, (int, float)) or touched.get("accepts") != "always":
        raise CascadeError("the cascade's stages name no thresholds")
    policy = str(b24.get("input") or "").removeprefix("bodypart-planes-").split(":", 1)[0]
    if policy != "b24":
        raise CascadeError("the cascade's middle stage is not the 24-frame reader")
    import lightgbm as lgb

    try:
        booster = lgb.Booster(model_str=dfr.artifact.read_text(encoding="utf-8"))
    except Exception as e:  # noqa: BLE001
        raise CascadeError("the deferral model is not a LightGBM model") from e
    if booster.num_feature() != D3_FEATURES:
        raise CascadeError(f"the deferral model reads {booster.num_feature()} features, not {D3_FEATURES}")
    return Cascade(cas, doc, stu, dfr, booster, float(d3), float(m3), policy, {k: float(th[k]) for k in MODES})


# --- cascade2_bp.py: the stage's answers and its features --------------------------------------------------------------

def logit(p: float) -> float:
    p = min(max(p, 1e-9), 1 - 1e-9)
    return math.log(p / (1 - p))


def eff(m: dict) -> str:
    return m["value"] if m["answers"] else "abstain"


def head_batch(model: fusion.Model, P: np.ndarray, h44: np.ndarray, cohorts) -> tuple[list[dict], np.ndarray]:
    """``planes_bp.head``: the booster once over the batch, then each
    stack's calibration; the answers and the head's log-probabilities."""
    X = np.concatenate([np.log(np.clip(P.astype(np.float64), 1e-6, 1)), h44.astype(np.float64)], 1).astype(np.float32)
    with model.lock:
        raw = model.booster.predict(X)
    p = np.clip(raw, 1e-9, 1)
    LP = np.log(p / p.sum(1, keepdims=True))
    return [fusion.calibrate(model, LP[i], (str(c) or None)) for i, c in enumerate(cohorts)], LP


def stage_table(model: fusion.Model, P: np.ndarray, h44: np.ndarray, cohorts) -> list[dict]:
    """Per stack (``P`` is (N, seeds, 6)): the mean seed's answer, whether
    every seed's effective outcome is the mean's in both modes, and the
    stage's 14 features, as ``cascade2_bp.stage_table`` computed them."""
    if not len(P):
        return []
    seeds = [head_batch(model, P[:, s], h44, cohorts)[0] for s in range(P.shape[1])]
    Pm = P.mean(1)
    mean, LP = head_batch(model, Pm, h44, cohorts)
    spread = P.std(1).max(1)
    out = []
    for j in range(len(P)):
        mj = mean[j]
        agree = all(eff(s[j][mo]) == eff(mj[mo]) for s in seeds for mo in MODES)
        feats = [s[j][mo]["confidence"] for s in seeds for mo in MODES]
        feats += [abs(logit(s[j][mo]["confidence"]) - logit(s[j][mo]["threshold"])) for s in seeds for mo in MODES]
        feats += [float(agree), float(spread[j])]
        out.append({"ans": mj, "agree": agree, "feats": feats, "LP": LP[j], "P": Pm[j]})
    return out


def d3_features(stage_feats: list, h44: np.ndarray, g: dict) -> list:
    """Stage 1's 62 features (``cascade3_bp.stages``'s ``feats``): the
    stage's 14, the 44 header features (missing as -1), the plane, the kind
    (a deterministic code), the tilt and the fingerprint's slice count."""
    base = list(np.nan_to_num(np.asarray(h44, np.float32), nan=-1.0)) + [
        PLANE_CODE.get(g.get("plane"), 3), sum(map(ord, str(g.get("kind") or ""))) % 97, float(g.get("tilt") or 0), float(g.get("fp_slices") or 0)]
    return list(stage_feats) + base


def defer_scores(cas: Cascade, rows: list[list]) -> np.ndarray:
    if not rows:
        return np.zeros(0)
    return np.asarray(cas.booster.predict(np.array(rows)), dtype=np.float64)


def margin(answers: dict, thresholds: dict) -> float:
    """``cascade3_bp.final``'s margin: the smaller of the modes' log-odds
    distances between the confidence (as its answers table wrote it, to six
    places) and the cascade's threshold."""
    return min(abs(logit(round(float(answers[m]["confidence"]), 6)) - logit(thresholds[m])) for m in MODES if m in answers)


# --- the cascade over a run's stacks ---------------------------------------------------------------------------------------

REASONS = ("sealed", "not-mr", "no-geometry", "not-sure", "unreadable")
# why stage 1 did not answer a stack it could score: it went straight to
# the planes (no study from the engine, a file whose registry geometry
# cannot stand in for its header, a multi-frame file or no slice to read),
# its slice did not decode, or D3 deferred it
STAGE1 = ("answered", "deferred", "no-study", "no-file-geometry", "multi-frame", "no-slice", "slice-unreadable")
# the student's threads on the CPU, its own and not the run's --threads, so
# its numbers do not depend on how many workers read: the count the graded
# CPU answers were made with (the speed study's A5.2 run line, 32 workers)
STUDENT_THREADS = 32
# stage 1's slices read at a time (rounded down to whole batches of the
# student): what the main process holds of them, about 37 kB a slice
STAGE1_BLOCK = 2048


@dataclass
class Outcome:
    """One stack's end in the cascade: its answer and the stage that gave
    it, or the reason it has none (``sealed``, ``not-mr``, ``no-geometry``,
    ``unreadable``); ``why`` is the unit's status and error where it has no
    answer. ``tile`` is the student's slice where it read one."""

    stack: object
    result: fusion.StackResult | None = None
    reason: str | None = None
    why: tuple | None = None
    tile: np.ndarray | None = None
    tile_ident: dict | None = None
    tile_path: str | None = None
    d3: float | None = None
    stage1: str | None = None  # how stage 1 ended for it (``STAGE1``); None where it was never a candidate
    tried: str | None = None  # a slot-0 file opened whose slice did not decode
    opened: set | None = None  # the files a stack with no answer had its reads open
    tile_cached: bool = False  # the student's slice came from the slice cache (A17b), no file read


# a stack stage 1 never scores and the planes never read: its reason and its unit's error
SKIPS = {"sealed": "sealed now: the stack is not scored", "not-mr": "not MR: the cascade answers MR stacks only",
         "no-geometry": "no fingerprint row: the model abstains"}


def plan_stack(st, g: dict, model: fusion.Model) -> tuple[str | None, tuple | None, tuple | None]:
    """Stage 1's plan of one stack from its registry row ``g``
    (``fewslice.row_of``): ``(why, registry inputs, slot-0 choice)``. ``why``
    is None for a stack the student reads; a reason of ``SKIPS`` for one no
    stage scores; else how stage 1 ended for a stack that goes straight to
    the planes (``STAGE1``). Needs no other stack: the session's top places
    the slice afterwards (``fewslice.slot0_geo``)."""
    from . import fewslice

    header = st.extra.get("header") if isinstance(st.extra.get("header"), dict) else {}
    if g.get("sealed"):
        return "sealed", None, None
    if g.get("modality") and not fewslice.is_mr(g):
        return "not-mr", None, None
    if not isinstance(header.get("fingerprint"), dict):
        return "no-geometry", None, None
    # the student reads the stack against its session: without the
    # engine's study id (an engine before P1b) it cannot, and the planes
    # answer
    reg = fewslice.registry_inputs(st, model) if g.get("study_id") is not None else None
    choice = fewslice.slot0_choice(g) if reg is not None else None
    if reg is None or choice is None:
        why = ("no-study" if g.get("study_id") is None else "multi-frame" if (g.get("max_frames") or 1) > 1
               else "no-file-geometry" if reg is None else "no-slice")
        return why, None, None
    return None, reg, choice


def _routed(st, why: str) -> Outcome:
    """The outcome of a stack stage 1 does not read: a reason, or how stage 1 ended."""
    from .results import SKIPPED

    if why in SKIPS:
        return Outcome(st, reason=why, why=(SKIPPED, SKIPS[why]))
    return Outcome(st, stage1=why)


def _rows(items: list) -> list:
    """The overlap's first work in a worker: per ``(stack, context)`` the
    stack's registry row without its files' list and, for a stack of the
    run, stage 1's plan (``plan_stack``): ``(None, row, *plan)``."""
    return [(None, *_row(st, is_context)) for st, is_context in items]


def _rows_text(arg: tuple) -> list:
    """``_rows`` over manifest entries as written (``manifest.texts``):
    each parsed here into its Stack, which comes back without its list of
    slices beside its row and plan."""
    from . import manifest
    from .scoring import _bare

    mounts, bases, items = arg
    e = manifest.Entries()
    e.mounts, e.bases = mounts, bases
    out = []
    for kind, text in items:
        st = _bare(e.stack(kind, text))
        out.append((st, *_row(st, kind == "context")))
    return out


def _row(st, is_context: bool) -> tuple:
    from . import fewslice
    from .scoring import _W

    g = fewslice.row_of(st)
    plan = (None, None, None) if is_context else plan_stack(st, g, _W["model"])
    g.pop("files", None)  # read by the choice of slot 0 alone
    return (g, *plan)


def _tokens(c: tuple) -> np.ndarray:
    """The student's 42 tokens of a candidate ``(stack, row, registry inputs, ...)``."""
    from . import fewslice

    return np.concatenate([fewslice.stack_tokens(c[1]), fewslice.geo_feats(c[2][0][None, :])[0]]).astype(np.float32)


def _stage1_fns(model, cas, out: dict, rest: list, deferred_to: list, net, student_factory, batch: int, keep_inputs: bool, fill: bool,
                cached: set | None = None):
    """Stage 1's two steps over a batch of read candidates, ``(candidate,
    read)``: ``predict``, the student's probabilities, and ``decide``, the
    head, the calibration and D3, and each stack's outcome."""

    def predict(take: list) -> np.ndarray:
        tiles = np.stack([t[0][0] for _, t in take])
        sgeo = np.stack([c[4] for c, _ in take]).astype(np.float32)
        # the tokens made beside the slice, else here
        tokens = np.stack([t[2] if len(t) > 2 else _tokens(c) for c, t in take]).astype(np.float32)
        if net is not None:
            return net.predict(tiles, sgeo, tokens, batch=batch)
        return np.asarray(student_factory(tiles, sgeo, tokens), np.float32)

    def decide(take: list, P: np.ndarray) -> None:
        deferred = []
        h44 = np.stack([c[2][1] for c, _ in take]).astype(np.float32)
        cohorts = [c[2][2] for c, _ in take]
        table = stage_table(model, P, h44, cohorts)
        X = [d3_features(t["feats"], c[2][1], c[1]) for (c, _), t in zip(take, table)]
        d3 = defer_scores(cas, X)
        for (c, t), row, q in zip(take, table, d3):
            st = c[0]
            path = st.files[c[3]][0]
            hit = cached is not None and st.stack_id in cached
            o = Outcome(st, tile=t[0][0] if keep_inputs or fill else None, tile_ident=t[0][1], tile_path=path, d3=float(q), stage1="answered",
                        tile_cached=hit)
            if q >= cas.d3_threshold:
                header = st.extra.get("header") or {}
                cls = header.get("classification")
                from . import header_features

                # a slice the cache gave read no file (A17b)
                o.result = fusion.StackResult(row["P"], c[2][1], row["LP"], row["ans"], c[2][2] or None, c[2][3], c[2][4],
                                              header_features.rules_row(cls), {"files_read": 0 if hit else 1}, stage="s1",
                                              idents={} if hit else {path: t[0][1]}, read_files=[] if hit else [path],
                                              opened=set() if hit else {path})
            else:
                o.stage1 = "deferred"
                rest.append(st)
                deferred.append(st)
            out[st.stack_id] = o
        if deferred_to:
            deferred_to[0].submit(deferred)

    return predict, decide


def score(model: fusion.Model, cas: Cascade, inputs: Path, stacks: list, *, workers: int = 1, device: str = "cpu", chunk: int = 4,
          depth: int = 1, student_batch: int = 64, fill: bool = False, keep_inputs: bool = False, student_factory=None,
          stats: dict | None = None, gpu_workers: int = 2, context: list | None = None, student_device: str | None = None,
          student_threads: int = STUDENT_THREADS, overlap: bool = False, cache=None) -> list[Outcome]:
    """Every stack's outcome, in the manifest's order.

    First the reasons that need no read: a stack sealed now (``sealed``),
    one whose modality is known and not MR (``not-mr``), one without a
    fingerprint row (``no-geometry``). Then stage 1 over the stacks the
    student can read (registry geometry for every file, one frame a file, a
    slot-0 file that decodes), the session's top computed over each study's
    unsealed MR stacks, the run's and the manifest's ``context`` (the rest
    of the session, never scored), so a stack's input is the same in any
    run that holds it; then the planes stages over the rest,
    in worker processes. ``student_factory`` stands in for the student in
    tests (a callable taking the tiles, slice geometry and tokens);
    ``student_device`` is where the student runs (by default the card where
    ``device`` is one and this torch can use it, else the CPU). With
    ``overlap`` (and more than one worker) one worker pool reads stage 1's
    slices and climbs the planes stages of each stack the moment stage 1
    defers it, while the student scores the next batch and a second thread
    runs the head, the calibration and D3: the same batches, functions and
    inputs, so the same answers, in another order of work."""
    from . import fewslice, scoring

    stats = stats if stats is not None else {}
    if overlap and workers > 1 and cache is None:
        return score_stream(model, cas, inputs, [("stack", st) for st in stacks] + [("context", c) for c in context or []], workers=workers,
                            device=device, chunk=chunk, depth=depth, student_batch=student_batch, fill=fill, keep_inputs=keep_inputs,
                            student_factory=student_factory, stats=stats, gpu_workers=gpu_workers, student_device=student_device,
                            student_threads=student_threads)
    rows = {st.stack_id: fewslice.row_of(st) for st in stacks}
    fewslice.session_tops(list(rows.values()) + [fewslice.row_of(c) for c in context or [] if c.stack_id not in rows])
    out: dict[int, Outcome] = {}
    cands: list[tuple] = []  # (stack, row, registry inputs, slot-0 file index, slice geometry)
    rest: list = []
    for st in stacks:
        g = rows[st.stack_id]
        why, reg, choice = plan_stack(st, g, model)
        if why is not None:
            out[st.stack_id] = _routed(st, why)
            if why not in SKIPS:
                rest.append(st)
            continue
        index, sgeo = fewslice.slot0_geo(g, choice)
        cands.append((st, g, reg, index, sgeo))
    stats.update(stage1_candidates=len(cands), straight_to_planes=len(rest), context_stacks=len(context or []))

    # stage 1: the slot-0 slices, the student, the head, D3. Streamed: the
    # slices are read a block at a time and the student is given them in
    # its own batches, in the run's order, so each batch is the one a single
    # pass over the run makes (the student's numbers depend on its batches;
    # the graded runs did not pad the last one, and neither does this), and
    # a slice is dropped once its stack's stage is done unless the cache
    # keeps it
    batch = max(1, int(student_batch))
    net = None
    if cands and student_factory is None:
        # loaded before any slice is read, so a student that cannot load
        # fails the run before its reads
        net = _student(cas, device, student_device, student_threads)
    pending: list = []  # (candidate, read) decoded, waiting for the student
    cached: set = set()  # the stacks whose slice the cache gave
    predict, decide = _stage1_fns(model, cas, out, rest, [], net, student_factory, batch, keep_inputs, fill, cached)

    def stage1(take: list) -> None:
        decide(take, predict(take))

    unreadable = 0
    block = batch * max(1, STAGE1_BLOCK // batch)  # slices read at a time, whole batches
    for i in range(0, len(cands), block):
        part = cands[i : i + block]
        # the slot-0 slices the cache holds are not read (A17b)
        from_cache = {}
        if cache is not None:
            for c in part:
                hit = cache.tile(c[0], c[3])
                if hit is not None:
                    from_cache[c[0].stack_id] = (hit, None)
                    cached.add(c[0].stack_id)
        reads = iter(scoring.tiles([c[0].files[c[3]][0] for c in part if c[0].stack_id not in from_cache], workers))
        read = [from_cache.get(c[0].stack_id) or next(reads) for c in part]
        for c, t in zip(part, read):
            if t[0] is None:
                rest.append(c[0])
                # to the planes: the slice did not decode
                out[c[0].stack_id] = Outcome(c[0], stage1="slice-unreadable", tried=c[0].files[c[3]][0])
                unreadable += 1
            else:
                pending.append((c, t))
        del read, part
        whole = len(pending) // batch * batch
        if whole:
            stage1(pending[:whole])
            del pending[:whole]
    if pending:
        stage1(pending)
        pending.clear()
    stats["stage1_unreadable"] = unreadable
    stats["stage1_accepted"] = sum(1 for o in out.values() if o.result is not None and o.result.stage == "s1")

    # stages 2 and 3, and the touched planes of stage 1's answers for the cache
    order = {st.stack_id: i for i, st in enumerate(stacks)}
    rest.sort(key=lambda st: order[st.stack_id])
    ladder = {"stages": [(cas.b24, cas.m3), ("touched", None)], "thresholds": cas.thresholds, "fill": fill, "planes": keep_inputs or fill}
    if cache is not None:
        # the planes the cache holds answer without a read (A17b)
        to_read = []
        for st in rest:
            r = cached_climb(model, st, ladder, cache)
            if r is None:
                to_read.append(st)
            else:
                _planes_answer(out, st, r, None)
        stats["planes_from_cache"] = len(rest) - len(to_read)
        rest = to_read
    for st, r, why in scoring.score(model, inputs, rest, workers=workers, device=device, chunk=chunk, depth=depth, ladder=ladder, stats=stats,
                                    gpu_workers=gpu_workers):
        _planes_answer(out, st, r, why)
    _fill(model, cas, inputs, out, order, fill, workers=workers, device=device, chunk=chunk, depth=depth, gpu_workers=gpu_workers, cache=cache)
    return [out[st.stack_id] for st in stacks]


def _from_cache(model, st, version: str, cache):
    """One planes stage's answer from the slice cache (A17b), or None: the
    planes and the geometry the cache kept, the encoder's six probabilities
    as it kept them where the run's encoder made them (else the encoder
    reads the planes again), and the head, the calibration and the header
    features from the manifest, as a read of the files would give them."""
    from . import slicecache, volume

    hit = cache.planes(st, version)
    if hit is None:
        return None
    arrays, rec = hit
    header = st.extra.get("header") if isinstance(st.extra.get("header"), dict) else {}
    if not isinstance(header.get("fingerprint"), dict):
        return None
    built = volume.Built(slicecache.vol_of(arrays["planes"]), dict(rec["meta"]))
    if rec.get("encoder") == model.enc.digest:
        P = np.asarray(arrays["image"], np.float32)
    else:
        P = model.encoder.probabilities(built.vol, built.geo())
    r = fusion.finish(model, fusion.Prepared(built, header), P)
    r.planes = np.asarray(arrays["planes"], np.uint8)
    r.read_files, r.kept, r.idents, r.opened = [], None, {}, set()
    r.reader = {"files_read": 0, "cache": version}
    r.cached = True
    return r


def cached_climb(model, st, ladder: dict, cache):
    """``scoring.climb`` from the slice cache (A17b): each stage's planes as
    the cache holds them, the same stages, margins and answer; None where a
    stage the climb reaches is not in the cache, and then the files are
    read as before. With ``fill`` the touched planes must be there too."""
    from . import slicecache

    answer = None
    for policy, need in ladder["stages"]:
        r = _from_cache(model, st, slicecache.PLANES[policy], cache)
        if r is None:
            return None
        r.stage = policy
        if need is None or margin(r.answers, ladder["thresholds"]) >= need:
            answer = r
            break
    if answer is None:
        answer = r
    if ladder.get("fill") and answer.stage != "touched":
        filled = _from_cache(model, st, slicecache.PLANES["touched"], cache)
        if filled is None:
            return None
        filled.stage = "touched"
        answer.filled = filled
    return answer


def _planes_answer(out: dict, st, r, why) -> None:
    """A planes stage's answer, or its reason, into the stack's outcome."""
    from .results import SKIPPED

    o = out.get(st.stack_id) or Outcome(st)
    if r is None:
        o.why = tuple(why[:2]) if why else why
        o.reason = "unreadable" if why and why[0] != SKIPPED else "no-geometry"
        # what its reads opened before they failed, its slice's file too
        o.opened = set(why[2] if why and len(why) > 2 else ()) | {p for p in (o.tile_path, o.tried) if p}
    else:
        if o.tile_path is not None and o.tile_cached:  # the cache gave its slice: no file read
            pass
        elif o.tile_path is not None:  # the student read its slice first
            r.idents = dict(r.idents or {})
            r.idents[o.tile_path] = o.tile_ident
            r.opened = set(r.opened or ()) | {o.tile_path}
            r.reader = dict(r.reader or {}, files_read=len(r.opened))
        elif o.tried is not None:  # its slice's file was opened, and did not decode
            r.opened = set(r.opened or ()) | {o.tried}
            r.reader = dict(r.reader or {}, files_read=len(r.opened))
        o.result = r
    out[st.stack_id] = o


def _fill(model, cas, inputs, out: dict, order: dict, fill: bool, *, workers, device, chunk, depth, gpu_workers, cache=None) -> None:
    """The touched planes of stage 1's answers, for the cache (R5): read
    only for a stack whose touched planes the cache does not hold yet."""
    from . import scoring, slicecache

    if fill:
        accepted = [o.stack for o in out.values() if o.result is not None and o.result.stage == "s1"]
        if cache is not None:
            accepted = [st for st in accepted if cache.planes(st, slicecache.PLANES["touched"]) is None]
        accepted.sort(key=lambda st: order[st.stack_id])
        fill_ladder = {"stages": [("touched", None)], "thresholds": cas.thresholds, "planes": True}
        for st, r, _why in scoring.score(model, inputs, accepted, workers=workers, device=device, chunk=chunk, depth=depth, ladder=fill_ladder,
                                         gpu_workers=gpu_workers):
            if r is not None:
                out[st.stack_id].result.filled = r


def _student(cas: Cascade, device: str, student_device: str | None, student_threads: int):
    from .student import Student

    if student_device is None:
        from .gpu import torch_cuda_available

        student_device = "cuda" if device == "cuda" and torch_cuda_available() else "cpu"
    return Student(cas.student.artifact, device=student_device, threads=max(1, int(student_threads)))


# how far the manifest's reading must be past a batch's stacks before the
# student scores it with each session's top so far: a session's stacks lie
# close together in the manifest, so its top is seldom raised after that,
# and a batch whose inputs it moves is scored again (``score_stream``)
STREAM_LAG = 512
# stacks planned at a time by a worker, and slices read at a time
STREAM_BLOCK = 32


class _Once:
    """The pool's planes stages for a stack at most once (a stack stage 1
    deferred keeps its planes when a second score of its batch defers it again)."""

    def __init__(self, lane) -> None:
        self.lane, self.sent = lane, set()

    def submit(self, stacks: list) -> None:
        new = [st for st in stacks if st.stack_id not in self.sent]
        self.sent.update(st.stack_id for st in new)
        if new:
            self.lane.submit(new)


def score_stream(model: fusion.Model, cas: Cascade, inputs: Path, items, *, workers: int, source_root: Path | None = None, device: str = "cpu",
                 chunk: int = 4, depth: int = 1,
                 student_batch: int = 64, fill: bool = False, keep_inputs: bool = False, student_factory=None, stats: dict | None = None,
                 gpu_workers: int = 2, student_device: str | None = None, student_threads: int = STUDENT_THREADS,
                 block: int | None = None, on_final=None) -> list[Outcome]:
    """``score`` with ``overlap``: the same outcomes, with the main process
    doing only what must be done in one place (the speed study's A5.2.3e).

    ``items`` is the manifest's path (with ``source_root`` for its
    sources), or its stacks and then its context stacks, ``("stack" |
    "context", Stack)``, as ``manifest.stream`` reads them. From a path, a
    process of its own reads the file (``manifest.split``) and the workers
    parse each entry. One worker pool takes everything else, in this order
    of priority:

    1. the planning: a thread hands the stacks to the pool as they are read,
       ``block`` at a time, and the workers make each one's registry row
       and stage 1's plan (``_rows``, ``_rows_text``: ``row_of``,
       ``plan_stack``);
    2. the slot-0 slices of the stacks the student reads (``tiles_of``);
    3. the planes stages of the stacks that go straight to them, of those
       whose slice does not decode and of those D3 defers.

    The pool's workers start while the student loads, then the manifest is
    read; no file is read before the student is in, so a student that
    cannot load fails the run before its reads. The student
    scores in the main process in ``score``'s batches (the readable
    candidates in order, ``student_batch`` at a time, the last one not
    padded) as their slices come in, while a second thread runs the head,
    the calibration and D3 on the batch before and hands the stacks D3
    defers to the pool.

    A slice's place and the stack's tokens depend on its session's top,
    which is known only once the whole manifest is read (its context comes
    last). The student scores a batch once the reading is ``STREAM_LAG``
    stacks past it, with each session's top so far; when the reading ends,
    every batch whose inputs the final tops move (byte for byte) is scored
    again, the same batch, and its decisions replace the first ones (the
    planes of a stack it no longer defers are dropped, a stack it now defers
    goes to the planes). Every input that decides an answer, every batch and
    every function that scores a stack is the sequential path's.

    ``on_final`` is called in this thread with each outcome once nothing can
    change it: a reason at once, stage 1's answers once every row is in (or
    as they are decided after that), the planes' answers as they come.
    Outcomes come back in the manifest's order all the same."""
    import logging
    import queue
    import threading
    import time
    from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait

    from . import fewslice, scoring

    log = logging.getLogger("nils_bodypart")
    t0 = time.monotonic()
    stats = stats if stats is not None else {}
    batch = max(1, int(student_batch))
    block = max(1, int(block or STREAM_BLOCK))
    if student_factory is None:
        try:
            # torch reads its threads' environment as it loads, before the
            # pool holds every library to one thread
            import torch  # noqa: F401
        except ImportError:
            pass  # the student says so as it loads
    ladder = {"stages": [(cas.b24, cas.m3), ("touched", None)], "thresholds": cas.thresholds, "fill": fill, "planes": keep_inputs or fill}
    lane = scoring.Lane(model, inputs, workers=workers, device=device, chunk=chunk, depth=depth, ladder=ladder, gpu_workers=gpu_workers,
                        hold_reads=True)
    stacks: list = []  # the run's, in the manifest's order
    counts = {"context": 0, "stacks": 0}
    planned: queue.Queue = queue.Queue()  # (items, future of _rows), then None at the end, or an error
    stop = threading.Event()
    splitter: list = []

    def from_stacks() -> None:
        part: list = []
        for kind, st in items:
            if stop.is_set():
                return
            ctx = kind == "context"
            counts["context" if ctx else "stacks"] += 1
            part.append((st, ctx))
            if len(part) >= block:
                planned.put((part, lane.first(_rows, [(scoring._bare(s), c) for s, c in part])))
                part = []
        if part:
            planned.put((part, lane.first(_rows, [(scoring._bare(s), c) for s, c in part])))

    def from_file() -> None:
        import multiprocessing as mp

        from . import manifest

        ctx_ = mp.get_context("spawn")
        recv, send = ctx_.Pipe(duplex=False)
        proc = ctx_.Process(target=manifest.split, args=(str(items), send), name="nils-bodypart-manifest", daemon=True)
        proc.start()
        splitter.append(proc)
        send.close()
        entries = manifest.Entries(source_root)
        part: list = []
        texts: list = []

        def flush() -> None:
            planned.put((list(part), lane.first(_rows_text, (entries.mounts, entries.bases, list(texts)))))
            part.clear()
            texts.clear()

        while not stop.is_set():
            try:
                msg = recv.recv()
            except EOFError:
                raise manifest.ManifestError("stacks.json could not be read: its reader ended") from None
            if msg is None:
                break
            if isinstance(msg, tuple):
                raise manifest.ManifestError(msg[1])
            for e in msg:
                if not entries.entry(e):
                    continue
                ctx = e[0] == "context"
                counts["context" if ctx else "stacks"] += 1
                part.append((None, ctx))
                texts.append((e[0], e[2]))
                if len(part) >= block:
                    flush()
        if part:
            flush()
        proc.join()

    def read() -> None:
        try:
            (from_file if isinstance(items, (str, Path)) else from_stacks)()
            log.info("the manifest: %d stacks and %d context stacks read in %.1f s", counts["stacks"], counts["context"], time.monotonic() - t0)
            planned.put(None)
        except BaseException as e:  # noqa: BLE001 - the main thread raises it
            planned.put(e)

    reader = threading.Thread(target=read, name="nils-bodypart-manifest", daemon=True)
    decider = ThreadPoolExecutor(max_workers=1)
    out: dict[int, Outcome] = {}
    rest: list = []
    once = _Once(lane)
    try:
        # the workers start while the student loads, and the manifest is
        # read after it: in this process both want the interpreter, and
        # nothing can be read before the student is in
        lane.warm()
        net = None
        if student_factory is None:
            net = _student(cas, device, student_device, student_threads)
            log.info("the student loaded at %.1f s", time.monotonic() - t0)
        lane.open_reads()
        reader.start()
        predict, decide = _stage1_fns(model, cas, out, rest, [once], net, student_factory, batch, keep_inputs, fill)

        rows: dict[int, dict] = {}
        context_rows: list = []
        tops: dict = {}  # each session's top so far
        cands: list = []  # [stack, row, registry inputs, slot-0 choice, place in the manifest]
        parts: list = []  # (items, future of _rows), in the manifest's order
        reads: list = []  # (candidates, future of tiles_of), in their order
        readable: list = []  # (candidate, read): slices in hand, waiting for a batch
        scored: list = []  # (batch, the inputs it was scored with, its decision) while the tops are not final
        fresh: list = []  # (batch, its decision) scored with the final tops, until it is decided
        again = 0
        straight: list = []
        n_straight = unreadable = 0
        done_reading = False
        all_rows = False

        def tile_block(b: list) -> None:
            reads.append((b, lane.read(scoring.tiles_of, [c[0].files[c[3][0]["index"]][0] for c in b])))

        def placed(c: list) -> tuple:
            """The candidate as the student reads it now: with its session's top so far."""
            g = c[1]
            if fewslice.in_top(g):
                g = dict(g)
                fewslice.place(g, tops[g["study_id"]])
            index, sgeo = fewslice.slot0_geo(g, c[3])
            return (c[0], g, c[2], index, sgeo)

        def taken(got) -> None:
            nonlocal done_reading
            if got is None:
                done_reading = True
            elif isinstance(got, BaseException):
                raise got
            else:
                parts.append(got)

        def answered(batch: list) -> None:
            """Stage 1's answers of a batch whose decisions are final, to ``on_final``."""
            if on_final is None:
                return
            for c, _ in batch:
                o = out[c[0].stack_id]
                if o.result is not None and o.result.stage == "s1":
                    on_final(o)

        def reconcile() -> None:
            """Every row is in: each session's top, and every batch scored
            before scored again where it moves the batch's inputs."""
            nonlocal again
            for _, _, fut in scored:
                fut.result()  # the second thread is done with them before any is scored again here
            fewslice.session_tops(list(rows.values()) + context_rows)
            for take, used, _ in scored:
                final = [(tuple(c[:3]) + fewslice.slot0_geo(c[1], c[3]), t) for c, t in take]
                final = [(c, (*t, _tokens(c))) for c, t in final]
                if not all(a[0][4].tobytes() == b[0][4].tobytes() and a[1][2].tobytes() == b[1][2].tobytes() for a, b in zip(final, used)):
                    again += 1
                    decide(final, predict(final))
                answered(take)
            scored.clear()

        while True:
            progressed = False
            while True:
                try:
                    taken(planned.get_nowait())
                except queue.Empty:
                    break
            # the plans in the manifest's order: the slices to read, the
            # stacks straight to the planes (in whole chunks, so a worker
            # reads ahead within one), and every row for the sessions' tops
            while parts and parts[0][1].done():
                part, fut = parts.pop(0)
                progressed = True
                for (st, ctx), (parsed, g, why, reg, choice) in zip(part, fut.result()):
                    st = st if st is not None else parsed
                    if fewslice.in_top(g):
                        tops[g["study_id"]] = max(tops.get(g["study_id"], g["z_hi"]), g["z_hi"])
                    if ctx:
                        context_rows.append(g)
                        continue
                    stacks.append(st)
                    rows[st.stack_id] = g
                    if why is None:
                        cands.append([st, g, reg, choice, len(rows)])
                        if len(cands) % block == 0:
                            tile_block(cands[-block:])
                        continue
                    out[st.stack_id] = _routed(st, why)
                    if why in SKIPS:
                        if on_final is not None:
                            on_final(out[st.stack_id])
                    else:
                        rest.append(st)
                        straight.append(st)
                        n_straight += 1
                if len(straight) >= chunk:
                    whole = len(straight) // chunk * chunk
                    lane.submit(straight[:whole])
                    del straight[:whole]
            if done_reading and not parts and not all_rows:
                all_rows = True
                progressed = True
                if len(cands) % block:
                    tile_block(cands[len(cands) // block * block :])
                if straight:
                    lane.submit(straight)
                    straight = []
                stats.update(stage1_candidates=len(cands), straight_to_planes=n_straight, context_stacks=counts["context"])
                log.info("every row in at %.1f s: %d candidates, %d straight to the planes", time.monotonic() - t0, len(cands), n_straight)
                n_early = len(scored)
                reconcile()
                log.info("the sessions' tops at %.1f s: %d batches scored before them, %d of them scored again", time.monotonic() - t0, n_early,
                         again)
            # stage 1's slices, in the candidates' order
            while reads and reads[0][1].done():
                b, fut = reads.pop(0)
                progressed = True
                for c, t in zip(b, fut.result()):
                    if t[0] is None:
                        rest.append(c[0])
                        # to the planes: the slice did not decode
                        out[c[0].stack_id] = Outcome(c[0], stage1="slice-unreadable", tried=c[0].files[c[3][0]["index"]][0])
                        unreadable += 1
                        once.submit([c[0]])
                    else:
                        readable.append((c, t))
            # the student, a whole batch at a time, once the reading is far enough past it
            last = all_rows and not reads
            while readable and (len(readable) >= batch or last):
                take = readable[:batch]
                if not last and counts["stacks"] - take[-1][0][4] < STREAM_LAG and not done_reading:
                    break
                del readable[:batch]
                progressed = True
                now = [(placed(c), t) for c, t in take]
                now = [(c, (*t, _tokens(c))) for c, t in now]
                fut = decider.submit(decide, now, predict(now))
                if all_rows:
                    fresh.append((take, fut))
                else:
                    scored.append((take, now, fut))
            while fresh and fresh[0][1].done():
                take, fut = fresh.pop(0)
                fut.result()
                answered(take)
            if last and not readable:
                break
            if not progressed:
                waiting = [f for f in (parts[0][1] if parts else None, reads[0][1] if reads else None) if f is not None]
                if waiting:
                    wait(waiting, timeout=0.05, return_when=FIRST_COMPLETED)
                else:
                    try:
                        taken(planned.get(timeout=0.05))
                    except queue.Empty:
                        pass
        for take, fut in fresh:
            fut.result()
            answered(take)
        stats["stage1_unreadable"] = unreadable
        stats["stage1_accepted"] = sum(1 for o in out.values() if o.result is not None and o.result.stage == "s1")
        stats["overlap_batches_again"] = again
        log.info("stage 1 done at %.1f s", time.monotonic() - t0)
        for st, r, why in lane.results():
            o = out.get(st.stack_id)
            if o is not None and o.result is not None and o.result.stage == "s1":
                continue  # stage 1 answered it in the end: its planes were read for nothing
            _planes_answer(out, st, r, why)
            if on_final is not None and not fill:
                on_final(out[st.stack_id])
        log.info("the planes done at %.1f s", time.monotonic() - t0)
        if lane.gpu_decode:
            stats.update(lane.decoded)
    finally:
        stop.set()
        for proc in splitter:
            if proc.is_alive():
                proc.terminate()
        if reader.ident is not None:  # started
            reader.join()
        for proc in splitter:
            proc.join(timeout=10)
        decider.shutdown(wait=True)
        lane.close()
    order = {st.stack_id: i for i, st in enumerate(stacks)}
    _fill(model, cas, inputs, out, order, fill, workers=workers, device=device, chunk=chunk, depth=depth, gpu_workers=gpu_workers)
    return [out[st.stack_id] for st in stacks]
