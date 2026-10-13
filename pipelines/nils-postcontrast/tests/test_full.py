# SPDX-License-Identifier: AGPL-3.0-only
"""The full pipeline (``nils-postcontrast full``): the arms' JSON gives
scikit-learn's probabilities, the tail features give what the study's
preprocessing gave through its files, and the console script end to end
on synthetic stacks with stand-ins for dcm2niix and FreeSurfer's tools
(every tool's output written as the real tool's is): candidates answered
by the cascade's digest, every other stack its reason, a stack whose
conversion fails once converted on the retry, a tool that dies taking its
pool with it never ending the run."""

from __future__ import annotations

import base64
import functools
import hashlib
import json
import os
import stat
import sys
from pathlib import Path

import numpy as np
import pytest

nib = pytest.importorskip("nibabel")
pytest.importorskip("scipy")
pytest.importorskip("skimage")
sitk = pytest.importorskip("SimpleITK")

from nils_postcontrast import cli, features, full, tools  # noqa: E402
from nils_postcontrast.arms import Arms  # noqa: E402
from nils_postcontrast.fusion import HCOLS  # noqa: E402

COLS = ["zT", "missT", "zU", "missU", "zV", "missV", *HCOLS, "header_masked", "pair_s", "pair_up", "pair_down", "has_pair", "pair_masked"]
HP_NAMES = ["pituitary", "cavernous_l", "cavernous_r", "sinus_midline", "sinus_transverse_l", "sinus_transverse_r", "nasal_mucosa",
            "choroid_plexus_atria", "wm_centrum", "csf_ventricles"]
VX_NAMES = ["va_straight", "va_deep", "va_sigmoid_l", "va_sigmoid_r", "va_transverse_l", "va_transverse_r", "va_sinus_midline", "va_cortical", "pons"]


# ---------- the arms ----------


def export(rf, u, v, tcols, ucols, vcols, atlas=None) -> dict:
    """export_arms.py's arithmetic (the study's tool), on fitted models."""
    def forest(m):
        trees = []
        for est in m.estimators_:
            t = est.tree_
            val = t.value[:, 0, :].astype(np.float64)
            s = val.sum(axis=1)
            s[s == 0] = 1.0
            trees.append({"left": t.children_left.tolist(), "right": t.children_right.tolist(), "feature": t.feature.tolist(),
                          "threshold": t.threshold.tolist(), "p1": (val / s[:, None])[:, 1].tolist()})
        return {"kind": "forest", "cols": tcols, "trees": trees}

    def logistic(pipe, cols):
        imp, sc, lr = (pipe.named_steps[k] for k in ("simpleimputer", "standardscaler", "logisticregression"))
        return {"kind": "logistic", "cols": cols, "fill": [None if x != x else float(x) for x in imp.statistics_], "mean": sc.mean_.tolist(),
                "scale": sc.scale_.tolist(), "coef": lr.coef_[0].tolist(), "intercept": float(lr.intercept_[0])}

    return {"u_source": "HP", "T": forest(rf), "U": logistic(u, ucols), "V": logistic(v, vcols), "atlas": atlas or {}, "source_sha256": "x"}


def test_the_arms_json_gives_scikit_learns_probabilities(tmp_path):
    sk = pytest.importorskip("sklearn")  # noqa: F841
    from sklearn.ensemble import RandomForestClassifier
    from sklearn.impute import SimpleImputer
    from sklearn.linear_model import LogisticRegression
    from sklearn.pipeline import make_pipeline
    from sklearn.preprocessing import StandardScaler
    import pandas as pd

    rng = np.random.default_rng(1)
    n = 400
    tcols = ["T:" + k for k in features.T_KEYS]
    ucols = ["HP:a:med_wm", "HP:b:med_wm", "HP:empty:ml"]
    vcols = ["V:x", "V:y"]
    X = pd.DataFrame(rng.normal(size=(n, 10)), columns=tcols + ucols + vcols)
    X["HP:empty:ml"] = np.nan  # a column the imputer drops
    X.loc[rng.random(n) < 0.2, "HP:a:med_wm"] = np.nan
    y = (X[tcols[0]] + X["HP:b:med_wm"] + rng.normal(scale=0.5, size=n) > 0).astype(float)
    rf = RandomForestClassifier(n_estimators=30, max_depth=6, min_samples_leaf=10, class_weight="balanced_subsample", random_state=4).fit(X[tcols], y)
    lr = lambda: make_pipeline(SimpleImputer(strategy="median"), StandardScaler(), LogisticRegression(max_iter=5000))  # noqa: E731
    u = lr().fit(X[ucols], y)
    v = lr().fit(X[vcols], y)
    (tmp_path / "arms.json").write_text(json.dumps(export(rf, u, v, tcols, ucols, vcols)))
    A = Arms.load(tmp_path / "arms.json")
    rows = X.to_dict("records")
    for r in rows:
        r.update(t_domain=True, HP_usable=1.0, V_ok=1.0)
    rows[0]["t_domain"] = float("nan")  # a missing flag is not a set one
    P = A.probabilities(rows)
    assert np.isnan(P["T"][0])
    assert np.abs(P["T"][1:] - rf.predict_proba(X[tcols])[1:, 1]).max() <= 1e-12
    assert np.abs(P["U"] - u.predict_proba(X[ucols])[:, 1]).max() <= 1e-12
    assert np.abs(P["V"] - v.predict_proba(X[vcols])[:, 1]).max() <= 1e-12


