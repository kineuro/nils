# SPDX-License-Identifier: AGPL-3.0-only
"""The few-slice student's input, from the manifest alone (record 55 E2,
P1b): the native-slice reader of nils-design studies
``2026-10-02-bodypart-few-slice`` (``tools/lib.py``, ``extract.py``,
``cache.py``) and ``2026-10-05-image-model-speed`` (``tools/mimic_bp.py``,
``pack_bp.py``), ported as the measured model ran it, quirks included.

The student read, per stack:

- the slice-selection rule's slot 0 (``select``): the inferior end of an
  axial stack, the middle of a sagittal, coronal or thin-slab one, the point
  a sixth of the way through a localiser by instance number, the middle file
  by instance number without geometry;
- that one native slice, windowed at its 0.5th and 99.5th percentile,
  resampled to 1.5 mm a pixel and cropped or padded to 192 x 192
  (``to_tile``), input ``bodypart-slice-192:v1``;
- the slice's four geometry tokens (``slice_geo``) and the stack's 27
  (``stack_tokens``), among them its box's height relative to the session's
  top: the highest box among the study's unsealed MR stacks;
- the certified encoder's 15 geometry numbers and the 44 header features as
  the registry alone gives them (``registry_inputs``).

The registry columns the research read (``extract.py``'s queries) are
rebuilt here from the manifest's geometry, header and stack facts
(``row_of``), with the research's arithmetic: the slice normal of the raw
orientation for the positions along it, as its SQL computed it, and
Postgres's rounding of a float to one decimal for the count of positions.
Two quirks are kept on purpose, since the student was measured with them:
the localiser test reads a series description under a name the rows never
carried, so it sees the protocol name and the image type only, and the
slice-geometry index uses Python's rounding where the position count used
Postgres's.

Nothing here logs a path, a UID or a header value.
"""

from __future__ import annotations

import io
import math
import re
from collections import defaultdict
from decimal import ROUND_HALF_UP, Decimal

import numpy as np

SLICE_VERSION = "bodypart-slice-192:v1"
PX, MM = 192, 1.5
ENHANCED_MR = "1.2.840.10008.5.1.4.1.1.4.1"
KINDS = ["axial", "sagittal", "coronal", "thin", "localizer", "nogeo"]
PLANES = ["axial", "sagittal", "coronal"]
TOKENS = [
    "plane_axial", "plane_sagittal", "plane_coronal", *[f"kind_{k}" for k in KINDS], "tilt", "n_x", "n_y", "n_z", "ext_x", "ext_y", "ext_z",
    "span_n", "log_n_pos", "log_n_files", "fov_x", "fov_y", "thick", "field", "z_lo_rel_top", "z_hi_rel_top", "enhanced", "four_d",
]
LOCALIZER_RE = re.compile(r"(^|[^a-z])(loc|locali[sz]er|scout|survey|tri.?pilot|3.?plane|3pl|aahead|aa.?scout|pilot)([^a-z]|$)")


# --- lib.py: geometry ---------------------------------------------------------------------------------------------

def vec(x, n):
    """``n`` finite numbers from a list or DICOM's ``a\\b\\c``, else None."""
    if x is None:
        return None
    if isinstance(x, (list, tuple, np.ndarray)):
        parts = list(x)
    else:
        parts = str(x).replace(",", "\\").split("\\")
    try:
        v = [float(t) for t in parts]
    except (TypeError, ValueError):
        return None
    if len(v) != n or not all(math.isfinite(t) for t in v):
        return None
    return np.array(v)


def frame_of(iop):
    """Row direction, column direction and slice normal (unit vectors) of an orientation."""
    o = vec(iop, 6)
    if o is None:
        return None
    r, c = o[:3], o[3:]
    if np.linalg.norm(r) < 0.5 or np.linalg.norm(c) < 0.5:
        return None
    r = r / np.linalg.norm(r)
    c = c / np.linalg.norm(c)
    n = np.cross(r, c)
    if np.linalg.norm(n) < 0.5:
        return None
    return r, c, n / np.linalg.norm(n)


