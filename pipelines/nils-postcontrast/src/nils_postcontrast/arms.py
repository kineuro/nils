# SPDX-License-Identifier: AGPL-3.0-only
"""Model r1's three frozen image arms (record 42's post-contrast model,
round 1), from their JSON form (``arms.json``), as the full pipeline
(``postcontrast-full``) reads them.

- **T**, the intensity tail: a random forest (300 trees) over five tail
  features of the skull-stripped stack, answered only on 3D gradient-echo
  T1 (its domain) where all five exist;
- **U**, the regions: a logistic regression over the template regions'
  intensities (the source ``HP``), behind a median imputer and a scaler,
  answered where the regions are usable;
- **V**, the structures and vessels: the same over the stack's own
  structures and the vessel filter, answered where the white matter holds
  more than 1,000 voxels.

The arithmetic follows scikit-learn's as the study ran it: a tree compares
the features as float32 against its float64 thresholds, and each tree's
leaf counts are normalised to probabilities before the forest's mean; the
regressions run in float64.
"""

from __future__ import annotations

import json
import math
from dataclasses import dataclass
from pathlib import Path

import numpy as np

T_DOMAIN = {"MPRAGE", "MEMPRAGE", "MP2RAGE", "FLASH", "TurboFLASH", "VIBE", "GRE"}


class ArmsError(ValueError):
    pass


@dataclass
class Tree:
    left: np.ndarray
    right: np.ndarray
    feature: np.ndarray
    threshold: np.ndarray
    missing_left: np.ndarray
    p1: np.ndarray  # each node's probability of class 1, as the tree's predict_proba gives it at a leaf

    def predict(self, X: np.ndarray) -> np.ndarray:
        """Class 1's probability of each row of X (float32)."""
        out = np.empty(len(X), np.float64)
        for i in range(len(X)):
            n = 0
            while self.left[n] != -1:
                v = X[i, self.feature[n]]
                if v != v:
                    go_left = bool(self.missing_left[n])
                else:
                    go_left = v <= self.threshold[n]
                n = self.left[n] if go_left else self.right[n]
            out[i] = self.p1[n]
        return out


@dataclass
class Forest:
    cols: list
    trees: list

    def predict(self, X: np.ndarray) -> np.ndarray:
        X = np.asarray(X, np.float64).astype(np.float32)
        acc = np.zeros(len(X), np.float64)
        for t in self.trees:
            acc += t.predict(X)
        return acc / len(self.trees)


@dataclass
class Logistic:
    cols: list
    fill: np.ndarray  # the imputer's medians, NaN for a column it dropped
    mean: np.ndarray
    scale: np.ndarray
    coef: np.ndarray
    intercept: float

    def predict(self, X: np.ndarray) -> np.ndarray:
        X = np.array(X, np.float64)
        keep = ~np.isnan(self.fill)
        X = X[:, keep]
        fill = self.fill[keep]
        miss = np.isnan(X)
        X[miss] = np.broadcast_to(fill, X.shape)[miss]
        z = ((X - self.mean) / self.scale) @ self.coef + self.intercept
        return 1.0 / (1.0 + np.exp(-z))


@dataclass
class Arms:
    T: Forest
    U: Logistic
    V: Logistic
    u_source: str
    version: str
    source_sha256: str

    @classmethod
    def load(cls, path) -> "Arms":
        try:
            d = json.loads(Path(path).read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as e:
            raise ArmsError("the arms file is not readable JSON") from e
        try:
            t = d["T"]
            if t["kind"] != "forest":
                raise ArmsError("arm T is not a forest")
            trees = []
            for tr in t["trees"]:
                trees.append(Tree(np.array(tr["left"], np.int64), np.array(tr["right"], np.int64), np.array(tr["feature"], np.int64),
                                  np.array(tr["threshold"], np.float64), np.array(tr.get("missing_left") or [0] * len(tr["left"]), np.int8),
                                  np.array(tr["p1"], np.float64)))
            if not trees:
                raise ArmsError("arm T has no trees")
            arms = {}
            for k in ("U", "V"):
                a = d[k]
                if a["kind"] != "logistic":
                    raise ArmsError(f"arm {k} is not a logistic regression")
                fill = np.array([np.nan if v is None else v for v in a["fill"]], np.float64)
                lg = Logistic(list(a["cols"]), fill, np.array(a["mean"], np.float64), np.array(a["scale"], np.float64),
                              np.array(a["coef"], np.float64), float(a["intercept"]))
                kept = int((~np.isnan(fill)).sum())
                if len(lg.cols) != len(fill) or not (len(lg.mean) == len(lg.scale) == len(lg.coef) == kept):
                    raise ArmsError(f"arm {k}'s imputer, scaler and head do not match its columns")
                arms[k] = lg
            out = cls(Forest(list(t["cols"]), trees), arms["U"], arms["V"], str(d.get("u_source") or ""), str(d.get("version") or ""),
                      str(d.get("source_sha256") or ""))
        except (KeyError, TypeError, ValueError) as e:
            if isinstance(e, ArmsError):
                raise
            raise ArmsError(f"the arms file lacks {e}") from e
        if out.u_source != "HP":
            raise ArmsError("arm U reads a region source this image does not make")
        for tr in out.T.trees:
            n = len(tr.left)
            if not (len(tr.right) == len(tr.feature) == len(tr.threshold) == len(tr.p1) == n):
                raise ArmsError("a tree of arm T is malformed")
            if tr.feature.max(initial=0) >= len(out.T.cols):
                raise ArmsError("a tree of arm T reads a column it does not name")
        return out

    def probabilities(self, rows: list) -> dict:
        """Each arm's probability for each row (a dict of the stack's feature
        columns, ``t_domain`` and the usable flags); NaN where the arm is not
        available, as the study's ``predict`` gave it."""
        n = len(rows)
        out = {k: np.full(n, np.nan) for k in "TUV"}

        def matrix(cols, idx):
            return np.array([[_num(rows[i].get(c)) for c in cols] for i in idx], np.float64).reshape(len(idx), len(cols))

        t_idx = [i for i, r in enumerate(rows) if _true(r.get("t_domain")) and all(_finite(r.get(c)) for c in self.T.cols)]
        u_idx = [i for i, r in enumerate(rows) if r.get(f"{self.u_source}_usable") == 1]
        v_idx = [i for i, r in enumerate(rows) if r.get("V_ok") == 1]
        if t_idx:
            out["T"][t_idx] = self.T.predict(matrix(self.T.cols, t_idx))
        if u_idx:
            out["U"][u_idx] = self.U.predict(matrix(self.U.cols, u_idx))
        if v_idx:
            out["V"][v_idx] = self.V.predict(matrix(self.V.cols, v_idx))
        return out


def _num(v) -> float:
    if v is None:
        return float("nan")
    try:
        return float(v)
    except (TypeError, ValueError):
        return float("nan")


def _true(v) -> bool:
    """A flag that is set: True or 1, never a missing value."""
    if v is None or isinstance(v, str):
        return False
    try:
        return bool(v) and v == v
    except (TypeError, ValueError):
        return False


def _finite(v) -> bool:
    x = _num(v)
    return not math.isnan(x)
