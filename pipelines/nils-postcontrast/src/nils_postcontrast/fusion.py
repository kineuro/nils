# SPDX-License-Identifier: AGPL-3.0-only
"""Model r1's frozen fusion (record 42's post-contrast model, round 1), from
its JSON form (``fusion.json``), and the cascade's margin rule.

The fusion is a logistic regression over 20 columns, standardised: for each
image arm (T, U, V) its logit (0 where the arm is missing) and a missing
flag; the eight header columns; a masked flag; the ratio-pair tier (score,
regions up, regions down, has a pair) and its masked flag. Its score is
calibrated per site (``<cohort> <make>``) by a Platt pair. A stack is
answered where the calibrated probability is at least t or at most 1 - t,
on brain; "not sure" elsewhere, and where the site has no calibration.

The arithmetic follows the study's code: the arm probabilities and their
logits are float32 (the student's outputs are), everything after them
float64.
"""

from __future__ import annotations

import json
import math
from dataclasses import dataclass
from pathlib import Path

import numpy as np

HCOLS = ["h_agent", "h_words", "h_rules_given", "h_ses_any_agent", "h_ses_all_agent", "h_at_or_after_first_tag", "h_before_first_tag",
         "h_time_trusted"]
PCOLS = ["pair_s", "pair_up", "pair_down"]
CODES = {"not sure": 0, "given": 1, "not given": 2}


class FusionError(ValueError):
    pass