def orient_of(n) -> tuple[str, float]:
    a = np.abs(n)
    i = int(np.argmax(a))
    tilt = math.degrees(math.acos(min(1.0, float(a[i]))))
    return ("sagittal", "coronal", "axial")[i], tilt


def spacing(ps):
    v = vec(ps, 2)
    return v if v is not None and (v > 0).all() else None


def slice_corners(ipp, r, c, rows, cols, ps):
    u = r * ps[1] * max(cols - 1, 0)
    v = c * ps[0] * max(rows - 1, 0)
    return np.array([ipp, ipp + u, ipp + v, ipp + u + v])


def box_of(g: dict):
    f = frame_of(g.get("iop"))
    lo, hi, ps = vec(g.get("ipp_lo"), 3), vec(g.get("ipp_hi"), 3), spacing(g.get("ps"))
    if f is None or lo is None or hi is None or ps is None or not g.get("rows") or not g.get("cols"):
        return None
    r, c, _ = f
    k = np.vstack([slice_corners(lo, r, c, g["rows"], g["cols"], ps), slice_corners(hi, r, c, g["rows"], g["cols"], ps)])
    return k


# --- lib.py: the slice-selection rule -----------------------------------------------------------------------------

def unique_positions(items, tol=0.5):
    items = sorted(items, key=lambda t: (t[0], t[1] if t[1] is not None else 1e18))
    out, last = [], None
    for d, inum, p in items:
        if last is None or d - last > tol:
            out.append([d, inum, p])
            last = d
        elif inum is not None and (out[-1][1] is None or inum < out[-1][1]):
            out[-1][1], out[-1][2] = inum, p
    return [(d, p) for d, _, p in out]


def _spread(m, fracs):
    return [min(m - 1, max(0, int(round(f * (m - 1))))) for f in fracs]


def order_rule(kind: str, m: int, inferior_is_low, posterior_is_high) -> list[int]:
    if m <= 0:
        return []
    mid = (m - 1) // 2
    lo_end, hi_end = 0, m - 1
    if kind == "localizer":
        seq = _spread(m, [1 / 6, 1 / 2, 5 / 6]) + [lo_end, hi_end]
    elif kind == "sagittal":
        seq = [mid, min(m - 1, mid + 1), max(0, mid - 1)] + _spread(m, [0.25, 0.75]) + [lo_end, hi_end]
    elif kind == "coronal":
        post = _spread(m, [2 / 3])[0] if posterior_is_high in (True, None) else _spread(m, [1 / 3])[0]
        ant = _spread(m, [1 / 3])[0] if posterior_is_high in (True, None) else _spread(m, [2 / 3])[0]
        seq = [mid, post, ant] + ([hi_end, lo_end] if posterior_is_high in (True, None) else [lo_end, hi_end])
    elif kind == "thin":
        inf, sup = (lo_end, hi_end) if inferior_is_low in (True, None) else (hi_end, lo_end)
        seq = [mid, inf, sup] + _spread(m, [0.25, 0.75])
    else:
        inf, sup = (lo_end, hi_end) if inferior_is_low in (True, None) else (hi_end, lo_end)
        seq = [inf, sup, mid] + _spread(m, [0.25, 0.75])
    out = []
    for i in seq:
        if i not in out:
            out.append(i)
    for i in range(m):
        if i not in out:
            out.append(i)
    return out


def stack_kind(g: dict) -> str:
    """The rule's stack type. The research read ``desc``, a name its rows
    never carried (they named the series description ``sdesc``), so only the
    protocol name and the image type find a localiser; kept as measured."""
    text = " ".join(str(g.get(k) or "") for k in ("desc", "prot")).lower()
    if "LOCALIZER" in str(g.get("image_type") or "").upper() or LOCALIZER_RE.search(text):
        return "localizer"
    f = frame_of(g.get("iop"))
    if f is None:
        return "nogeo"
    o, _ = orient_of(f[2])
    m = g.get("n_pos") or 0
    if o in ("axial", "coronal") and 0 < m < 15:
        return "thin"
    return o


