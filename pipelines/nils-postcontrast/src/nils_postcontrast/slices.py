# SPDX-License-Identifier: AGPL-3.0-only
"""Stage 1's input: three native slices of a stack, chosen from the
registry's geometry alone, and the stack's thirteen tokens.

Every function here is the speed study's code (round 4, the `fs` student:
`stack_geometry`, `fs_pick`, `stack_tokens`, `tile`), ported line for line,
because the student was trained on exactly these inputs:

- the stack's files are put in order along the slice normal (the normal is
  the cross product of the stack's ImageOrientationPatient rows), a file
  whose position along it repeats one already seen (within 0.05 mm, by
  rounding) is dropped, and a stack with a multi-frame file, or a file with
  no position, or no orientation, has no input;
- an axial stack's targets are fractions of the top 150 mm of its extent
  (oriented superior); a sagittal or coronal one's are its middle, the
  middle 5 mm either side, then quarter and eighth points; the nearest
  unpicked slice to each target is taken, the first k for the k-slice
  student;
- a slice is decoded whole, scaled by its own 1st and 99th percentiles
  (over every second pixel) and clipped to [-0.5, 4], resampled to 1 mm
  pixels (bilinear) and centred on a 256 x 256 grid, as float16.
"""

from __future__ import annotations

import math

import numpy as np

PX = 256  # the tile, in 1 mm pixels
KMAX = 8
NSG = 4  # a slice's tokens
NTOK = 13  # a stack's tokens
AXF = [0.5, 0.25, 0.75, 0.125, 0.875, 0.375, 0.625, 0.0625]


def vec(v, n: int):
    """A DICOM multi-value as n finite numbers, else None. Takes the
    registry's text (``a\\b\\c``) or the engine's parsed list."""
    if v is None:
        return None
    if isinstance(v, (list, tuple)):
        try:
            a = np.array([float(x) for x in v])
        except (TypeError, ValueError):
            return None
    else:
        try:
            a = np.array([float(x) for x in str(v).replace(",", "\\").split("\\")])
        except ValueError:
            return None
    return a if len(a) == n and np.all(np.isfinite(a)) else None


def stack_geometry(iop, files, multi_frame: bool):
    """The files in order along the slice normal.

    ``files`` is a list of ``(path, ipp, ps, rows, cols, inum, nframes)`` in
    the registry's order (instance number, then path); None when there is no
    usable geometry or the stack holds a multi-frame file."""
    if multi_frame:
        return None
    o = vec(iop, 6)
    if o is None:
        return None
    r, c = o[:3], o[3:]
    if np.linalg.norm(r) < 0.5 or np.linalg.norm(c) < 0.5:
        return None
    n = np.cross(r / np.linalg.norm(r), c / np.linalg.norm(c))
    if np.linalg.norm(n) < 0.5:
        return None
    n = n / np.linalg.norm(n)
    items, seen = [], set()
    for path, ipp, ps, rows, cols, inum, nf in files:
        p = vec(ipp, 3)
        if p is None or (nf or 1) > 1:
            return None
        s = float(p @ n)
        key = round(s / 0.05)
        if key in seen:
            continue
        seen.add(key)
        items.append((s, path, vec(ps, 2), rows, cols, inum))
    if not items:
        return None
    items.sort(key=lambda t: t[0])
    ab = np.abs(n)
    i = int(np.argmax(ab))
    return {"n": n, "items": items, "plane": ("sagittal", "coronal", "axial")[i], "tilt": math.degrees(math.acos(min(1.0, float(ab[i]))))}


