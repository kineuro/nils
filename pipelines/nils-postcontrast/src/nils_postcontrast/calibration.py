# SPDX-License-Identifier: AGPL-3.0-only
"""A site's calibration of model r1's fusion (0.3.0).

The fusion's score is a log-odds z per stack, calibrated per site
(``<cohort> <make>``) by a Platt pair (a, b): p = sigmoid(a z + b). The
frozen fusion carries the pairs of the sites it was fitted on; a site it
has none for gets no answer (``no-calibration``). This module gives such a
site its own pair without a new model bundle or image:

1. a run with ``--raw-scores`` writes z (``score_raw``) for every T1-type
   brain stack whose site has no pair, and no probability, so nothing
   becomes a proposal;
2. the site labels those stacks (``stack``, ``label``, ``source``) and
   ``nils-postcontrast calibrate`` fits the pair on z by a plain logistic
   regression (Platt's, with his smoothed targets so a separable set still
   gives a finite pair), refusing under 20 rows of either class (model r2's
   rule), and writes it as a pair file with its fit per label source;
3. a run given that file (``--calibration``, or the descriptor's site file
   through ``NILS_PC_CALIBRATION``) merges its pairs over the fusion's and
   scores the site as any calibrated one.

A refusal names a row by its line number, never by its content.
"""

from __future__ import annotations

import csv
import json
import math
import os
import sys
import time
from pathlib import Path

import numpy as np

from . import __version__
from .fusion import Fusion

LABELS = {"given": 1, "not_given": 0}
SOURCES = ("header", "name", "manual")
FLOOR = 20
ENV = "NILS_PC_CALIBRATION"


class CalibrationError(ValueError):
    pass


# ---------------------------------------------------------------- the pairs

def load_pairs(path) -> dict:
    """The site pairs of a calibration file: one pair file (``site``, ``a``,
    ``b``) or a map of ``{site: [a, b]}`` (a value may also be a pair
    file's object)."""
    try:
        d = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as e:
        raise CalibrationError("the calibration file is not readable JSON") from e
    if not isinstance(d, dict) or not d:
        raise CalibrationError("the calibration file holds no site pair")
    if {"site", "a", "b"} <= set(d):
        d = {d["site"]: [d["a"], d["b"]]}
    out: dict = {}
    for site, v in d.items():
        if isinstance(v, dict):
            v = [v.get("a"), v.get("b")]
        try:
            a, b = (float(x) for x in v)
        except (TypeError, ValueError) as e:
            raise CalibrationError(f"the pair of site {site!r} is not two numbers") from e
        if not (isinstance(site, str) and site.strip()) or not (math.isfinite(a) and math.isfinite(b)):
            raise CalibrationError(f"the pair of site {site!r} is not two finite numbers under a site key")
        out[site] = (a, b)
    return out


def apply(F: Fusion, path) -> tuple:
    """The fusion with the file's pairs merged over its own: (fusion, the
    sites the file set)."""
    pairs = load_pairs(path)
    cal = dict(F.cal)
    cal.update(pairs)
    G = Fusion(cols=F.cols, mean=F.mean, scale=F.scale, coef=F.coef, intercept=F.intercept, cal=cal, t=F.t, use_V=F.use_V,
               cohort_aliases=F.cohort_aliases, untrusted_time_cohorts=F.untrusted_time_cohorts)
    return G, sorted(pairs)


def for_run(F: Fusion, path, extra: dict) -> Fusion:
    """The fusion a run scores with: its own pairs, with a calibration
    file's merged over them where there is one, the file ``--calibration``
    names, else the one the engine mounted as the descriptor's site file
    (``NILS_PC_CALIBRATION``). The run records the sites the file set in
    its results (``calibration``), never the pairs."""
    path = path or os.environ.get(ENV) or None
    if not path:
        return F
    G, sites = apply(F, path)
    extra["calibration"] = {"sites": sites}
    return G


def add_run_options(c) -> None:
    """The options the cascade and the full pipeline share for a site's
    calibration (0.3.0)."""
    c.add_argument("--raw-scores", action="store_true",
                   help="score T1-type brain stacks at a site with no calibration too and write their fused log-odds (score_raw), "
                        "never a probability or a proposal, for the site to fit its pair on")
    c.add_argument("--calibration", default=None,
                   help="a JSON file of site pairs ({site: [a, b]}, or one pair file from calibrate) merged over the fusion's; "
                        f"default: the file ${ENV} names, where the engine mounted the site file")