# ---------- the tail ----------


def tail_as_the_study(stripped: Path, tmp: Path) -> dict:
    """The copied t1_pre_post preprocessing as the study ran it: files
    written and read back (process_single_file, extract_features_from_file)."""
    orig = sitk.ReadImage(str(stripped))
    od = sitk.GetArrayFromImage(orig)
    om = sitk.GetImageFromArray((od > 0).astype(np.uint8))
    om.CopyInformation(orig)
    img = sitk.ReadImage(str(stripped))
    size, sp = img.GetSize(), img.GetSpacing()
    rs = sitk.ResampleImageFilter()
    rs.SetOutputSpacing((1.0, 1.0, 1.0))
    rs.SetSize([int(round(a * b / 1.0)) for a, b in zip(size, sp)])
    rs.SetOutputDirection(img.GetDirection())
    rs.SetOutputOrigin(img.GetOrigin())
    rs.SetInterpolator(sitk.sitkBSpline)
    r = rs.Execute(img)
    d = sitk.GetArrayFromImage(r)
    mr = sitk.ResampleImageFilter()
    mr.SetReferenceImage(r)
    mr.SetInterpolator(sitk.sitkNearestNeighbor)
    m = sitk.GetArrayFromImage(mr.Execute(om))
    bv = d[m > 0]
    p50, p95, p990, p992, p995, p999 = np.percentile(bv, [50.0, 95.0, 99.0, 99.2, 99.5, 99.9])
    nz = d[d > 0]
    lo, hi = np.percentile(nz, (0.5, 99.5))
    c = np.clip(d, lo, hi)
    nz = c[c > 0]
    z = (c - np.mean(nz)) / np.std(nz)
    nib.save(nib.Nifti1Image(z, np.eye(4)), str(tmp / "z.nii.gz"))
    nib.save(nib.Nifti1Image(m, np.eye(4)), str(tmp / "m.nii.gz"))
    zz = nib.load(str(tmp / "z.nii.gz")).get_fdata()
    mm = nib.load(str(tmp / "m.nii.gz")).get_fdata()
    b = zz[mm > 0]
    p50, p95, p990, p992, p995, p999 = (float(x) for x in (p50, p95, p990, p992, p995, p999))
    eps = 1e-8
    s2, s3 = (p995 - p992) / 0.3, (p999 - p995) / 0.4
    return {"bright_voxel_ratio": float(np.sum(b > 3.0) / b.size), "upper_tail_acceleration": s3 / (s2 + eps),
            "upper_tail_leverage": (p999 - p995) / (p995 - p990 + eps), "extreme_high_slope_ratio": ((p999 - p990) / 0.9) / (((p990 - p95) / 4.0) + eps),
            "tail_steepness_ratio": (p995 - p95) / (p95 - p50 + eps)}


@pytest.mark.parametrize("dtype", [np.float32, np.int16])
def test_the_tail_is_the_studys_through_its_files(tmp_path, dtype):
    vol = brain_volume((40, 48, 30), seed=3).astype(dtype)
    aff = np.diag([1.2, 1.1, 2.5, 1.0])
    nib.save(nib.Nifti1Image(vol, aff), str(tmp_path / "strip.nii.gz"))
    got = features.tail(str(tmp_path / "strip.nii.gz"))
    want = tail_as_the_study(tmp_path / "strip.nii.gz", tmp_path)
    assert got["ok"]
    for k in features.T_KEYS:
        assert got[k] == want[k], k


# ---------- the console script, end to end ----------


