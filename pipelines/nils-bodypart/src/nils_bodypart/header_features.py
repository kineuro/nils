# SPDX-License-Identifier: AGPL-3.0-only
"""The 44 header features the certified body-part head reads (record 50),
from a stack's fingerprint, its rules' body_part answer and its volume's
geometry.

In order: the fingerprint's field of view, pixel spacing, matrix, slice
count, span, thickness, spacing, aspect ratio and field strength (the
normalised one, else the stated one); the volume's extent along the three
patient axes and its obliquity; the orientation as four flags; CT; four
flags of the receive coil's name; six word families found in the stack's
own text (series description, protocol, sequence name, comments) and six in
its stated body part; a portrait sagittal flag; and the rules' body_part
answer as five flags and its confidence (0 where the rules gave none). A
number the fingerprint lacks is NaN, which the head reads as missing.

The words are matched in lowercase text in which runs of ``_ / \\ ( ) , ; :
-`` are spaces, with a space before and after. ``halsrygg`` (the cervical
spine, in Swedish) is not a neck for the neck families.
"""

from __future__ import annotations

import re

import numpy as np

WORDS: dict[str, list[str]] = {
    "spine": [
        "spine", "spinal", "cervical", "cerv", "thoracic", "thor ", "lumbar", "lumb", "c-spine", "t-spine", "l-spine", "cspine", "ctspine",
        "vertebral", "vertebra", "medulla", "rygg", "totalcolumna", "columna", "sag cc", "th col", " c col", "wirbel", "hws", "bws", "lws",
        "rachis", "cervicale", "dorsale", "lombaire", "moelle", "wervelkolom", "myelit", "myelopathy", " cord",
    ],
    "neck": ["neck", "nacke", "hals", "nakke", " nek"],
    "necksoft": [
        "carotid", "karotis", "carotis", "larynx", "laryng", "thyroid", "thyreo", "tyreo", "parotid", "pharyn", "tongue", "soft tissue",
        "weichteil", "lymph", "angio", " tof", "mra",
    ],
    "brain": [
        "fmri", "bold", "resting state", "hippocampus", "amygdala", "thalamus", "cortex", "cortical", "white matter", "cerebral", "cerebell",
        "frontal", "parietal", "temporal", "occipital", "brainstem", "pons", "brain", "head", "neuro", "hj rna", "hjarna", "hj?rna", "huvud",
        "kopf", "gehirn", "hirn", "schadel", "cerveau", "encephale", "crane", "hersenen", "hoofd", "skull", "orbit", "pituitar", "hypofys", "iac",
    ],
    "chest": ["chest", "thorax", "lung", "cardiac", "heart", "hjart", "breast", "mamma", "pulmo", "aorta"],
    "localizer": ["loc", "survey", "scout", "3plane", "3 plane", "tri-pilot", "aahead", "aaspine", "mobiview"],
}

NAMES = (
    [
        "fov_x_mm", "fov_y_mm", "px_row_mm", "px_col_mm", "rows", "cols", "n_slices", "span_mm", "thick_mm", "spacing_mm", "aspect", "field_T",
        "cover_lr_mm", "cover_ap_mm", "cover_si_mm", "oblique_deg", "ori_ax", "ori_cor", "ori_sag", "ori_other", "ct",
        "coil_spine", "coil_head", "coil_neck", "coil_known",
    ]
    + [f"own_{k}" for k in WORDS]
    + [f"stated_{k}" for k in WORDS]
    + ["portrait_sag", "rules_brain", "rules_brain_neck", "rules_neck", "rules_spine", "rules_none", "rules_conf"]
)
assert len(NAMES) == 44


def flags(text: str | None) -> list[int]:
    t = " " + re.sub(r"[_/\\(),;:\-]+", " ", (text or "").lower()) + " "
    tn = t.replace("halsrygg", " ").replace("hals rygg", " ")
    return [int(any(w in (tn if k in ("neck", "necksoft") else t) for w in ws)) for k, ws in WORDS.items()]


def _num(x) -> float:
    """A fingerprint number, NaN where it has none."""
    if x is None or isinstance(x, bool):
        return float("nan")
    if isinstance(x, str):
        x = x.strip()
        if not x:
            return float("nan")
    try:
        return float(x)
    except (TypeError, ValueError):
        return float("nan")


def _text(x) -> str:
    return "" if x is None else str(x)


def rules_row(classification: dict | None) -> dict | None:
    """The rules' body_part answer the features read: the axis's first row."""
    rows = (classification or {}).get("body_part") or []
    return rows[0] if rows and isinstance(rows[0], dict) else None


def features(fingerprint: dict, classification: dict | None, meta: dict) -> np.ndarray:
    """The 44 features, float32, NaN where a number is missing. ``meta`` is
    the volume's rounded geometry (``volume.Built.meta``)."""
    fp = fingerprint
    ori = _text(fp.get("orientation")).lower()
    coil = _text(fp.get("receive_coil_name")).lower()
    own = flags(" ".join(_text(fp.get(k)) for k in ("text_series_description_ci", "text_protocol_name_ci", "text_sequence_name_ci", "text_series_comments_ci")))
    stated = flags(_text(fp.get("text_body_part_ci")))
    fx, fy, asp = _num(fp.get("fov_x")), _num(fp.get("fov_y")), _num(fp.get("aspect_ratio"))

    def z(v: float) -> float:  # a missing number as 0 in a comparison
        return 0.0 if np.isnan(v) else v

    port = int(ori == "sagittal" and z(fx) > 0 and z(fy) > z(fx) and z(asp) >= 1.4)
    fsn = fp.get("field_strength_normalized")
    field_t = _num(fsn) if fsn not in (None, "") else _num(fp.get("magnetic_field_strength"))
    mod = _text(fp.get("modality")) or _text(meta.get("modality"))
    r = rules_row(classification) or {}
    v = _text(r.get("value"))
    row = [
        fx, fy, _num(fp.get("pixel_spacing_row")), _num(fp.get("pixel_spacing_col")), _num(fp.get("rows")), _num(fp.get("columns")),
        _num(fp.get("n_slices")), _num(fp.get("slice_span_mm")), _num(fp.get("slice_thickness")), _num(fp.get("spacing_between_slices")),
        asp, field_t,
        _num(meta.get("ext_x")), _num(meta.get("ext_y")), _num(meta.get("ext_z")), _num(meta.get("oblique_deg")),
        int(ori == "axial"), int(ori == "coronal"), int(ori == "sagittal"), int(ori not in ("axial", "coronal", "sagittal")),
        int(mod == "CT"),
        int("spine" in coil and "head" not in coil and "neck" not in coil), int("head" in coil), int("neck" in coil), int(bool(coil)),
    ] + own + stated + [
        port, int(v == "brain"), int(v == "brain-neck"), int(v == "neck"), int(v == "spine"),
        int(v not in ("brain", "brain-neck", "neck", "spine")),
        _num(r.get("confidence")) if v else 0.0,
    ]
    return np.array(row, np.float32)
