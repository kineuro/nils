# SPDX-License-Identifier: AGPL-3.0-only
"""Fixtures: tiny synthetic DICOM files and a three-stack manifest.

Every image here is drawn from a formula; no file comes from an archive and
no identifier is a person's.
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pytest


def write_dicom(path: Path, frames: np.ndarray, *, slope: float = 1.0, intercept: float = 0.0) -> None:
    """A minimal MR image, one frame or many, uncompressed little endian."""
    from pydicom.dataset import Dataset, FileMetaDataset
    from pydicom.uid import ExplicitVRLittleEndian, MRImageStorage, generate_uid

    frames = np.asarray(frames, dtype=np.uint16)
    meta = FileMetaDataset()
    meta.MediaStorageSOPClassUID = MRImageStorage
    meta.MediaStorageSOPInstanceUID = generate_uid()
    meta.TransferSyntaxUID = ExplicitVRLittleEndian
    ds = Dataset()
    ds.file_meta = meta
    ds.SOPClassUID = MRImageStorage
    ds.SOPInstanceUID = meta.MediaStorageSOPInstanceUID
    ds.Modality = "MR"
    ds.PatientID = "SYNTHETIC"
    ds.SamplesPerPixel = 1
    ds.PhotometricInterpretation = "MONOCHROME2"
    ds.BitsAllocated = 16
    ds.BitsStored = 16
    ds.HighBit = 15
    ds.PixelRepresentation = 0
    ds.RescaleSlope = slope
    ds.RescaleIntercept = intercept
    if frames.ndim == 3:
        ds.NumberOfFrames = frames.shape[0]
        ds.Rows, ds.Columns = frames.shape[1:]
    else:
        ds.Rows, ds.Columns = frames.shape
    ds.PixelData = frames.tobytes()
    path.parent.mkdir(parents=True, exist_ok=True)
    ds.save_as(path, enforce_file_format=True)


def blob(shape: tuple[int, int], cy: float, cx: float, r: float, seed: int) -> np.ndarray:
    """A bright disc on noise: a stand-in for anatomy."""
    rng = np.random.default_rng(seed)
    y, x = np.mgrid[: shape[0], : shape[1]]
    img = 200.0 * (((y - cy) ** 2 + (x - cx) ** 2) < r**2) + rng.normal(50, 10, shape)
    return np.clip(img, 0, 4000).astype(np.uint16)


@pytest.fixture
def synthetic(tmp_path: Path) -> dict:
    """Three stacks: an axial one of 10 single-frame files, a sagittal one of
    6 files, and a coronal multi-frame file of 8 frames."""
    # The manifest names /source/0, which --source-root puts under tmp_path.
    src = tmp_path / "source" / "0"
    stacks = []
    files = []
    for i in range(10):
        p = f"s1/{i:03d}.dcm"
        write_dicom(src / p, blob((48, 40), 24 - i, 20, 6 + i % 3, i))
        files.append({"source": 0, "path": p})
    stacks.append({"unit": "stack-101", "stack_id": 101, "files": files, "orientation": "axial", "body_part": "brain", "technique": "SE"})
    files = []
    for i in range(6):
        p = f"s2/{i:03d}.dcm"
        write_dicom(src / p, blob((32, 64), 16, 10 + 5 * i, 5, 100 + i), slope=2.0, intercept=-10.0)
        files.append({"source": 0, "path": p})
    stacks.append({"unit": "stack-102", "stack_id": 102, "files": files, "orientation": "sagittal", "body_part": None, "technique": "TSE"})
    mf = np.stack([blob((40, 40), 20, 20, 3 + k, 200 + k) for k in range(8)])
    write_dicom(src / "s3/mf.dcm", mf)
    stacks.append(
        {"unit": "stack-103", "stack_id": 103, "files": [{"source": 0, "path": "s3/mf.dcm", "frames": "1-8"}], "orientation": "coronal", "body_part": "spine", "technique": "TSE"}
    )
    manifest = tmp_path / "stacks.json"
    manifest.write_text(json.dumps({"contract": "job/v1", "sources": [{"id": 0, "mount": "/source/0"}], "stacks": stacks}))
    return {"root": tmp_path, "source_root": tmp_path / "source", "stacks": manifest}
