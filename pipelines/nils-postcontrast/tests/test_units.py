# SPDX-License-Identifier: AGPL-3.0-only
"""The pieces of the cascade without a model: the slice choice, the fusion's
arithmetic and margin rule, the header evidence, the parts' checks and the
manifest. The student's own test needs torch (test_student.py)."""

from __future__ import annotations

import hashlib
import json
import math
from pathlib import Path

import numpy as np
import pytest

from nils_postcontrast import evidence, manifest, parts, slices
from nils_postcontrast.fusion import HCOLS, Fusion, FusionError, arm_probs, fuse, margin, stage1

COLS = ["zT", "missT", "zU", "missU", "zV", "missV", *HCOLS, "header_masked", "pair_s", "pair_up", "pair_down", "has_pair", "pair_masked"]


def fusion_doc(**kw):
    d = {"cols": COLS, "scaler": {"mean": [0.0] * 20, "scale": [1.0] * 20},
         "head": {"coef": [1.0, 0, 1.0, 0, 1.0, 0] + [0.5] * 8 + [0, 0, 0, 0, 0, 0], "intercept": 0.0},
         "calibration": {"cohort-a Siemens": [1.0, 0.0], "cohort-b Philips": [2.0, -0.5]}, "t": 0.9, "use_V": True,
         "cohort_aliases": {"dataset-a": "cohort-a"}, "untrusted_time_cohorts": ["cohort-b"]}
    d.update(kw)
    return d


@pytest.fixture
def F(tmp_path):
    p = tmp_path / "fusion.json"
    p.write_text(json.dumps(fusion_doc()))
    return Fusion.load(p)


# ---------------------------------------------------------------- slices

def axial(n=40, step=4.0, files_extra=None):
    iop = [1, 0, 0, 0, 1, 0]
    files = [(f"/s/{i:03d}.dcm", [0.0, 0.0, i * step], [0.5, 0.5], 512, 512, i + 1, 1) for i in range(n)]
    return iop, files


def test_geometry_orders_along_the_normal_and_drops_repeats():
    iop, files = axial(10)
    files = list(reversed(files)) + [("/s/dup.dcm", [0.0, 0.0, 8.01], [0.5, 0.5], 512, 512, 99, 1)]
    G = slices.stack_geometry(iop, files, False)
    assert G["plane"] == "axial"
    assert [round(t[0], 2) for t in G["items"]] == [i * 4.0 for i in range(10)]


def test_geometry_refuses_multi_frame_missing_position_and_orientation():
    iop, files = axial(5)
    assert slices.stack_geometry(iop, files, True) is None
    assert slices.stack_geometry(None, files, False) is None
    bad = files[:2] + [("/s/x.dcm", None, [0.5, 0.5], 512, 512, 3, 1)]
    assert slices.stack_geometry(iop, bad, False) is None
    mf = files[:2] + [("/s/x.dcm", [0, 0, 99], [0.5, 0.5], 512, 512, 3, 4)]
    assert slices.stack_geometry(iop, mf, False) is None


def test_axial_pick_reads_the_top_150_mm_in_the_rule_order():
    iop, files = axial(60, 4.0)  # 236 mm
    G = slices.stack_geometry(iop, files, False)
    picks = slices.fs_pick(G, 3)
    top, lo = 236.0, 236.0 - 150.0
    want = [lo + f * (top - lo) for f in (0.5, 0.25, 0.75)]
    got = [G["items"][j][0] for j, _ in picks]
    assert all(abs(g - w) <= 2.0 for g, w in zip(got, want))
    assert [round(sg[0], 3) for _, sg in picks] == [0.5, 0.25, 0.75]
    assert len({j for j, _ in picks}) == 3


def test_sagittal_pick_is_the_middle_and_its_neighbours():
    iop = [0, 1, 0, 0, 0, -1]
    files = [(f"/s/{i}.dcm", [i * 1.0, 0, 0], [1, 1], 256, 256, i, 1) for i in range(160)]
    G = slices.stack_geometry(iop, files, False)
    assert G["plane"] == "sagittal"
    got = [G["items"][j][0] for j, _ in slices.fs_pick(G, 3)]
    mid = -79.5  # the normal of these rows and columns points to -x
    assert abs(got[0] - mid) <= 0.5 and abs(got[1] - (mid - 5)) <= 0.5 and abs(got[2] - (mid + 5)) <= 0.5


def test_stack_tokens_are_thirteen():
    iop, files = axial(30, 5.0)
    G = slices.stack_geometry(iop, files, False)
    t = slices.stack_tokens(G, 1.0, 5.0)
    assert len(t) == slices.NTOK
    assert t[0] == 1.0 and t[11] == 1.0 and t[12] == 0.0
    assert math.isclose(t[4], 0.5) and math.isclose(t[5], 0.5)