def brain_volume(shape, seed=0) -> np.ndarray:
    rng = np.random.default_rng(seed)
    i, j, k = np.indices(shape)
    c = [(s - 1) / 2 for s in shape]
    r = ((i - c[0]) / (shape[0] * 0.4)) ** 2 + ((j - c[1]) / (shape[1] * 0.4)) ** 2 + ((k - c[2]) / (shape[2] * 0.4)) ** 2
    v = np.where(r < 1, 400.0 + 200.0 * (r < 0.3), 0.0) + rng.normal(scale=20, size=shape)
    v[(r < 1) & (rng.random(shape) < 0.01)] = 1500.0  # bright vessels
    return np.clip(v, 0, None).astype(np.float32)


STANDIN = r'''#!{python}
import os, sys
import numpy as np, nibabel as nib
a = sys.argv[1:]
tool = os.path.basename(sys.argv[0])
def shape_of(p):
    return nib.load(p)
def save(arr, like, out):
    nib.save(nib.Nifti1Image(arr.astype(np.int16), nib.load(like).affine), out)
def labels(nat, names):
    lab = np.zeros(nib.load(nat).shape[:3], np.int16)
    for n in range(1, names + 1):
        lab[33:37, 4 * n:4 * n + 3, 0:3] = n
    return lab
if tool == "dcm2niix":
    o, lk = a[a.index("-o") + 1], a[-1]
    flag = os.path.join(o, ".failed-once")
    if os.environ.get("STANDIN_FAIL_ONCE") and os.path.basename(o) == os.environ["STANDIN_FAIL_ONCE"] and not os.path.exists(flag):
        open(flag, "w").close(); sys.exit(1)
    n = len(os.listdir(lk))
    sys.path.insert(0, os.environ["STANDIN_TESTS"])
    from test_full import brain_volume
    v = brain_volume((40, 48, max(n, 8)), seed=n)
    nib.save(nib.Nifti1Image(v, np.diag([1.0, 1.0, 2.0, 1.0])), os.path.join(o, "nat.nii.gz"))
elif tool == "mri_synthmorph" and a[0] == "register":
    open(a[a.index("-T") + 1], "w").write("affine\n")
elif tool == "mri_synthmorph" and a[0] == "apply":
    lta = a[a.index("nearest") + 1]
    nat = os.path.join(os.path.dirname(os.path.dirname(lta)), "nat.nii.gz")
    pairs = a[a.index("nearest") + 2:]
    for src, out in zip(pairs[::2], pairs[1::2]):
        hp = "hp" in os.path.basename(src)
        lab = labels(nat, 8 if hp else 9)
        if hp:  # the white-matter and ventricle references: large
            lab[0:22, 0:22, 0:6] = 9
            lab[22:32, 0:10, 0:6] = 10
        save(lab, nat, out)
elif tool == "mri_synthseg":
    nat, out = a[a.index("--i") + 1], a[a.index("--o") + 1]
    if os.environ.get("STANDIN_SYNTHSEG_DIES"):
        os._exit(137)  # as a tool the kernel killed; it never signals another process
    v = nib.load(nat).get_fdata()
    lab = np.zeros(v.shape, np.int16)
    lab[v > 300] = 2
    lab[v > 550] = 4
    lab[:, :, :2][v[:, :, :2] > 300] = 16
    save(lab, nat, out)
elif tool == "mri_synthstrip":
    nat, out, m = a[a.index("-i") + 1], a[a.index("-o") + 1], a[a.index("-m") + 1]
    i = nib.load(nat)
    v = i.get_fdata().astype(np.float32)
    nib.save(nib.Nifti1Image(np.where(v > 100, v, 0).astype(np.float32), i.affine), out)
    nib.save(nib.Nifti1Image((v > 100).astype(np.uint8), i.affine), m)
else:
    sys.exit(2)
'''


def standins(tmp: Path) -> Path:
    b = tmp / "bin"
    b.mkdir()
    for t in ("dcm2niix", "mri_synthmorph", "mri_synthseg", "mri_synthstrip"):
        p = b / t
        p.write_text(STANDIN.replace("{python}", sys.executable))
        p.chmod(p.stat().st_mode | stat.S_IEXEC)
    return b


