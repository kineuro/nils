# SPDX-License-Identifier: AGPL-3.0-only
"""Model r1's per-stack image features, as its study computed them (the
study's ``feat.py``, ``armu.py`` and ``armt.py``, and the intensity-tail
preprocessing they copied), on the files the tools made in a stack's work
folder:

- ``nat.nii.gz``, the stack converted by dcm2niix;
- ``u/synthseg.nii.gz`` (SynthSeg), ``u/hp_aff.nii.gz`` and
  ``u/vx_aff.nii.gz`` (the template regions and the venous regions placed
  on the stack by SynthMorph's affine);
- ``strip.nii.gz`` (SynthStrip).

**Base** (arms U and V): the template regions' intensities over a white
matter and a ventricle reference (``HP``), and the stack's own structures
against its own white matter, with a Frangi vessel filter in the venous
zones (``V``). **T**: five tail features of the skull-stripped stack after
a 1 mm B-spline resample, without the N4 step (round 2's class II
candidate ``n4none``, which changed no answer).

Every number is computed as the study computed it, so a feature here is
the study's feature; the columns the arms read are made from them by
:func:`columns`.
"""

from __future__ import annotations

import os
import time

import numpy as np

HP_TARGETS = ["pituitary", "cavernous_l", "cavernous_r", "sinus_midline", "sinus_transverse_l", "sinus_transverse_r", "nasal_mucosa",
              "choroid_plexus_atria"]
T_KEYS = ["tail_steepness_ratio", "upper_tail_acceleration", "upper_tail_leverage", "extreme_high_slope_ratio", "bright_voxel_ratio"]

# where the Frangi filter runs: "cpu", or "card" (cuCIM's port of
# scikit-image's filter, a volume too large for the card's share filtered
# on the CPU)
_FRANGI = {"where": "cpu", "card_failed": 0}


def frangi_on_card(limit_mib: int = 1536) -> bool:
    """Round 2's ``frangi-b``: scikit-image's Frangi filter replaced in this
    process by cuCIM's port of it on the card, this process's share of the
    card capped; a volume too large for it is filtered on the CPU. False
    when cuCIM or a card is not there (the CPU filter stays)."""
    try:
        import cupy as cp
        from cucim.skimage.filters import frangi as gf

        cp.cuda.runtime.getDeviceCount()
    except Exception:  # noqa: BLE001 - no card, no cuCIM: the CPU's filter
        return False
    cp.get_default_memory_pool().set_limit(size=limit_mib * 2**20)
    _FRANGI.update(where="card", cp=cp, gf=gf)
    return True


def _frangi(image, sigmas, black_ridges):
    from skimage.filters import frangi

    if _FRANGI["where"] == "card":
        cp, gf = _FRANGI["cp"], _FRANGI["gf"]
        try:
            out = gf(cp.asarray(image), sigmas=sigmas, black_ridges=black_ridges)
            res = cp.asnumpy(out)
            del out
            cp.get_default_memory_pool().free_all_blocks()
            return res
        except cp.cuda.memory.OutOfMemoryError:
            cp.get_default_memory_pool().free_all_blocks()
            _FRANGI["card_failed"] += 1
    return frangi(image, sigmas=sigmas, black_ridges=black_ridges)


def load_native(path):
    import nibabel as nib

    ni = nib.load(path)
    img = np.asanyarray(ni.dataobj).astype(np.float32)
    return ni, (img[..., 0] if img.ndim > 3 else img)


def to_native(lab_path, nat_img) -> np.ndarray:
    """A label volume resampled to the native grid by world coordinates,
    nearest neighbour (``armu.to_native``)."""
    import nibabel as nib
    from scipy.ndimage import map_coordinates

    li = nib.load(lab_path)
    lab = np.asanyarray(li.dataobj)
    if lab.ndim > 3:
        lab = lab[..., 0]
    sh = nat_img.shape[:3]
    ijk = np.indices(sh).reshape(3, -1)
    M = np.linalg.inv(li.affine) @ nat_img.affine
    c = M[:3, :3] @ ijk + M[:3, 3:4]
    return map_coordinates(lab, c, order=0, mode="constant", cval=0).reshape(sh).astype(np.int32)


def _stats(img, mask):
    v = img[mask]
    if v.size == 0:
        return None
    return float(np.median(v)), float(np.percentile(v, 90)), int(v.size)