def _by_inum(files):
    return sorted(files, key=lambda x: (x.get("inum") is None, x.get("inum") or 0))


def select(g: dict, files: list[dict], k: int) -> tuple[list[dict], int | None]:
    """The rule on one stack's files (each with ``ipp`` and ``inum``): the
    chosen files in reading order, at most ``k``, and the index among them of
    the oriented inferior slice (None without geometry)."""
    f = frame_of(g.get("iop"))
    kind = stack_kind(g)
    if f is None or kind == "nogeo":
        fs = _by_inum(files)
        m = len(fs)
        idx = [int(round((m - 1) / 2))] + [0, m - 1] if m else []
        seen, out = set(), []
        for i in idx + list(range(m)):
            if i not in seen:
                seen.add(i)
                out.append(fs[i])
            if len(out) >= k:
                break
        return out, None
    r, c, n = f
    items = []
    for x in files:
        p = vec(x.get("ipp"), 3)
        items.append((float(p @ n) if p is not None else 0.0, x.get("inum"), x))
    pos = unique_positions(items)
    m = len(pos)
    inferior_is_low = posterior_is_high = None
    ps = spacing(g.get("ps"))
    if m >= 2 and ps is not None and g.get("rows") and g.get("cols"):
        p0, p1 = vec(pos[0][1].get("ipp"), 3), vec(pos[-1][1].get("ipp"), 3)
        if p0 is not None and p1 is not None:
            z0 = slice_corners(p0, r, c, g["rows"], g["cols"], ps)[:, 2].min()
            z1 = slice_corners(p1, r, c, g["rows"], g["cols"], ps)[:, 2].min()
            inferior_is_low = bool(z0 <= z1)
            posterior_is_high = bool(p1[1] >= p0[1])
    if kind == "localizer":
        fs = _by_inum(files)
        pos = [(i, x) for i, x in enumerate(fs)]
        m = len(pos)
    seq = order_rule(kind, m, inferior_is_low, posterior_is_high)
    out, seen = [], set()
    inf_at = None
    inf_idx = None if kind == "localizer" else (0 if inferior_is_low in (True, None) else m - 1)
    for i in seq:
        x = pos[i][1]
        if id(x) in seen:
            continue
        seen.add(id(x))
        if i == inf_idx:
            inf_at = len(out)
        out.append(x)
        if len(out) >= k:
            break
    return out, inf_at


# --- extract.py: the stack's registry row, from the manifest --------------------------------------------------------

def _pg_round1(d: float) -> Decimal:
    """``round(d::numeric, 1)`` as Postgres computes it: the float cast to
    numeric at 15 significant digits, rounded half away from zero."""
    return Decimal(format(d, ".15g")).quantize(Decimal("0.1"), rounding=ROUND_HALF_UP)


def _ps_key(ps):
    v = vec(ps, 2)
    return tuple(v) if v is not None else None


