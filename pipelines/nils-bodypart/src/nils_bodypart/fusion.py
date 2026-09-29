# SPDX-License-Identifier: AGPL-3.0-only
"""The certified body-part model (record 50): one frozen model, two modes.

A run is given the model's registered parts as typed inputs of type
``model``, each named in ``/inputs/manifest.json`` (``models``: the input's
id, the model's name, version, digest, kind, whole card and artifact path)
and mounted read-only at ``/inputs/<id>/``:

- ``encoder``: the image encoder (``tiny.Encoder``), six log-probabilities
  from the stack's 8 mm volume and geometry;
- ``head``: a LightGBM booster (text) over those six and the 44 header
  features, whose card carries the fine calibration
  (``params.calibration``) and the threshold;
- ``coarse`` (optional): a JSON mode file that maps the six fine values to
  four regions, with its own temperatures and threshold.

Before anything runs every artifact's sha256 is the digest the manifest
names, the head's card names the encoder, the mode file names the head, and
the encoder's classes are the six in order; otherwise the run is refused.

Per stack: the head's log-probabilities LP, then

- fine: softmax(LP / T), T the stack's cohort's temperature or the global;
- coarse: the fine probabilities at the global fine temperature summed per
  region, logged, and softmaxed at the cohort's coarse temperature or the
  global one.

The cohort is the first a calibration names of: the stack's first ingest
batch, each part of that batch's name after a ``-`` (left to right), then
the subject's open cohorts (sorted). The batch comes first because it is
what the calibration was fitted by: round 7 took a stack's cohort from its
first batch's name without the ``p0-`` or ``p0ext-`` before it, and a
subject may be an open member of more than one cohort. A mode answers with its most probable
value when that value's probability is at or above its threshold, and
abstains below it.
"""

from __future__ import annotations

import hashlib
import json
import math
import threading
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from . import header_features, volume
from .tiny import Encoder, EncoderError, softmax_T

FINE = ["brain", "brain-neck", "neck", "spine", "chest", "other"]
COARSE = ["head", "spine", "chest", "other"]
FINE_AXIS = "body_part"
COARSE_AXIS = "body_region"


class ModelError(ValueError):
    """The model's parts are not what the run may use."""


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return "sha256:" + h.hexdigest()


def column(value: str) -> str:
    """A value as it stands in a column name: ``brain-neck`` is ``brain_neck``."""
    return value.replace("-", "_")


@dataclass
class Part:
    input: str
    name: str | None
    version: str | None
    digest: str
    card: dict
    artifact: Path

    def ref(self) -> dict:
        return {"name": self.name, "version": self.version, "digest": self.digest}


