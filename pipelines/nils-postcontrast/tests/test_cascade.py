# SPDX-License-Identifier: AGPL-3.0-only
"""The console script end to end on synthetic DICOM files, with a student of
random weights: every stack gets a table, the reasons are the right ones,
answered stacks get proposals by the cascade's digest, and a refused part
ends the run with exit 1 and results.json saying why. Needs torch and timm."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import numpy as np
import pytest

torch = pytest.importorskip("torch")
pytest.importorskip("timm")

from nils_postcontrast import cli  # noqa: E402
from nils_postcontrast.fusion import HCOLS  # noqa: E402
from nils_postcontrast.student import Student, net  # noqa: E402

COLS = ["zT", "missT", "zU", "missU", "zV", "missV", *HCOLS, "header_masked", "pair_s", "pair_up", "pair_down", "has_pair", "pair_masked"]


def dicom(path: Path, z: float, value: int):
    from pydicom.dataset import FileDataset, FileMetaDataset
    from pydicom.uid import ExplicitVRLittleEndian, generate_uid

    meta = FileMetaDataset()
    meta.TransferSyntaxUID = ExplicitVRLittleEndian
    meta.MediaStorageSOPClassUID = "1.2.840.10008.5.1.4.1.1.4"
    meta.MediaStorageSOPInstanceUID = generate_uid()
    ds = FileDataset(str(path), {}, file_meta=meta, preamble=b"\0" * 128)
    rng = np.random.default_rng(int(z * 10) + value)
    a = (rng.random((64, 64)) * value).astype(np.uint16)
    ds.Rows, ds.Columns = 64, 64
    ds.BitsAllocated, ds.BitsStored, ds.HighBit, ds.PixelRepresentation = 16, 16, 15, 0
    ds.SamplesPerPixel, ds.PhotometricInterpretation = 1, "MONOCHROME2"
    ds.PixelSpacing = [4.0, 4.0]
    ds.PixelData = a.tobytes()
    path.parent.mkdir(parents=True, exist_ok=True)
    ds.save_as(str(path), enforce_file_format=True)


def setup_run(tmp: Path, intercept: float):
    src = tmp / "src"
    stacks = []

    def stack(sid, series, *, base="T1w", bp="brain", sealed=False, n=20, files=True, agent=None, t="10:00:00", sn=1, datasets=("cohort-a",)):
        fl = []
        if files:
            for i in range(n):
                p = src / f"s{sid}" / f"{i:03d}.dcm"
                dicom(p, i * 5.0, 1000 + sid)
                fl.append({"source": 0, "path": f"s{sid}/{i:03d}.dcm", "frames": None,
                           "geometry": {"image_position_patient": [0.0, 0.0, i * 5.0], "pixel_spacing": [4.0, 4.0], "rows": 64, "columns": 64,
                                        "instance_number": i + 1, "number_of_frames": 1}})
        stacks.append({"unit": f"stack-{sid}", "stack_id": sid, "series_id": series, "modality": "MR", "files": fl,
                       "geometry": {"image_orientation_patient": [1, 0, 0, 0, 1, 0], "slice_thickness": 5.0, "study": 1, "sealed": sealed},
                       "header": {"fingerprint": {"manufacturer": "SIEMENS", "slice_thickness": 5.0, "modality": "MR"},
                                  "classification": {"base": [{"value": base}], "technique": [{"value": "MPRAGE"}], "body_part": [{"value": bp}],
                                                     "disposition": [{"value": "acquisition"}], "directory_type": [{"value": "anat"}]},
                                  "datasets": list(datasets)},
                       "contrast": {"contrast_bolus_agent": agent, "acquisition_time": t, "series_number": sn, "mr_acquisition_type": "3D",
                                    "derived": False, "post_contrast_rules": []}})

    stack(1, 11, t="10:00:00", sn=1)
    stack(2, 12, t="10:20:00", sn=5, agent="Gadovist")
    stack(3, 13, bp="spine", t="10:05:00", sn=2)
    stack(4, 14, base="T2w", t="10:10:00", sn=3)
    stack(5, 15, sealed=True, files=False, t="10:12:00", sn=4)
    stack(6, 16, files=False, t="10:14:00", sn=6)  # no files: no geometry, deferred
    stack(7, 17, datasets=("cohort-c",), t="10:16:00", sn=7)
    manifest = {"contract": "job/v1", "sources": [{"id": 0, "mount": str(src)}], "stacks": stacks}
    (tmp / "stacks.json").write_text(json.dumps(manifest))
    I = tmp / "inputs"
    for d in ("student", "fusion", "cascade"):
        (I / d).mkdir(parents=True, exist_ok=True)
    from safetensors.torch import save_file

    sd = {}
    for s in range(3):
        torch.manual_seed(s)
        for k, v in net().state_dict().items():
            sd[f"s{s}.{k}"] = v.contiguous()
    save_file(sd, str(I / "student" / "student.safetensors"))
    fdoc = {"cols": COLS, "scaler": {"mean": [0.0] * 20, "scale": [1.0] * 20},
            "head": {"coef": [0.0] * 20, "intercept": intercept}, "calibration": {"cohort-a Siemens": [1.0, 0.0]}, "t": 0.9, "use_V": True}
    (I / "fusion" / "fusion.json").write_text(json.dumps(fdoc))
    sha = lambda p: "sha256:" + hashlib.sha256(p.read_bytes()).hexdigest()  # noqa: E731
    sdg, fdg = sha(I / "student" / "student.safetensors"), sha(I / "fusion" / "fusion.json")
    (I / "cascade" / "cascade.json").write_text(json.dumps({"name": "c1-fs3-m", "stage1": {"rule": "margin", "m": 4, "k": 3},
                                                            "parts": {"student": sdg, "fusion": fdg}}))
    ms = [{"input": "student", "digest": sdg, "artifact": "student/student.safetensors"},
          {"input": "fusion", "digest": fdg, "artifact": "fusion/fusion.json", "card": {"threshold": 0.9}},
          {"input": "cascade", "digest": sha(I / "cascade" / "cascade.json"), "artifact": "cascade/cascade.json",
           "card": {"threshold": 0.9, "parts": {"student": sdg, "fusion": fdg}}}]
    (I / "manifest.json").write_text(json.dumps({"models": ms}))
    return tmp / "stacks.json", I


def run(tmp, intercept):
    stacks, I = setup_run(tmp, intercept)
    out = tmp / "out"
    rc = cli.main(["cascade", "--stacks", str(stacks), "--inputs", str(I), "--output", str(out), "--threads", "2", "--device", "cpu"])
    return rc, out


def tables(out):
    return {int(p.stem): json.loads(p.read_text()) for p in (out / "postcontrast").glob("*.json")}


def test_a_confident_fusion_answers_and_proposes(tmp_path):
    rc, out = run(tmp_path, 9.0)  # every row's probability 0.99988: far beyond t, the seeds agree
    assert rc == 0
    T = tables(out)
    assert set(T) == {1, 2, 3, 4, 5, 6, 7}
    assert T[1]["answers"] == 1 and T[1]["value"] == "given" and T[1]["stage"] == "fs3" and T[1]["files_read"] == 3
    assert T[3]["reason"] == "not-t1-brain" and T[4]["reason"] == "not-t1-brain"
    assert T[5]["reason"] == "sealed" and T[6]["reason"] == "deferred" and T[6]["stage1"] == "no-geometry"
    assert T[7]["reason"] == "no-calibration"
    res = json.loads((out / "results.json").read_text())
    props = {p["stack_id"]: p for p in res["proposals"]}
    assert set(props) == {1, 2}
    assert props[1]["axis"] == "post_contrast" and props[1]["model_digest"] == T[1]["cascade_digest"]
    assert abs(sum(props[1]["probabilities"].values()) - 1) < 1e-6
    assert res["metrics"]["deferred"] == [6]


def test_an_unsure_fusion_defers(tmp_path):
    rc, out = run(tmp_path, 0.5)  # probability 0.62: between the thresholds, never accepted
    T = tables(out)
    assert rc == 0 and T[1]["reason"] == "deferred" and T[1]["stage1"] == "deferred" and T[1]["answers"] == 0
    res = json.loads((out / "results.json").read_text())
    assert "proposals" not in res and sorted(res["metrics"]["deferred"]) == [1, 2, 6]


def test_a_tampered_part_refuses_the_run(tmp_path):
    stacks, I = setup_run(tmp_path, 9.0)
    (I / "fusion" / "fusion.json").write_text("{}")
    out = tmp_path / "out"
    rc = cli.main(["cascade", "--stacks", str(stacks), "--inputs", str(I), "--output", str(out), "--device", "cpu"])
    assert rc == 1 and "digest" in json.loads((out / "results.json").read_text())["error"]


def test_the_student_is_three_seeds(tmp_path):
    from safetensors.torch import save_file

    sd = {f"s{s}.{k}": v.contiguous() for s in range(3) for k, v in net().state_dict().items()}
    save_file(sd, str(tmp_path / "s.safetensors"))
    st = Student(tmp_path / "s.safetensors")
    o = st.predict(np.zeros((2, 3, 256, 256), np.float16), np.ones((2, 3), bool), np.zeros((2, 3, 4), np.float32), np.zeros((2, 13), np.float32))
    assert o.shape == (2, 3, 6) and o.dtype == np.float32
    save_file({k: v for k, v in sd.items() if not k.startswith("s2.")}, str(tmp_path / "two.safetensors"))
    with pytest.raises(ValueError, match="seed 2"):
        Student(tmp_path / "two.safetensors")


# ---------------------------------------------------------------- a site's calibration (0.3.0)

# the columns before 0.3.0, and the raw-score mode's five, on every row of every run (null without the mode), so the table's
# shape is constant and the engine's measures never miss a declared column
EARLIER_COLUMNS = ["p_given", "value", "confidence", "answers", "stage", "stage1", "reason", "margin", "seeds_agree", "files_read",
                   "cascade_digest", "student_digest", "fusion_digest", "stack_id"]
RAW_COLUMNS = ["score_raw", "score_raw_seeds", "site", "make", "cohort_candidates"]
COLUMNS = EARLIER_COLUMNS[:10] + RAW_COLUMNS + EARLIER_COLUMNS[10:]


def test_without_the_raw_scores_flag_the_output_is_the_earlier_versions_with_the_five_columns_null(tmp_path):
    rc, out = run(tmp_path, 9.0)
    assert rc == 0
    T = tables(out)
    for doc in T.values():
        assert list(doc) == COLUMNS
        assert all(doc[k] is None for k in RAW_COLUMNS), doc
    assert T[1]["answers"] == 1 and T[1]["value"] == "given" and T[7]["reason"] == "no-calibration" and T[6]["reason"] == "deferred"
    res = json.loads((out / "results.json").read_text())
    assert res["params"] == {"workers": 2, "batch": 64, "device": "cpu"} and "calibration" not in res
    assert all(set(u["metrics"]) == {"value", "confidence", "stage", "stage1", "reason", "files_read"} for u in res["units"])


def test_raw_scores_write_the_log_odds_of_a_stack_with_no_calibration_and_never_propose(tmp_path):
    stacks, I = setup_run(tmp_path, 9.0)
    out = tmp_path / "out"
    rc = cli.main(["cascade", "--stacks", str(stacks), "--inputs", str(I), "--output", str(out), "--threads", "2", "--device", "cpu", "--raw-scores"])
    assert rc == 0
    T = tables(out)
    t7 = T[7]  # a cohort the fusion has no pair for
    assert t7["reason"] == "no-calibration" and t7["stage1"] == "raw" and t7["answers"] == 0
    assert t7["p_given"] is None and t7["value"] is None and t7["confidence"] is None
    assert abs(t7["score_raw"] - 9.0) < 1e-5  # every coefficient 0: the log-odds is the intercept
    seeds = [float(x) for x in t7["score_raw_seeds"].split()]
    assert len(seeds) == 3 and all(abs(z - 9.0) < 1e-5 for z in seeds)
    assert t7["site"] is None and t7["make"] == "Siemens" and t7["cohort_candidates"] == json.dumps(["cohort-c"]) and t7["files_read"] == 3
    # every other stack as before, its site columns filled and no raw score
    assert T[1]["answers"] == 1 and T[1]["value"] == "given" and T[1]["score_raw"] is None and T[1]["site"] == "cohort-a Siemens"
    assert T[1]["cohort_candidates"] == json.dumps(["cohort-a"])
    assert T[3]["reason"] == "not-t1-brain" and T[3]["score_raw"] is None and T[6]["reason"] == "deferred"
    assert list(T[1]) == COLUMNS
    res = json.loads((out / "results.json").read_text())
    assert {p["stack_id"] for p in res["proposals"]} == {1, 2} and res["params"]["raw_scores"] is True
    units = {u["unit_id"]: u for u in res["units"]}
    assert units["stack-7"]["status"] == "succeeded" and units["stack-7"]["metrics"]["score_raw"] == t7["score_raw"]
    assert res["metrics"]["by_outcome"]["no-calibration"] == 1 and res["metrics"]["deferred"] == [6] and res["metrics"]["raw_scored"] == 1


def test_raw_scores_defer_a_stack_the_student_could_not_read_at_a_site_with_no_pair(tmp_path):
    stacks, I = setup_run(tmp_path, 9.0)
    # a stack at the uncalibrated site with no files: nothing was scored,
    # so its reason is the read's, never no-calibration
    m = json.loads(stacks.read_text())
    eight = dict(m["stacks"][-1], unit="stack-8", stack_id=8, series_id=18, files=[])
    m["stacks"].append(eight)
    stacks.write_text(json.dumps(m))
    out = tmp_path / "out"
    rc = cli.main(["cascade", "--stacks", str(stacks), "--inputs", str(I), "--output", str(out), "--threads", "2", "--device", "cpu", "--raw-scores"])
    assert rc == 0
    T = tables(out)
    assert T[8]["reason"] == "deferred" and T[8]["stage1"] == "no-geometry" and T[8]["score_raw"] is None and T[8]["cohort_candidates"] == json.dumps(["cohort-c"])
    assert T[7]["reason"] == "no-calibration" and T[7]["score_raw"] is not None
    res = json.loads((out / "results.json").read_text())
    assert res["metrics"]["deferred"] == [6, 8] and res["metrics"]["raw_scored"] == 1 and res["metrics"]["by_outcome"]["no-calibration"] == 1


def test_a_calibration_file_scores_a_site_the_fusion_has_no_pair_for(tmp_path, monkeypatch):
    stacks, I = setup_run(tmp_path, 9.0)
    cal = tmp_path / "pairs.json"
    cal.write_text(json.dumps({"cohort-c Siemens": [1.0, 0.0]}))
    out = tmp_path / "out"
    rc = cli.main(["cascade", "--stacks", str(stacks), "--inputs", str(I), "--output", str(out), "--threads", "2", "--device", "cpu",
                   "--calibration", str(cal)])
    assert rc == 0
    T = tables(out)
    assert T[7]["answers"] == 1 and T[7]["value"] == "given" and T[7]["reason"] is None and list(T[7]) == COLUMNS and T[7]["site"] is None
    res = json.loads((out / "results.json").read_text())
    # a map names no fusion: taken as it is, and said to be unchecked
    assert {p["stack_id"] for p in res["proposals"]} == {1, 2, 7} and res["calibration"] == {"sites": {"cohort-c Siemens": 1}, "unchecked": True}
    # the descriptor's site file reaches the run through the environment
    monkeypatch.setenv("NILS_PC_CALIBRATION", str(cal))
    out2 = tmp_path / "out2"
    rc = cli.main(["cascade", "--stacks", str(stacks), "--inputs", str(I), "--output", str(out2), "--threads", "2", "--device", "cpu"])
    assert rc == 0 and tables(out2)[7]["answers"] == 1
    monkeypatch.delenv("NILS_PC_CALIBRATION")
    # a pair file fitted on this fusion is checked; one fitted on another is refused
    fdg = "sha256:" + hashlib.sha256((I / "fusion" / "fusion.json").read_bytes()).hexdigest()
    pair = tmp_path / "pair.json"
    pair.write_text(json.dumps({"site": "cohort-c Siemens", "a": 1.0, "b": 0.0, "model": {"fusion_digest": fdg}}))
    out4 = tmp_path / "out4"
    rc = cli.main(["cascade", "--stacks", str(stacks), "--inputs", str(I), "--output", str(out4), "--threads", "2", "--device", "cpu",
                   "--calibration", str(pair)])
    res = json.loads((out4 / "results.json").read_text())
    assert rc == 0 and res["calibration"] == {"sites": {"cohort-c Siemens": 1}}
    pair.write_text(json.dumps({"site": "cohort-c Siemens", "a": 1.0, "b": 0.0, "model": {"fusion_digest": "sha256:" + "e" * 64}}))
    out5 = tmp_path / "out5"
    rc = cli.main(["cascade", "--stacks", str(stacks), "--inputs", str(I), "--output", str(out5), "--device", "cpu", "--calibration", str(pair)])
    assert rc == 1 and "another fusion" in json.loads((out5 / "results.json").read_text())["error"]
    # a file none of whose keys is any stack's site runs, and the results say so
    cal.write_text(json.dumps({"cohort-z Siemens": [1.0, 0.0]}))
    out6 = tmp_path / "out6"
    rc = cli.main(["cascade", "--stacks", str(stacks), "--inputs", str(I), "--output", str(out6), "--threads", "2", "--device", "cpu",
                   "--calibration", str(cal)])
    res = json.loads((out6 / "results.json").read_text())
    assert rc == 0 and res["calibration"] == {"sites": {"cohort-z Siemens": 0}, "unchecked": True, "matched_none": ["cohort-z Siemens"]}
    assert tables(out6)[7]["reason"] == "no-calibration"
    # a file that is not there refuses the run
    out3 = tmp_path / "out3"
    rc = cli.main(["cascade", "--stacks", str(stacks), "--inputs", str(I), "--output", str(out3), "--device", "cpu",
                   "--calibration", str(tmp_path / "missing.json")])
    assert rc == 1 and "calibration file" in json.loads((out3 / "results.json").read_text())["error"]
