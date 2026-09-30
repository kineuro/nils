# SPDX-License-Identifier: AGPL-3.0-only
"""A stack as the certified body-part model sees it (record 50): its 8 mm
volume in patient axes and its geometry, preprocessing
``bpthumb-vol-8mm-64:v1``.

The frames are read as the model's training read them:

- every frame of the stack's files (the frames the manifest lists for a
  multi-frame file) with its position, orientation and pixel spacing, from
  the file's header or, in a multi-frame file, its functional groups;
- the files training listed for the stack (``select_v1.py``, ``r5_fresh.py``
  and ``r7_drawn.py`` in the design repository): the files ordered along the
  stack's normal (instance number, then path, breaks a tie), one per
  position (more than 0.05 mm on from the last one kept), at most 96 of
  them kept evenly, and v0's slices beside them (the centre three, and five
  at 20 to 80 % of an axial stack); the frames of the other files are not
  read, and their positions do not count;
- the frames with all three kept, then the orientation most of them have
  (rounded to two decimals) and the shape most of those have;
- ordered along the normal (instance number breaks a tie), one frame per
  position (0.05 mm apart or more), and at most 96 of them kept evenly;
- only the kept frames decoded: RGB to its mean, rescaled by slope and
  intercept, shrunk to at most 256 pixels a side (box filter).

The volume is those frames resampled trilinearly at 8 mm into a cube of 64
voxels a side centred on the stack's box, ``vol[z S->I, y A->P, x R->L]``,
windowed at the stack's percentiles 0.5 and 99.5 (of its foreground) to
uint8 and zero where the stack has no support. The geometry is the stack's
extent along each patient axis, its field of view, span, slice spacing and
thickness, how many slices it has and kept, its normal and obliquity, and
its modality, each rounded as training rounded it.

A stack whose frames have no position, orientation or spacing, or whose
orientation is degenerate, has no geometry and gets no answer. Nothing here
logs a path or a header value.
"""

from __future__ import annotations

import collections
import math
from dataclasses import dataclass, field

import numpy as np

PREPROCESSING = "bpthumb-vol-8mm-64:v1"
MAXK = 96  # the frames kept along the normal
MAXDIM = 256  # the in-plane size a frame is shrunk to at most
N = 64  # voxels a side
STEP = 8.0  # mm a voxel

# The fifteen geometry numbers the encoder reads, in its order; the last is
# 1 for CT and 0 otherwise.
GEO_KEYS = ("ext_x", "ext_y", "ext_z", "fov_r", "fov_c", "span", "spacing", "thick", "K", "n_unique", "nx", "ny", "nz", "oblique_deg")


class NoGeometry(Exception):
    """The stack has no geometry the volume can be built from."""


class Unreadable(Exception):
    """No file of the stack could be read at all."""


@dataclass
class Frame:
    path: str
    index: int  # the frame within its file, from 0
    shape: tuple[int, int]
    ipp: list[float] | None
    iop: list[float] | None
    ps: list[float] | None
    thick: float | None
    inum: int
    modality: str
    instance: int = 0  # the file's InstanceNumber


@dataclass
class Built:
    vol: np.ndarray  # (64, 64, 64) uint8
    meta: dict
    errors: dict = field(default_factory=dict)

    def geo(self) -> np.ndarray:
        """The fifteen raw geometry numbers, float32, from the rounded meta."""
        g = [float(self.meta[k]) if self.meta.get(k) not in (None, "") else 0.0 for k in GEO_KEYS]
        return np.array(g + [1.0 if self.meta.get("modality") == "CT" else 0.0], np.float32)


def _floats(x, n=None):
    try:
        v = [float(t) for t in x]
        return v if n is None or len(v) == n else None
    except Exception:
        return None


def _fg_get(perframe, shared, i, seqname, attr):
    for src in ((perframe[i] if perframe is not None and i < len(perframe) else None), (shared[0] if shared else None)):
        if src is None:
            continue
        s = getattr(src, seqname, None)
        if s and hasattr(s[0], attr):
            return getattr(s[0], attr)
    return None