def region_features(img, vox_ml, regions, wm, csf) -> dict:
    """``armu.region_features``: each region's median and 90th percentile
    over the white-matter and the CSF reference's medians, its voxels and
    mL."""
    out: dict = {}
    ws, cs = _stats(img, wm), _stats(img, csf)
    out["ref_wm_median"] = ws[0] if ws else None
    out["ref_wm_vox"] = ws[2] if ws else 0
    out["ref_csf_median"] = cs[0] if cs else None
    out["ref_csf_vox"] = cs[2] if cs else 0
    for name, m in regions.items():
        s = _stats(img, m)
        if s is None:
            out[name] = None
            continue
        med, p90, n = s
        r = {"vox": n, "ml": round(n * vox_ml, 3)}
        if ws and ws[0] > 0:
            r.update(med_wm=med / ws[0], p90_wm=p90 / ws[0])
        if cs and cs[0] > 0:
            r.update(med_csf=med / cs[0], p90_csf=p90 / cs[0])
        out[name] = r
    return out


def usable(feats, targets) -> bool:
    """Phase 2's definition: a white-matter reference over 1,000 voxels and
    every target region at 5 voxels or more."""
    return bool(feats and (feats.get("ref_wm_vox") or 0) > 1000 and all((feats.get(t) or {}).get("vox", 0) >= 5 for t in targets))


def _q(v, p):
    return float(np.percentile(v, p)) if v.size else None


def vesselness(img, zooms, box):
    """Frangi bright-ridge response on the image scaled by its white-matter
    median, inside a bounding box: 3D when the voxels are near isotropic
    (largest over smallest spacing at most 2), else 2D slice by slice across
    the thick axis."""
    sl = tuple(slice(a, b) for a, b in box)
    sub = img[sl].astype(np.float32)
    z = np.asarray(zooms, float)
    if z.max() / z.min() <= 2:
        sig = [s / z.mean() for s in (0.8, 1.2, 2.0)]
        return _frangi(sub, sig, False), sl
    ax = int(np.argmax(z))
    inpl = float(np.mean(np.delete(z, ax)))
    sig = [s / inpl for s in (0.8, 1.2, 2.0)]
    out = np.zeros_like(sub)
    for k in range(sub.shape[ax]):
        idx = [slice(None)] * 3
        idx[ax] = k
        idx = tuple(idx)
        out[idx] = _frangi(sub[idx], sig, False)
    return out, sl