def row_of(stack) -> dict:
    """One stack as ``extract.py`` read it from the registry: the stack's
    facts and fingerprint columns, the geometry aggregated over its files,
    and the derived kind, plane, tilt and box (``derive``)."""
    header = stack.extra.get("header") if isinstance(stack.extra.get("header"), dict) else {}
    fp = header.get("fingerprint") if isinstance(header.get("fingerprint"), dict) else {}
    sg = stack.extra.get("geometry") if isinstance(stack.extra.get("geometry"), dict) else {}
    g: dict = {
        "id": stack.stack_id,
        "study_id": sg.get("study"),
        "iop": sg.get("iop"),
        "image_type": sg.get("image_type"),
        "n_instances": sg.get("n_instances"),
        "sealed": bool(sg.get("sealed")),
        "sop_class": sg.get("sop_class"),
        "modality": stack.extra.get("modality") or sg.get("modality") or fp.get("modality"),
        "prot": fp.get("text_protocol_name_ci"),
        "field": fp.get("field_strength_normalized"),
        "fov_x": fp.get("fov_x"),
        "fov_y": fp.get("fov_y"),
        "thick": fp.get("slice_thickness"),
        "fp_slices": fp.get("n_slices"),
    }
    files = [dict(geo or {}) for geo in (stack.file_geo or [None] * len(stack.files))]
    if len(files) != len(stack.files):
        files = [{} for _ in stack.files]
    g["n_files"] = len(stack.files)
    if not any(files):
        _derive(g)
        return g
    # the geometry query of extract.py: positions along the raw orientation's normal
    o = vec(g["iop"], 6)
    nraw = None
    if o is not None:
        nraw = (o[1] * o[5] - o[2] * o[4], o[2] * o[3] - o[0] * o[5], o[0] * o[4] - o[1] * o[3])
    ds = []
    for x in files:
        p = vec(x.get("ipp"), 3)
        d = None
        if p is not None and nraw is not None:
            d = float(p[0]) * nraw[0] + float(p[1]) * nraw[1] + float(p[2]) * nraw[2]
        ds.append(d)
    g["n_inst_reg"] = g["n_instances"] if isinstance(g["n_instances"], int) else len(files)
    g["n_geo"] = sum(d is not None for d in ds)
    g["n_pos"] = len({_pg_round1(d) for d in ds if d is not None})
    with_d = [(d, x.get("inum"), x) for d, x in zip(ds, files) if d is not None]
    if with_d:
        g["d_min"] = min(t[0] for t in with_d)
        g["d_max"] = max(t[0] for t in with_d)
        # ORDER BY d, inum: a null number sorts last, as Postgres sorts it
        lo = sorted(with_d, key=lambda t: (t[0], t[1] is None, t[1] or 0))[0]
        hi = sorted(with_d, key=lambda t: (-t[0], t[1] is None, t[1] or 0))[0]
        g["ipp_lo"], g["ipp_hi"] = lo[2].get("ipp"), hi[2].get("ipp")
    rows = [x.get("rows") for x in files if isinstance(x.get("rows"), int)]
    cols = [x.get("cols") for x in files if isinstance(x.get("cols"), int)]
    g["rows"] = max(rows) if rows else None
    g["cols"] = max(cols) if cols else None
    # min(pixel_spacing) over the instances: the research took the smallest
    # text; with numbers, the smallest pair (the same where every spacing is
    # written alike, as a stack's are)
    pss = [_ps_key(x.get("ps")) for x in files]
    pss = [p for p in pss if p is not None]
    g["ps"] = list(min(pss)) if pss else None
    g["max_frames"] = max(int(x.get("frames") or 1) for x in files)
    g["files"] = [{"ipp": x.get("ipp"), "inum": x.get("inum"), "index": i} for i, x in enumerate(files)]
    _derive(g)
    return g


def _derive(g: dict) -> None:
    f = frame_of(g.get("iop"))
    g["kind"] = stack_kind(g)
    g["enhanced"] = int((g.get("max_frames") or 1) > 1 or g.get("sop_class") == ENHANCED_MR)
    if f is None:
        return
    plane, tilt = orient_of(f[2])
    g["plane"], g["tilt"] = plane, round(tilt, 3)
    g["n_abs"] = [round(float(x), 4) for x in np.abs(f[2])]
    k = box_of(g)
    if k is None:
        return
    lo, hi = k.min(0), k.max(0)
    g["ext"] = [round(float(x), 2) for x in (hi - lo)]
    g["z_lo"], g["z_hi"] = round(float(lo[2]), 2), round(float(hi[2]), 2)
    g["span_n"] = round(float((g.get("d_max") or 0) - (g.get("d_min") or 0)), 2)


def is_mr(g: dict) -> bool:
    return str(g.get("modality") or "").upper() == "MR"


def session_tops(rows: list[dict]) -> None:
    """The heights relative to the session's top, in place: within each
    study, over its unsealed MR stacks with a box (the research's frame).
    ``rows`` holds the run's stacks and the manifest's context (the rest of
    each session), so the top is the whole study's in any run."""
    by_study = defaultdict(list)
    for g in rows:
        if "z_lo" in g and is_mr(g) and not g.get("sealed") and g.get("study_id") is not None:
            by_study[g.get("study_id")].append(g)
    for gs in by_study.values():
        top = max(x["z_hi"] for x in gs)
        for x in gs:
            x["z_lo_rel_top"] = round(x["z_lo"] - top, 2)
            x["z_hi_rel_top"] = round(x["z_hi"] - top, 2)