def read_header_frames(path: str, frames: list[int] | None) -> list[Frame]:
    """The frames of one file with their geometry, from its header alone."""
    import pydicom

    ds = pydicom.dcmread(path, stop_before_pixels=True, force=True)
    nf = int(getattr(ds, "NumberOfFrames", 1) or 1)
    rows, cols = int(ds.Rows), int(ds.Columns)
    mod = str(getattr(ds, "Modality", "") or "")
    shared = getattr(ds, "SharedFunctionalGroupsSequence", None)
    perframe = getattr(ds, "PerFrameFunctionalGroupsSequence", None)
    wanted = range(nf) if frames is None else sorted({i for i in frames if 0 <= i < nf})
    out = []
    inum = int(getattr(ds, "InstanceNumber", 0) or 0)
    for i in wanted:
        ipp = _floats(getattr(ds, "ImagePositionPatient", None) or [], 3)
        iop = _floats(getattr(ds, "ImageOrientationPatient", None) or [], 6)
        ps = _floats(getattr(ds, "PixelSpacing", None) or [], 2)
        thick = getattr(ds, "SliceThickness", None)
        if nf > 1:
            ipp = _floats(_fg_get(perframe, shared, i, "PlanePositionSequence", "ImagePositionPatient") or [], 3) or ipp
            iop = _floats(_fg_get(perframe, shared, i, "PlaneOrientationSequence", "ImageOrientationPatient") or [], 6) or iop
            ps = _floats(_fg_get(perframe, shared, i, "PixelMeasuresSequence", "PixelSpacing") or [], 2) or ps
            thick = _fg_get(perframe, shared, i, "PixelMeasuresSequence", "SliceThickness") or thick
        try:
            thick = float(thick)
        except Exception:
            thick = None
        out.append(Frame(path, i, (rows, cols), ipp, iop, ps, thick, inum * 10000 + i, mod, inum))
    return out


def decode_frames(path: str, indices: list[int], ds=None, plugin: str = "") -> dict[int, np.ndarray]:
    """The listed frames of one file, rescaled, float32: RGB (or YBR) to its
    mean, then slope and intercept. ``ds`` is the file already read, and
    ``plugin`` the pydicom decoding plugin to use (the GPU path's, which gives
    the same pixels); by default pydicom chooses, as it always has."""
    import pydicom

    if ds is None:
        ds = pydicom.dcmread(path, force=True)
    photo = str(getattr(ds, "PhotometricInterpretation", "") or "")
    nf = int(getattr(ds, "NumberOfFrames", 1) or 1)
    slope = float(getattr(ds, "RescaleSlope", 1) or 1)
    icpt = float(getattr(ds, "RescaleIntercept", 0) or 0)
    colour = photo.startswith("RGB") or photo.startswith("YBR")

    def finish(arr: np.ndarray) -> np.ndarray:
        return arr.astype(np.float32) * slope + icpt

    out: dict[int, np.ndarray] = {}
    if nf == 1:
        if plugin:
            ds.pixel_array_options(decoding_plugin=plugin)
        arr = ds.pixel_array
        if colour or (arr.ndim == 3 and arr.shape[-1] == 3):
            arr = arr.astype(np.float32).mean(axis=-1)
        if arr.ndim == 3:
            frames = [arr[i] for i in range(arr.shape[0])]
        elif arr.ndim == 2:
            frames = [arr]
        else:
            frames = [arr.reshape(arr.shape[-2], arr.shape[-1])]
        for i in indices:
            if i < len(frames):
                out[i] = finish(frames[i])
        return out
    from pydicom.pixels import pixel_array

    for i in indices:
        arr = pixel_array(ds, index=i, decoding_plugin=plugin)
        if colour:
            arr = arr.astype(np.float32).mean(axis=-1)
        out[i] = finish(arr)
    return out


