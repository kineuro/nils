# SPDX-License-Identifier: AGPL-3.0-only
"""The head: calibration on every kind, its metrics, and its JSON form."""

from __future__ import annotations

import json

import numpy as np
import pytest

from nils_bodypart.head import (
    JsonHead,
    TrainConfig,
    brier_score,
    canonical_json,
    expected_calibration_error,
    fit_head,
    fit_temperature,
    head_document,
    softmax,
)


def overconfident_set(n=240, d=150, classes=3, seed=0):
    """Many noise features and noisy labels: an unregularised logistic
    regression fits them and is far too sure of itself."""
    from sklearn.datasets import make_classification

    X, y = make_classification(
        n_samples=n, n_features=d, n_informative=4, n_redundant=0, n_classes=classes, n_clusters_per_class=1, flip_y=0.15, random_state=seed
    )
    names = np.array(["brain", "spine", "chest", "neck"])[:classes]
    return X.astype(np.float32), names[y].astype(object)


def test_metric_functions():
    y = np.array([0, 1, 1, 0])
    perfect = np.eye(2)[y]
    assert expected_calibration_error(perfect, y) == 0.0
    assert brier_score(perfect, y) == 0.0
    half = np.full((4, 2), 0.5)
    assert brier_score(half, y) == pytest.approx(0.5)
    sure_wrong = np.eye(2)[1 - y]
    assert expected_calibration_error(sure_wrong, y) == pytest.approx(1.0)


def test_temperature_undoes_a_known_overconfidence():
    rng = np.random.default_rng(1)
    true_logits = rng.normal(0, 1.5, (4000, 3))
    p = softmax(true_logits)
    y = np.array([rng.choice(3, p=row) for row in p])
    t = fit_temperature(true_logits * 4.0, y)
    assert t == pytest.approx(4.0, rel=0.15)


def test_calibration_improves_or_keeps_ece_and_keeps_every_label():
    X, y = overconfident_set()
    head = fit_head(X, y, TrainConfig(auto_tune=False, pca_components=None, logreg_C=100.0))
    m = head.metrics
    assert m["calibrated"]["ece"] <= m["uncalibrated"]["ece"]
    assert m["calibrated"]["brier"] <= m["uncalibrated"]["brier"] + 1e-9
    assert m["calibrated"]["accuracy"] == m["uncalibrated"]["accuracy"]
    assert head.temperature > 1.0
    # The fitted head's labels are the uncalibrated ones.
    raw = head.estimator.predict(X)
    assert (np.array(head.classes)[head.predict_proba(X).argmax(axis=1)] == raw).all()


@pytest.mark.parametrize("kind", ["rf", "svm"])
def test_forests_and_svms_are_calibrated_too(kind):
    X, y = overconfident_set(n=90, d=20, seed=2)
    head = fit_head(X, y, TrainConfig(auto_tune=False, pca_components=8, estimator_kind=kind))
    assert head.estimator.steps[-1][1].__class__.__name__ == "CalibratedClassifierCV"
    assert set(head.metrics["calibrated"]) == {"accuracy", "ece", "brier"}
    p = head.predict_proba(X[:5])
    np.testing.assert_allclose(p.sum(axis=1), 1.0, rtol=1e-6)


def test_auto_tune_chooses_pca_by_cross_validation():
    X, y = overconfident_set(n=120, d=80, seed=3)
    head = fit_head(X, y, TrainConfig(auto_tune=True))
    rep = head.tune_report
    # 120 samples: outer folds of 96, nested splits of 76, so 80 (all of D)
    # no longer fits, and with 64 alone of v0's sizes, 16 and 32 join it
    assert rep["grid"] and rep["best_pca"] in {None, 16, 32, 64}
    assert {g["pca"] for g in rep["grid"]} == {None, 16, 32, 64}


@pytest.mark.parametrize("classes,pca", [(3, 16), (2, None)])
def test_the_json_head_answers_as_the_fitted_one(classes, pca):
    X, y = overconfident_set(n=80, d=30, classes=classes, seed=4)
    head = fit_head(X, y, TrainConfig(auto_tune=False, pca_components=pca, logreg_C=0.5))
    doc = head_document(head, encoder_chain=[{"name": "x", "digest": "sha256:" + "0" * 64, "dim": 30}], preprocess_version="v", n_train_slices=3)
    again = JsonHead(json.loads(canonical_json(doc)))
    np.testing.assert_allclose(again.predict_proba(X), head.predict_proba(X), rtol=1e-6, atol=1e-8)
    assert again.classes == head.classes
    assert canonical_json(doc) == canonical_json(json.loads(canonical_json(doc)))


def test_too_few_classes_are_refused():
    with pytest.raises(ValueError):
        fit_head(np.zeros((5, 4)), np.array(["brain"] * 5, dtype=object))


@pytest.mark.parametrize("n", [40, 60, 85, 99])
def test_auto_tune_fits_a_label_set_under_a_hundred(n):
    """Wave 43's proof: with 85 labels the tuning took PCA 64, which fits
    an outer fold's 68 samples, and the temperature was then fitted on a
    nested split of 54, where 64 components do not fit. Every PCA size the
    grid offers fits the smallest split any fit sees."""
    X, y = overconfident_set(n=n, d=150, seed=5)
    head = fit_head(X, y, TrainConfig(auto_tune=True))
    assert head.metrics["folds"] >= 2 and "calibrated" in head.metrics
    rep = head.tune_report
    smallest = rep["smallest_fit"]
    assert smallest < n
    assert all(g["pca"] is None or g["pca"] <= smallest for g in rep["grid"])
    # a label set this small still has sizes to choose between
    assert len({g["pca"] for g in rep["grid"]}) >= 2


def test_a_fixed_pca_is_clamped_to_the_smallest_split():
    """auto_tune off, and a PCA larger than the nested split holds: clamped,
    not a failure."""
    X, y = overconfident_set(n=85, d=150, seed=6)
    head = fit_head(X, y, TrainConfig(auto_tune=False, pca_components=128))
    assert head.n_components is not None and head.n_components < 68
    assert "calibrated" in head.metrics
