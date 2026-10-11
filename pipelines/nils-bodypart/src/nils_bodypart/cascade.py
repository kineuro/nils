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


def score(model: fusion.Model, cas: Cascade, inputs: Path, stacks: list, *, workers: int = 1, device: str = "cpu", chunk: int = 4,
          depth: int = 1, student_batch: int = 64, fill: bool = False, keep_inputs: bool = False, student_factory=None,
          stats: dict | None = None, gpu_workers: int = 2) -> list[Outcome]:
    """Every stack's outcome, in the manifest's order.

    First the reasons that need no read: a stack sealed now (``sealed``),
    one whose modality is known and not MR (``not-mr``), one without a
    fingerprint row (``no-geometry``). Then stage 1 over the stacks the
    student can read (registry geometry for every file, one frame a file, a
    slot-0 file that decodes), the session's top computed over the run's
    unsealed MR stacks of each study; then the planes stages over the rest,
    in worker processes. ``student_factory`` stands in for the student in
    tests (a callable taking the tiles, slice geometry and tokens)."""
    from . import fewslice, scoring
    from .results import SKIPPED

    stats = stats if stats is not None else {}
    rows = {st.stack_id: fewslice.row_of(st) for st in stacks}
    fewslice.session_tops(list(rows.values()))
    out: dict[int, Outcome] = {}
    cands: list[tuple] = []  # (stack, row, registry inputs, slot-0 file index, slice geometry)
    rest: list = []
    for st in stacks:
        g = rows[st.stack_id]
        header = st.extra.get("header") if isinstance(st.extra.get("header"), dict) else {}
        if g.get("sealed"):
            out[st.stack_id] = Outcome(st, reason="sealed", why=(SKIPPED, "sealed now: the stack is not scored"))
            continue
        if g.get("modality") and not fewslice.is_mr(g):
            out[st.stack_id] = Outcome(st, reason="not-mr", why=(SKIPPED, "not MR: the cascade answers MR stacks only"))
            continue
        if not isinstance(header.get("fingerprint"), dict):
            out[st.stack_id] = Outcome(st, reason="no-geometry", why=(SKIPPED, "no fingerprint row: the model abstains"))
            continue
        # the student reads the stack against its session: without the
        # engine's study id (an engine before P1b) it cannot, and the
        # planes answer
        reg = fewslice.registry_inputs(st, model) if g.get("study_id") is not None else None
        s0 = fewslice.slot0(g) if reg is not None else None
        if reg is None or s0 is None:
            rest.append(st)
            continue
        cands.append((st, g, reg, s0[0], s0[1]))
    stats.update(stage1_candidates=len(cands), straight_to_planes=len(rest))

    # stage 1: the slot-0 slices, the student, the head, D3
    read = scoring.tiles([c[0].files[c[3]][0] for c in cands], workers)
    ok = [(c, t) for c, t in zip(cands, read) if t[0] is not None]
    for c, t in zip(cands, read):
        if t[0] is None:
            rest.append(c[0])
            out[c[0].stack_id] = Outcome(c[0])  # deferred: the slice did not decode
    stats["stage1_unreadable"] = len(cands) - len(ok)
    if ok:
        tiles = np.stack([t[0][0] for _, t in ok])
        sgeo = np.stack([c[4] for c, _ in ok]).astype(np.float32)
        tokens = np.stack([np.concatenate([fewslice.stack_tokens(c[1]), fewslice.geo_feats(c[2][0][None, :])[0]]) for c, _ in ok]).astype(np.float32)
        if student_factory is None:
            from .student import Student

            net = Student(cas.student.artifact, device="cuda" if device == "cuda" else "cpu", threads=max(1, workers))
            P = net.predict(tiles, sgeo, tokens, batch=student_batch)
        else:
            P = np.asarray(student_factory(tiles, sgeo, tokens), np.float32)
        h44 = np.stack([c[2][1] for c, _ in ok]).astype(np.float32)
        cohorts = [c[2][2] for c, _ in ok]
        table = stage_table(model, P, h44, cohorts)
        X = [d3_features(t["feats"], c[2][1], c[1]) for (c, _), t in zip(ok, table)]
        d3 = defer_scores(cas, X)
        for (c, t), row, q in zip(ok, table, d3):
            st = c[0]
            path = st.files[c[3]][0]
            o = Outcome(st, tile=t[0][0] if keep_inputs or fill else None, tile_ident=t[0][1], tile_path=path, d3=float(q))
            if q >= cas.d3_threshold:
                header = st.extra.get("header") or {}
                cls = header.get("classification")
                from . import header_features

                o.result = fusion.StackResult(row["P"], c[2][1], row["LP"], row["ans"], c[2][2] or None, c[2][3], c[2][4],
                                              header_features.rules_row(cls), {"files_read": 1}, stage="s1", idents={path: t[0][1]},
                                              read_files=[path], opened={path})
            else:
                rest.append(st)
            out[st.stack_id] = o
    stats["stage1_accepted"] = sum(1 for o in out.values() if o.result is not None and o.result.stage == "s1")

    # stages 2 and 3, and the touched planes of stage 1's answers for the cache
    order = {st.stack_id: i for i, st in enumerate(stacks)}
    rest.sort(key=lambda st: order[st.stack_id])
    ladder = {"stages": [(cas.b24, cas.m3), ("touched", None)], "thresholds": cas.thresholds, "fill": fill, "planes": keep_inputs or fill}
    for st, r, why in scoring.score(model, inputs, rest, workers=workers, device=device, chunk=chunk, depth=depth, ladder=ladder, stats=stats,
                                    gpu_workers=gpu_workers):
        o = out.get(st.stack_id) or Outcome(st)
        if r is None:
            o.why = why
            o.reason = "unreadable" if why and why[0] != SKIPPED else "no-geometry"
        else:
            if o.tile_path is not None:  # the student read its slice first
                r.idents = dict(r.idents or {})
                r.idents[o.tile_path] = o.tile_ident
                r.opened = set(r.opened or ()) | {o.tile_path}
                r.reader = dict(r.reader or {}, files_read=len(r.opened))
            o.result = r
        out[st.stack_id] = o
    if fill:
        accepted = [o.stack for o in out.values() if o.result is not None and o.result.stage == "s1"]
        accepted.sort(key=lambda st: order[st.stack_id])
        fill_ladder = {"stages": [("touched", None)], "thresholds": cas.thresholds, "planes": True}
        for st, r, _why in scoring.score(model, inputs, accepted, workers=workers, device=device, chunk=chunk, depth=depth, ladder=fill_ladder,
                                         gpu_workers=gpu_workers):
            if r is not None:
                out[st.stack_id].result.filled = r
    return [out[st.stack_id] for st in stacks]