def shrink(img: np.ndarray, ps: list[float], maxdim: int = MAXDIM):
    from PIL import Image

    h, w = img.shape
    m = max(h, w)
    if m <= maxdim:
        return img, ps
    sc = maxdim / m
    nh, nw = max(1, int(round(h * sc))), max(1, int(round(w * sc)))
    im = Image.fromarray(img.astype(np.float32), mode="F").resize((nw, nh), Image.Resampling.BOX)
    return np.asarray(im, dtype=np.float32), [ps[0] * h / nh, ps[1] * w / nw]


def to_u8(x, lo, hi):
    y = (x - lo) / max(hi - lo, 1e-6)
    return (np.clip(y, 0, 1) * 255).astype(np.uint8)


def resample(V, o, M, pts, tol_k):
    """Values of V (K, H, W) at LPS points (N, 3): trilinear, 0 outside; and the support."""
    from scipy.ndimage import map_coordinates

    Minv = np.linalg.inv(M)
    idx = (pts - o) @ Minv.T
    K, H, W = V.shape
    k = idx[:, 0]
    ok = (idx[:, 1] >= -0.5) & (idx[:, 1] <= H - 0.5) & (idx[:, 2] >= -0.5) & (idx[:, 2] <= W - 0.5)
    ok &= (k >= -tol_k) & (k <= K - 1 + tol_k)
    kk = np.clip(k, 0, K - 1)
    vals = map_coordinates(V, [kk, idx[:, 1], idx[:, 2]], order=1, mode="nearest")
    vals = np.where(ok, vals, 0.0)
    return vals.astype(np.float32), ok


def v0_indices(n: int, orientation: str | None) -> list[int]:
    """v0's slices of a stack of ``n`` files (``select_v1.py``): the centre
    three, and for an axial stack five at 20 to 80 %."""
    if n <= 0:
        return []
    want = set()
    mid = n // 2
    for i in (mid - 1, mid, mid + 1):
        want.add(max(0, min(n - 1, i)))
    if (orientation or "").lower() == "axial":
        for f in (0.20, 0.35, 0.50, 0.65, 0.80):
            want.add(max(0, min(n - 1, int(f * n))))
    return sorted(want)


def preselect(frames: list[Frame], orientation: str | None) -> set[str]:
    """The files training read of a stack (``select_v1.py``): one item per
    file at its first frame's position along the stack's normal (0 without
    a position), sorted with the instance number and the path; one per
    position, more than 0.05 mm from the last one kept, at most 96 kept
    evenly; and v0's slices of the sorted files beside them."""
    first: dict[str, Frame] = {}
    for f in frames:
        first.setdefault(f.path, f)
    with_iop = [f for f in first.values() if f.iop is not None]
    nrm = np.array([0.0, 0.0, 1.0])
    if with_iop:
        key = collections.Counter(tuple(np.round(f.iop, 2)) for f in with_iop).most_common(1)[0][0]
        iop = next(f.iop for f in with_iop if tuple(np.round(f.iop, 2)) == key)
        nrm = np.cross(iop[:3], iop[3:])
    items = sorted((float(np.dot(f.ipp, nrm)) if f.ipp is not None else 0.0, f.instance, p) for p, f in first.items())
    v0i = set(v0_indices(len(items), orientation))
    uniq, last = [], None
    for i, it in enumerate(items):
        if last is None or abs(it[0] - last) > 0.05:
            uniq.append(i)
            last = it[0]
    if len(uniq) > MAXK:
        uniq = [uniq[int(round(x))] for x in np.linspace(0, len(uniq) - 1, MAXK)]
    return {items[i][2] for i in set(uniq) | v0i}