# --- cache.py: the tokens and the tile --------------------------------------------------------------------------------

def stack_tokens(g: dict) -> np.ndarray:
    def f(x, s=1.0):
        try:
            return float(x) / s if x is not None else 0.0
        except (TypeError, ValueError):
            return 0.0

    ext = g.get("ext") or [0, 0, 0]
    na = g.get("n_abs") or [0, 0, 0]
    t = [*(float(g.get("plane") == p) for p in PLANES), *(float(g.get("kind") == k) for k in KINDS), f(g.get("tilt"), 45), *map(float, na),
         *(e / 500 for e in ext), f(g.get("span_n"), 500), np.log1p(g.get("n_pos") or 0) / 6, np.log1p(g.get("n_files") or 0) / 6,
         f(g.get("fov_x"), 500), f(g.get("fov_y"), 500), f(g.get("thick"), 10), f(g.get("field"), 7), f(g.get("z_lo_rel_top"), 500),
         f(g.get("z_hi_rel_top"), 500), float(g.get("enhanced") or 0), float((g.get("n_inst_reg") or 0) > (g.get("n_pos") or 0) > 0)]
    return np.array(t, dtype=np.float32)


def slice_geo(g: dict, x: dict, inf_at, slot: int, m: int, pos_index: int, top) -> np.ndarray:
    f = frame_of(g.get("iop"))
    p = vec(x.get("ipp"), 3)
    ps = spacing(g.get("ps"))
    if f is None or p is None:
        return np.array([0.0, 0.0, 0.5 if m <= 1 else pos_index / (m - 1), 0.0], np.float32)
    d = float(p @ f[2])
    c = ((g.get("d_min") or d) + (g.get("d_max") or d)) / 2
    zl = 0.0
    if ps is not None and g.get("rows") and g.get("cols") and top is not None:
        zl = (float(slice_corners(p, f[0], f[1], g["rows"], g["cols"], ps)[:, 2].min()) - top) / 500
    return np.array([(d - c) / 100, zl, 0.5 if m <= 1 else pos_index / (m - 1), float(slot == inf_at)], np.float32)


def slot0(g: dict) -> tuple[int, np.ndarray] | None:
    """The slot-0 file (its index in the manifest's list) and its four
    geometry tokens, as ``cache.plan`` chose and placed it; None for a stack
    the student never reads (no files, or a multi-frame file)."""
    fl = g.get("files") or []
    if not fl or (g.get("max_frames") or 1) > 1:
        return None
    chosen, inf_at = select(g, fl, 1)
    if not chosen:
        return None
    x = chosen[0]
    top = (g["z_hi"] - g["z_hi_rel_top"]) if "z_hi_rel_top" in g else None
    f_ = frame_of(g.get("iop"))
    ds_ = sorted({round(float(vec(y["ipp"], 3) @ f_[2]), 1) for y in fl if vec(y.get("ipp"), 3) is not None}) if f_ is not None else []
    m = len(ds_) or len(fl)
    pi = 0
    if f_ is not None and vec(x.get("ipp"), 3) is not None and ds_:
        pi = int(np.searchsorted(ds_, round(float(vec(x["ipp"], 3) @ f_[2]), 1)))
    return x["index"], slice_geo(g, x, inf_at, 0, m, pi, top)