def setup_run(tmp: Path, intercept: float = 6.0):
    src = tmp / "src"
    stacks = []

    def stack(sid, series, *, base="T1w", bp="brain", sealed=False, n=12, datasets=("cohort-a",), t="10:00:00", sn=1):
        fl = []
        for i in range(n):
            p = src / f"s{sid}" / f"{i:03d}.dcm"
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(b"not read by the stand-in")
            fl.append({"source": 0, "path": f"s{sid}/{i:03d}.dcm", "frames": None, "geometry": {"instance_number": i + 1}})
        stacks.append({"unit": f"stack-{sid}", "stack_id": sid, "series_id": series, "modality": "MR", "files": fl,
                       "geometry": {"image_orientation_patient": [1, 0, 0, 0, 1, 0], "slice_thickness": 2.0, "study": 1, "sealed": sealed},
                       "header": {"fingerprint": {"manufacturer": "SIEMENS", "slice_thickness": 2.0, "modality": "MR"},
                                  "classification": {"base": [{"value": base}], "technique": [{"value": "MPRAGE"}], "body_part": [{"value": bp}],
                                                     "disposition": [{"value": "acquisition"}], "directory_type": [{"value": "anat"}]},
                                  "datasets": list(datasets)},
                       "contrast": {"contrast_bolus_agent": None, "acquisition_time": t, "series_number": sn, "mr_acquisition_type": "3D",
                                    "derived": False, "post_contrast_rules": []}})

    stack(1, 11, n=12)
    stack(2, 12, n=14, t="10:20:00", sn=5)
    stack(3, 13, base="T2w", t="10:10:00", sn=3)
    stack(4, 14, sealed=True, t="10:12:00", sn=4)
    stack(5, 15, datasets=("cohort-c",), t="10:16:00", sn=7)
    (tmp / "stacks.json").write_text(json.dumps({"contract": "job/v1", "sources": [{"id": 0, "mount": str(src)}], "stacks": stacks}))
    I = tmp / "inputs"
    for d in ("fusion", "arms", "cascade"):
        (I / d).mkdir(parents=True, exist_ok=True)
    coef = [0.0] * 20
    coef[COLS.index("zV")] = 1.0
    coef[COLS.index("missV")] = -3.0
    fdoc = {"cols": COLS, "scaler": {"mean": [0.0] * 20, "scale": [1.0] * 20}, "head": {"coef": coef, "intercept": intercept},
            "calibration": {"cohort-a Siemens": [1.0, 0.0]}, "t": 0.9, "use_V": True, "source_sha256": "x"}
    (I / "fusion" / "fusion.json").write_text(json.dumps(fdoc))
    nii = tmp / "atlas.nii.gz"
    nib.save(nib.Nifti1Image(np.zeros((4, 4, 4), np.int16), np.eye(4)), str(nii))
    raw = nii.read_bytes()
    at = lambda names: {"nifti_gz": base64.b64encode(raw).decode(), "sha256": hashlib.sha256(raw).hexdigest(),  # noqa: E731
                        "names": {k: {"label": i + 1} for i, k in enumerate(names)}}
    tree = {"left": [1, -1, -1], "right": [2, -1, -1], "feature": [4, -2, -2], "threshold": [0.001, -2.0, -2.0], "p1": [0.5, 0.1, 0.9]}
    adoc = {"u_source": "HP", "source_sha256": "x", "T": {"kind": "forest", "cols": ["T:" + k for k in features.T_KEYS], "trees": [tree]},
            "U": {"kind": "logistic", "cols": ["HP:pituitary:med_wm"], "fill": [1.0], "mean": [1.0], "scale": [1.0], "coef": [0.5], "intercept": 0.0},
            "V": {"kind": "logistic", "cols": ["V:wm_vox", "V:sss_med_wm"], "fill": [5.0, 1.0], "mean": [5.0, 1.0], "scale": [1.0, 1.0],
                  "coef": [0.0, 0.1], "intercept": 4.0},
            "atlas": {"HP": at(HP_NAMES), "VX": at(VX_NAMES)}}
    (I / "arms" / "arms.json").write_text(json.dumps(adoc))
    sha = lambda p: "sha256:" + hashlib.sha256(p.read_bytes()).hexdigest()  # noqa: E731
    fdg, adg = sha(I / "fusion" / "fusion.json"), sha(I / "arms" / "arms.json")
    (I / "cascade" / "cascade.json").write_text(json.dumps({"name": "c1-fs3-m", "stage1": {"rule": "margin", "m": 4, "k": 3},
                                                            "parts": {"student": "sha256:" + "0" * 64, "fusion": fdg}}))
    ms = [{"input": "fusion", "digest": fdg, "artifact": "fusion/fusion.json", "card": {"threshold": 0.9}},
          {"input": "arms", "digest": adg, "artifact": "arms/arms.json"},
          {"input": "cascade", "digest": sha(I / "cascade" / "cascade.json"), "artifact": "cascade/cascade.json",
           "card": {"threshold": 0.9, "parts": {"fusion": fdg, "arms": adg}}}]
    (I / "manifest.json").write_text(json.dumps({"models": ms}))
    return tmp / "stacks.json", I