def _select(frames: list[Frame]):
    """The kept frames along the normal, or why there are none."""
    geo = [f for f in frames if f.ipp is not None and f.iop is not None and f.ps is not None]
    if not geo:
        raise NoGeometry("no frame has a position, an orientation and a pixel spacing")
    iop_key = collections.Counter(tuple(np.round(f.iop, 2)) for f in geo).most_common(1)[0][0]
    shp = collections.Counter(f.shape for f in geo if tuple(np.round(f.iop, 2)) == iop_key).most_common(1)[0][0]
    geo = [f for f in geo if tuple(np.round(f.iop, 2)) == iop_key and f.shape == shp]
    iop = np.array(geo[0].iop)
    r_dir, c_dir = iop[:3], iop[3:]
    n = np.cross(r_dir, c_dir)
    nn = np.linalg.norm(n)
    if nn < 1e-3:
        raise NoGeometry("the orientation is degenerate")
    n /= nn
    pos = [float(np.dot(f.ipp, n)) for f in geo]
    order = np.argsort(np.array(pos) + 1e-9 * np.array([f.inum for f in geo]))
    uniq, last = [], None
    for oi in order:
        pv = round(pos[oi], 2)
        if last is None or abs(pv - last) > 0.05:
            uniq.append(oi)
            last = pv
    n_frames_all, n_unique = len(geo), len(uniq)
    if len(uniq) > MAXK:
        uniq = [uniq[int(round(i))] for i in np.linspace(0, len(uniq) - 1, MAXK)]
    return geo, uniq, n, r_dir, c_dir, n_frames_all, n_unique


def decode_each(need: dict[str, list[int]]) -> dict[str, dict[int, np.ndarray] | Exception]:
    """The frames each file must give, decoded one file after another on the
    CPU: per file its frames, or the exception that stopped it."""
    out: dict[str, dict[int, np.ndarray] | Exception] = {}
    for path, idx in need.items():
        try:
            out[path] = decode_frames(path, idx)
        except Exception as e:  # noqa: BLE001 - counted by kind by the caller
            out[path] = e
    return out


def build(files: list[tuple[str, list[int] | None]], orientation: str | None = None, decode=None) -> Built:
    """The volume and geometry of a stack from its files in order, each with
    the frames that are the stack's (None for every frame). ``orientation``
    is the stack's fingerprint orientation, which chooses v0's slices among
    the files read (:func:`preselect`). ``decode`` decodes the frames the
    volume needs (:func:`decode_each`, the default, or the GPU path's, which
    gives the same pixels)."""
    decode = decode or decode_each
    errors: collections.Counter = collections.Counter()
    frames: list[Frame] = []
    seen: set[str] = set()
    for path, which in files:
        if path in seen:
            continue
        try:
            got = read_header_frames(path, which)
        except Exception as e:  # noqa: BLE001 - counted by kind, never named
            errors["read:" + type(e).__name__] += 1
            continue
        seen.add(path)
        frames.extend(got)
    if not frames:
        raise Unreadable("no file of the stack could be read")
    chosen = preselect(frames, orientation)
    frames = [f for f in frames if f.path in chosen]
    seen = seen & chosen
    # A file whose kept frames cannot be decoded is left out and the frames
    # chosen again, as a file that could not be read at all is.
    bad: set[str] = set()
    while True:
        usable = [f for f in frames if f.path not in bad]
        if not usable:
            raise Unreadable("no frame of the stack could be decoded")
        geo, uniq, n, r_dir, c_dir, n_frames_all, n_unique = _select(usable)
        need: dict[str, list[int]] = {}
        for oi in uniq:
            need.setdefault(geo[oi].path, []).append(geo[oi].index)
        pixels: dict[tuple[str, int], np.ndarray] = {}
        failed = False
        decoded = decode(need)
        for path, idx in need.items():
            got = decoded[path]
            if isinstance(got, Exception):
                errors["decode:" + type(got).__name__] += 1
                bad.add(path)
                failed = True
                continue
            if any(i not in got for i in idx):
                errors["decode:frames"] += 1
                bad.add(path)
                failed = True
                continue
            for i in idx:
                if got[i].shape != geo[0].shape:
                    errors["decode:shape"] += 1
                    bad.add(path)
                    failed = True
                    break
                pixels[(path, i)] = got[i]
        if not failed:
            break
    mod = geo[0].modality
    ps0 = geo[0].ps
    thick = geo[0].thick or 0.0
    imgs, ps = [], None
    for oi in uniq:
        im, ps = shrink(pixels[(geo[oi].path, geo[oi].index)], geo[oi].ps)
        imgs.append(im)
    V = np.stack(imgs).astype(np.float32)
    K = V.shape[0]
    o = np.array(geo[uniq[0]].ipp, dtype=np.float64)
    if K > 1:
        sv = (np.array(geo[uniq[-1]].ipp) - o) / (K - 1)
    else:
        sv = n * max(thick, 1.0)
    spacing = float(np.linalg.norm(sv))
    M = np.stack([sv, c_dir * ps[0], r_dir * ps[1]], axis=1)  # columns: k, i, j
    meta0 = dict(
        modality=mod,
        n_files=len(seen),
        n_frames=n_frames_all,
        n_unique=n_unique,
        rows0=geo[0].shape[0],
        cols0=geo[0].shape[1],
        ps_r=round(ps0[0], 4),
        ps_c=round(ps0[1], 4),
    )
    vol, meta = represent(V, o, M, n, thick, spacing, meta0)
    return Built(vol, meta, dict(errors))


