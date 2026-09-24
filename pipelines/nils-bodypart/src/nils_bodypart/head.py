# SPDX-License-Identifier: AGPL-3.0-only
"""The per-task head: scaler, PCA chosen by cross-validation, a classifier,
and a calibration on every head.

Ported from v0 (``train.py``), with what record 43 adds:

- v0's pipeline is kept: ``StandardScaler``, optional ``PCA``, then balanced
  logistic regression (the default), a random forest, or an RBF SVM. With
  at least 20 samples the PCA size and the hyperparameters are chosen by
  stratified cross-validation over v0's grids (PCA from {none, 64, 128, 256,
  512, D} within what a fold can hold, C from 0.01 to 100 for logistic
  regression); below 20 v0's fixed PCA of 128, clamped, is used.
- **Every head is calibrated**, not only the SVM. Logistic regression gets
  temperature scaling, one scalar fitted by log-loss on out-of-fold logits,
  so the argmax, and with it every label, is the uncalibrated head's. The
  random forest and the SVM are wrapped in ``CalibratedClassifierCV``
  (sigmoid, three folds), as v0 wrapped the SVM.
- The metrics are cross-validated: accuracy, expected calibration error and
  the Brier score, before and after calibration where a head has a before.
  The temperature of each outer fold is fitted inside that fold, so the
  calibrated numbers are not fitted on what they are measured on.
- A logistic-regression head is written as JSON coefficients, the plain
  format the model contract asks of a head (a pickle executes code when it is
  loaded). A forest or an SVM cannot be, and is written with joblib; the card
  says so, and inference refuses it unless asked to trust a pickle.
"""

from __future__ import annotations

import hashlib
import json
import logging
from dataclasses import dataclass, field
from typing import Any

import numpy as np

logger = logging.getLogger(__name__)

ESTIMATOR_KINDS = ("logreg", "rf", "svm")
HEAD_FORMAT = "nils-bodypart-head/1"


@dataclass(frozen=True)
class TrainConfig:
    pca_components: int | None = 128
    pca_floor: int = 16
    logreg_C: float = 1.0
    random_state: int = 0
    n_train_slices: int = 3
    auto_tune: bool = True
    estimator_kind: str = "logreg"
    ece_bins: int = 15


DEFAULT_TRAIN_CONFIG = TrainConfig()


def build_estimator(
    *,
    n_components: int | None,
    C: float,
    random_state: int,
    estimator_kind: str = "logreg",
    estimator_hyperparams: dict | None = None,
) -> Any:
    """v0's pipeline. The forest is calibrated here as the SVM always was."""
    from sklearn.calibration import CalibratedClassifierCV
    from sklearn.decomposition import PCA
    from sklearn.linear_model import LogisticRegression
    from sklearn.pipeline import Pipeline
    from sklearn.preprocessing import StandardScaler

    steps: list[tuple[str, Any]] = [("scale", StandardScaler(with_mean=True, with_std=True))]
    if n_components is not None:
        steps.append(("pca", PCA(n_components=n_components, random_state=random_state)))
    hp = estimator_hyperparams or {}
    if estimator_kind == "rf":
        from sklearn.ensemble import RandomForestClassifier

        rf = RandomForestClassifier(
            n_estimators=hp.get("n_estimators", 300),
            max_depth=hp.get("max_depth", None),
            class_weight="balanced",
            random_state=random_state,
            n_jobs=-1,
        )
        steps.append(("clf", CalibratedClassifierCV(rf, method="sigmoid", cv=3)))
    elif estimator_kind == "svm":
        from sklearn.svm import SVC

        svc = SVC(C=hp.get("C", C), kernel="rbf", gamma="scale", class_weight="balanced", random_state=random_state)
        steps.append(("clf", CalibratedClassifierCV(svc, method="sigmoid", cv=3)))
    elif estimator_kind == "logreg":
        steps.append(
            ("clf", LogisticRegression(C=C, class_weight="balanced", max_iter=2000, solver="lbfgs", random_state=random_state))
        )
    else:
        raise ValueError(f"unknown estimator kind {estimator_kind!r}")
    return Pipeline(steps)


def _raw_estimator(kind: str, **kw: Any) -> Any:
    """The forest before its calibration, for the before-and-after metrics."""
    est = build_estimator(estimator_kind=kind, **kw)
    if kind == "rf":
        est.steps[-1] = ("clf", est.steps[-1][1].estimator)
    return est


def n_folds_for(y: np.ndarray) -> int:
    _, counts = np.unique(y, return_counts=True)
    return max(2, min(5, int(counts.min())))


