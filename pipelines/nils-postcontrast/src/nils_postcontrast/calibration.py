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
   rule), and writes it as a pair file, one line, with its fit per label
   source;
3. a run given that file (``--calibration``, or the descriptor's site file
   through ``NILS_PC_CALIBRATION``) adds its pairs beside the fusion's and
   scores the site as any calibrated one; a pair file fitted on another
   fusion than the run's is refused, as is one naming a site the frozen
   (certified) fusion has a pair for, which no site file replaces; a file
   naming no fusion is taken as it is and said to be unchecked.

The site file is configuration, not a secret: the descriptor declares it
``sweep: false``, so the engine keeps it where secrets are kept but does
not remove the tables, which echo its digest and site lines. An engine
released before that flag (before 1.0.0-beta.3) sweeps it as a secret,
line by line, which is why ``calibrate`` writes the file as one line: the
whole document is then its only needle, and no table holds it. A refusal
names a row by its line number, never by its content.
"""

from __future__ import annotations

import csv
import dataclasses
import json
import math
import os
import sys
import time
from pathlib import Path

import numpy as np

from . import __version__
from .evidence import MAKES
from .fusion import Fusion

LABELS = {"given": 1, "not_given": 0}
SOURCES = ("header", "name", "manual")
FLOOR = 20
ENV = "NILS_PC_CALIBRATION"


class CalibrationError(ValueError):
    pass


# ---------------------------------------------------------------- the pairs

def site_key(site: str) -> tuple:
    """A site key ``<cohort> <make>`` as a run spells it, as ``(cohort,
    make)``: trimmed, the make word taken without case and written as
    ``evidence.make_of`` spells it (``MAKES``); a make word the fusion does
    not know is refused. Every key, a pair file's, a map's or
    ``calibrate``'s, goes through here, so the run's lookup, which is exact,
    finds it."""
    cohort, _, make = site.strip().rpartition(" ")
    if not cohort.strip() or not make:
        raise CalibrationError("the site key is <cohort> <make>, the make as the tables spell it")
    canon = {m.lower(): m for m in MAKES}.get(make.lower())
    if canon is None:
        raise CalibrationError("the site key's make is none the fusion knows: " + ", ".join(MAKES))
    return cohort.strip(), canon


def _key_of(site: str) -> str:
    """A pair file's or a map's key through ``site_key``, refused by one
    line naming it as the file spells it."""
    try:
        cohort, make = site_key(site)
    except CalibrationError as e:
        raise CalibrationError(f"the site {site!r}: {e}") from None
    return f"{cohort} {make}"


def _same_key(site: str, key: str) -> bool:
    """Whether a site, as a file spells it, is the canonical key."""
    try:
        return _key_of(site) == key
    except CalibrationError:
        return False


def _read_file(path) -> tuple:
    """(the site pairs of a calibration file by their canonical key, the
    fusion digests it names): one pair file (``site``, ``a``, ``b``, its
    ``model.fusion_digest``) or a map of ``{site: [a, b]}`` (a value may
    also be a pair file's object, whose own ``site``, where it is a string,
    must be the key's). Every key goes through ``site_key``. The file is
    read as JSON whatever its layout: ``calibrate`` writes it as one line,
    and a pretty-printed one reads the same."""
    try:
        d = json.loads(Path(path).read_text(encoding="utf-8-sig"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as e:
        raise CalibrationError("the calibration file is not readable JSON") from e
    if not isinstance(d, dict) or not d:
        raise CalibrationError("the calibration file holds no site pair")
    digests: list = []

    def digest_of(obj: dict) -> None:
        m = obj.get("model")
        if isinstance(m, dict) and isinstance(m.get("fusion_digest"), str) and m["fusion_digest"] not in digests:
            digests.append(m["fusion_digest"])

    if {"site", "a", "b"} <= set(d):
        if not isinstance(d["site"], str):
            raise CalibrationError("the pair file's site is not a string (the site key, <cohort> <make>)")
        digest_of(d)
        d = {d["site"]: [d["a"], d["b"]]}
    out: dict = {}
    for site, v in d.items():
        if not (isinstance(site, str) and site.strip()):
            raise CalibrationError(f"the pair of site {site!r} is not under a site key, <cohort> <make>")
        key = _key_of(site)
        if isinstance(v, dict):
            own = v.get("site")
            # a pair file under a map key is that key's: one naming another site itself is refused
            if isinstance(own, str) and not (own.strip() and _same_key(own, key)):
                raise CalibrationError(f"the pair under the key {site!r} names another site itself")
            digest_of(v)
            v = [v.get("a"), v.get("b")]
        # a value is [a, b], two numbers, or a pair file's object; nothing else
        if not isinstance(v, (list, tuple)) or len(v) != 2 or not all(isinstance(x, (int, float)) and not isinstance(x, bool) for x in v):
            raise CalibrationError(f"the pair of site {site!r} is not a list of two numbers, [a, b]")
        a, b = float(v[0]), float(v[1])
        if not (math.isfinite(a) and math.isfinite(b)):
            raise CalibrationError(f"the pair of site {site!r} is not two finite numbers")
        if key in out:
            raise CalibrationError(f"the calibration file names the site {key!r} twice")
        out[key] = (a, b)
    return out, digests


def load_pairs(path, fusion_digest=None) -> tuple:
    """(the site pairs of a calibration file, whether they were held to the
    run's fusion): ``_read_file``'s pairs; with the run's fusion digest, a
    file fitted on another fusion is refused, and a file naming none, or a
    call giving no digest to hold it to, is unchecked (False)."""
    pairs, digests = _read_file(path)
    if fusion_digest is not None and any(d != fusion_digest for d in digests):
        raise CalibrationError("the calibration file was fitted on another fusion than this run's: one pair fits one model")
    return pairs, fusion_digest is not None and bool(digests)


def apply(F: Fusion, path, fusion_digest=None) -> tuple:
    """The fusion with the file's pairs added beside its own: (fusion, the
    sites the file set, whether the file was held to the run's fusion,
    ``load_pairs``). The file's keys are canonical (``site_key``) and their
    cohorts go through the fusion's cohort aliases, as a stack's do, so the
    guard and the run's lookup compare like with like. A file naming a site
    the frozen fusion has a pair for is refused: a certified pair is never
    replaced. The fusion keeps its own sites apart (``frozen_keys``), so
    ``evidence.cohort_of`` resolves a stack against them first and against
    the file's cohorts only where they name none."""
    pairs, checked = load_pairs(path, fusion_digest)
    pairs = _aliased(pairs, F.cohort_aliases)
    frozen = sorted(set(pairs) & F.frozen_sites())
    if frozen:
        raise CalibrationError("the calibration file names a site the frozen fusion is certified for, which no site file replaces: "
                               + ", ".join(frozen))
    cal = dict(F.cal)
    cal.update(pairs)
    G = dataclasses.replace(F, cal=cal, frozen_keys=frozenset(F.frozen_sites()))
    return G, sorted(pairs), checked


def _aliased(pairs: dict, aliases: dict) -> dict:
    """The pairs under the keys a run looks a stack's site up by: each
    key's cohort through the fusion's cohort aliases. Two keys that meet
    under one alias are refused."""
    out: dict = {}
    for key, ab in pairs.items():
        cohort, make = key.rsplit(" ", 1)
        k2 = f"{aliases.get(cohort, cohort)} {make}"
        if k2 in out:
            raise CalibrationError(f"the calibration file names the site {k2!r} twice through the fusion's cohort aliases")
        out[k2] = ab
    return out


def for_run(F: Fusion, path, extra: dict, fusion_digest=None) -> Fusion:
    """The fusion a run scores with: its own pairs, with a calibration
    file's added beside them where there is one, the file ``--calibration``
    names, else the one the engine mounted as the descriptor's site file
    (``NILS_PC_CALIBRATION``). A file whose ``model.fusion_digest`` names
    another fusion than the run's is refused, as is one naming a site the
    frozen fusion has a pair for (``apply``). The run records in its
    results (``calibration``) the sites the file set, each with the stacks
    the pair calibrated once ``record`` has counted them, and ``unchecked``
    where the file named no fusion; never the pairs."""
    path = path or os.environ.get(ENV) or None
    if not path:
        return F
    G, sites, checked = apply(F, path, fusion_digest)
    extra["calibration"] = {"sites": {s: 0 for s in sites}}
    if not checked:
        extra["calibration"]["unchecked"] = True
    return G


def record(extra: dict, sites_of: dict, tables: dict) -> None:
    """After the run: per site key of the file, the stacks the pair
    calibrated (``calibration.sites: {key: n}``, a stack with a probability
    from it), and ``calibration.matched_none: [keys]`` where no key was the
    site of any stack of the run, which is no refusal."""
    cal = extra.get("calibration")
    if not cal:
        return
    matched: set = set()
    for sid, doc in tables.items():
        site = sites_of.get(sid)
        if site in cal["sites"]:
            matched.add(site)
            if doc.get("p_given") is not None:
                cal["sites"][site] += 1
    if not matched:
        cal["matched_none"] = sorted(cal["sites"])


def add_run_options(c) -> None:
    """The options the cascade and the full pipeline share for a site's
    calibration (0.3.0)."""
    c.add_argument("--raw-scores", action="store_true",
                   help="score T1-type brain stacks at a site with no calibration too and write their fused log-odds (score_raw), "
                        "never a probability or a proposal, for the site to fit its pair on")
    c.add_argument("--calibration", default=None,
                   help="a JSON file of site pairs ({site: [a, b]}, or one pair file from calibrate) added beside the fusion's (a site the fusion has a pair for is refused); "
                        f"default: the file ${ENV} names, where the engine mounted the site file")


# ---------------------------------------------------------------- the labels and the scores

def read_labels(path) -> dict:
    """The label rows by stack id: ``{stack: (label 0/1, source)}``. The
    file is tab-separated with a header naming ``stack``, ``label`` and
    ``source``; a bad row is refused by its line number."""
    try:
        # utf-8-sig: a spreadsheet's export may start with a byte order mark
        text = Path(path).read_text(encoding="utf-8-sig")
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


def _tables(path: Path) -> list:
    """[(file, table)] of each table under a path: a run's output folder
    (its ``postcontrast/<stack>.json``) or one file. A path with no
    post-contrast table under it is refused, never read as an empty run."""
    if path.is_dir():
        sub = path / "postcontrast"
        folder = sub if sub.is_dir() else path
        files = sorted(p for p in folder.glob("*.json") if p.name != "results.json")
    else:
        files = [path]
    out = []
    for f in files:
        try:
            doc = json.loads(f.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as e:
            raise CalibrationError(f"the scores file {f} is not readable JSON") from e
        if isinstance(doc, dict) and "stack_id" in doc:
            out.append((f, doc))
    if not out:
        raise CalibrationError(f"no post-contrast tables under {path}")
    return out


def _candidates_of(f: Path, doc: dict) -> list:
    """The ``cohort_candidates`` column of a table as the names it lists (a
    JSON list in the text column; a run writes it on every row it scores),
    no names where it is null; anything else is refused by the file's
    path."""
    v = doc.get("cohort_candidates")
    if v is None:
        return []
    try:
        names = json.loads(v) if isinstance(v, str) else v
    except json.JSONDecodeError:
        names = None
    if not isinstance(names, list) or not all(isinstance(c, str) for c in names):
        raise CalibrationError(f"the table {f} has a cohort_candidates column that is not a JSON list of names")
    return names


def read_scores(paths) -> dict:
    """The raw scores by stack id from the raw-score mode's outputs: each
    path a run's output folder (its ``postcontrast/<stack>.json`` tables)
    or one table. ``{stack: {"z", "site", "make", "cohort_candidates",
    "fusion_digest", "cascade_digest"}}`` (``cohort_candidates`` the
    column's names); a stack in several outputs keeps the last. A path with
    no table under it, and a table that is not what a run writes (a stack
    id that is not an integer, a score that is not a number), are refused
    by the path, never by a value."""
    out: dict = {}
    for p in paths:
        p = Path(p)
        if not p.exists():
            raise CalibrationError(f"the scores path {p} does not exist")
        for f, doc in _tables(p):
            z = doc.get("score_raw")
            if z is None:
                continue
            sid = doc["stack_id"]
            if isinstance(sid, bool) or not isinstance(sid, int):
                raise CalibrationError(f"the table {f} has no integer stack_id")
            try:
                z = float(z)
            except (TypeError, ValueError) as e:
                raise CalibrationError(f"the table {f} has a score_raw that is not a number") from e
            out[sid] = {"z": z, "site": doc.get("site"), "make": doc.get("make"), "cohort_candidates": _candidates_of(f, doc),
                        "fusion_digest": doc.get("fusion_digest"), "cascade_digest": doc.get("cascade_digest")}
    return out


# ---------------------------------------------------------------- the fit

def fit_platt(z: np.ndarray, y: np.ndarray, iters: int = 200) -> tuple:
    """Platt's pair (a, b) for p = sigmoid(a z + b), and its fit: the
    logistic regression of the labels on z with Platt's smoothed targets
    ((n+ + 1) / (n+ + 2) for a positive, 1 / (n- + 2) for a negative), by
    Newton's method with halving. A candidate is adopted only when it lowers
    the mean loss; when no halving does, the fit stops where it stands,
    converged if the gradient is flat there and stuck otherwise. Returns
    ``(a, b, {"iterations": n, "converged": bool})``."""
    z = np.asarray(z, np.float64)
    y = np.asarray(y, np.float64)
    n = max(1, len(z))
    npos, nneg = float(y.sum()), float((1 - y).sum())
    t = np.where(y > 0.5, (npos + 1) / (npos + 2), 1 / (nneg + 2))
    X = np.column_stack([z, np.ones_like(z)])
    w = np.zeros(2)

    def loss(w):
        f = X @ w
        return float(np.sum(np.logaddexp(0, f) - t * f)) / n

    cur = loss(w)
    done, converged = 0, False
    while done < iters:
        p = 1 / (1 + np.exp(-(X @ w)))
        g = X.T @ (p - t) / n
        flat = float(np.max(np.abs(g))) < 1e-8
        if flat:
            converged = True
            break
        H = (X * (p * (1 - p))[:, None]).T @ X / n + 1e-12 * np.eye(2)
        step = np.linalg.solve(H, g)
        better = None
        s = 1.0
        while s > 1e-6:
            cand = w - s * step
            new = loss(cand)
            if new < cur:
                better = (cand, new)
                break
            s /= 2
        if better is None:
            # no halving lowers the loss: the fit stands here, converged where
            # the gradient is flat at working precision, stuck otherwise
            converged = float(np.max(np.abs(g))) < 1e-6
            break
        done += 1
        moved = float(np.max(np.abs(better[0] - w)))
        w, cur = better
        if moved < 1e-9:
            converged = True
            break
    return float(w[0]), float(w[1]), {"iterations": done, "converged": converged}


def _measures(z: np.ndarray, y: np.ndarray, a: float, b: float) -> dict:
    p = np.clip(1 / (1 + np.exp(-(a * z + b))), 1e-12, 1 - 1e-12)
    ll = -float(np.mean(y * np.log(p) + (1 - y) * np.log(1 - p)))
    acc = float(np.mean((p >= 0.5) == (y > 0.5)))
    return {"log_loss": round(ll, 6), "accuracy": round(acc, 6)}


def calibrate(labels: dict, scores: dict, site: str) -> dict:
    """The site's pair document from the labels and the raw scores: the
    fit, its counts and its measures overall and per label source. The key
    is written as the run spells it (``site_key``). Rows with no score,
    rows whose table names another site, rows of another make than the
    key's (compared without case) and rows whose ``cohort_candidates`` do
    not name the key's cohort are counted and left out; a row whose table
    names the key itself as its site needs no candidates check (the site
    proves the cohort). Fewer than 20 rows of a class refuse, and the
    refusal says how many rows were left out and why, so a mis-spelled or
    alias-spelled key is explained."""
    key_cohort, make = site_key(site)
    site = f"{key_cohort} {make}"
    key_make = make.lower()
    zs, ys, srcs = [], [], []
    n_without_score = n_other_site = n_other_make = n_other_cohort = 0
    digests = {"fusion": set(), "cascade": set()}
    for sid, (y, src) in labels.items():
        r = scores.get(sid)
        if r is None:
            n_without_score += 1
            continue
        if r["site"] is not None and r["site"] != site:
            n_other_site += 1
            continue
        if str(r.get("make") or "").strip().lower() != key_make:
            n_other_make += 1
            continue
        if r["site"] != site and key_cohort not in r["cohort_candidates"]:
            # the table names no site: the key's cohort must be among the
            # stack's candidates (the run writes them alias-resolved)
            n_other_cohort += 1
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
        left = [(n_without_score, "labelled with no score"), (n_other_site, "at another site"), (n_other_make, "of another make"),
                (n_other_cohort, "of another cohort")]
        total = sum(n for n, _ in left)
        why = (f"{total} row{'s were' if total != 1 else ' was'} left out: " + ", ".join(f"{n} {w}" for n, w in left if n)) if total \
            else "no row was left out"
        raise CalibrationError(f"a site's pair needs at least {FLOOR} labelled stacks of each class with a score: "
                               f"{n_given} given and {n_not} not_given here; {why}")
    a, b, fit = fit_platt(z, y)
    per: dict = {}
    for s in SOURCES:
        m = np.array([x == s for x in srcs], bool)
        if m.any():
            per[s] = {"n": int(m.sum()), "n_given": int(y[m].sum()), **_measures(z[m], y[m], a, b)}
    return {"site": site, "a": round(a, 8), "b": round(b, 8), "n": int(len(y)), "n_given": n_given, "n_not_given": n_not,
            "n_without_score": n_without_score, "n_other_site": n_other_site, "n_other_make": n_other_make, "n_other_cohort": n_other_cohort,
            "fit": fit,
            "per_source": per, "overall": _measures(z, y, a, b),
            "model": {"fusion_digest": next(iter(digests["fusion"]), None), "cascade_digests": sorted(digests["cascade"]),
                      "image": "nils-postcontrast", "version": __version__},
            "fitted_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}


def report(doc: dict) -> str:
    lines = [f"site {doc['site']}: a = {doc['a']:.6f}, b = {doc['b']:.6f}",
             f"{doc['n']} stacks fitted ({doc['n_given']} given, {doc['n_not_given']} not_given); "
             f"{doc['n_without_score']} labelled with no score, {doc['n_other_site']} at another site, {doc['n_other_make']} of another make, "
             f"{doc['n_other_cohort']} of another cohort",
             f"fit: {doc['fit']['iterations']} iterations, {'converged' if doc['fit']['converged'] else 'stuck before it converged'}",
             f"overall: log loss {doc['overall']['log_loss']:.4f}, accuracy {doc['overall']['accuracy']:.4f}"]
    for s, m in doc["per_source"].items():
        lines.append(f"  {s}: {m['n']} stacks ({m['n_given']} given), log loss {m['log_loss']:.4f}, accuracy {m['accuracy']:.4f}")
    return "\n".join(lines)


def unusable(doc: dict) -> str | None:
    """Why a fitted pair must not be written, or None: the fit stopped
    before it converged (stuck), or its slope ``a`` is not above 0, a
    degenerate or inverted pair under which a higher score would not mean
    a higher probability. There is no override: the labels or the scores
    are what to look at."""
    if not doc["fit"]["converged"]:
        return f"it stopped before it converged, stuck after {doc['fit']['iterations']} iterations"
    if not doc["a"] > 0:
        return f"its slope a is not above 0 (a = {doc['a']:.6f}), a degenerate or inverted pair"
    return None


def cmd_calibrate(a) -> int:
    try:
        labels = read_labels(a.labels)
        scores = read_scores(a.scores)
        doc = calibrate(labels, scores, a.site)
        why = unusable(doc)
        if why:
            raise CalibrationError(f"the fit of site {doc['site']} is unusable and nothing was written: {why}")
    except CalibrationError as e:
        print(f"nils-postcontrast calibrate: {e}", file=sys.stderr)
        return 1
    out = Path(a.out)
    try:
        out.parent.mkdir(parents=True, exist_ok=True)
        # one line, compact: an engine released before sweep: false (1.0.0-beta.3)
        # sweeps the site file as a secret, each of its lines a needle, and
        # would remove every table echoing one; a whole document on one line
        # is a needle no table holds. A reader takes either layout.
        out.write_text(json.dumps(doc, separators=(",", ":")) + "\n", encoding="utf-8")
    except OSError as e:
        print(f"nils-postcontrast calibrate: the pair file {out} cannot be written ({e.strerror or e.__class__.__name__})", file=sys.stderr)
        return 1
    print(report(doc))
    return 0


def add_parser(sub) -> None:
    c = sub.add_parser("calibrate", help="fit a site's Platt pair on the raw scores a run wrote and the site's labels")
    c.add_argument("--labels", required=True, help="a tab-separated file with a header: stack, label (given or not_given), source (header, name, manual)")
    c.add_argument("--scores", nargs="+", required=True, help="the output folders (or tables) of runs with --raw-scores")
    c.add_argument("--site", required=True, help="the site key the pair is for, <cohort> <make> (the make as the tables spell it, taken "
                                                 "without case; a scored stack's cohort_candidates column names the cohorts the key may use)")
    c.add_argument("--out", required=True, help="the pair file to write")
    c.set_defaults(fn=cmd_calibrate)