def _parts(inputs: Path) -> dict[str, Part]:
    """The model inputs of ``/inputs/manifest.json``, by input id, with each
    artifact found under ``inputs`` (the runner writes ``/inputs/...``)."""
    mf = inputs / "manifest.json"
    try:
        doc = json.loads(mf.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        raise ModelError("the inputs have no readable manifest.json") from e
    out: dict[str, Part] = {}
    for m in doc.get("models") or []:
        if not isinstance(m, dict) or not m.get("input"):
            continue
        art = m.get("artifact")
        if not art:
            raise ModelError(f"the model input {m['input']} has no artifact")
        rel = Path(art)
        if rel.is_absolute():
            parts = rel.parts
            rel = Path(*parts[2:]) if len(parts) > 2 and parts[1] == "inputs" else Path(*parts[1:])
        if ".." in rel.parts:
            raise ModelError(f"the model input {m['input']}'s artifact leaves the inputs")
        out[m["input"]] = Part(m["input"], m.get("name"), m.get("version"), str(m.get("digest") or ""), m.get("card") or {}, inputs / rel)
    return out


@dataclass
class Model:
    encoder: Encoder
    enc: Part
    head_part: Part
    booster: object
    calibration: dict
    fine_threshold: float
    coarse_part: Part | None = None
    mode: dict | None = None
    coarse_threshold: float | None = None
    # one prediction at a time: stacks are read in threads, the booster is shared
    lock: threading.Lock = field(default_factory=threading.Lock, repr=False)

    def refs(self) -> dict:
        r = {"encoder": self.enc.ref(), "head": self.head_part.ref()}
        if self.coarse_part is not None:
            r["coarse"] = self.coarse_part.ref()
        return r


def load(inputs: Path) -> Model:
    """The model's parts from the run's inputs, each checked."""
    parts = _parts(Path(inputs))
    for need in ("encoder", "head"):
        if need not in parts:
            raise ModelError(f"the run was given no {need} model")
    for p in parts.values():
        if p.input not in ("encoder", "head", "coarse"):
            continue
        if not p.artifact.is_file():
            raise ModelError(f"the {p.input} model's artifact is not in the inputs")
        if not p.digest.startswith("sha256:") or sha256_file(p.artifact) != p.digest:
            raise ModelError(f"the {p.input} model's artifact is not the one its digest names")
        if p.card.get("digest") not in (None, p.digest):
            raise ModelError(f"the {p.input} model's card names another artifact")
    enc, head = parts["encoder"], parts["head"]
    try:
        encoder = Encoder(enc.artifact)
    except EncoderError as e:
        raise ModelError(str(e)) from e
    if encoder.classes != FINE:
        raise ModelError(f"the encoder's classes are not the six of body_part in order ({', '.join(FINE)})")
    named = [e.get("digest") for e in head.card.get("encoders") or [] if isinstance(e, dict)]
    if enc.digest not in named:
        raise ModelError("the head's card does not name the encoder it was given")
    cal = (head.card.get("params") or {}).get("calibration")
    if not isinstance(cal, dict) or not isinstance(cal.get("global_fine"), (int, float)):
        raise ModelError("the head's card carries no calibration")
    threshold = head.card.get("threshold")
    if not isinstance(threshold, (int, float)):
        raise ModelError("the head's card names no threshold")
    import lightgbm as lgb

    try:
        booster = lgb.Booster(model_str=head.artifact.read_text(encoding="utf-8"))
    except Exception as e:  # noqa: BLE001
        raise ModelError("the head is not a LightGBM model") from e
    if booster.num_feature() != 6 + len(header_features.NAMES):
        raise ModelError(f"the head reads {booster.num_feature()} features, not {6 + len(header_features.NAMES)}")
    model = Model(encoder, enc, head, booster, cal, float(threshold))
    coarse = parts.get("coarse")
    if coarse is not None:
        try:
            mode = json.loads(coarse.artifact.read_text(encoding="utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as e:
            raise ModelError("the coarse mode file is not JSON") from e
        if not isinstance(mode, dict) or mode.get("mode") != "coarse":
            raise ModelError("the coarse model is not a coarse mode file")
        if (mode.get("head") or {}).get("digest") != head.digest:
            raise ModelError("the coarse mode file names another head than the one the run was given")
        if list(mode.get("classes") or []) != COARSE:
            raise ModelError(f"the coarse mode's classes are not {', '.join(COARSE)}")
        mp = mode.get("map") or {}
        if set(mp) != set(FINE) or not set(mp.values()) <= set(COARSE):
            raise ModelError("the coarse mode's map does not take each of the six values to a region")
        for k in ("fine_global_T", "coarse_global_T"):
            if not isinstance(mode.get(k), (int, float)):
                raise ModelError(f"the coarse mode file has no {k}")
        ths = [t for t in (coarse.card.get("threshold"), mode.get("threshold")) if isinstance(t, (int, float))]
        if not ths:
            raise ModelError("the coarse mode names no threshold")
        model.coarse_part, model.mode, model.coarse_threshold = coarse, mode, float(max(ths))
    return model


def resolve_cohort(header: dict, model: Model) -> tuple[str | None, str | None]:
    """The cohort whose temperatures the stack takes, and where it was
    found: ``cohort``, ``batch`` or ``batch_part``; (None, None) for the
    global ones."""
    fine = {c for c, v in (model.calibration.get("per_cohort") or {}).items() if isinstance(v, dict) and v.get("T") is not None}
    coarse = set((model.mode or {}).get("coarse_T_per_cohort") or {})
    known = fine | coarse
    cands: list[tuple[str, str]] = []
    batch = header.get("batch")
    if isinstance(batch, str) and batch:
        cands.append((batch, "batch"))
        parts = batch.split("-")
        cands += [("-".join(parts[i:]), "batch_part") for i in range(1, len(parts))]
    cands += [(c, "cohort") for c in sorted(str(c) for c in header.get("cohorts") or [])]
    for c, where in cands:
        if c in known:
            return c, where
    return None, None


def _mode_answer(p: np.ndarray, classes: list[str], threshold: float, T: float) -> dict:
    i = int(np.argmax(p))
    conf = float(p[i])
    return {
        "probabilities": {c: float(v) for c, v in zip(classes, p)},
        "value": classes[i],
        "confidence": conf,
        "threshold": threshold,
        "answers": conf >= threshold,
        "temperature": T,
    }


def head_logp(model: Model, P: np.ndarray, header44: np.ndarray) -> np.ndarray:
    X = np.concatenate([np.log(np.clip(P.astype(np.float64), 1e-6, 1)), header44.astype(np.float64)])[None, :].astype(np.float32)
    with model.lock:
        raw = model.booster.predict(X)
    p = np.clip(raw, 1e-9, 1)
    return np.log(p / p.sum(1, keepdims=True))[0]


def calibrate(model: Model, LP: np.ndarray, cohort: str | None) -> dict:
    """The fine and (with a coarse model) coarse answers from the head's
    log-probabilities."""
    per = model.calibration.get("per_cohort") or {}
    Tf = ((per.get(cohort) or {}).get("T") if cohort else None) or model.calibration["global_fine"]
    out = {"fine": _mode_answer(softmax_T(LP[None, :], float(Tf))[0], FINE, model.fine_threshold, float(Tf))}
    if model.mode is not None:
        mp = model.mode["map"]
        agg = np.zeros((len(FINE), len(COARSE)))
        for i, v in enumerate(FINE):
            agg[i, COARSE.index(mp[v])] = 1
        lpc = np.log(np.clip(softmax_T(LP[None, :], float(model.mode["fine_global_T"])) @ agg, 1e-12, 1))
        Tc = (model.mode.get("coarse_T_per_cohort") or {}).get(cohort) if cohort else None
        Tc = float(Tc) if Tc is not None else float(model.mode["coarse_global_T"])
        out["coarse"] = _mode_answer(softmax_T(lpc, Tc)[0], COARSE, float(model.coarse_threshold), Tc)
    return out


@dataclass
class StackResult:
    image: np.ndarray  # the encoder's six probabilities
    header44: np.ndarray
    LP: np.ndarray
    answers: dict
    cohort: str | None
    matched_from: str | None
    meta: dict
    rules: dict | None


class NoFingerprint(Exception):
    pass


def predict(model: Model, files: list[tuple[str, list[int] | None]], header: dict | None) -> StackResult:
    """One stack. Raises ``NoFingerprint``, ``volume.NoGeometry`` (both
    abstentions) or ``volume.Unreadable``."""
    header = header if isinstance(header, dict) else {}
    fp = header.get("fingerprint")
    if not isinstance(fp, dict):
        raise NoFingerprint("the stack has no fingerprint row")
    built = volume.build(files, fp.get("orientation"))
    P = model.encoder.probabilities(built.vol, built.geo())
    cls = header.get("classification")
    h44 = header_features.features(fp, cls, built.meta)
    LP = head_logp(model, P, h44)
    cohort, where = resolve_cohort(header, model)
    return StackResult(P, h44, LP, calibrate(model, LP, cohort), cohort, where, built.meta, header_features.rules_row(cls))


def proposed(a: dict, nd: int = 6) -> dict[str, float]:
    """A mode's probabilities as its proposal carries them: rounded to
    ``nd`` places, and the value's never at or above the threshold where
    the mode abstains, so the engine, which stages a proposal at or above
    the card's threshold, stages exactly the answers the mode gives."""
    r = {k: round(float(v), nd) for k, v in a["probabilities"].items()}
    if not a["answers"]:
        r[a["value"]] = min(r[a["value"]], math.floor(float(a["confidence"]) * 10**nd) / 10**nd)
    return r


def rounded(probs: dict[str, float], nd: int = 4) -> dict[str, float]:
    """Probabilities rounded, still summing to 1 within 0.01: the largest
    takes what rounding left over."""
    r = {k: round(float(v), nd) for k, v in probs.items()}
    top = max(r, key=lambda k: r[k])
    r[top] = round(min(1.0, max(0.0, r[top] + 1.0 - sum(r.values()))), nd)
    return r
