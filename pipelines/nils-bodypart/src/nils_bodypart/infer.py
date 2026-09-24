# SPDX-License-Identifier: AGPL-3.0-only
"""Inference: slice probabilities to one answer per stack.

Ported from v0 (``infer.py``), in the pack's values:

- an **axial** stack is read at five slices (20 to 80 %); when the first 40 %
  of them say brain and the last 40 % say spine, the stack is brain-neck,
  with the smaller of the two as its confidence; otherwise the plain mean;
- a **sagittal or coronal** stack is read at its three centre slices, the
  middle one weighted double;
- any other stack is the plain mean of its three centre slices;
- a stack whose confidence is below 0.70 is marked ``needs_check``.

"First" and "last" are the stack's own slice order, the order its files are
given in the manifest, as v0 ordered them by slice location.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass, field

import numpy as np

from .preprocess import orientation_slice_indices

BRAIN, SPINE, BRAIN_NECK = "brain", "spine", "brain-neck"


@dataclass(frozen=True)
class InferConfig:
    n_slices: int = 3
    manual_review_below: float = 0.70


DEFAULT_INFER_CONFIG = InferConfig()


@dataclass
class StackPrediction:
    stack: str
    label: str | None
    confidence: float
    probs: dict[str, float]
    needs_check: bool
    n_slices_used: int
    reasoning: dict = field(default_factory=dict)


def axial_compose(probs_per_slice: np.ndarray, classes: Sequence[str]) -> tuple[str, float, dict[str, float], dict]:
    """v0's Brain-Neck rule over an axial stack's slices, top first."""
    n = probs_per_slice.shape[0]
    idx = {c: i for i, c in enumerate(classes)}
    bi, si, bni = idx.get(BRAIN), idx.get(SPINE), idx.get(BRAIN_NECK)
    if n >= 4 and bi is not None and si is not None and bni is not None:
        n_top = max(1, n * 2 // 5)
        n_bot = max(1, n * 2 // 5)
        top = probs_per_slice[:n_top].mean(axis=0)
        bot = probs_per_slice[-n_bot:].mean(axis=0)
        top_label = classes[int(np.argmax(top))]
        bot_label = classes[int(np.argmax(bot))]
        if top_label == BRAIN and bot_label == SPINE:
            conf = min(float(top[bi]), float(bot[si]))
            probs = {c: 0.0 for c in classes}
            probs[BRAIN_NECK] = conf
            probs[BRAIN] = float(top[bi]) * (1.0 - conf)
            probs[SPINE] = float(bot[si]) * (1.0 - conf)
            total = sum(probs.values())
            if total > 0:
                probs = {c: v / total for c, v in probs.items()}
            reasoning = {
                "aggregation": "axial_compose",
                "top_label": top_label,
                "top_brain_prob": round(float(top[bi]), 3),
                "bot_label": bot_label,
                "bot_spine_prob": round(float(bot[si]), 3),
            }
            return BRAIN_NECK, conf, probs, reasoning
    avg = probs_per_slice.mean(axis=0)
    best = int(np.argmax(avg))
    return classes[best], float(avg[best]), {c: float(avg[i]) for i, c in enumerate(classes)}, {"aggregation": "axial_mean"}


def weighted_center_avg(probs_per_slice: np.ndarray, classes: Sequence[str]) -> tuple[str, float, dict[str, float], dict]:
    """The middle slice weighted double."""
    n = probs_per_slice.shape[0]
    w = np.ones(n, dtype=np.float64)
    w[n // 2] = 2.0
    w /= w.sum()
    avg = (probs_per_slice * w[:, None]).sum(axis=0)
    best = int(np.argmax(avg))
    probs = {c: float(avg[i]) for i, c in enumerate(classes)}
    return classes[best], float(avg[best]), probs, {"aggregation": "center_weighted", "center_weight": 2.0}


def compose(probs_per_slice: np.ndarray, classes: Sequence[str], orientation: str | None) -> tuple[str, float, dict[str, float], dict]:
    ori = (orientation or "").strip().lower()
    if ori == "axial":
        return axial_compose(probs_per_slice, classes)
    if ori in ("sagittal", "coronal"):
        return weighted_center_avg(probs_per_slice, classes)
    avg = probs_per_slice.mean(axis=0)
    best = int(np.argmax(avg))
    return classes[best], float(avg[best]), {c: float(avg[i]) for i, c in enumerate(classes)}, {"aggregation": "mean"}


def predict_stack(
    *,
    head,
    stack: str,
    num_slices: int,
    orientation: str | None,
    slice_features: dict[int, np.ndarray],
    config: InferConfig = DEFAULT_INFER_CONFIG,
) -> StackPrediction:
    """One stack: the slices its orientation asks for, those that have
    features, their probabilities, composed. ``slice_features`` maps a slice
    index to the concatenated features of the encoder chain."""
    classes = list(head.classes_)
    want = orientation_slice_indices(num_slices, orientation, n_default=config.n_slices)
    rows = [slice_features[i] for i in want if i in slice_features]
    if not rows:
        return StackPrediction(stack, None, 0.0, {c: 0.0 for c in classes}, True, 0, {"aggregation": "none"})
    probs_per_slice = np.asarray(head.predict_proba(np.stack(rows)))
    label, conf, probs, reasoning = compose(probs_per_slice, classes, orientation)
    return StackPrediction(
        stack=stack,
        label=label,
        confidence=conf,
        probs=probs,
        needs_check=conf < config.manual_review_below,
        n_slices_used=len(rows),
        reasoning=reasoning,
    )
