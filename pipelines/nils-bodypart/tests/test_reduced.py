# SPDX-License-Identifier: AGPL-3.0-only
"""The reduced reader: from the manifest's geometry, its ``full`` policy
gives the full reader's planes and geometry bit for bit, and its smaller
policies read fewer frames and still give an answer."""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pytest

pytest.importorskip("lightgbm")
pytest.importorskip("safetensors")

from nils_bodypart import cli, manifest, reduced, volume  # noqa: E402

from . import fusion_data as fd  # noqa: E402


def _ds(x) -> str:
    return "\\".join(repr(float(v)) for v in x)


def with_geometry(doc: dict, source_root: Path) -> dict:
    """The manifest with each single-frame file's geometry as the registry
    holds it (DS strings) and each stack's orientation, thickness and
    modality, read here from the synthetic files' headers."""
    import pydicom

    for st in doc["stacks"]:
        sg = None
        for f in st["files"]:
            ds = pydicom.dcmread(source_root / "0" / f["path"], stop_before_pixels=True)
            nf = int(getattr(ds, "NumberOfFrames", 1) or 1)
            ipp = getattr(ds, "ImagePositionPatient", None)
            f["geometry"] = {
                "ipp": _ds(ipp) if ipp is not None else None,
                "ps": _ds(ds.PixelSpacing) if getattr(ds, "PixelSpacing", None) is not None else None,
                "rows": int(ds.Rows),
                "cols": int(ds.Columns),
                "inum": int(getattr(ds, "InstanceNumber", 0) or 0),
                "frames": nf,
            }
            if sg is None and getattr(ds, "ImageOrientationPatient", None) is not None:
                sg = {"iop": _ds(ds.ImageOrientationPatient), "thick": float(ds.SliceThickness) if getattr(ds, "SliceThickness", None) is not None else None, "modality": str(ds.Modality)}
        st["geometry"] = sg or {}
    return doc


@pytest.fixture(scope="module")
def world(tmp_path_factory) -> dict:
    root = tmp_path_factory.mktemp("reduced")
    w = {"root": root, **fd.write_stacks(root), **fd.write_inputs(root)}
    doc = with_geometry(json.loads(Path(w["stacks"]).read_text()), w["source_root"])
    geo = root / "stacks-geo.json"
    geo.write_text(json.dumps(doc))
    w["stacks_geo"] = geo
    return w


def stacks(world) -> list[manifest.Stack]:
    return manifest.load(world["stacks_geo"], source_root=world["source_root"])


@pytest.mark.parametrize("sid", [11, 12, 13])
def test_full_policy_is_the_full_readers_planes_bit_for_bit(world, sid):
    st = next(s for s in stacks(world) if s.stack_id == sid)
    orient = st.extra["header"]["fingerprint"]["orientation"]
    full = volume.build(st.files, orient)
    red = reduced.build(st.files, st.file_geo, st.extra.get("geometry"), orient, "full")
    m = reduced._MASK
    assert np.array_equal(full.vol[m], red.vol[m])
    assert not red.vol[~m].any()
    assert full.meta == red.meta
    assert np.array_equal(full.geo(), red.geo())
    # the multi-frame stack's file has its header read; the others none
    assert red.stats.get("header_reads", 0) == (1 if sid == 13 else 0)


def test_touched_policy_gives_the_full_planes_values(world):
    st = next(s for s in stacks(world) if s.stack_id == 11)
    full = reduced.build(st.files, st.file_geo, st.extra.get("geometry"), "axial", "full")
    t = reduced.build(st.files, st.file_geo, st.extra.get("geometry"), "axial", "touched")
    assert t.stats["frames_decoded"] == t.stats["touched"] <= t.stats["kept"]
    assert t.meta == full.meta