def represent(V, o, M, n, thick, spacing, meta0):
    """The 8 mm volume and the geometry of a stack volume V (K, H, W) whose
    voxel (k, i, j) sits at o + M @ (k, i, j) in LPS."""
    K, H, W = V.shape
    tol_k = 0.5 if K > 1 else max(0.5, (thick or 1.0) / 2.0 / max(spacing, 1e-3))
    corners = np.array([o + M @ np.array([k, i, j]) for k in (0, K - 1) for i in (0, H - 1) for j in (0, W - 1)])
    if K == 1:
        corners = np.concatenate([corners - n * thick / 2, corners + n * thick / 2])
    lo_c, hi_c = corners.min(0), corners.max(0)
    ctr = (lo_c + hi_c) / 2
    ext = hi_c - lo_c
    fg_vals = V[V > np.percentile(V, 5)] if V.size > 100 else V.ravel()
    ilo, ihi = np.percentile(V, 0.5), np.percentile(fg_vals if fg_vals.size else V, 99.5)
    g = (np.arange(N) - (N - 1) / 2.0) * STEP
    Z, Y, X = np.meshgrid(g[::-1], g, g, indexing="ij")  # vol[z(S->I), y(A->P), x(R->L)]
    P = np.stack([X.ravel(), Y.ravel(), Z.ravel()], 1) + ctr
    vals, ok = resample(V, o, M, P, tol_k)
    vol = (to_u8(vals, ilo, ihi) * ok).reshape(N, N, N)
    nabs = np.abs(n)
    meta = dict(meta0)
    meta.update(
        K=K,
        H=H,
        W=W,
        thick=round(thick or 0, 3),
        spacing=round(spacing, 3),
        nx=round(nabs[0], 4),
        ny=round(nabs[1], 4),
        nz=round(nabs[2], 4),
        oblique_deg=round(math.degrees(math.acos(min(1.0, nabs.max()))), 2),
        ext_x=round(ext[0], 1),
        ext_y=round(ext[1], 1),
        ext_z=round(ext[2], 1),
        fov_r=round(meta0["rows0"] * meta0["ps_r"], 1),
        fov_c=round(meta0["cols0"] * meta0["ps_c"], 1),
        span=round(K * spacing if K > 1 else (thick or 0), 1),
    )
    # plain numbers, as training read them back from its tables
    meta = {k: (v.item() if isinstance(v, np.generic) else v) for k, v in meta.items()}
    return vol.astype(np.uint8), meta