@pytest.fixture
def standin_env(tmp_path, monkeypatch):
    b = standins(tmp_path)
    monkeypatch.setenv("PATH", f"{b}{os.pathsep}{os.environ['PATH']}")
    monkeypatch.setenv("STANDIN_TESTS", str(Path(__file__).parent))
    monkeypatch.setenv("NILS_PC_FS_HOME", str(tmp_path / "fs"))
    monkeypatch.setenv("CUDA_VISIBLE_DEVICES", "")
    return tmp_path


def run(tmp, *extra):
    stacks, I = setup_run(tmp)
    out = tmp / "out"
    rc = cli.main(["full", "--stacks", str(stacks), "--inputs", str(I), "--output", str(out), "--threads", "2", "--convert-workers", "2",
                   "--device", "cpu", "--work", str(tmp / "work"), "--records", str(tmp / "records.jsonl"), *extra])
    return rc, out


def tables(out):
    return {int(p.stem): json.loads(p.read_text()) for p in (out / "postcontrast").glob("*.json")}


def test_the_full_pipeline_answers_candidates_and_gives_every_other_stack_its_reason(standin_env, monkeypatch):
    monkeypatch.setenv("STANDIN_FAIL_ONCE", "2")  # stack 2's first conversion fails; the retry converts it
    rc, out = run(standin_env)
    assert rc == 0
    T = tables(out)
    assert set(T) == {1, 2, 3, 4, 5}
    for s in (1, 2):
        assert T[s]["answers"] == 1 and T[s]["value"] == "given" and T[s]["stage"] == "full", T[s]
        assert "V" in T[s]["arms"] and "U" in T[s]["arms"] and T[s]["failed_steps"] is None, T[s]
    assert T[3]["reason"] == "not-t1-brain" and T[4]["reason"] == "sealed" and T[5]["reason"] == "no-calibration"
    res = json.loads((out / "results.json").read_text())
    props = {p["stack_id"]: p for p in res["proposals"]}
    assert set(props) == {1, 2} and props[1]["model_digest"] == T[1]["cascade_digest"]
    assert res["metrics"]["candidates"] == 2 and res["metrics"]["by_outcome"]["answered"] == 2
    rec = {json.loads(line)["stack_id"]: json.loads(line) for line in (standin_env / "records.jsonl").read_text().splitlines()}
    assert rec[2]["convert"]["attempts"] == 2 and rec[1]["convert"]["attempts"] == 1
    assert all(k.startswith(("HP", "V", "T:")) for k in rec[1]["cols"])


def test_a_tool_the_kernel_kills_does_not_end_the_run(standin_env, monkeypatch):
    monkeypatch.setenv("STANDIN_SYNTHSEG_DIES", "1")  # SynthSeg ends as a killed process (137), every time
    rc, out = run(standin_env)
    assert rc == 0
    T = tables(out)
    # without a segmentation the stack keeps the arms it has, as model r1
    for s in (1, 2):
        assert T[s]["stage"] == "full" and T[s]["failed_steps"], T[s]
        assert "V" not in (T[s]["arms"] or "")


def test_the_parts_must_belong_together(standin_env):
    stacks, I = setup_run(standin_env)
    m = json.loads((I / "manifest.json").read_text())
    m["models"][2]["card"]["parts"]["arms"] = "sha256:" + "1" * 64
    (I / "manifest.json").write_text(json.dumps(m))
    out = standin_env / "out"
    rc = cli.main(["full", "--stacks", str(stacks), "--inputs", str(I), "--output", str(out), "--device", "cpu"])
    assert rc == 1 and "arms" in json.loads((out / "results.json").read_text())["error"]


def test_a_lane_gives_a_dead_workers_task_again_and_then_gives_up(tmp_path):
    lane = full.Lane(1, (tools.Env(fs_home=str(tmp_path)), "cpu", ""), tries=2)  # one worker: the dying task runs first
    lane.submit("dies", functools.partial(os._exit, 3))
    lane.submit("lives", abs, -4)
    got, dead = {}, []
    for _ in range(200):
        out, d = lane.done(0.5)
        got.update(dict(out))
        dead += d
        if not lane.pending:
            break
    lane.shutdown()
    assert dead == ["dies"] and got["lives"] == 4 and lane.rebuilt >= 2