@dataclass
class Fusion:
    cols: list
    mean: np.ndarray
    scale: np.ndarray
    coef: np.ndarray
    intercept: float
    cal: dict
    t: float
    use_V: bool
    cohort_aliases: dict
    untrusted_time_cohorts: list
    # the sites the frozen fusion carries pairs for, where a site file's pairs
    # were merged into cal over them (calibration.apply); None: all of cal
    frozen_keys: frozenset | None = None

    @classmethod
    def load(cls, path) -> "Fusion":
        try:
            d = json.loads(Path(path).read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as e:
            raise FusionError("the fusion file is not readable JSON") from e
        try:
            f = cls(cols=list(d["cols"]), mean=np.array(d["scaler"]["mean"], np.float64), scale=np.array(d["scaler"]["scale"], np.float64),
                    coef=np.array(d["head"]["coef"], np.float64), intercept=float(d["head"]["intercept"]),
                    cal={k: (float(v[0]), float(v[1])) for k, v in d["calibration"].items()}, t=float(d["t"]), use_V=bool(d["use_V"]),
                    cohort_aliases=dict(d.get("cohort_aliases") or {}), untrusted_time_cohorts=list(d.get("untrusted_time_cohorts") or []))
        except (KeyError, TypeError, ValueError) as e:
            raise FusionError(f"the fusion file lacks {e}") from e
        n = len(f.cols)
        if not (len(f.mean) == len(f.scale) == len(f.coef) == n):
            raise FusionError("the fusion file's scaler and head do not match its columns")
        return f

    def sites(self) -> set:
        return set(self.cal)

    def frozen_sites(self) -> set:
        """The sites the frozen fusion itself has a pair for; a site file's
        merged pairs (``calibration.apply``) are the rest of ``sites()``."""
        return set(self.frozen_keys) if self.frozen_keys is not None else set(self.cal)


def logit32(p):
    """pc_swap.logit, in the dtype it is given."""
    p = np.clip(p, 1e-4, 1 - 1e-4)
    return np.log(p / (1 - p))


def arm_probs(o: np.ndarray) -> dict:
    """A seed's (or the mean's) six outputs, (n, 6) float32, as arm
    probabilities: sigmoid of the logit where the availability logit is
    above 0, NaN elsewhere (float32, as the study's)."""
    return {k: np.where(o[:, 3 + j] > 0, 1 / (1 + np.exp(-o[:, j])), np.nan) for j, k in enumerate("TUV")}


def log_odds(F: Fusion, rows: list, P: dict) -> np.ndarray:
    """Each row's fused log-odds z before the site's calibration: the
    standardised columns through the head. The raw score a site fits its
    Platt pair on (``nils-postcontrast calibrate``).

    ``rows`` are dicts with the header columns, the pair tier (``has_pair``
    and its columns), ``site`` and ``brain``; ``P`` the arm probabilities
    (NaN = missing)."""
    n = len(rows)
    Z: dict = {}
    for k in ["T", "U"] + (["V"] if F.use_V else []):
        p = P[k]
        Z["z" + k] = logit32(np.where(np.isnan(p), np.float32(0.5), p).astype(p.dtype)).astype(np.float64)
        Z["miss" + k] = np.isnan(p).astype(np.float64)
    for c in HCOLS:
        Z[c] = np.array([_num(r.get(c)) for r in rows], np.float64)
    Z["header_masked"] = np.zeros(n)
    hp = np.array([r.get("has_pair") == 1 for r in rows], bool)
    for c in PCOLS:
        Z[c] = np.where(hp, np.array([_num(r.get(c)) for r in rows], np.float64), 0.0)
    Z["has_pair"] = hp.astype(np.float64)
    Z["pair_masked"] = np.zeros(n)
    try:
        X = np.column_stack([Z[c] for c in F.cols]).astype(np.float64)
    except KeyError as e:
        raise FusionError(f"the fusion reads a column {e} this image does not make") from e
    return ((X - F.mean) / F.scale) @ F.coef + F.intercept


def calibrated(F: Fusion, rows: list, z: np.ndarray):
    """The calibrated probability and the answer of each row from its
    log-odds: the site's Platt pair, then the threshold on brain; NaN and
    "not sure" where the site has no pair."""
    a_ = np.array([F.cal[r.get("site")][0] if r.get("site") in F.cal else np.nan for r in rows])
    b_ = np.array([F.cal[r.get("site")][1] if r.get("site") in F.cal else np.nan for r in rows])
    p = 1 / (1 + np.exp(-(a_ * z + b_)))
    brain = np.array([bool(r.get("brain")) for r in rows], bool)
    ans = ((p >= F.t) | (p <= 1 - F.t)) & brain & ~np.isnan(p)
    return p, np.where(ans, np.where(p >= 0.5, "given", "not given"), "not sure")


def fuse(F: Fusion, rows: list, P: dict):
    """The calibrated probability and the answer of each row
    (``log_odds`` then ``calibrated``)."""
    return calibrated(F, rows, log_odds(F, rows, P))


def _num(v) -> float:
    try:
        x = float(v)
    except (TypeError, ValueError):
        return 0.0
    return 0.0 if math.isnan(x) else x


def _logit64(x: float) -> float:
    x = min(max(x, 1e-4), 1 - 1e-4)
    return math.log(x / (1 - x))


def margin(p: float, t: float) -> float:
    """The mean seed's log-odds distance from the nearer threshold (the
    study's ``lo``): 0 where there is no probability."""
    if p != p:
        return 0.0
    x = min(max(p, 1e-9), 1 - 1e-9)
    return abs(_logit64(x) - (_logit64(t) if x >= 0.5 else -_logit64(t)))


@dataclass
class Stage1:
    """One stack's stage-1 outcome."""

    p: float
    answer: str
    seeds: list
    agree: bool
    margin: float
    accepted: bool
    # the mean's fused log-odds before the site's calibration and each
    # seed's (0.3.0, the raw-score mode)
    z: float = float("nan")
    seeds_z: list | None = None


def stage1(F: Fusion, rows: list, out: np.ndarray, m: float) -> list:
    """Each row's stage-1 outcome from the student's (n, seeds, 6) outputs:
    every seed fused, the mean of the seeds' outputs fused, accepted where
    the seeds all give the mean's answer and its margin is at least m."""
    zs = [log_odds(F, rows, arm_probs(out[:, s].astype(np.float32))) for s in range(out.shape[1])]
    seeds = [calibrated(F, rows, z) for z in zs]
    om = np.nanmean(out.astype(np.float32), 1)
    zm = log_odds(F, rows, arm_probs(om))
    pm, am = calibrated(F, rows, zm)
    res = []
    for i in range(len(rows)):
        sa = [str(s[1][i]) for s in seeds]
        agree = all(a == am[i] for a in sa)
        lo = margin(float(pm[i]), F.t)
        res.append(Stage1(float(pm[i]), str(am[i]), sa, agree, lo, bool(agree and lo >= m), float(zm[i]), [float(z[i]) for z in zs]))
    return res