def base(d: str, hpn: dict, vxn: dict) -> dict:
    """Arms U's (HP) and V's features of the stack in work folder ``d``
    (``feat.base``; the study's VA source, which the frozen arms do not
    read, is left out)."""
    from scipy import ndimage as ndi

    rec: dict = {}
    u = d + "/u"
    try:
        missing = [f for f in ("synthseg", "hp_aff", "vx_aff") if not os.path.exists(f"{u}/{f}.nii.gz")]
        if missing:
            rec["fail"] = "missing " + ",".join(missing)
            return rec
        ni, img = load_native(d + "/nat.nii.gz")
        vox_ml = float(np.prod(ni.header.get_zooms()[:3])) / 1000.0
        zooms = [float(x) for x in ni.header.get_zooms()[:3]]
        rec["zooms"] = zooms
        rec["shape"] = list(img.shape)
        ss = to_native(u + "/synthseg.nii.gz", ni)
        hp = to_native(u + "/hp_aff.nii.gz", ni)
        vx = to_native(u + "/vx_aff.nii.gz", ni)
        wm = np.isin(ss, [2, 41])
        csf = np.isin(ss, [4, 43])
        stem = ss == 16
        brain = ss > 0
        H = {k: hp == v["label"] for k, v in hpn.items()}
        V = {k: vx == v["label"] for k, v in vxn.items()}
        f_hp = region_features(img, vox_ml, {k: H[k] for k in HP_TARGETS}, H["wm_centrum"], H["csf_ventricles"])
        rec["HP"] = f_hp
        rec["HP_usable"] = usable(f_hp, HP_TARGETS)
        wmv = img[wm]
        ref = float(np.median(wmv)) if wmv.size > 1000 else None
        v: dict = {"wm_vox": int(wmv.size), "brain_ml": round(float(brain.sum() * vox_ml), 1)}
        if ref and ref > 0:
            cv = img[csf]
            csf_med = float(np.median(cv)) if cv.size >= 50 else None
            pons = V.get("pons", np.zeros_like(stem)) & stem
            pv = img[pons] if pons.sum() >= 20 else img[stem]
            pons_med = float(np.median(pv)) if pv.size >= 20 else None
            zones = {"pituitary": H["pituitary"], "cavernous": H["cavernous_l"] | H["cavernous_r"], "choroid": H["choroid_plexus_atria"],
                     "mucosa": H["nasal_mucosa"], "sss": V["va_sinus_midline"], "transverse": V["va_transverse_l"] | V["va_transverse_r"],
                     "sigmoid": V["va_sigmoid_l"] | V["va_sigmoid_r"], "straight": V["va_straight"], "deep": V["va_deep"],
                     "cortical": V["va_cortical"]}
            for k, m in zones.items():
                x = img[m] / ref
                v[k + "_vox"] = int(x.size)
                if x.size >= 5:
                    v[k + "_med_wm"] = float(np.median(x))
                    v[k + "_p90_wm"] = _q(x, 90)
                    v[k + "_p99_wm"] = _q(x, 99)
                    v[k + "_bright_share"] = float((x > 1.5).mean())
            if csf_med and csf_med > 0 and H["choroid_plexus_atria"].sum() >= 5:
                v["choroid_p90_over_ventricle_csf"] = _q(img[H["choroid_plexus_atria"]], 90) / csf_med
            if pons_med and pons_med > 0:
                for k in ("pituitary", "cavernous", "sss", "transverse"):
                    if zones[k].sum() >= 5:
                        v[k + "_p90_over_pons"] = _q(img[zones[k]], 90) / pons_med
            if csf_med:
                v["csf_over_wm"] = csf_med / ref
            t0 = time.process_time()
            aff = ni.affine
            zw = (aff[2, :3] @ np.indices(img.shape).reshape(3, -1) + aff[2, 3]).reshape(img.shape)
            bz = zw[brain]
            top = zw > (np.percentile(bz, 50) if bz.size else np.inf)
            mm = float(np.mean(zooms))
            shell = ndi.binary_dilation(brain, iterations=max(1, int(round(6 / mm)))) & ~ndi.binary_dilation(brain, iterations=1) & top
            vz = {"sinus": zones["sss"] | zones["transverse"] | zones["sigmoid"], "straight_deep": zones["straight"] | zones["deep"],
                  "cortical": zones["cortical"], "shell": shell, "wm": wm}
            allz = np.zeros_like(brain)
            for m in vz.values():
                allz |= m
            if allz.any():
                idx = np.argwhere(allz)
                lo = np.maximum(idx.min(0) - 3, 0)
                hi = np.minimum(idx.max(0) + 4, img.shape)
                vs, sl = vesselness(img / ref, zooms, list(zip(lo, hi)))
                for k, m in vz.items():
                    x = vs[m[sl]]
                    if x.size >= 20:
                        v["vess_" + k + "_p95"] = _q(x, 95)
                        v["vess_" + k + "_mean"] = float(x.mean())
                wb = v.get("vess_wm_p95")
                for k in ("sinus", "straight_deep", "cortical", "shell"):
                    if wb is not None and "vess_" + k + "_p95" in v:
                        v["vess_" + k + "_over_wm"] = v["vess_" + k + "_p95"] / (wb + 1e-6)
            v["vess_cpu_s"] = round(time.process_time() - t0, 1)
        rec["V"] = v
    except Exception as e:  # noqa: BLE001 - a stack's features fail, the run goes on
        rec["fail"] = type(e).__name__
    return rec


