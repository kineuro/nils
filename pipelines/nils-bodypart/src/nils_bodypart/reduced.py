# SPDX-License-Identifier: AGPL-3.0-only
"""The certified body-part model's input read from the frames it needs
(nils-design study ``2026-10-02-bodypart-one-hour``).

The encoder (``tiny.planes``) reads six planes of the stack's 8 mm volume:
the two axial planes next to its centre (``vol[31]``, ``vol[32]``), the two
coronal (``vol[:, 31]``, ``vol[:, 32]``) and the two sagittal
(``vol[:, :, 31]``, ``vol[:, :, 32]``), each 64 by 64 points 8 mm apart.
Nothing else of the 64-voxel cube reaches the network. Each point is
interpolated trilinearly between two kept frames, so a point needs two
frames, and a plane parallel to the slices needs two to four frames in all.
A plane across the slices needs a pair of frames per 8 mm row it crosses.

This reader builds those six planes, and nothing else, from:

- the geometry of every file from the stack's manifest (the registry's
  ``ImagePositionPatient``, ``PixelSpacing``, ``Rows``, ``Columns``,
  ``InstanceNumber`` per file, and the stack's ``ImageOrientationPatient``,
  ``SliceThickness`` and modality), so no header is read to choose frames;
  a file the manifest gives no geometry for, or that holds more than one
  frame, has its header read as ``volume.build`` reads it;
- the same choice of kept frames as ``volume.build`` (``volume.preselect``
  and ``volume._select``: one per position, at most 96), so the stack's
  geometry and the planes' points are the full reader's, number for number;
- only the frames a policy names, decoded and shrunk as ``volume.build``
  decodes and shrinks them.

Policies:

- ``full``: every kept frame. The planes and the window are ``volume.build``'s,
  bit for bit; only the headers are not read.
- ``touched``: every kept frame that a plane point interpolates from. The
  planes' values are exact; the window (percentiles 0.5 and 99.5) is taken
  over the frames read, not over every kept frame.
- ``b<N>`` (``b8``, ``b16``, ...): at most N frames. When the touched frames
  are N or fewer, they are read (``touched``). Otherwise: the frames the two
  planes parallel to the slices touch (two to four; one nearest frame per
  plane when N is small), and the rest spread evenly over the touched range,
  its two ends included. A touched frame that is not read is the linear
  blend of the nearest frames read on either side, by kept index. The window
  is taken over the frames read.

Every count it keeps is a count: no path or header value is logged.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass, field

import numpy as np

from . import volume
from .volume import MAXDIM, STEP, Built, Frame, N, NoGeometry, Unreadable

# The six planes the encoder reads: index 31 and 32 of each axis.
_MID = (31, 32)
_MASK = np.zeros((N, N, N), bool)
_MASK[_MID[0]] = _MASK[_MID[1]] = True
_MASK[:, _MID[0]] = _MASK[:, _MID[1]] = True
_MASK[:, :, _MID[0]] = _MASK[:, :, _MID[1]] = True
_FLAT = np.flatnonzero(_MASK.ravel())  # the plane points in vol's C order
_IZ, _IY, _IX = np.unravel_index(_FLAT, (N, N, N))
# per plane pair, which of the plane points lie on it
PAIRS = {"axial": np.isin(_IZ, _MID), "coronal": np.isin(_IY, _MID), "sagittal": np.isin(_IX, _MID)}
_G = (np.arange(N) - (N - 1) / 2.0) * STEP
_ZZ, _YY, _XX = np.meshgrid(_G[::-1], _G, _G, indexing="ij")
_GRID = np.stack([_XX.ravel(), _YY.ravel(), _ZZ.ravel()], 1)  # every voxel, as represent() builds it

_POLICY = re.compile(r"^(full|touched|b(\d+))$")


class PolicyError(ValueError):
    pass


def check_policy(policy: str) -> str:
    m = _POLICY.match(policy or "")
    if not m or (m.group(2) is not None and int(m.group(2)) < 2):
        raise PolicyError(f"the reduced reader's policy is full, touched or b<N> with N at least 2, not {policy!r}")
    return policy


def _floats(text, n):
    if text is None:
        return None
    if isinstance(text, (list, tuple)):
        parts = list(text)
    else:
        parts = str(text).replace(",", "\\").split("\\")
    try:
        v = [float(p) for p in parts]
    except (TypeError, ValueError):
        return None
    return v if len(v) == n and all(np.isfinite(v)) else None


def _int(x, default=0):
    try:
        return int(x)
    except (TypeError, ValueError):
        return default


def registry_frame(path: str, geo: dict | None, stack_geo: dict) -> Frame | None:
    """A single-frame file's Frame from the manifest's geometry, as
    ``volume.read_header_frames`` would read it from the header; None when
    the manifest's geometry cannot stand in for the header (no geometry,
    more than one frame, no matrix)."""
    if not isinstance(geo, dict):
        return None
    if _int(geo.get("frames"), 1) > 1:
        return None
    rows, cols = _int(geo.get("rows")), _int(geo.get("cols"))
    if rows <= 0 or cols <= 0:
        return None
    ipp = _floats(geo.get("ipp"), 3)
    ps = _floats(geo.get("ps"), 2)
    iop = _floats(stack_geo.get("iop"), 6)
    thick = stack_geo.get("thick")
    try:
        thick = float(thick) if thick is not None else None
    except (TypeError, ValueError):
        thick = None
    inum = _int(geo.get("inum"), 0)
    return Frame(path, 0, (rows, cols), ipp, iop, ps, thick, inum * 10000, str(stack_geo.get("modality") or ""), inum)


def _shrunk(shape, ps):
    """``volume.shrink``'s size and spacing, without the pixels."""
    h, w = shape
    m = max(h, w)
    if m <= MAXDIM:
        return (h, w), list(ps)
    sc = MAXDIM / m
    nh, nw = max(1, int(round(h * sc))), max(1, int(round(w * sc)))
    return (nh, nw), [ps[0] * h / nh, ps[1] * w / nw]


