# SPDX-License-Identifier: AGPL-3.0-only
"""Which slices, and how one slice is prepared for the encoders.

Ported from v0 (``qc/body_part/preprocess.py``) without a change of meaning:

- the DICOM window is ignored on purpose: a slice is rescaled by its slope
  and intercept, clipped at percentiles 1 and 99, z-scored, stretched to
  uint8, letterboxed to 224 and copied to three channels;
- seeding reads the centre slice (``n // 2``), training averages the three
  centre slices, and inference reads five slices at 20 to 80 % of an axial
  stack and the three centre slices otherwise.

``slices_to_embed`` is new: the embed entry point computes, once, every slice
the three later steps read, so a stack is embedded one time.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass

import numpy as np

logger = logging.getLogger(__name__)

TARGET_SIZE: int = 224


@dataclass(frozen=True)
class PreprocessConfig:
    clip_pct: tuple[float, float] = (1.0, 99.0)
    zscore: bool = True
    size: int = TARGET_SIZE


DEFAULT_PREPROCESS = PreprocessConfig()


def central_slice_indices(num_slices: int, n: int = 3) -> list[int]:
    """Up to ``n`` slice indices clustered around the middle, clamped and
    without duplicates (a thin stack gives fewer)."""
    if num_slices <= 0:
        return []
    mid = num_slices // 2
    half = n // 2
    raw = [mid + i - half for i in range(n)]
    out: list[int] = []
    seen: set[int] = set()
    for idx in raw:
        clamped = max(0, min(num_slices - 1, idx))
        if clamped not in seen:
            seen.add(clamped)
            out.append(clamped)
    return out


AXIAL_FRACTIONS = (0.20, 0.35, 0.50, 0.65, 0.80)


def orientation_slice_indices(num_slices: int, orientation: str | None, n_default: int = 3) -> list[int]:
    """The slices inference reads: five spread over an axial stack (so the top
    and the bottom are seen apart), else the ``n_default`` centre slices."""
    if num_slices <= 0:
        return []
    ori = (orientation or "").strip().lower()
    if ori == "axial":
        out: list[int] = []
        seen: set[int] = set()
        for f in AXIAL_FRACTIONS:
            idx = max(0, min(num_slices - 1, int(f * num_slices)))
            if idx not in seen:
                seen.add(idx)
                out.append(idx)
        return out
    return central_slice_indices(num_slices, n=n_default)


def seed_slice_index(num_slices: int) -> int | None:
    """The one slice seeding reads: v0's ``floor(num_slices / 2)``."""
    if num_slices <= 0:
        return None
    return num_slices // 2


def slices_to_embed(num_slices: int, orientation: str | None) -> list[int]:
    """Every slice seeding, training and inference will read, sorted."""
    want = set(central_slice_indices(num_slices, 3))
    want.update(orientation_slice_indices(num_slices, orientation, 3))
    s = seed_slice_index(num_slices)
    if s is not None:
        want.add(s)
    return sorted(want)


def preprocess_slice(
    arr: np.ndarray,
    *,
    rescale_slope: float = 1.0,
    rescale_intercept: float = 0.0,
    config: PreprocessConfig = DEFAULT_PREPROCESS,
) -> np.ndarray:
    """A raw 2-D slice to a (size, size, 3) uint8 image, v0's six steps."""
    if arr.ndim != 2:
        raise ValueError(f"expected a 2-D slice, got shape {arr.shape}")
    a = arr.astype(np.float32) * float(rescale_slope) + float(rescale_intercept)

    lo, hi = np.percentile(a, list(config.clip_pct))
    if hi <= lo:
        hi = lo + 1.0
    a = np.clip(a, lo, hi)

    if config.zscore:
        std = a.std()
        if std > 1e-6:
            a = (a - a.mean()) / std

    a_min, a_max = a.min(), a.max()
    if a_max - a_min > 1e-6:
        a = (a - a_min) / (a_max - a_min) * 255.0
    else:
        a = np.zeros_like(a)
    a = a.astype(np.uint8)

    a = letterbox_to_square(a, config.size)
    return np.stack([a, a, a], axis=-1)


def letterbox_to_square(arr: np.ndarray, size: int) -> np.ndarray:
    """Fit ``arr`` inside (size, size) keeping its aspect, padded with zeros."""
    from PIL import Image

    h, w = arr.shape
    img = Image.fromarray(arr, mode="L")
    scale = size / max(h, w)
    new_w = max(1, int(round(w * scale)))
    new_h = max(1, int(round(h * scale)))
    img = img.resize((new_w, new_h), Image.Resampling.BILINEAR)
    canvas = Image.new("L", (size, size), 0)
    canvas.paste(img, ((size - new_w) // 2, (size - new_h) // 2))
    return np.asarray(canvas, dtype=np.uint8)


def load_frames(path: str, frames: list[int]) -> dict[int, tuple[np.ndarray, float, float]]:
    """Read the given frames of one DICOM file once: {frame: (pixels, slope,
    intercept)}. A classic single-frame file gives its one image for any
    frame asked, as v0's reader did. A file or pixel data that cannot be read
    gives nothing, and the caller counts the slice as missing. Nothing about
    the file is logged but the fact, so no path reaches a log."""
    import pydicom

    try:
        ds = pydicom.dcmread(path)
        arr = ds.pixel_array
    except Exception:
        logger.warning("a DICOM file or its pixel data could not be read")
        return {}
    slope = float(getattr(ds, "RescaleSlope", 1) or 1)
    intercept = float(getattr(ds, "RescaleIntercept", 0) or 0)
    out: dict[int, tuple[np.ndarray, float, float]] = {}
    for f in frames:
        a = arr
        if a.ndim == 3:
            a = a[max(0, min(f, a.shape[0] - 1))]
        out[f] = (a, slope, intercept)
    return out