def tail(stripped: str) -> dict:
    """Arm T's five tail features of the skull-stripped stack, without N4
    (``armt.features`` over the copied preprocessing: the 1 mm B-spline
    resample, the mask resampled nearest, the tail percentiles before the
    clip, the clip at the 0.5th and 99.5th percentiles, the z-score, the
    share of voxels over 3)."""
    import SimpleITK as sitk

    out: dict = {"ok": False}
    try:
        orig = sitk.ReadImage(stripped)
        orig_data = sitk.GetArrayFromImage(orig)
        orig_mask = (orig_data > 0).astype(np.uint8)
        orig_mask_sitk = sitk.GetImageFromArray(orig_mask)
        orig_mask_sitk.CopyInformation(orig)
        img = sitk.ReadImage(stripped)
        spacing, size = img.GetSpacing(), img.GetSize()
        target = (1.0, 1.0, 1.0)
        new_size = [int(round(o * s / t)) for o, s, t in zip(size, spacing, target)]
        rs = sitk.ResampleImageFilter()
        rs.SetOutputSpacing(target)
        rs.SetSize(new_size)
        rs.SetOutputDirection(img.GetDirection())
        rs.SetOutputOrigin(img.GetOrigin())
        rs.SetInterpolator(sitk.sitkBSpline)
        res = rs.Execute(img)
        data = sitk.GetArrayFromImage(res)
        mr = sitk.ResampleImageFilter()
        mr.SetReferenceImage(res)
        mr.SetInterpolator(sitk.sitkNearestNeighbor)
        mask = sitk.GetArrayFromImage(mr.Execute(orig_mask_sitk))
        bv = data[mask > 0]
        if bv.size == 0:
            out["error"] = "empty mask"
            return out
        p50, p95, p990, p992, p995, p999 = (float(x) for x in np.percentile(bv, [50.0, 95.0, 99.0, 99.2, 99.5, 99.9]))
        nz = data[data > 0]
        if nz.size == 0:
            out["error"] = "no voxel"
            return out
        lo, hi = np.percentile(nz, (0.5, 99.5))
        clipped = np.clip(data, lo, hi)
        nz = clipped[clipped > 0]
        if nz.size == 0:
            out["error"] = "no voxel"
            return out
        mean, std = np.mean(nz), np.std(nz)
        if std == 0:
            out["error"] = "zero deviation"
            return out
        norm = (clipped - mean) / std
        # the study wrote the volume and its mask to files and read them back
        # as float64; the values are the same
        z = np.asarray(norm, dtype=norm.dtype).astype(np.float64)
        brain = z[mask.astype(np.float64) > 0]
        if brain.size == 0:
            out["error"] = "empty mask"
            return out
        eps = 1e-8
        s2 = (p995 - p992) / 0.3
        s3 = (p999 - p995) / 0.4
        out.update({
            "bright_voxel_ratio": float(np.sum(brain > 3.0) / brain.size),
            "upper_tail_acceleration": s3 / (s2 + eps),
            "upper_tail_leverage": (p999 - p995) / (p995 - p990 + eps),
            "extreme_high_slope_ratio": ((p999 - p990) / 0.9) / (((p990 - p95) / 4.0) + eps),
            "tail_steepness_ratio": (p995 - p95) / (p95 - p50 + eps),
            "small": bool(min(size) < 20),
            "ok": True,
        })
    except Exception as e:  # noqa: BLE001
        out["error"] = type(e).__name__
    return out


def _lg(v):
    return float(np.log(v)) if v is not None and v == v and v > 0 else np.nan


def _region_cols(f, src, ok) -> dict:
    d = {f"{src}_usable": float(bool(ok))}
    if not f:
        return d
    for k, v in f.items():
        if isinstance(v, dict):
            for m in ("med_wm", "p90_wm", "med_csf", "p90_csf"):
                d[f"{src}:{k}:{m}"] = _lg(v.get(m))
            d[f"{src}:{k}:ml"] = float(np.log1p(v.get("ml") or 0))
    return d


def columns(b: dict | None, t: dict | None) -> dict:
    """The arms' columns of a stack from its base and tail records, as the
    study's table made them (``m_table.py``, ``pc_swap.feature_row``)."""
    d: dict = {}
    if b and not b.get("fail"):
        d.update(_region_cols(b.get("HP"), "HP", b.get("HP_usable")))
        for k, v in (b.get("V") or {}).items():
            if k == "vess_cpu_s" or v is None:
                continue
            d["V:" + k] = _lg(v) if k.endswith(("_wm", "_over_pons", "_over_ventricle_csf", "csf_over_wm", "_over_wm")) else (
                float(np.log1p(v)) if k.endswith(("_vox", "brain_ml")) else float(v))
        d["V_ok"] = float((b.get("V") or {}).get("wm_vox", 0) > 1000)
    if t and t.get("ok"):
        for k in T_KEYS:
            d["T:" + k] = t.get(k)
    return d