def to_tile(a, ps) -> np.ndarray:
    """One decoded slice, windowed at its 0.5th and 99.5th percentile,
    resampled to 1.5 mm a pixel and cropped or padded to 192 x 192."""
    from PIL import Image

    a = np.asarray(a, dtype=np.float32)
    if a.ndim == 3:
        a = a.mean(-1) if a.shape[-1] in (3, 4) else a[0]
    sub = a[:: max(1, a.shape[0] // 128), :: max(1, a.shape[1] // 128)]
    lo, hi = np.percentile(sub, 0.5), np.percentile(sub, 99.5)
    a = np.clip((a - lo) / max(1e-6, hi - lo), 0, 1) * 255
    h, w = a.shape
    im = Image.fromarray(a.astype(np.uint8)).resize((max(1, int(round(w * ps[1] / MM))), max(1, int(round(h * ps[0] / MM)))), Image.BILINEAR)
    W, H = im.size
    out = np.zeros((PX, PX), np.uint8)
    x0, y0 = (W - PX) // 2, (H - PX) // 2
    crop = np.asarray(im.crop((max(0, x0), max(0, y0), max(0, x0) + min(PX, W), max(0, y0) + min(PX, H))))
    out[max(0, -y0) : max(0, -y0) + crop.shape[0], max(0, -x0) : max(0, -x0) + crop.shape[1]] = crop
    return out


def read_tile(path: str) -> tuple[np.ndarray, dict]:
    """The slot-0 file read whole, as ``cache.work`` read it: its tile and
    its identity (SOP Instance UID, the sha256 of its bytes and its study's
    UID) for the slice cache. Raises where the file cannot be decoded."""
    import hashlib

    import pydicom

    with open(path, "rb") as f:
        data = f.read()
    ds = pydicom.dcmread(io.BytesIO(data))
    ps = spacing("\\".join(str(x) for x in getattr(ds, "PixelSpacing", [1, 1]))) if hasattr(ds, "PixelSpacing") else None
    tile = to_tile(ds.pixel_array, ps if ps is not None else np.array([1.0, 1.0]))
    ident = {"sop_instance_uid": str(getattr(ds, "SOPInstanceUID", "") or ""), "sha256": hashlib.sha256(data).hexdigest(),
             "study_instance_uid": str(getattr(ds, "StudyInstanceUID", "") or "")}
    return tile, ident


# --- mimic_bp.py and pack_bp.py: the registry's geometry numbers and header features ------------------------------------

def geo_feats(g: np.ndarray) -> np.ndarray:
    """``mimic_bp.geo_feats``: the student's view of the encoder's 15 geometry numbers."""
    g = np.nan_to_num(np.asarray(g, np.float32))
    return np.concatenate([np.log1p(np.maximum(g[:, :10], 0)), g[:, 10:13], g[:, 13:14] / 90.0, g[:, 14:15]], 1).astype(np.float32)


def registry_inputs(stack, model):
    """``pack_bp.registry_only``: the encoder's 15 geometry numbers, the 44
    header features and the cohort from the manifest alone, with no file
    read; None where a file's geometry cannot stand in for its header."""
    from . import fusion, header_features, reduced, volume

    header = stack.extra.get("header") if isinstance(stack.extra.get("header"), dict) else {}
    fp = header.get("fingerprint") if isinstance(header.get("fingerprint"), dict) else None
    sg = stack.extra.get("geometry") if isinstance(stack.extra.get("geometry"), dict) else {}
    if fp is None or not stack.file_geo:
        return None
    try:
        frames, seen = [], set()
        for (path, which), g in zip(stack.files, stack.file_geo):
            if path in seen:
                continue
            seen.add(path)
            f = reduced.registry_frame(path, g, sg) if which is None else None
            if f is None:
                return None
            frames.append(f)
        p = reduced.plan(frames, fp.get("orientation"), "touched")
        g0 = p.geo[0]
        meta0 = dict(modality=g0.modality, n_files=len(p.seen), n_frames=p.n_frames_all, n_unique=p.n_unique, rows0=g0.shape[0], cols0=g0.shape[1],
                     ps_r=round(g0.ps[0], 4), ps_c=round(g0.ps[1], 4))
        meta = reduced._meta(p, meta0)
        geo = volume.Built(None, meta).geo()
        h44 = header_features.features(fp, header.get("classification"), meta)
        cohort, where = fusion.resolve_cohort(header, model)
    except Exception:  # noqa: BLE001 - a stack the registry cannot describe goes to the planes
        return None
    return np.asarray(geo, np.float32), np.asarray(h44, np.float32), cohort or "", where, meta
