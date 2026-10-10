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


def engine_named(doc: dict) -> dict:
    """The same manifest as the engine writes it with x-nils.input.geometry
    (stacks.schema.json): its names, and numbers where the registry's text
    held DS values."""
    import copy

    def nums(x, n):
        v = reduced._floats(x, n)
        return [float(a) for a in v] if v is not None else None

    out = copy.deepcopy(doc)
    for st in out["stacks"]:
        sg = st.get("geometry") or {}
        st["modality"] = sg.get("modality")  # every entry carries it
        st["geometry"] = {
            "image_orientation_patient": nums(sg.get("iop"), 6),
            "slice_thickness": sg.get("thick"),
            "spacing_between_slices": None,
        }
        for f in st["files"]:
            g = f["geometry"]
            f["geometry"] = {
                "image_position_patient": nums(g.get("ipp"), 3),
                "pixel_spacing": nums(g.get("ps"), 2),
                "rows": g.get("rows"),
                "columns": g.get("cols"),
                "instance_number": g.get("inum"),
                "number_of_frames": g.get("frames") if g.get("frames", 1) > 1 else None,
            }
    return out


def test_the_engines_geometry_reads_as_the_registrys_text(world, tmp_path):
    """The engine's geometry (record 55 E2) is the reader's under its own
    names: every policy builds the same planes and geometry from it as from
    the manifest the one-hour study wrote."""
    doc = json.loads(Path(world["stacks_geo"]).read_text())
    eng = tmp_path / "engine.json"
    eng.write_text(json.dumps(engine_named(doc)))
    ours = {s.stack_id: s for s in manifest.load(eng, source_root=world["source_root"])}
    for st in stacks(world):
        e = ours[st.stack_id]
        sg = e.extra["geometry"]
        assert set(sg) == {"iop", "thick", "modality"}
        assert sg["modality"] == st.extra["geometry"].get("modality")
        assert reduced._floats(sg["iop"], 6) == reduced._floats(st.extra["geometry"].get("iop"), 6)
        for a, b in zip(e.file_geo, st.file_geo):
            assert set(a) == {"ipp", "ps", "rows", "cols", "inum", "frames"}
            assert reduced._floats(a["ipp"], 3) == reduced._floats(b["ipp"], 3)
            assert (a["rows"], a["cols"], a["inum"]) == (b["rows"], b["cols"], b["inum"])
        if st.stack_id not in (11, 12, 13):
            continue
        orient = st.extra["header"]["fingerprint"]["orientation"]
        for policy in ("full", "touched", "b8"):
            x = reduced.build(st.files, st.file_geo, st.extra.get("geometry"), orient, policy)
            y = reduced.build(e.files, e.file_geo, e.extra.get("geometry"), orient, policy)
            assert np.array_equal(x.vol, y.vol), (st.stack_id, policy)
            assert x.meta == y.meta and np.array_equal(x.geo(), y.geo())
            assert x.stats == y.stats


def test_a_manifest_without_geometry_has_none():
    assert manifest.file_geometry(None) is None
    assert manifest.stack_geometry({"modality": "MR"}) is None
    assert manifest.file_geometry({"ipp": "1\\2\\3"}) == {"ipp": "1\\2\\3"}


def test_a_card_decoder_gives_the_cpu_paths_answer(world):
    """A card's decoder returns pixels alone; the reader reads each decoded
    file's header for its own orientation and thickness, as the CPU path
    does, so the card's answer is the CPU's (the speed study, round 2)."""
    import copy

    st = next(s for s in stacks(world) if s.stack_id == 12)
    sg = copy.deepcopy(st.extra.get("geometry"))
    sg["thick"] = 9.5  # the registry's value is not the files'

    def pixels_only(todo):
        return reduced._decode_with_headers(todo, {})

    cpu = reduced.build(st.files, st.file_geo, sg, "sagittal", "b8")
    card = reduced.build(st.files, st.file_geo, sg, "sagittal", "b8", decode=pixels_only)
    assert card.meta == cpu.meta == volume.build(st.files, "sagittal").meta
    assert np.array_equal(card.vol, cpu.vol)
    assert card.stats.get("thick_from_file") == cpu.stats.get("thick_from_file") == 1