@dataclass
class Plan:
    """A stack's kept frames, its geometry and the frames a policy reads."""

    geo: list[Frame]
    uniq: list  # indices into geo, the kept frames in order
    n: np.ndarray
    r_dir: np.ndarray
    c_dir: np.ndarray
    n_frames_all: int
    n_unique: int
    o: np.ndarray
    M: np.ndarray
    sv: np.ndarray
    spacing: float
    thick: float
    H: int
    W: int
    idx: np.ndarray  # (n plane points, 3): k, i, j
    ok: np.ndarray
    touched: list[int]  # kept indices a plane point interpolates from
    read: list[int]  # kept indices the policy reads
    seen: set = field(default_factory=set)

    def need(self) -> dict[str, list[int]]:
        out: dict[str, list[int]] = {}
        for t in self.read:
            f = self.geo[self.uniq[t]]
            out.setdefault(f.path, []).append(f.index)
        return out


def _touched(k: np.ndarray, ok: np.ndarray, K: int) -> set[int]:
    kk = np.clip(k[ok], 0, K - 1)
    lo = np.floor(kk).astype(np.int64)
    hi = np.minimum(lo + 1, K - 1)
    frac = kk - lo
    return set(lo.tolist()) | set(hi[frac > 0].tolist())


def _choose(policy: str, k: np.ndarray, ok: np.ndarray, K: int, touched: list[int]) -> list[int]:
    if policy == "full":
        return list(range(K))
    if policy == "touched" or not touched:
        return list(touched)
    budget = int(policy[1:])
    if len(touched) <= budget:
        return list(touched)
    # the planes parallel to the slices: the pair whose points touch the fewest frames
    par: list[set[int]] = []
    for sel in PAIRS.values():
        m = ok & sel
        if m.any():
            par.append(_touched(k, m, K))
    par.sort(key=len)
    base: set[int] = set()
    if par and len(par[0]) <= 4:
        base = set(par[0])
        if len(base) + 2 > budget:
            # one nearest frame for each of the two planes
            kk = np.clip(k[ok & _parallel_mask(k, ok, K)], 0, K - 1)
            base = {int(round(float(np.min(kk)))), int(round(float(np.max(kk))))}
    rest = max(0, budget - len(base))
    lo, hi = touched[0], touched[-1]
    spread: set[int] = set()
    if rest >= 2:
        spread = {int(round(x)) for x in np.linspace(lo, hi, rest)}
    elif rest == 1:
        spread = {int(round((lo + hi) / 2))}
    chosen = sorted(base | spread)
    # fill up to the budget from the touched frames farthest from any chosen one
    pool = [t for t in touched if t not in chosen]
    while len(chosen) < budget and pool:
        arr = np.array(chosen)
        best = max(pool, key=lambda t: int(np.min(np.abs(arr - t))))
        chosen.append(best)
        pool.remove(best)
        chosen.sort()
    return chosen[:budget] if len(chosen) > budget else chosen