# ---------------------------------------------------------------- the labels and the scores

def read_labels(path) -> dict:
    """The label rows by stack id: ``{stack: (label 0/1, source)}``. The
    file is tab-separated with a header naming ``stack``, ``label`` and
    ``source``; a bad row is refused by its line number."""
    try:
        text = Path(path).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as e:
        raise CalibrationError("the labels file is not readable text") from e
    rows = list(csv.reader(text.splitlines(), delimiter="\t"))
    if not rows:
        raise CalibrationError("the labels file is empty")
    head = [c.strip().lower() for c in rows[0]]
    for c in ("stack", "label", "source"):
        if c not in head:
            raise CalibrationError(f"the labels file's header lacks the column {c}")
    ix = {c: head.index(c) for c in ("stack", "label", "source")}
    out: dict = {}
    for ln, row in enumerate(rows[1:], start=2):
        if not row or all(not c.strip() for c in row):
            continue
        if len(row) <= max(ix.values()):
            raise CalibrationError(f"line {ln} of the labels file has too few columns")
        try:
            sid = int(row[ix["stack"]].strip())
        except ValueError as e:
            raise CalibrationError(f"line {ln} of the labels file has no stack id") from e
        label = row[ix["label"]].strip()
        if label not in LABELS:
            raise CalibrationError(f"line {ln} of the labels file has a label that is neither given nor not_given")
        source = row[ix["source"]].strip()
        if source not in SOURCES:
            raise CalibrationError(f"line {ln} of the labels file has a source that is none of header, name, manual")
        if sid in out:
            raise CalibrationError(f"line {ln} of the labels file repeats a stack")
        out[sid] = (LABELS[label], source)
    return out