def fs_pick(G, kmax: int = KMAX):
    """[(item index, slice tokens)] in the rule's order, at most kmax."""
    s = np.array([t[0] for t in G["items"]])
    lo_s, hi_s = float(s.min()), float(s.max())
    mid = (lo_s + hi_s) / 2
    if G["plane"] == "axial":
        sup = s if G["n"][2] >= 0 else -s
        top = float(sup.max())
        lo = max(float(sup.min()), top - 150.0)
        targets = [(lo + f * (top - lo), f) for f in AXF]
        coord = sup
    else:
        e = hi_s - lo_s
        fr = lambda x: (x - lo_s) / e if e > 0 else 0.5  # noqa: E731
        targets = [(mid, 0.5), (mid - 5.0, fr(mid - 5.0)), (mid + 5.0, fr(mid + 5.0))] + [(lo_s + f * e, f) for f in (0.25, 0.75, 0.375, 0.625, 0.125)]
        coord, top = s, None
    used, out = set(), []
    for t, f in targets[:kmax]:
        order = np.argsort(np.abs(coord - t), kind="stable")
        j = next((int(x) for x in order if int(x) not in used), None)
        if j is None:
            break
        used.add(j)
        sg = [float(f), (s[j] - mid) / 100.0, ((top - coord[j]) / 150.0) if top is not None else 0.0, 1.0]
        out.append((j, sg))
    return out


def stack_tokens(G, acq3d: float, thick: float) -> list:
    s = np.array([t[0] for t in G["items"]])
    sp = float(np.median(np.diff(s))) if len(s) > 1 else 0.0
    ps, rows, cols = G["items"][0][2], G["items"][0][3] or 0, G["items"][0][4] or 0
    psm = float(np.mean(ps)) if ps is not None else 0.0
    fr = float(rows * ps[0]) if ps is not None else 0.0
    fc = float(cols * ps[1]) if ps is not None else 0.0
    pl = G["plane"]
    return [float(pl == "axial"), float(pl == "sagittal"), float(pl == "coronal"), G["tilt"] / 45.0, sp / 10.0, (thick or 0.0) / 10.0,
            float(s.max() - s.min()) / 200.0, fr / 300.0, fc / 300.0, psm / 2.0, math.log1p(len(s)) / 6.0, float(acq3d), 0.0]


def tile(path: str):
    """One native slice as the student reads it, or None for a multi-frame file."""
    import pydicom
    from PIL import Image

    ds = pydicom.dcmread(path)
    if int(getattr(ds, "NumberOfFrames", 1) or 1) > 1:
        return None
    a = np.asarray(ds.pixel_array, dtype=np.float32)
    if a.ndim == 3:
        a = a.mean(-1) if a.shape[-1] in (3, 4) else a[0]
    try:
        ps = [float(x) for x in ds.PixelSpacing]
    except Exception:  # noqa: BLE001 - no spacing: 1 mm assumed
        ps = [1.0, 1.0]
    p1, p99 = np.percentile(a[::2, ::2], [1, 99])
    x = np.clip((a - p1) / max(float(p99 - p1), 1e-6), -0.5, 4.0).astype(np.float32)
    h, w = x.shape
    im = Image.fromarray(x).resize((max(1, int(round(w * ps[1]))), max(1, int(round(h * ps[0])))), Image.BILINEAR)
    y = np.asarray(im, dtype=np.float32)
    out = np.zeros((PX, PX), np.float32)
    H, Wd = y.shape
    y0, x0 = (H - PX) // 2, (Wd - PX) // 2
    crop = y[max(0, y0) : max(0, y0) + min(PX, H), max(0, x0) : max(0, x0) + min(PX, Wd)]
    out[max(0, -y0) : max(0, -y0) + crop.shape[0], max(0, -x0) : max(0, -x0) + crop.shape[1]] = crop
    return out.astype(np.float16)


def stack_input(G, k: int, acq3d: float, thick: float, read=tile):
    """The student's input for one stack: (tiles k x 256 x 256 float16, valid
    k bools, slice tokens k x 4, stack tokens 13, files opened), or None with
    the files opened when no slot holds a slice. A slot whose file cannot be
    read stays empty, as the study's cache left it."""
    picks = fs_pick(G, k)
    tiles = np.zeros((k, PX, PX), np.float16)
    valid = np.zeros(k, bool)
    sg = np.zeros((k, NSG), np.float32)
    opened = 0
    for slot, (j, s) in enumerate(picks):
        opened += 1
        try:
            t = read(G["items"][j][1])
        except Exception:  # noqa: BLE001 - an empty slot, as in the study
            t = None
        if t is not None:
            tiles[slot], valid[slot], sg[slot] = t, True, s
    if not valid.any():
        return None, opened
    return (tiles, valid, sg, np.array(stack_tokens(G, acq3d, thick), np.float32)), opened