@pytest.mark.parametrize("policy", ["b4", "b8", "b16"])
def test_a_budget_reads_at_most_its_frames(world, policy):
    for st in stacks(world):
        if st.stack_id not in (11, 12, 13):
            continue
        orient = st.extra["header"]["fingerprint"]["orientation"]
        b = reduced.build(st.files, st.file_geo, st.extra.get("geometry"), orient, policy)
        assert b.stats["frames_decoded"] <= int(policy[1:])
        assert b.vol.any()
        assert b.meta == volume.build(st.files, orient).meta


def test_the_reduced_readers_run_answers_as_the_full_one_with_full_policy(world, tmp_path):
    def run(out: Path, *more: str) -> dict:
        code = cli.main(["infer-fusion", "--stacks", str(world["stacks_geo"]), "--source-root", str(world["source_root"]),
                         "--inputs", str(world["inputs"]), "--output", str(out), "--device", "cpu", *more])
        assert code == 0
        return json.loads((out / "results.json").read_text())

    a = run(tmp_path / "a", "--threads", "1")
    b = run(tmp_path / "b", "--threads", "2", "--reader", "reduced:full", "--readahead", "3")
    assert a["proposals"] == b["proposals"]
    assert [(u["unit_id"], u["status"], u.get("error")) for u in a["units"]] == [(u["unit_id"], u["status"], u.get("error")) for u in b["units"]]
    for p in sorted((tmp_path / "a" / "bodypart-fusion").glob("*.json")):
        da = json.loads(p.read_text())
        db = json.loads((tmp_path / "b" / "bodypart-fusion" / p.name).read_text())
        assert db.pop("reader")["policy"] == "full"
        assert da == db
    assert b["metrics"]["reader"]["frames_decoded"] > 0
    c = run(tmp_path / "c", "--threads", "2", "--reader", "reduced:b8")
    assert c["metrics"]["reader"]["frames_decoded"] < b["metrics"]["reader"]["frames_decoded"]


def test_a_policy_that_is_not_one_is_refused(world, tmp_path):
    code = cli.main(["infer-fusion", "--stacks", str(world["stacks_geo"]), "--inputs", str(world["inputs"]), "--output", str(tmp_path / "o"), "--reader", "reduced:b1"])
    assert code == 1


def test_the_files_own_thickness_wins_over_the_manifests(world):
    """The registry has no thickness per file; the reader takes the decoded
    files' own, as the full reader takes the first file's."""
    import copy

    st = next(s for s in stacks(world) if s.stack_id == 12)
    sg = copy.deepcopy(st.extra.get("geometry"))
    sg["thick"] = 9.5
    red = reduced.build(st.files, st.file_geo, sg, "sagittal", "b8")
    assert red.meta == volume.build(st.files, "sagittal").meta
    assert red.stats.get("thick_from_file") == 1


def test_a_stack_of_mixed_orientations_is_read_from_its_headers(world, tmp_path):
    """A three-plane localiser holds files of more than one orientation
    where the registry holds one: the decoded files show it, and the reader
    falls back to every file's header, as the full reader reads it."""
    src = tmp_path / "loc"
    files, geo = [], []
    for k, (iop, ipp) in enumerate([(fd.AXIAL, [-128.0, -128.0, 0.0]), (fd.AXIAL, [-128.0, -128.0, 6.0]), (fd.SAGITTAL, [0.0, -128.0, 128.0]),
                                    (fd.AXIAL, [-128.0, -128.0, 12.0])]):
        p = src / f"{k}.dcm"
        fd.write_slice(p, ipp=ipp, iop=iop, ps=[4.0, 4.0], rows=64, cols=64, thick=6.0, inum=k + 1)
        files.append((str(p), None))
        geo.append({"ipp": _ds(ipp), "ps": "4.0\\4.0", "rows": 64, "cols": 64, "inum": k + 1, "frames": 1})
    sg = {"iop": _ds(fd.SAGITTAL), "thick": 6.0, "modality": "MR"}
    full = volume.build(files, "axial")
    red = reduced.build(files, geo, sg, "axial", "full")
    assert red.stats.get("header_reads") == 4
    assert red.meta == full.meta
    assert np.array_equal(red.vol[reduced._MASK], full.vol[reduced._MASK])