def test_stack_input_keeps_a_slot_empty_when_a_file_fails():
    iop, files = axial(30)
    G = slices.stack_geometry(iop, files, False)
    calls = []

    def read(p):
        calls.append(p)
        if len(calls) == 2:
            raise OSError("unreadable")
        return np.ones((256, 256), np.float16)

    x, opened = slices.stack_input(G, 3, 0.0, 4.0, read=read)
    tiles, valid, sg, tok = x
    assert opened == 3 and valid.tolist() == [True, False, True]
    assert sg[1].tolist() == [0, 0, 0, 0] and tiles[1].max() == 0
    none, opened = slices.stack_input(G, 3, 0.0, 4.0, read=lambda p: None)
    assert none is None and opened == 3


def test_tile_scales_resamples_and_centres(tmp_path):
    pydicom = pytest.importorskip("pydicom")
    from pydicom.dataset import FileDataset, FileMetaDataset
    from pydicom.uid import ExplicitVRLittleEndian, generate_uid

    meta = FileMetaDataset()
    meta.TransferSyntaxUID = ExplicitVRLittleEndian
    meta.MediaStorageSOPClassUID = "1.2.840.10008.5.1.4.1.1.4"
    meta.MediaStorageSOPInstanceUID = generate_uid()
    ds = FileDataset(str(tmp_path / "a.dcm"), {}, file_meta=meta, preamble=b"\0" * 128)
    a = (np.arange(128 * 128).reshape(128, 128) % 1000).astype(np.uint16)
    ds.Rows, ds.Columns = 128, 128
    ds.BitsAllocated, ds.BitsStored, ds.HighBit, ds.PixelRepresentation = 16, 16, 15, 0
    ds.SamplesPerPixel, ds.PhotometricInterpretation = 1, "MONOCHROME2"
    ds.PixelSpacing = [2.0, 2.0]
    ds.PixelData = a.tobytes()
    ds.save_as(str(tmp_path / "a.dcm"), enforce_file_format=True)
    t = slices.tile(str(tmp_path / "a.dcm"))
    assert t.shape == (256, 256) and t.dtype == np.float16
    assert float(t.min()) >= -0.5 and float(t.max()) <= 4.0
    assert pydicom


# ---------------------------------------------------------------- fusion

def test_fusion_file_is_checked(tmp_path):
    p = tmp_path / "f.json"
    p.write_text(json.dumps(fusion_doc(cols=COLS[:-1])))
    with pytest.raises(FusionError):
        Fusion.load(p)
    p.write_text("not json")
    with pytest.raises(FusionError):
        Fusion.load(p)


def test_fusion_matches_its_formula(F):
    rows = [{"site": "cohort-a Siemens", "brain": True, **{c: 0.0 for c in HCOLS}}]
    P = {"T": np.array([0.99], np.float32), "U": np.array([np.nan], np.float32), "V": np.array([0.9], np.float32)}
    p, a = fuse(F, rows, P)
    zT = float(np.log(np.float32(0.99) / (1 - np.float32(0.99))))
    zV = float(np.log(np.float32(0.9) / (1 - np.float32(0.9))))
    want = 1 / (1 + math.exp(-(zT + 0.0 + zV)))
    assert abs(p[0] - want) < 1e-6 and a[0] == "given"


def test_no_site_and_no_brain_are_not_sure(F):
    P = {k: np.array([0.999, 0.999], np.float32) for k in "TUV"}
    rows = [{"site": "cohort-c GE", "brain": True}, {"site": "cohort-a Siemens", "brain": False}]
    p, a = fuse(F, rows, P)
    assert np.isnan(p[0]) and a.tolist() == ["not sure", "not sure"]


def test_margin_is_the_studys():
    t = 0.9
    lg = lambda x: math.log(x / (1 - x))  # noqa: E731
    assert margin(float("nan"), t) == 0.0
    assert abs(margin(0.999, t) - (lg(0.999) - lg(0.9))) < 1e-12
    assert abs(margin(0.001, t) - abs(lg(0.001) + lg(0.9))) < 1e-12
    assert margin(0.5, t) < 4  # between the thresholds the margin cannot reach 4


def test_stage1_accepts_only_agreeing_seeds_far_from_the_threshold(F):
    rows = [{"site": "cohort-a Siemens", "brain": True}] * 3
    big = np.array([12.0, 12.0, 12.0, 1, 1, 1], np.float32)
    small = np.array([1.0, 1.0, 1.0, 1, 1, 1], np.float32)
    flip = np.array([-12.0, -12.0, -12.0, 1, 1, 1], np.float32)
    out = np.stack([np.stack([big, big, big]), np.stack([small, small, small]), np.stack([big, big, flip])]).astype(np.float32)
    r = stage1(F, rows, out, 4.0)
    assert [x.accepted for x in r] == [True, False, False]
    assert r[0].answer == "given" and r[2].agree is False


