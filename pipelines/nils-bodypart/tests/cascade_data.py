# SPDX-License-Identifier: AGPL-3.0-only
"""Synthetic data for body part's cascade (record 55 E2, P1b): two sessions
of phantom stacks in the manifest the engine writes with
``x-nils.input.geometry`` (each file's geometry and the stack's facts), a
deferral model, a stand-in student, a random student of the frozen shape,
and the cascade file naming every part by digest.

Every image is drawn from a formula and every identifier is made up.
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import numpy as np

from . import fusion_data as fd

AXIAL_DOWN = [1.0, 0.0, 0.0, 0.0, -1.0, 0.0]  # an axial stack whose normal points down
MR = "1.2.840.10008.5.1.4.1.1.4"


def _file(path: str, ipp, ps=(4.0, 4.0), rows=64, cols=64, inum=1, frames=None) -> dict:
    return {"source": 0, "path": path, "frames": None,
            "geometry": {"image_position_patient": [float(v) for v in ipp] if ipp is not None else None, "pixel_spacing": [float(v) for v in ps],
                         "rows": rows, "columns": cols, "instance_number": inum, "number_of_frames": frames}}


def _stack(sid: int, files: list, iop, *, study: int, orientation: str, modality: str = "MR", image_type: str = "ORIGINAL\\PRIMARY\\M\\ND",
           sealed: bool = False, header: dict | None | bool = True, thick: float = 4.0, sop: str = MR, **fp) -> dict:
    if header is True:
        header = {"fingerprint": fd.fingerprint(orientation, slice_thickness=thick, n_slices=len(files), **fp), "classification": {}, "batch": None,
                  "cohorts": []}
    return {"unit": f"stack-{sid}", "stack_id": sid, "files": files, "modality": modality, "orientation": orientation, "header": header,
            "geometry": {"image_orientation_patient": [float(v) for v in iop] if iop is not None else None, "slice_thickness": thick,
                         "spacing_between_slices": None, "study": study, "image_type": image_type, "sop_class_uid": sop,
                         "n_instances": len(files), "sealed": sealed}}


def study_uid(study: int) -> str:
    return f"1.2.826.0.1.3680043.8.498.55.{study}"


def stamp(path: Path, study: int) -> None:
    """The file's StudyInstanceUID, made up from the session's number."""
    import pydicom

    ds = pydicom.dcmread(path)
    ds.StudyInstanceUID = study_uid(study)
    ds.save_as(path, enforce_file_format=True)


def _series(src: Path, folder: str, positions, iop, *, study: int, ps=(4.0, 4.0), rows=64, cols=64, thick=4.0, inums=None, write=True) -> list:
    files = []
    for k, ipp in enumerate(positions):
        p = f"{folder}/{k:03d}.dcm"
        inum = inums[k] if inums is not None else k + 1
        if write:
            fd.write_slice(src / p, ipp=ipp, iop=iop, ps=list(ps), rows=rows, cols=cols, thick=thick, inum=inum)
            stamp(src / p, study)
        files.append(_file(p, ipp, ps, rows, cols, inum))
    return files


def write_stacks(root: Path) -> dict:
    """Two sessions under ``root/source/0`` and their ``stacks.json``:

    study 100:
    - 21: axial, 30 files, numbered from the top down (the inferior end is
      the highest number);
    - 22: sagittal, 21 files, the session's highest box (its top);
    - 23: coronal, 20 files;
    - 24: a localiser by its image type, 9 axial files;
    - 25: a thin axial slab of 6 files, its normal pointing down;
    - 26: sealed now, the highest box of all (never the session's top);
    - 27: a CT stack, higher still;
    - 28: MR with no fingerprint row;
    - 29: one multi-frame file of 12 frames;
    - 30: 10 axial files the manifest lists and the disk lacks;
    study 200:
    - 31: axial, 20 files, its own session.
    """
    src = root / "source" / "0"
    st = []
    ax = [[-128.0, -128.0, -60.0 + 4.0 * k] for k in range(30)]
    st.append(_stack(21, _series(src, "s21", ax, fd.AXIAL, study=100, inums=[30 - k for k in range(30)]), fd.AXIAL, study=100, orientation="axial"))
    sag = [[-40.0 + 4.0 * k, -130.0, 130.0] for k in range(21)]
    st.append(_stack(22, _series(src, "s22", sag, fd.SAGITTAL, study=100), fd.SAGITTAL, study=100, orientation="sagittal"))
    cor = [[-128.0, -60.0 + 5.0 * k, 128.0] for k in range(20)]
    st.append(_stack(23, _series(src, "s23", cor, fd.CORONAL, study=100), fd.CORONAL, study=100, orientation="coronal"))
    loc = [[-128.0, -128.0, -20.0 + 6.0 * k] for k in range(9)]
    st.append(_stack(24, _series(src, "s24", loc, fd.AXIAL, study=100, inums=[5, 1, 9, 3, 7, 2, 8, 4, 6]), fd.AXIAL, study=100, orientation="axial",
                     image_type="ORIGINAL\\PRIMARY\\LOCALIZER"))
    thin = [[-128.0, 128.0, 10.0 + 3.0 * k] for k in range(6)]
    st.append(_stack(25, _series(src, "s25", thin, AXIAL_DOWN, study=100), AXIAL_DOWN, study=100, orientation="axial"))
    hi = [[-128.0, -128.0, 280.0 + 4.0 * k] for k in range(10)]
    st.append(_stack(26, _series(src, "s26", hi, fd.AXIAL, study=100), fd.AXIAL, study=100, orientation="axial", sealed=True))
    ct = [[-128.0, -128.0, 380.0 + 4.0 * k] for k in range(10)]
    st.append(_stack(27, _series(src, "s27", ct, fd.AXIAL, study=100), fd.AXIAL, study=100, orientation="axial", modality="CT"))
    nf = [[-128.0, -128.0, -30.0 + 4.0 * k] for k in range(10)]
    st.append(_stack(28, _series(src, "s28", nf, fd.AXIAL, study=100), fd.AXIAL, study=100, orientation="axial",
                     header={"fingerprint": None, "classification": {}, "batch": None, "cohorts": []}))
    positions = [[-128.0, -128.0, -20.0 + 5.0 * k] for k in range(12)]
    fd.write_enhanced(src / "s29/mf.dcm", positions=positions, iop=fd.AXIAL, ps=[4.0, 4.0], rows=64, cols=64, thick=5.0)
    stamp(src / "s29/mf.dcm", 100)
    mf = _file("s29/mf.dcm", positions[0], frames=12)
    st.append(_stack(29, [mf], fd.AXIAL, study=100, orientation="axial", sop="1.2.840.10008.5.1.4.1.1.4.1"))
    gone = [[-128.0, -128.0, -40.0 + 4.0 * k] for k in range(10)]
    st.append(_stack(30, _series(src, "s30", gone, fd.AXIAL, study=100, write=False), fd.AXIAL, study=100, orientation="axial"))
    b = [[-128.0, -128.0, 200.0 + 4.0 * k] for k in range(20)]
    st.append(_stack(31, _series(src, "s31", b, fd.AXIAL, study=200), fd.AXIAL, study=200, orientation="axial"))
    manifest = root / "stacks.json"
    manifest.write_text(json.dumps({"contract": "job/v1", "sources": [{"id": 0, "mount": "/source/0"}], "stacks": st}, indent=1))
    return {"stacks": manifest, "source_root": root / "source", "doc": st}


def stand_in_student(tiles: np.ndarray, sgeo: np.ndarray, tokens: np.ndarray) -> np.ndarray:
    """Three seeds' six probabilities from the slice's mean and the stack's
    tokens: a fixed formula, so a test knows what the student said."""
    m = tiles.reshape(len(tiles), -1).mean(1) / 255.0
    base = np.stack([m, 1 - m, 0.5 * m, tokens[:, 0], tokens[:, 1], 0.3 + 0 * m], 1).astype(np.float64)
    out = []
    for s in range(3):
        z = base * (2.0 + s) + 0.1 * s
        e = np.exp(z - z.max(1, keepdims=True))
        out.append(e / e.sum(1, keepdims=True))
    return np.stack(out, 1).astype(np.float32)


def write_deferral(path: Path, mode: str = "mixed", seed: int = 5) -> Path:
    """A LightGBM binary model over the 62 features: ``all`` agrees with
    every stack, ``none`` with none, ``mixed`` by the first seed's fine
    confidence."""
    import lightgbm as lgb

    rng = np.random.default_rng(seed)
    X = rng.random((400, 62))
    y = {"all": np.ones(400), "none": np.zeros(400), "mixed": (X[:, 0] > 0.5).astype(float)}[mode]
    if mode != "mixed":
        y[:2] = 1 - y[:2]  # LightGBM wants both classes; two of 400 hardly move it
    params = {"objective": "binary", "num_leaves": 7, "learning_rate": 0.3, "min_data_in_leaf": 5, "verbose": -1, "seed": 1, "num_threads": 1}
    bst = lgb.train(params, lgb.Dataset(X, y), num_boost_round=60)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(bst.model_to_string())
    return path


def write_student(path: Path, seeds=(1, 2, 3)) -> Path:
    """A student of the frozen shape with random weights (torch and timm)."""
    import torch
    from safetensors.torch import save_file

    from nils_bodypart import student

    tensors = {}
    for s in seeds:
        torch.manual_seed(s)
        net = student.build()
        for k, v in net.state_dict().items():
            tensors[f"s{s}.{k}"] = v.detach().clone().contiguous()
    path.parent.mkdir(parents=True, exist_ok=True)
    save_file(tensors, str(path), metadata={"seeds": ",".join(str(s) for s in seeds), "input": "bodypart-slice-192:v1"})
    return path


def add_cascade(inputs: dict, *, deferral: str = "mixed", student: Path | None = None, d3: float = 0.99, m3: float = 0.5,
                thresholds: dict | None = None, name_parts: dict | None = None) -> dict:
    """The cascade's three inputs added to ``fusion_data.write_inputs``'s
    tree: the student (an opaque file unless one is given), the deferral
    model and the cascade file, each in ``manifest.json``."""
    root = Path(inputs["inputs"])
    stu = root / "student" / "bodypart-student-test.safetensors"
    stu.parent.mkdir(parents=True, exist_ok=True)
    if student is not None:
        shutil.copyfile(student, stu)
    else:
        stu.write_bytes(b"a stand-in student: the tests give the cascade a formula instead")
    dfr = write_deferral(root / "deferral" / "bodypart-defer-test.lgbm.txt", deferral)
    ds, dd = fd.digest(stu), fd.digest(dfr)
    parts = {"encoder": inputs["encoder"], "head": inputs["head"], "coarse": inputs["coarse"], "student": ds, "deferral": dd}
    parts.update(name_parts or {})
    doc = {
        "cascade": "bodypart-cascade", "version": "test",
        "parts": parts,
        "thresholds": thresholds or {"fine": 0.5, "coarse": 0.6},
        "stages": [
            {"stage": "s1", "input": "bodypart-slice-192:v1", "model": "student", "accepts": {"deferral_at_least": d3}},
            {"stage": "b24", "input": "bodypart-planes-b24:v1", "model": "encoder", "accepts": {"margin_logit_at_least": m3}},
            {"stage": "touched", "input": "bodypart-planes-touched:v1", "model": "encoder", "accepts": "always"},
        ],
    }
    cf = root / "cascade" / "bodypart-cascade-test.json"
    cf.parent.mkdir(parents=True, exist_ok=True)
    cf.write_text(json.dumps(doc, indent=1, sort_keys=True))
    dc = fd.digest(cf)
    mf = json.loads((root / "manifest.json").read_text())
    mf["models"] = [m for m in mf["models"] if m["input"] not in ("student", "deferral", "cascade")]
    for i, (inp, path, d, kind) in enumerate(((("student", stu, ds, "encoder")), ("deferral", dfr, dd, "head"), ("cascade", cf, dc, "head"))):
        card = {"name": f"bodypart-{inp}", "version": "test", "kind": kind, "digest": d, "task": "axis:body_part" if inp == "cascade" else f"part:{inp}",
                "slot": "site", "artifact": {"format": "other"}}
        if inp == "cascade":
            card.update(parts=parts, threshold=0.5)
        mf["models"].append({"input": inp, "model_id": 10 + i, "name": card["name"], "version": "test", "digest": d, "kind": kind, "card": card,
                             "encoder_model_ids": [], "artifact": f"/inputs/{inp}/{path.name}"})
    (root / "manifest.json").write_text(json.dumps(mf, indent=1))
    return {**inputs, "student": ds, "deferral": dd, "cascade": dc, "cascade_doc": doc}


def frozen_inputs(models: Path, dest: Path) -> Path:
    """An ``/inputs`` tree of the frozen cascade and r7's register set
    (a site's copy, outside the repository), as its registered model inputs.
    Only for the opt-in test; the files never enter the repository."""
    reg = Path(models) / "bodypart" / "r7" / "register"
    frz = Path(models) / "image-model-speed" / "freeze"
    items = [
        ("encoder", reg / "bodypart-tiny-r7.safetensors", reg / "card-encoder.json"),
        ("head", reg / "bodypart-fusion-r7.lgbm.txt", reg / "card-head.json"),
        ("coarse", reg / "coarse" / "bodypart-fusion-r7-coarse.json", reg / "coarse" / "card-coarse.json"),
        ("student", frz / "bodypart-student-s1.safetensors", None),
        ("deferral", frz / "bodypart-defer-d3.lgbm.txt", None),
        ("cascade", frz / "bodypart-cascade-r7.json", None),
    ]
    models_doc = []
    for i, (inp, art, card_path) in enumerate(items):
        (dest / inp).mkdir(parents=True, exist_ok=True)
        target = dest / inp / art.name
        if not target.exists():
            target.symlink_to(art)
        d = fd.digest(art)
        card = json.loads(card_path.read_text()) if card_path is not None else {"name": f"bodypart-{inp}", "version": "r7-fast", "digest": d}
        models_doc.append({"input": inp, "model_id": i + 1, "name": card.get("name"), "version": card.get("version"), "digest": d,
                           "kind": card.get("kind", "head"), "card": card, "encoder_model_ids": [], "artifact": f"/inputs/{inp}/{art.name}"})
    doc = {"contract": 1, "run": 1, "pipeline": {"name": "bodypart-infer-fusion", "version": 1}, "params": {}, "level": "stack", "layout": "stacks",
           "units": [], "models": models_doc, "label_set": None, "derivatives": {}, "secrets": []}
    (dest / "manifest.json").write_text(json.dumps(doc, indent=1))
    return dest
