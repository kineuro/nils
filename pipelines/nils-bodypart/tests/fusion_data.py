# SPDX-License-Identifier: AGPL-3.0-only
"""Synthetic data for the certified model's entry point (record 50): DICOM
stacks of a phantom with real geometry, a random encoder of the right
shapes, a small LightGBM head trained on random numbers, their cards, a
coarse mode file, and an ``/inputs`` tree as the runner writes it.

Every image is drawn from a formula and every identifier is made up.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import numpy as np

CLASSES = ["brain", "brain-neck", "neck", "spine", "chest", "other"]
COARSE_MAP = {"brain": "head", "brain-neck": "head", "neck": "spine", "spine": "spine", "chest": "chest", "other": "other"}


def digest(path: Path) -> str:
    return "sha256:" + hashlib.sha256(Path(path).read_bytes()).hexdigest()


def phantom(p: np.ndarray) -> np.ndarray:
    """Intensity at LPS points (N, 3): an ellipsoid with a brighter core and a rod."""
    x, y, z = p[:, 0], p[:, 1], p[:, 2]
    body = ((x / 90) ** 2 + (y / 110) ** 2 + ((z - 10) / 140) ** 2) < 1
    core = ((x / 40) ** 2 + ((y + 20) / 50) ** 2 + ((z - 60) / 50) ** 2) < 1
    rod = ((x / 12) ** 2 + ((y - 50) / 12) ** 2 < 1) & (np.abs(z + 40) < 90)
    return 300.0 * body + 500.0 * core + 250.0 * rod + 20.0 * np.sin(x / 7.0) * np.cos(y / 9.0)


def _base(modality: str = "MR"):
    from pydicom.dataset import Dataset, FileMetaDataset
    from pydicom.uid import ExplicitVRLittleEndian, MRImageStorage, generate_uid

    meta = FileMetaDataset()
    meta.MediaStorageSOPClassUID = MRImageStorage
    meta.MediaStorageSOPInstanceUID = generate_uid()
    meta.TransferSyntaxUID = ExplicitVRLittleEndian
    ds = Dataset()
    ds.file_meta = meta
    ds.SOPClassUID = MRImageStorage
    ds.SOPInstanceUID = meta.MediaStorageSOPInstanceUID
    ds.Modality = modality
    ds.PatientID = "SYNTHETIC"
    ds.SamplesPerPixel = 1
    ds.PhotometricInterpretation = "MONOCHROME2"
    ds.BitsAllocated = 16
    ds.BitsStored = 16
    ds.HighBit = 15
    ds.PixelRepresentation = 0
    return ds


def _plane(ipp, iop, ps, rows, cols) -> np.ndarray:
    r_dir, c_dir = np.array(iop[:3]), np.array(iop[3:])
    i, j = np.mgrid[:rows, :cols]
    pts = np.array(ipp)[None, :] + (i.ravel()[:, None] * ps[0]) * c_dir[None, :] + (j.ravel()[:, None] * ps[1]) * r_dir[None, :]
    return phantom(pts).reshape(rows, cols)


def write_slice(path: Path, *, ipp, iop, ps, rows, cols, thick, inum, geometry=True, slope=1.0, intercept=0.0):
    ds = _base()
    img = _plane(ipp, iop, ps, rows, cols)
    raw = np.clip((img - intercept) / slope, 0, 65535).astype(np.uint16)
    ds.Rows, ds.Columns = rows, cols
    ds.InstanceNumber = inum
    ds.RescaleSlope = slope
    ds.RescaleIntercept = intercept
    if geometry:
        ds.ImagePositionPatient = [float(v) for v in ipp]
        ds.ImageOrientationPatient = [float(v) for v in iop]
    ds.PixelSpacing = [float(ps[0]), float(ps[1])]
    ds.SliceThickness = float(thick)
    ds.PixelData = raw.tobytes()
    path.parent.mkdir(parents=True, exist_ok=True)
    ds.save_as(path, enforce_file_format=True)


def write_enhanced(path: Path, *, positions, iop, ps, rows, cols, thick):
    """A multi-frame file whose geometry is in its functional groups."""
    from pydicom.dataset import Dataset
    from pydicom.sequence import Sequence

    ds = _base()
    frames = [np.clip(_plane(p, iop, ps, rows, cols), 0, 65535).astype(np.uint16) for p in positions]
    ds.NumberOfFrames = len(frames)
    ds.Rows, ds.Columns = rows, cols
    ds.InstanceNumber = 1
    shared = Dataset()
    pm = Dataset()
    pm.PixelSpacing = [float(ps[0]), float(ps[1])]
    pm.SliceThickness = float(thick)
    shared.PixelMeasuresSequence = Sequence([pm])
    po = Dataset()
    po.ImageOrientationPatient = [float(v) for v in iop]
    shared.PlaneOrientationSequence = Sequence([po])
    ds.SharedFunctionalGroupsSequence = Sequence([shared])
    per = []
    for p in positions:
        f = Dataset()
        pp = Dataset()
        pp.ImagePositionPatient = [float(v) for v in p]
        f.PlanePositionSequence = Sequence([pp])
        per.append(f)
    ds.PerFrameFunctionalGroupsSequence = Sequence(per)
    ds.PixelData = np.stack(frames).tobytes()
    path.parent.mkdir(parents=True, exist_ok=True)
    ds.save_as(path, enforce_file_format=True)


AXIAL = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
SAGITTAL = [0.0, 1.0, 0.0, 0.0, 0.0, -1.0]
CORONAL = [1.0, 0.0, 0.0, 0.0, 0.0, -1.0]


def fingerprint(orientation: str, **more) -> dict:
    fp = {
        "manufacturer": "SYNTHETIC",
        "modality": "MR",
        "orientation": orientation,
        "fov_x": 256.0,
        "fov_y": 256.0,
        "pixel_spacing_row": 4.0,
        "pixel_spacing_col": 4.0,
        "rows": 64,
        "columns": 64,
        "n_slices": 40,
        "slice_span_mm": 120.0,
        "slice_thickness": 3.0,
        "spacing_between_slices": 3.0,
        "aspect_ratio": 1.0,
        "field_strength_normalized": 3.0,
        "magnetic_field_strength": 3.0,
        "receive_coil_name": "head_32",
        "text_series_description_ci": "t2_tse_tra brain",
        "text_protocol_name_ci": "t2 tra",
        "text_sequence_name_ci": "*tse2d1_15",
        "text_series_comments_ci": None,
        "text_body_part_ci": "head",
    }
    fp.update(more)
    return fp


def write_stacks(root: Path) -> dict:
    """Five stacks under ``root/source/0`` and their ``stacks.json``:

    - 11: axial, 110 single-frame files (more than the 96 kept), one
      position twice, cohorts that the fine calibration names;
    - 12: sagittal, 30 files of 300 x 280 (shrunk to 256), a batch whose
      last part the coarse calibration names;
    - 13: coronal, one multi-frame file of 24 frames, geometry in its
      functional groups, 20 of its frames listed;
    - 14: axial with no ImagePositionPatient: no geometry;
    - 15: the axial files of 11 with no fingerprint row.
    """
    src = root / "source" / "0"
    stacks = []
    files = []
    for k in range(110):
        z = -65.0 + k * 1.2
        p = f"ax/{k:03d}.dcm"
        write_slice(src / p, ipp=[-128.0, -128.0, z], iop=AXIAL, ps=[4.0, 4.0], rows=64, cols=64, thick=1.2, inum=k + 1, slope=2.0, intercept=-100.0)
        files.append({"source": 0, "path": p, "frames": None})
    write_slice(src / "ax/dup.dcm", ipp=[-128.0, -128.0, -65.0 + 50 * 1.2], iop=AXIAL, ps=[4.0, 4.0], rows=64, cols=64, thick=1.2, inum=500)
    files.append({"source": 0, "path": "ax/dup.dcm", "frames": None})
    header11 = {
        "fingerprint": fingerprint("axial", n_slices=111),
        "classification": {"body_part": [{"value": "brain", "confidence": 0.65, "tier": "keywords"}], "technique": [{"value": "TSE", "confidence": 0.9, "tier": "rules"}]},
        "batch": "import-2026",
        "cohorts": ["gamma", "alpha"],
    }
    stacks.append({"unit": "stack-11", "stack_id": 11, "files": files, "orientation": "axial", "body_part": "brain", "technique": "TSE", "header": header11})
    files12 = []
    for k in range(30):
        x = -45.0 + k * 3.0
        p = f"sag/{k:03d}.dcm"
        write_slice(src / p, ipp=[x, -130.0, 130.0], iop=SAGITTAL, ps=[0.9, 0.95], rows=300, cols=280, thick=3.0, inum=k + 1)
        files12.append({"source": 0, "path": p, "frames": None})
    header12 = {
        "fingerprint": fingerprint(
            "sagittal", fov_x=266.0, fov_y=270.0, rows=300, columns=280, pixel_spacing_row=0.9, pixel_spacing_col=0.95, aspect_ratio=1.5,
            receive_coil_name="spine_18", text_series_description_ci="t2_tse_sag c-spine", text_body_part_ci="cspine",
            field_strength_normalized=None, magnetic_field_strength=1.5, spacing_between_slices=None,
        ),
        "classification": {"body_part": [{"value": "spine", "confidence": 0.8, "tier": "keywords"}]},
        "batch": "site-2025-beta",
        "cohorts": [],
    }
    stacks.append({"unit": "stack-12", "stack_id": 12, "files": files12, "orientation": "sagittal", "header": header12})
    positions = [[-128.0, -60.0 + k * 5.0, 128.0] for k in range(24)]
    write_enhanced(src / "cor/mf.dcm", positions=positions, iop=CORONAL, ps=[4.0, 4.0], rows=64, cols=64, thick=5.0)
    header13 = {"fingerprint": fingerprint("coronal", manufacturer=None, text_body_part_ci=None), "classification": {}, "batch": None, "cohorts": []}
    stacks.append({"unit": "stack-13", "stack_id": 13, "files": [{"source": 0, "path": "cor/mf.dcm", "frames": "3-22"}], "orientation": "coronal", "header": header13})
    files14 = []
    for k in range(8):
        p = f"nogeo/{k:03d}.dcm"
        write_slice(src / p, ipp=[-128.0, -128.0, k * 5.0], iop=AXIAL, ps=[4.0, 4.0], rows=64, cols=64, thick=5.0, inum=k + 1, geometry=False)
        files14.append({"source": 0, "path": p, "frames": None})
    stacks.append({"unit": "stack-14", "stack_id": 14, "files": files14, "header": {"fingerprint": fingerprint("axial"), "classification": {}, "batch": None, "cohorts": []}})
    stacks.append({"unit": "stack-15", "stack_id": 15, "files": files[:20], "header": {"fingerprint": None, "classification": {}, "batch": None, "cohorts": []}})
    manifest = root / "stacks.json"
    manifest.write_text(json.dumps({"contract": "job/v1", "sources": [{"id": 0, "mount": "/source/0"}], "stacks": stacks}, indent=1))
    return {"stacks": manifest, "source_root": root / "source", "doc": stacks}


def state_dict(rng: np.random.Generator) -> dict[str, np.ndarray]:
    """A tiny2d state dict with random weights of the right shapes."""
    ch = (3, 16, 32, 48, 64)
    t: dict[str, np.ndarray] = {}
    for b in range(4):
        i, o = ch[b], ch[b + 1]
        t[f"f.{b}.0.weight"] = (rng.standard_normal((o, i, 3, 3)) * np.sqrt(2.0 / (i * 9))).astype(np.float32)
        t[f"f.{b}.1.weight"] = rng.uniform(0.5, 1.5, o).astype(np.float32)
        t[f"f.{b}.1.bias"] = rng.normal(0, 0.1, o).astype(np.float32)
        t[f"f.{b}.1.running_mean"] = rng.normal(0, 0.2, o).astype(np.float32)
        t[f"f.{b}.1.running_var"] = rng.uniform(0.3, 2.0, o).astype(np.float32)
        t[f"f.{b}.1.num_batches_tracked"] = np.array(600, np.int64)
    t["h.head.0.weight"] = (rng.standard_normal((64, 79)) * 0.15).astype(np.float32)
    t["h.head.0.bias"] = rng.normal(0, 0.05, 64).astype(np.float32)
    t["h.head.3.weight"] = (rng.standard_normal((6, 64)) * 0.3).astype(np.float32)
    t["h.head.3.bias"] = rng.normal(0, 0.05, 6).astype(np.float32)
    return t


def write_encoder(path: Path, seed: int = 7, classes=CLASSES) -> Path:
    from safetensors.numpy import save_file

    rng = np.random.default_rng(seed)
    tensors, cards = {}, {}
    for s in range(3):
        for k, v in state_dict(rng).items():
            tensors[f"s{s}.{k}"] = v
        cards[f"s{s}"] = {
            "geo_mean": [float(v) for v in rng.uniform(0, 4, 15)],
            "geo_sd": [float(v) for v in rng.uniform(0.5, 2, 15)],
            "temperature": float(rng.uniform(0.8, 1.6)),
            "steps": 600,
        }
    meta = {"arch": "tiny2d", "classes": json.dumps(classes), "seeds": json.dumps(cards), "preprocessing": "bpthumb-vol-8mm-64:v1"}
    path.parent.mkdir(parents=True, exist_ok=True)
    save_file(tensors, str(path), metadata=meta)
    return path


def write_head(path: Path, seed: int = 3) -> Path:
    """A LightGBM multiclass model over 50 random columns, 6 classes."""
    import lightgbm as lgb

    rng = np.random.default_rng(seed)
    X = rng.standard_normal((900, 50)).astype(np.float32)
    X[:, :6] = np.log(rng.dirichlet(np.ones(6), 900)).astype(np.float32)
    X[rng.random(X.shape) < 0.05] = np.nan
    y = (np.argmax(X[:, :6], 1) + (rng.random(900) < 0.2) * rng.integers(0, 6, 900)) % 6
    params = {"objective": "multiclass", "num_class": 6, "num_leaves": 7, "learning_rate": 0.2, "min_data_in_leaf": 10, "verbose": -1, "seed": 1, "num_threads": 1}
    bst = lgb.train(params, lgb.Dataset(X, y), num_boost_round=25)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(bst.model_to_string())
    return path


def calibration() -> dict:
    return {
        "global_fine": 1.25,
        "global_coarse": 1.1,
        "per_cohort": {"alpha": {"n": 40, "T": 0.7}, "gamma": {"n": 5, "T": None}, "zeta": {"n": 25, "T": 1.6}},
        "min_n": 20,
        "rule": "a cohort with at least 20 read stacks uses its own temperature; any other the global one",
    }


def write_inputs(root: Path, *, coarse: bool = True, head_threshold: float = 0.5) -> dict:
    """``root/inputs``: the three parts and ``manifest.json`` as the runner writes it."""
    inputs = root / "inputs"
    enc = write_encoder(inputs / "encoder" / "bodypart-tiny-test.safetensors")
    head = write_head(inputs / "head" / "bodypart-fusion-test.lgbm.txt")
    de, dh = digest(enc), digest(head)
    card_enc = {
        "name": "bodypart-tiny", "version": "t1", "kind": "encoder", "digest": de, "task": "features:bodypart_image", "slot": "site",
        "artifact": {"format": "safetensors"}, "preprocessing": {"version": "bpthumb-vol-8mm-64:v1"},
    }
    card_head = {
        "name": "bodypart-fusion", "version": "t1", "kind": "head", "digest": dh, "task": "axis:body_part", "slot": "site",
        "encoders": [{"digest": de, "name": "bodypart-tiny", "version": "t1"}], "threshold": head_threshold,
        "artifact": {"format": "other"}, "params": {"calibration": calibration()},
    }
    models = [
        {"input": "encoder", "model_id": 1, "name": "bodypart-tiny", "version": "t1", "digest": de, "kind": "encoder", "card": card_enc,
         "encoder_model_ids": [], "artifact": f"/inputs/encoder/{enc.name}"},
        {"input": "head", "model_id": 2, "name": "bodypart-fusion", "version": "t1", "digest": dh, "kind": "head", "card": card_head,
         "encoder_model_ids": [1], "artifact": f"/inputs/head/{head.name}"},
    ]
    out = {"inputs": inputs, "encoder": de, "head": dh, "cards": {"encoder": card_enc, "head": card_head}}
    if coarse:
        mode = {
            "mode": "coarse",
            "head": {"name": "bodypart-fusion", "version": "t1", "digest": dh},
            "classes": ["head", "spine", "chest", "other"],
            "map": COARSE_MAP,
            "recipe": "the fine probabilities at the global fine temperature, summed per region, at the coarse temperature",
            "fine_global_T": 1.25,
            "coarse_global_T": 1.1,
            "coarse_T_per_cohort": {"alpha": 0.9, "beta": 1.4},
            "threshold": 0.6,
        }
        mf = inputs / "coarse" / "bodypart-fusion-coarse-test.json"
        mf.parent.mkdir(parents=True, exist_ok=True)
        mf.write_text(json.dumps(mode, indent=1))
        dc = digest(mf)
        card_coarse = {
            "name": "bodypart-fusion-coarse", "version": "t1", "kind": "head", "digest": dc, "task": "axis:body_region", "slot": "site",
            "encoders": [{"digest": de, "name": "bodypart-tiny", "version": "t1"}], "threshold": 0.55, "artifact": {"format": "json"},
        }
        models.append(
            {"input": "coarse", "model_id": 3, "name": "bodypart-fusion-coarse", "version": "t1", "digest": dc, "kind": "head", "card": card_coarse,
             "encoder_model_ids": [1], "artifact": f"/inputs/coarse/{mf.name}"}
        )
        out.update(coarse=dc, mode=mode)
        out["cards"]["coarse"] = card_coarse
    doc = {"contract": 1, "run": 1, "pipeline": {"name": "bodypart-infer-fusion", "version": 1}, "params": {}, "level": "stack", "layout": "stacks",
           "units": [], "models": models, "label_set": None, "derivatives": {}, "secrets": []}
    (inputs / "manifest.json").write_text(json.dumps(doc, indent=1))
    return out