def test_arm_availability_is_the_second_half():
    o = np.array([[2.0, 2.0, 2.0, 1.0, -1.0, 0.0]], np.float32)
    P = arm_probs(o)
    assert not np.isnan(P["T"][0]) and np.isnan(P["U"][0]) and np.isnan(P["V"][0])
    assert P["T"].dtype == np.float32


# ---------------------------------------------------------------- evidence

def entry(sid, series, study, *, base="T1w", tech="MPRAGE", bp="brain", agent=None, time=None, sn=None, disp="acquisition", derived=False,
          rules=(), pc=None, datasets=("cohort-a",), make="SIEMENS", sealed=False, modality="MR", dirtype="anat"):
    cls = {"base": [{"value": base}], "technique": [{"value": tech}], "body_part": [{"value": bp}], "disposition": [{"value": disp}],
           "directory_type": [{"value": dirtype}]}
    if pc is not None:
        cls["post_contrast"] = [{"value": pc}]
    return manifest.Entry(sid, f"stack-{sid}", series, modality, {"study": study, "sealed": sealed, "image_orientation_patient": [1, 0, 0, 0, 1, 0]},
                          {"fingerprint": {"manufacturer": make, "slice_thickness": 1.0}, "classification": cls, "datasets": list(datasets)},
                          {"contrast_bolus_agent": agent, "acquisition_time": time, "series_number": sn, "mr_acquisition_type": "3D", "derived": derived,
                           "post_contrast_rules": list(rules)})


def test_session_evidence_and_time_trust(F):
    s = [entry(1, 10, 7, time="10:00:00", sn=1), entry(2, 11, 7, time="10:06:00", sn=2, base="T2w", tech="TSE"),
         entry(3, 12, 7, time="10:20:00", sn=3, agent="Gadovist", rules=("positive",), pc="1")]
    R = evidence.rows_for(s, [], F)
    assert R[3]["h_agent"] == 1.0 and R[3]["h_words"] == 1.0 and R[3]["h_rules_given"] == 1.0
    assert R[1]["h_ses_any_agent"] == 1.0 and R[1]["h_ses_all_agent"] == 0.0
    assert R[1]["h_time_trusted"] == 1.0 and R[1]["h_before_first_tag"] == 1.0 and R[3]["h_at_or_after_first_tag"] == 1.0
    assert R[1]["site"] == "cohort-a Siemens" and R[1]["family"] == "T1-type" and R[1]["brain"]


def test_a_word_for_none_is_no_tag_and_context_counts(F):
    s = [entry(1, 10, 7, time="10:00:00", sn=1, agent="  none ")]
    ctx = [entry(2, 11, 7, time="10:30:00", sn=2, agent="Dotarem")]
    R = evidence.rows_for(s, ctx, F)
    assert R[1]["h_agent"] == 0.0 and R[1]["h_ses_any_agent"] == 1.0 and R[1]["h_before_first_tag"] == 1.0


def test_untrusted_clocks(F):
    one = [entry(1, 10, 7, time="10:00:00", sn=1)]
    assert evidence.rows_for(one, [], F)[1]["h_time_trusted"] == 0.0  # one series
    mid = [entry(1, 10, 7, time="00:00:00", sn=1), entry(2, 11, 7, time="00:10:00", sn=2)]
    assert evidence.rows_for(mid, [], F)[1]["h_time_trusted"] == 0.0  # midnight
    back = [entry(i, 10 + i, 7, time=f"10:{10 * (3 - i):02d}:00", sn=i) for i in range(4)]
    assert evidence.rows_for(back, [], F)[1]["h_time_trusted"] == 0.0  # time against series number
    ms = [entry(1, 10, 7, time="10:00:00", sn=1, datasets=("cohort-b",), make="Philips"),
          entry(2, 11, 7, time="10:10:00", sn=2, datasets=("cohort-b",), make="Philips")]
    r = evidence.rows_for(ms, [], F)
    assert r[1]["h_time_trusted"] == 0.0 and r[1]["site"] == "cohort-b Philips"


def test_candidacy(F):
    s = [entry(1, 10, 7), entry(2, 11, 7, bp="spine"), entry(3, 12, 7, tech="Perfusion"), entry(4, 13, 7, derived=True),
         entry(5, 14, 7, dirtype="localizer"), entry(6, 15, 7, base="T2w", tech="TSE")]
    R = evidence.rows_for(s, [], F)
    ok = {k: v["acquired"] and v["family"] == "T1-type" and v["brain"] for k, v in R.items()}
    assert ok == {1: True, 2: False, 3: False, 4: False, 5: False, 6: False}


