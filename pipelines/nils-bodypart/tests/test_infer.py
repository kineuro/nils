# SPDX-License-Identifier: AGPL-3.0-only
"""v0's composition rules: the axial Brain-Neck rule and the aggregations."""

from __future__ import annotations

import numpy as np
import pytest

from nils_bodypart.infer import InferConfig, axial_compose, compose, predict_stack, weighted_center_avg

CLASSES = ["brain", "brain-neck", "chest", "spine"]


def rows(*labels, p=0.8):
    out = []
    for lab in labels:
        r = np.full(len(CLASSES), (1 - p) / (len(CLASSES) - 1))
        r[CLASSES.index(lab)] = p
        out.append(r)
    return np.stack(out)


def test_brain_above_spine_below_is_brain_neck():
    probs = rows("brain", "brain", "spine", "spine", "spine")
    label, conf, dist, why = axial_compose(probs, CLASSES)
    assert label == "brain-neck"
    assert why["aggregation"] == "axial_compose"
    # The first 40 % (two slices) and the last 40 % (two slices).
    assert conf == pytest.approx(min(0.8, 0.8))
    assert sum(dist.values()) == pytest.approx(1.0)
    assert dist["brain-neck"] == max(dist.values())


def test_spine_above_brain_below_is_not_composed():
    label, _, _, why = axial_compose(rows("spine", "spine", "brain", "brain", "brain"), CLASSES)
    assert why["aggregation"] == "axial_mean" and label != "brain-neck"


def test_fewer_than_four_slices_or_a_missing_class_is_the_plain_mean():
    label, _, _, why = axial_compose(rows("brain", "spine", "spine"), CLASSES)
    assert why["aggregation"] == "axial_mean" and label == "spine"
    no_bn = ["brain", "spine"]
    p = np.array([[0.9, 0.1], [0.9, 0.1], [0.1, 0.9], [0.1, 0.9], [0.1, 0.9]])
    label, _, _, why = axial_compose(p, no_bn)
    assert why["aggregation"] == "axial_mean"


def test_the_centre_slice_counts_double_on_sagittal_and_coronal():
    probs = np.array([[0.9, 0.1], [0.2, 0.8], [0.9, 0.1]])
    label, conf, _, why = weighted_center_avg(probs, ["a", "b"])
    assert why["center_weight"] == 2.0
    assert conf == pytest.approx((0.9 + 0.4 + 0.9) / 4)
    assert label == "a"
    probs = np.array([[0.4, 0.6], [0.2, 0.8], [0.6, 0.4]])
    assert weighted_center_avg(probs, ["a", "b"])[0] == "b"
    assert compose(probs, ["a", "b"], "Coronal")[3]["aggregation"] == "center_weighted"
    assert compose(probs, ["a", "b"], None)[3]["aggregation"] == "mean"


class FixedHead:
    classes_ = CLASSES

    def __init__(self, per_slice):
        self.per_slice = per_slice

    def predict_proba(self, X):
        return np.stack([self.per_slice[int(x[0])] for x in X])


def test_predict_stack_reads_the_orientation_slices_and_marks_low_confidence():
    n = 10
    per = {i: rows("brain" if i < 5 else "spine")[0] for i in range(n)}
    feats = {i: np.array([i, 0.0]) for i in range(n)}
    pred = predict_stack(head=FixedHead(per), stack="7", num_slices=n, orientation="axial", slice_features=feats)
    assert pred.label == "brain-neck" and pred.n_slices_used == 5 and not pred.needs_check
    low = predict_stack(
        head=FixedHead(per), stack="7", num_slices=n, orientation="axial", slice_features=feats, config=InferConfig(manual_review_below=0.9)
    )
    assert low.needs_check
    none = predict_stack(head=FixedHead(per), stack="8", num_slices=n, orientation="axial", slice_features={})
    assert none.label is None and none.needs_check and none.n_slices_used == 0