def _parallel_mask(k: np.ndarray, ok: np.ndarray, K: int) -> np.ndarray:
    best, bestn = None, None
    for sel in PAIRS.values():
        m = ok & sel
        if not m.any():
            continue
        c = len(_touched(k, m, K))
        if bestn is None or c < bestn:
            best, bestn = sel, c
    return best if best is not None else np.zeros_like(ok)


def plan(frames: list[Frame], orientation: str | None, policy: str, bad: set[str] | None = None) -> Plan:
    """The kept frames and the frames to read, from the frames' geometry
    alone, as ``volume.build`` would choose and place them."""
    chosen = volume.preselect(frames, orientation)
    frames = [f for f in frames if f.path in chosen]
    usable = [f for f in frames if f.path not in (bad or set())]
    if not usable:
        raise Unreadable("no frame of the stack could be decoded")
    geo, uniq, n, r_dir, c_dir, n_frames_all, n_unique = volume._select(usable)
    thick = geo[0].thick or 0.0
    ps = None
    for oi in uniq:
        (H, W), ps = _shrunk(geo[oi].shape, geo[oi].ps)
    K = len(uniq)
    o = np.array(geo[uniq[0]].ipp, dtype=np.float64)
    if K > 1:
        sv = (np.array(geo[uniq[-1]].ipp) - o) / (K - 1)
    else:
        sv = n * max(thick, 1.0)
    spacing = float(np.linalg.norm(sv))
    M = np.stack([sv, c_dir * ps[0], r_dir * ps[1]], axis=1)
    # the plane points' indices, computed over every voxel as represent() does
    corners = np.array([o + M @ np.array([k, i, j]) for k in (0, K - 1) for i in (0, H - 1) for j in (0, W - 1)])
    if K == 1:
        corners = np.concatenate([corners - n * thick / 2, corners + n * thick / 2])
    ctr = (corners.min(0) + corners.max(0)) / 2
    P = _GRID + ctr
    Minv = np.linalg.inv(M)
    idx = ((P - o) @ Minv.T)[_FLAT]
    tol_k = 0.5 if K > 1 else max(0.5, (thick or 1.0) / 2.0 / max(spacing, 1e-3))
    k = idx[:, 0]
    ok = (idx[:, 1] >= -0.5) & (idx[:, 1] <= H - 0.5) & (idx[:, 2] >= -0.5) & (idx[:, 2] <= W - 0.5)
    ok &= (k >= -tol_k) & (k <= K - 1 + tol_k)
    touched = sorted(_touched(k, ok, K))
    read = _choose(policy, k, ok, K, touched)
    if not read:
        read = [K // 2]
    return Plan(geo, uniq, n, r_dir, c_dir, n_frames_all, n_unique, o, M, sv, spacing, thick, H, W, idx, ok, touched, read,
                {f.path for f in frames})


def stack_frames(files: list[tuple[str, list[int] | None]], file_geo: list | None, stack_geo: dict | None, stats: dict) -> list[Frame]:
    """The frames of a stack's files: from the manifest's geometry where it
    stands in for the header, else from the header."""
    stack_geo = stack_geo if isinstance(stack_geo, dict) else {}
    frames: list[Frame] = []
    seen: set[str] = set()
    for i, (path, which) in enumerate(files):
        if path in seen:
            continue
        g = file_geo[i] if file_geo is not None and i < len(file_geo) else None
        f = registry_frame(path, g, stack_geo) if which is None else None
        if f is not None:
            seen.add(path)
            frames.append(f)
            continue
        try:
            got = volume.read_header_frames(path, which)
        except Exception:  # noqa: BLE001 - counted, never named
            stats["header_errors"] = stats.get("header_errors", 0) + 1
            continue
        stats["header_reads"] = stats.get("header_reads", 0) + 1
        seen.add(path)
        frames.extend(got)
    if not frames:
        raise Unreadable("no file of the stack could be read")
    return frames


def files_to_read(files, file_geo, stack_geo, orientation, policy) -> list[str]:
    """The files a stack's read will open, from its geometry alone (for
    read-ahead); every file when a header must be read to know."""
    stats: dict = {}
    try:
        if file_geo is None or any(registry_frame(p, g, stack_geo or {}) is None or w is not None for (p, w), g in zip(files, file_geo)):
            return [p for p, _ in files]
        frames = stack_frames(files, file_geo, stack_geo, stats)
        return list(plan(frames, orientation, policy).need())
    except Exception:  # noqa: BLE001
        return []


def build(files, file_geo, stack_geo, orientation: str | None, policy: str, decode=None) -> Built:
    """The stack's six planes in a (64, 64, 64) volume that is zero
    elsewhere, and its geometry, as ``volume.build`` gives them for the
    planes the encoder reads."""
    check_policy(policy)
    decode = decode or volume.decode_each
    stats: dict = {"policy": policy}
    errors: dict = {}
    frames = stack_frames(files, file_geo, stack_geo, stats)
    bad: set[str] = set()
    while True:
        p = plan(frames, orientation, policy, bad)
        need = p.need()
        decoded = decode(need)
        pixels: dict[int, np.ndarray] = {}
        failed = False
        for t in p.read:
            f = p.geo[p.uniq[t]]
            got = decoded.get(f.path)
            if isinstance(got, Exception) or got is None or f.index not in got:
                kind = "decode:" + (type(got).__name__ if isinstance(got, Exception) else "frames")
                errors[kind] = errors.get(kind, 0) + 1
                bad.add(f.path)
                failed = True
                continue
            img = got[f.index]
            if img.shape != p.geo[0].shape:
                errors["decode:shape"] = errors.get("decode:shape", 0) + 1
                bad.add(f.path)
                failed = True
                continue
            im, _ = volume.shrink(img, f.ps)
            pixels[t] = im
        if not failed:
            break
    stats.update(files_read=len(need), frames_decoded=len(p.read), kept=len(p.uniq), touched=len(p.touched))
    read = sorted(pixels)
    Vr = np.stack([pixels[t] for t in read]).astype(np.float32)
    K = len(p.uniq)
    V = np.zeros((K, p.H, p.W), np.float32)
    for t in read:
        V[t] = pixels[t]
    if p.read != p.touched or len(read) != K:
        ra = np.array(read)
        for t in p.touched:
            if t in pixels:
                continue
            below, above = ra[ra < t], ra[ra > t]
            if below.size and above.size:
                a, b = int(below[-1]), int(above[0])
                w = np.float32((t - a) / (b - a))
                V[t] = V[a] + w * (V[b] - V[a])
            else:
                V[t] = V[int(below[-1])] if below.size else V[int(above[0])]
    fg_vals = Vr[Vr > np.percentile(Vr, 5)] if Vr.size > 100 else Vr.ravel()
    ilo, ihi = np.percentile(Vr, 0.5), np.percentile(fg_vals if fg_vals.size else Vr, 99.5)
    from scipy.ndimage import map_coordinates

    kk = np.clip(p.idx[:, 0], 0, K - 1)
    vals = map_coordinates(V, [kk, p.idx[:, 1], p.idx[:, 2]], order=1, mode="nearest")
    vals = np.where(p.ok, vals, 0.0).astype(np.float32)
    vol = np.zeros(N * N * N, np.uint8)
    vol[_FLAT] = volume.to_u8(vals, ilo, ihi) * p.ok
    vol = vol.reshape(N, N, N)
    g0 = p.geo[0]
    meta0 = dict(
        modality=g0.modality,
        n_files=len(p.seen),
        n_frames=p.n_frames_all,
        n_unique=p.n_unique,
        rows0=g0.shape[0],
        cols0=g0.shape[1],
        ps_r=round(g0.ps[0], 4),
        ps_c=round(g0.ps[1], 4),
    )
    meta = _meta(p, meta0)
    b = Built(vol, meta, errors)
    b.stats = stats
    return b


def _meta(p: Plan, meta0: dict) -> dict:
    """``volume.represent``'s geometry, without the pixels."""
    import math

    K, H, W, M, o, n = len(p.uniq), p.H, p.W, p.M, p.o, p.n
    thick, spacing = p.thick, p.spacing
    corners = np.array([o + M @ np.array([k, i, j]) for k in (0, K - 1) for i in (0, H - 1) for j in (0, W - 1)])
    if K == 1:
        corners = np.concatenate([corners - n * thick / 2, corners + n * thick / 2])
    ext = corners.max(0) - corners.min(0)
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
    return {k: (v.item() if isinstance(v, np.generic) else v) for k, v in meta.items()}


def file_bytes(paths) -> int:
    total = 0
    for pth in paths:
        try:
            total += os.stat(pth).st_size
        except OSError:
            pass
    return total


__all__ = ["build", "plan", "files_to_read", "check_policy", "PolicyError", "NoGeometry", "Unreadable"]