def test_cohort_aliases_and_batch(F):
    e = entry(1, 10, 7, datasets=("dataset-a",))
    assert evidence.cohort_of(e, F) == ("cohort-a", "dataset")
    e = entry(1, 10, 7, datasets=())
    e.header["batch"] = "reread-2026-cohort-a"
    assert evidence.cohort_of(e, F) == ("cohort-a", "batch_part")
    e.header["batch"] = "dcm-raw-2026-10-01"
    assert evidence.cohort_of(e, F) == (None, None)


def test_times_parse_both_ways():
    e = entry(1, 10, 7, time="10:15:30.25")
    assert evidence.tsec(e) == 36930.25
    e.contrast["acquisition_time"] = "101530"
    assert evidence.tsec(e) == 36930
    e.contrast["acquisition_time"] = "noon"
    assert evidence.tsec(e) is None


# ---------------------------------------------------------------- parts and manifest

def write_parts(tmp, F_doc, m=4, k=3, tamper=None):
    I = tmp / "inputs"
    for d in ("student", "fusion", "cascade"):
        (I / d).mkdir(parents=True, exist_ok=True)
    (I / "student" / "student.safetensors").write_bytes(b"weights")
    (I / "fusion" / "fusion.json").write_text(json.dumps(F_doc))
    sha = lambda p: "sha256:" + hashlib.sha256(p.read_bytes()).hexdigest()  # noqa: E731
    sd, fd = sha(I / "student" / "student.safetensors"), sha(I / "fusion" / "fusion.json")
    cas = {"name": "c1-fs3-m", "stage1": {"rule": "margin", "m": m, "k": k}, "parts": {"student": sd, "fusion": tamper or fd}}
    (I / "cascade" / "cascade.json").write_text(json.dumps(cas))
    ms = [{"input": "student", "digest": sd, "artifact": "student/student.safetensors"},
          {"input": "fusion", "digest": fd, "artifact": "fusion/fusion.json"},
          {"input": "cascade", "digest": sha(I / "cascade" / "cascade.json"), "artifact": "cascade/cascade.json",
           "card": {"threshold": 0.9, "parts": {"student": sd, "fusion": fd}}}]
    (I / "manifest.json").write_text(json.dumps({"models": ms}))
    return I


def test_parts_are_checked(tmp_path):
    I = write_parts(tmp_path, fusion_doc())
    c = parts.cascade(I)
    assert c.m == 4 and c.name == "c1-fs3-m"
    I2 = write_parts(tmp_path / "b", fusion_doc(), tamper="sha256:" + "0" * 64)
    with pytest.raises(parts.PartsError, match="another fusion"):
        parts.cascade(I2)
    I3 = write_parts(tmp_path / "c", fusion_doc(), k=5)
    with pytest.raises(parts.PartsError, match="three-slice"):
        parts.cascade(I3)
    (I / "fusion" / "fusion.json").write_text("{}")
    with pytest.raises(parts.PartsError, match="digest"):
        parts.cascade(I)


def test_manifest_reads_stacks_and_context(tmp_path):
    doc = {"contract": "job/v1", "sources": [{"id": 3, "mount": "/source/3"}],
           "stacks": [{"unit": "stack-5", "stack_id": 5, "series_id": 9, "modality": "MR",
                       "files": [{"source": 3, "path": "a/2.dcm", "frames": None, "geometry": {"image_position_patient": [0, 0, 2], "instance_number": 2}},
                                 {"source": 3, "path": "a/1.dcm", "frames": None, "geometry": {"image_position_patient": [0, 0, 1], "instance_number": 1}}],
                       "geometry": {"study": 4, "sealed": False}, "header": {}, "contrast": {}}],
           "context": [{"stack_id": 6, "series_id": 10, "modality": "MR", "geometry": {"study": 4}, "header": {}, "contrast": {}}]}
    p = tmp_path / "stacks.json"
    p.write_text(json.dumps(doc))
    st, ctx = manifest.read(p, Path("/x"))
    assert st[0].files[0][0] == "/x/3/a/2.dcm" and ctx[0].stack_id == 6 and not ctx[0].scored
    assert [f[5] for f in manifest.ordered_files(st[0])] == [1, 2]
    assert not manifest.multi_frame(st[0])
    doc["stacks"][0]["files"][0]["path"] = "../escape.dcm"
    p.write_text(json.dumps(doc))
    with pytest.raises(manifest.ManifestError):
        manifest.read(p)