def _tables(path: Path):
    if path.is_dir():
        sub = path / "postcontrast"
        folder = sub if sub.is_dir() else path
        files = sorted(p for p in folder.glob("*.json") if p.name != "results.json")
    else:
        files = [path]
    for f in files:
        try:
            doc = json.loads(f.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as e:
            raise CalibrationError("a scores file is not readable JSON") from e
        if isinstance(doc, dict) and "stack_id" in doc:
            yield doc


def read_scores(paths) -> dict:
    """The raw scores by stack id from the raw-score mode's outputs: each
    path a run's output folder (its ``postcontrast/<stack>.json`` tables)
    or one table. ``{stack: {"z", "site", "fusion_digest", "cascade_digest"}}``;
    a stack in several outputs keeps the last."""
    out: dict = {}
    for p in paths:
        p = Path(p)
        if not p.exists():
            raise CalibrationError("a scores path does not exist")
        for doc in _tables(p):
            z = doc.get("score_raw")
            if z is None:
                continue
            try:
                z = float(z)
            except (TypeError, ValueError) as e:
                raise CalibrationError("a table's score_raw is not a number") from e
            out[int(doc["stack_id"])] = {"z": z, "site": doc.get("site"), "fusion_digest": doc.get("fusion_digest"),
                                         "cascade_digest": doc.get("cascade_digest")}
    return out


# ---------------------------------------------------------------- the fit

def fit_platt(z: np.ndarray, y: np.ndarray, iters: int = 200) -> tuple:
    """Platt's pair (a, b) for p = sigmoid(a z + b): the logistic regression
    of the labels on z with Platt's smoothed targets ((n+ + 1) / (n+ + 2) for
    a positive, 1 / (n- + 2) for a negative), by Newton's method with
    halving where a step does not lower the loss."""
    z = np.asarray(z, np.float64)
    y = np.asarray(y, np.float64)
    npos, nneg = float(y.sum()), float((1 - y).sum())
    t = np.where(y > 0.5, (npos + 1) / (npos + 2), 1 / (nneg + 2))
    X = np.column_stack([z, np.ones_like(z)])
    w = np.zeros(2)

    def loss(w):
        f = X @ w
        return float(np.sum(np.logaddexp(0, f) - t * f))

    cur = loss(w)
    for _ in range(iters):
        p = 1 / (1 + np.exp(-(X @ w)))
        g = X.T @ (p - t)
        H = (X * (p * (1 - p))[:, None]).T @ X + 1e-12 * np.eye(2)
        step = np.linalg.solve(H, g)
        s = 1.0
        while s > 1e-6:
            cand = w - s * step
            new = loss(cand)
            if new <= cur:
                break
            s /= 2
        if abs(cur - new) < 1e-12 and float(np.max(np.abs(s * step))) < 1e-9:
            w = cand
            break
        w, cur = cand, new
    return float(w[0]), float(w[1])


def _measures(z: np.ndarray, y: np.ndarray, a: float, b: float) -> dict:
    p = np.clip(1 / (1 + np.exp(-(a * z + b))), 1e-12, 1 - 1e-12)
    ll = -float(np.mean(y * np.log(p) + (1 - y) * np.log(1 - p)))
    acc = float(np.mean((p >= 0.5) == (y > 0.5)))
    return {"log_loss": round(ll, 6), "accuracy": round(acc, 6)}


def calibrate(labels: dict, scores: dict, site: str) -> dict:
    """The site's pair document from the labels and the raw scores: the
    fit, its counts and its measures overall and per label source. Rows
    with no score, and rows whose table names another site, are counted
    and left out; fewer than 20 rows of a class refuse."""
    zs, ys, srcs = [], [], []
    n_without_score = n_other_site = 0
    digests = {"fusion": set(), "cascade": set()}
    for sid, (y, src) in labels.items():
        r = scores.get(sid)
        if r is None:
            n_without_score += 1
            continue
        if r["site"] is not None and r["site"] != site:
            n_other_site += 1
            continue
        zs.append(r["z"])
        ys.append(y)
        srcs.append(src)
        for k in digests:
            if r.get(k + "_digest"):
                digests[k].add(r[k + "_digest"])
    if len(digests["fusion"]) > 1:
        raise CalibrationError("the scores come from more than one fusion: one pair fits one model")
    z, y = np.array(zs, np.float64), np.array(ys, np.float64)
    n_given, n_not = int(y.sum()), int(len(y) - y.sum())
    if n_given < FLOOR or n_not < FLOOR:
        raise CalibrationError(f"a site's pair needs at least {FLOOR} labelled stacks of each class with a score: "
                               f"{n_given} given and {n_not} not_given here")
    a, b = fit_platt(z, y)
    per: dict = {}
    for s in SOURCES:
        m = np.array([x == s for x in srcs], bool)
        if m.any():
            per[s] = {"n": int(m.sum()), "n_given": int(y[m].sum()), **_measures(z[m], y[m], a, b)}
    return {"site": site, "a": round(a, 8), "b": round(b, 8), "n": int(len(y)), "n_given": n_given, "n_not_given": n_not,
            "n_without_score": n_without_score, "n_other_site": n_other_site, "per_source": per, "overall": _measures(z, y, a, b),
            "model": {"fusion_digest": next(iter(digests["fusion"]), None), "cascade_digests": sorted(digests["cascade"]),
                      "image": "nils-postcontrast", "version": __version__},
            "fitted_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}


def report(doc: dict) -> str:
    lines = [f"site {doc['site']}: a = {doc['a']:.6f}, b = {doc['b']:.6f}",
             f"{doc['n']} stacks fitted ({doc['n_given']} given, {doc['n_not_given']} not_given); "
             f"{doc['n_without_score']} labelled with no score, {doc['n_other_site']} at another site",
             f"overall: log loss {doc['overall']['log_loss']:.4f}, accuracy {doc['overall']['accuracy']:.4f}"]
    for s, m in doc["per_source"].items():
        lines.append(f"  {s}: {m['n']} stacks ({m['n_given']} given), log loss {m['log_loss']:.4f}, accuracy {m['accuracy']:.4f}")
    return "\n".join(lines)


def cmd_calibrate(a) -> int:
    try:
        labels = read_labels(a.labels)
        scores = read_scores(a.scores)
        doc = calibrate(labels, scores, a.site)
    except CalibrationError as e:
        print(f"nils-postcontrast calibrate: {e}", file=sys.stderr)
        return 1
    out = Path(a.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    print(report(doc))
    return 0


def add_parser(sub) -> None:
    c = sub.add_parser("calibrate", help="fit a site's Platt pair on the raw scores a run wrote and the site's labels")
    c.add_argument("--labels", required=True, help="a tab-separated file with a header: stack, label (given or not_given), source (header, name, manual)")
    c.add_argument("--scores", nargs="+", required=True, help="the output folders (or tables) of runs with --raw-scores")
    c.add_argument("--site", required=True, help="the site key the pair is for, <cohort> <make>")
    c.add_argument("--out", required=True, help="the pair file to write")
    c.set_defaults(fn=cmd_calibrate)