def auto_tune(
    X: np.ndarray, y: np.ndarray, *, random_state: int, pca_floor: int, estimator_kind: str = "logreg"
) -> tuple[int | None, dict, dict]:
    """v0's grid search: (best PCA, best hyperparameters, report). The PCA
    sizes are capped at a fold's training size, so no fold asks for more
    components than it has samples."""
    from sklearn.model_selection import StratifiedKFold, cross_val_score

    N, D = X.shape
    n_folds = n_folds_for(y)
    upper = min(N * (n_folds - 1) // n_folds, D)
    cv = StratifiedKFold(n_splits=n_folds, shuffle=True, random_state=random_state)
    if estimator_kind == "rf":
        pca_candidates: list[int | None] = [None]
        grid = [{"n_estimators": n, "max_depth": d} for n in (100, 300, 600) for d in (None, 10, 20)]
    elif estimator_kind == "svm":
        pca_candidates = [None] + sorted({c for c in (64, 128, 256) if pca_floor <= c <= upper})
        grid = [{"C": c} for c in (0.1, 1.0, 10.0)]
    else:
        pca_candidates = [None] + sorted({c for c in (64, 128, 256, 512, D) if pca_floor <= c <= upper})
        grid = [{"C": c} for c in (0.01, 0.1, 1.0, 10.0, 100.0)]

    best_score, best_pca, best_hp = -1.0, None, {}
    results: list[dict] = []
    for pca_n in pca_candidates:
        for hp in grid:
            est = build_estimator(
                n_components=pca_n,
                C=hp.get("C", 1.0),
                random_state=random_state,
                estimator_kind=estimator_kind,
                estimator_hyperparams=hp,
            )
            try:
                scores = cross_val_score(est, X, y, cv=cv, scoring="accuracy")
            except ValueError:
                continue
            mean = float(scores.mean())
            results.append({"pca": pca_n, **hp, "mean_cv_acc": round(mean, 4), "std_cv_acc": round(float(scores.std()), 4)})
            if mean > best_score:
                best_score, best_pca, best_hp = mean, pca_n, hp
    if not results:
        best_pca, best_hp = None, {"C": 1.0}
    report = {
        "grid": results,
        "best_pca": best_pca,
        "best_hp": best_hp,
        "best_cv_accuracy": round(best_score, 4),
        "estimator_kind": estimator_kind,
    }
    return best_pca, best_hp, report


# ------------------------------------------------------------- calibration


def softmax(z: np.ndarray) -> np.ndarray:
    z = z - z.max(axis=1, keepdims=True)
    e = np.exp(z)
    return e / e.sum(axis=1, keepdims=True)


def logits_of(estimator: Any, X: np.ndarray) -> np.ndarray:
    """A fitted logistic-regression pipeline's logits, two columns for a
    binary head ([0, z], whose softmax is its sigmoid)."""
    z = estimator.decision_function(X)
    if z.ndim == 1:
        z = np.stack([np.zeros_like(z), z], axis=1)
    return np.asarray(z, dtype=np.float64)


def fit_temperature(logits: np.ndarray, y_idx: np.ndarray) -> float:
    """The temperature T > 0 that minimises the log-loss of softmax(z / T)."""
    from scipy.optimize import minimize_scalar

    def nll(log_t: float) -> float:
        p = softmax(logits / np.exp(log_t))
        return float(-np.log(np.clip(p[np.arange(len(y_idx)), y_idx], 1e-12, None)).mean())

    res = minimize_scalar(nll, bounds=(np.log(0.05), np.log(20.0)), method="bounded")
    t = float(np.exp(res.x))
    # A temperature that does not beat T = 1 is not taken.
    return t if nll(res.x) < nll(0.0) else 1.0


def expected_calibration_error(probs: np.ndarray, y_idx: np.ndarray, n_bins: int = 15) -> float:
    """Top-label ECE over equal-width confidence bins."""
    conf = probs.max(axis=1)
    pred = probs.argmax(axis=1)
    correct = (pred == y_idx).astype(np.float64)
    edges = np.linspace(0.0, 1.0, n_bins + 1)
    ece = 0.0
    for lo, hi in zip(edges[:-1], edges[1:]):
        m = (conf > lo) & (conf <= hi) if lo > 0 else (conf >= lo) & (conf <= hi)
        if m.any():
            ece += float(m.mean()) * abs(float(correct[m].mean()) - float(conf[m].mean()))
    return ece


def brier_score(probs: np.ndarray, y_idx: np.ndarray) -> float:
    """The multi-class Brier score: the mean over samples of the squared
    distance between the probabilities and the one-hot truth."""
    onehot = np.zeros_like(probs)
    onehot[np.arange(len(y_idx)), y_idx] = 1.0
    return float(((probs - onehot) ** 2).sum(axis=1).mean())


def _metrics(probs: np.ndarray, y_idx: np.ndarray, bins: int) -> dict:
    return {
        "accuracy": round(float((probs.argmax(axis=1) == y_idx).mean()), 4),
        "ece": round(expected_calibration_error(probs, y_idx, bins), 4),
        "brier": round(brier_score(probs, y_idx), 4),
    }


def _oof_temperature(est_factory, X: np.ndarray, y: np.ndarray, classes: list[str], random_state: int) -> float:
    """The temperature fitted on out-of-fold logits of ``X``."""
    from sklearn.model_selection import StratifiedKFold

    idx = np.array([classes.index(v) for v in y])
    folds = StratifiedKFold(n_splits=n_folds_for(y), shuffle=True, random_state=random_state)
    z = np.zeros((len(y), len(classes)))
    for tr, te in folds.split(X, y):
        est = est_factory().fit(X[tr], y[tr])
        z[te] = _align(logits_of(est, X[te]), list(est.classes_), classes, fill=-1e9)
    return fit_temperature(z, idx)


def _align(cols: np.ndarray, have: list[str], want: list[str], fill: float = 0.0) -> np.ndarray:
    """Columns in ``have`` order put into ``want`` order (a fold may miss a class)."""
    if have == want:
        return cols
    out = np.full((cols.shape[0], len(want)), fill, dtype=np.float64)
    for j, c in enumerate(have):
        out[:, want.index(c)] = cols[:, j]
    return out


def cross_validated_metrics(
    est_factory, raw_factory, X: np.ndarray, y: np.ndarray, classes: list[str], *, kind: str, config: TrainConfig
) -> dict:
    """Out-of-fold accuracy, ECE and Brier, before and after calibration."""
    from sklearn.model_selection import StratifiedKFold

    y_idx = np.array([classes.index(v) for v in y])
    n_folds = n_folds_for(y)
    folds = StratifiedKFold(n_splits=n_folds, shuffle=True, random_state=config.random_state)
    raw = np.zeros((len(y), len(classes)))
    cal = np.zeros((len(y), len(classes)))
    temps: list[float] = []
    for tr, te in folds.split(X, y):
        if kind == "logreg":
            est = est_factory().fit(X[tr], y[tr])
            z = _align(logits_of(est, X[te]), list(est.classes_), classes, fill=-1e9)
            t = _oof_temperature(est_factory, X[tr], y[tr], classes, config.random_state)
            temps.append(t)
            raw[te] = softmax(z)
            cal[te] = softmax(z / t)
        else:
            est = est_factory().fit(X[tr], y[tr])
            cal[te] = _align(est.predict_proba(X[te]), list(est.classes_), classes)
            if raw_factory is not None:
                r = raw_factory().fit(X[tr], y[tr])
                raw[te] = _align(r.predict_proba(X[te]), list(r.classes_), classes)
    out: dict[str, Any] = {"folds": n_folds, "calibrated": _metrics(cal, y_idx, config.ece_bins)}
    if kind != "svm":
        out["uncalibrated"] = _metrics(raw, y_idx, config.ece_bins)
    if temps:
        out["fold_temperatures"] = [round(t, 4) for t in temps]
    return out


# ------------------------------------------------------------------ fitting


@dataclass
class FittedHead:
    kind: str
    classes: list[str]
    estimator: Any
    temperature: float | None
    n_components: int | None
    hyperparams: dict
    metrics: dict
    tune_report: dict | None = None
    counts: dict = field(default_factory=dict)

    def predict_proba(self, X: np.ndarray) -> np.ndarray:
        if self.kind == "logreg":
            z = _align(logits_of(self.estimator, X), list(self.estimator.classes_), self.classes, fill=-1e9)
            return softmax(z / (self.temperature or 1.0))
        return _align(self.estimator.predict_proba(X), list(self.estimator.classes_), self.classes)


def fit_head(X: np.ndarray, y: np.ndarray, config: TrainConfig = DEFAULT_TRAIN_CONFIG) -> FittedHead:
    """Fit a calibrated head on (X, y). v0's checks: two classes at least,
    two samples and two features at least."""
    if config.estimator_kind not in ESTIMATOR_KINDS:
        raise ValueError(f"unknown estimator kind {config.estimator_kind!r}")
    classes = sorted({str(v) for v in y.tolist()})
    if len(classes) < 2:
        raise ValueError(f"training needs at least two classes, got {classes}")
    if X.shape[0] < 2 or X.shape[1] < 2:
        raise ValueError(f"need at least 2 samples and 2 features, got {X.shape}")
    y = np.asarray([str(v) for v in y], dtype=object)

    upper = min(X.shape[0], X.shape[1])
    hp: dict = {"C": config.logreg_C}
    tune_report = None
    if config.auto_tune and X.shape[0] >= 20:
        n_components, hp, tune_report = auto_tune(
            X, y, random_state=config.random_state, pca_floor=config.pca_floor, estimator_kind=config.estimator_kind
        )
    elif config.pca_components is None:
        n_components = None
    else:
        n_components = min(config.pca_components, upper)
        n_components = max(min(config.pca_floor, upper), n_components, 2)
        n_components = min(n_components, upper)

    def factory():
        return build_estimator(
            n_components=n_components,
            C=hp.get("C", config.logreg_C),
            random_state=config.random_state,
            estimator_kind=config.estimator_kind,
            estimator_hyperparams=hp,
        )

    def raw_factory():
        return _raw_estimator(
            config.estimator_kind,
            n_components=n_components,
            C=hp.get("C", config.logreg_C),
            random_state=config.random_state,
            estimator_hyperparams=hp,
        )

    _, counts = np.unique(y, return_counts=True)
    if int(counts.min()) >= 2:
        metrics = cross_validated_metrics(
            factory,
            raw_factory if config.estimator_kind == "rf" else None,
            X,
            y,
            classes,
            kind=config.estimator_kind,
            config=config,
        )
    else:
        metrics = {"folds": 0}

    estimator = factory().fit(X, y)
    temperature = None
    if config.estimator_kind == "logreg":
        temperature = _oof_temperature(factory, X, y, classes, config.random_state) if int(counts.min()) >= 2 else 1.0
    return FittedHead(
        kind=config.estimator_kind,
        classes=classes,
        estimator=estimator,
        temperature=temperature,
        n_components=n_components,
        hyperparams=hp,
        metrics=metrics,
        tune_report=tune_report,
        counts={c: int((y == c).sum()) for c in classes},
    )


# ---------------------------------------------------------- the artifact


def canonical_json(obj: Any) -> bytes:
    return (json.dumps(obj, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n").encode()


def head_document(head: FittedHead, *, encoder_chain: list[dict], preprocess_version: str, n_train_slices: int) -> dict:
    """The JSON form of a logistic-regression head: every number inference
    needs and nothing that executes."""
    if head.kind != "logreg":
        raise ValueError("only a logistic-regression head has a JSON form")
    steps = dict(head.estimator.named_steps)
    scale = steps["scale"]
    doc: dict[str, Any] = {
        "format": HEAD_FORMAT,
        "kind": "logreg",
        "classes": head.classes,
        "encoder_chain": encoder_chain,
        "preprocess_version": preprocess_version,
        "n_train_slices": n_train_slices,
        "scaler": {"mean": scale.mean_.tolist(), "scale": scale.scale_.tolist()},
        "pca": None,
        "calibration": {"method": "temperature", "temperature": head.temperature},
    }
    if "pca" in steps:
        pca = steps["pca"]
        doc["pca"] = {"mean": pca.mean_.tolist(), "components": pca.components_.tolist()}
    clf = steps["clf"]
    coef, intercept = clf.coef_, clf.intercept_
    if coef.shape[0] == 1:
        # Binary: two columns [0, z], whose softmax is the sigmoid of z.
        coef = np.vstack([np.zeros_like(coef), coef])
        intercept = np.concatenate([[0.0], intercept])
    order = [list(clf.classes_).index(c) for c in head.classes]
    doc["linear"] = {"coef": coef[order].tolist(), "intercept": intercept[order].tolist()}
    return doc


class JsonHead:
    """A head read back from its JSON document: numpy only."""

    def __init__(self, doc: dict):
        if doc.get("format") != HEAD_FORMAT:
            raise ValueError(f"not a {HEAD_FORMAT} document")
        self.doc = doc
        self.classes = list(doc["classes"])
        self.mean = np.asarray(doc["scaler"]["mean"], dtype=np.float64)
        self.scale = np.asarray(doc["scaler"]["scale"], dtype=np.float64)
        self.pca = None
        if doc.get("pca"):
            self.pca = (
                np.asarray(doc["pca"]["mean"], dtype=np.float64),
                np.asarray(doc["pca"]["components"], dtype=np.float64),
            )
        self.coef = np.asarray(doc["linear"]["coef"], dtype=np.float64)
        self.intercept = np.asarray(doc["linear"]["intercept"], dtype=np.float64)
        self.temperature = float((doc.get("calibration") or {}).get("temperature") or 1.0)
        self.encoder_chain = doc.get("encoder_chain") or []

    def logits(self, X: np.ndarray) -> np.ndarray:
        x = (np.asarray(X, dtype=np.float64) - self.mean) / self.scale
        if self.pca is not None:
            x = (x - self.pca[0]) @ self.pca[1].T
        return x @ self.coef.T + self.intercept

    def predict_proba(self, X: np.ndarray) -> np.ndarray:
        return softmax(self.logits(X) / self.temperature)

    @property
    def classes_(self) -> list[str]:
        return self.classes


def sha256_bytes(b: bytes) -> str:
    return "sha256:" + hashlib.sha256(b).hexdigest()
