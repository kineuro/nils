# SPDX-License-Identifier: AGPL-3.0-only
"""The certified model's entry point (record 50) on synthetic stacks and
synthetic models: the volume, the header features, the encoder, the head,
the calibration in both modes, the outputs and the refusals."""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import numpy as np
import pytest

pytest.importorskip("lightgbm")
pytest.importorskip("safetensors")

from nils_bodypart import cli, fusion, header_features, manifest, tiny, volume  # noqa: E402

from . import fusion_data as fd  # noqa: E402

FINE_COLUMNS = [f"fine_{fusion.column(v)}" for v in fusion.FINE]
COARSE_COLUMNS = [f"coarse_{v}" for v in fusion.COARSE]


@pytest.fixture(scope="module")
def world(tmp_path_factory) -> dict:
    root = tmp_path_factory.mktemp("fusion")
    s = fd.write_stacks(root)
    i = fd.write_inputs(root)
    return {"root": root, **s, **i}


def run(world, out: Path, inputs: Path | None = None) -> tuple[int, dict]:
    code = cli.main(
        ["infer-fusion", "--stacks", str(world["stacks"]), "--source-root", str(world["source_root"]),
         "--inputs", str(inputs or world["inputs"]), "--output", str(out), "--threads", "2"]
    )
    return code, json.loads((out / "results.json").read_text())


def stack(world, sid: int) -> manifest.Stack:
    return next(s for s in manifest.load(world["stacks"], source_root=world["source_root"]) if s.stack_id == sid)


# ------------------------------------------------------------------ volume


def test_the_volume_keeps_96_frames_one_per_position(world):
    st = stack(world, 11)
    b = volume.build(st.files)
    assert b.vol.shape == (64, 64, 64) and b.vol.dtype == np.uint8 and b.vol.max() > 0
    # 111 files, two at one position: 110 positions, of which training's
    # file list keeps 96 evenly, v0's seven slices among them
    assert b.meta["n_unique"] == 96 and b.meta["K"] == 96 and b.meta["n_frames"] == 96
    assert (b.meta["nx"], b.meta["ny"], b.meta["nz"], b.meta["oblique_deg"]) == (0.0, 0.0, 1.0, 0.0)
    assert b.meta["fov_r"] == 256.0 and b.meta["thick"] == 1.2 and b.meta["modality"] == "MR"
    g = b.geo()
    assert g.dtype == np.float32 and g.shape == (15,) and g[8] == 96 and g[14] == 0.0


def test_the_files_read_are_the_ones_training_listed():
    """select_v1.py's file list, which the model's training and round 7's
    test read: of 200 positions, 96 evenly and v0's slices beside them (the
    centre three, and for an axial stack five at 20 to 80 %)."""

    def frames(iop):
        return [volume.Frame(f"{k:03d}.dcm", 0, (64, 64), [0.0, 0.0, float(k)] if iop == AX else [float(k), 0.0, 0.0], iop, [1.0, 1.0], 1.0, (k + 1) * 10000, "MR", k + 1) for k in range(200)]

    AX, SAG = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0], [0.0, 1.0, 0.0, 0.0, 0.0, -1.0]
    even = {f"{int(round(x)):03d}.dcm" for x in np.linspace(0, 199, 96)}
    axial = volume.preselect(frames(AX), "Axial")
    assert len(axial) == 100 and axial - even == {"070.dcm", "099.dcm", "100.dcm", "160.dcm"}
    sagittal = volume.preselect(frames(SAG), "sagittal")
    assert len(sagittal) == 98 and sagittal - even == {"099.dcm", "100.dcm"}
    # a file with no position sorts at 0 before the file there, which is
    # then a second at its position; a file at a position already kept is
    # still listed where it is one of v0's slices (the 6th and 7th of 12)
    fs = frames(AX)[:10]
    fs.append(volume.Frame("again.dcm", 0, (64, 64), [0.0, 0.0, 4.0], AX, [1.0, 1.0], 1.0, 0, "MR", 99))
    fs.append(volume.Frame("nopos.dcm", 0, (64, 64), None, AX, [1.0, 1.0], 1.0, 0, "MR", 0))
    assert volume.preselect(fs, "sagittal") == {f"{k:03d}.dcm" for k in range(1, 10)} | {"nopos.dcm", "again.dcm"}
    assert volume.v0_indices(0, "axial") == [] and volume.v0_indices(1, "axial") == [0]


def test_a_large_frame_is_shrunk_and_a_multi_frame_file_read_by_its_listed_frames(world):
    sag = volume.build(stack(world, 12).files)
    assert (sag.meta["H"], sag.meta["W"]) == (256, 239) and sag.meta["nx"] == 1.0
    cor = volume.build(stack(world, 13).files)
    assert cor.meta["n_frames"] == 20 and cor.meta["ny"] == 1.0
    every = volume.build([(p, None) for p, _ in stack(world, 13).files])
    assert every.meta["n_frames"] == 24


def test_no_geometry_and_no_file_are_told_apart(world, tmp_path):
    with pytest.raises(volume.NoGeometry):
        volume.build(stack(world, 14).files)
    with pytest.raises(volume.Unreadable):
        volume.build([(str(tmp_path / "missing.dcm"), None)])


# --------------------------------------------------------- header features


def test_the_44_header_features():
    fp = fd.fingerprint("sagittal", fov_x=200.0, fov_y=300.0, aspect_ratio=1.5, field_strength_normalized=None, magnetic_field_strength=1.5,
                        text_series_description_ci="t2_sag halsrygg", text_body_part_ci=None, receive_coil_name="Spine_12", spacing_between_slices=None)
    meta = {"ext_x": 40.0, "ext_y": 250.0, "ext_z": 260.0, "oblique_deg": 3.5, "modality": "MR"}
    cls = {"body_part": [{"value": "spine", "confidence": 0.8, "tier": "keywords"}]}
    x = header_features.features(fp, cls, meta)
    f = dict(zip(header_features.NAMES, x.tolist()))
    assert len(x) == 44 and x.dtype == np.float32
    assert f["field_T"] == 1.5 and np.isnan(f["spacing_mm"]) and f["cover_si_mm"] == 260.0
    assert (f["ori_sag"], f["ori_ax"], f["ori_other"], f["portrait_sag"]) == (1, 0, 0, 1)
    assert (f["coil_spine"], f["coil_head"], f["coil_known"]) == (1, 0, 1)
    # halsrygg is the cervical spine: a spine word, not a neck one
    assert (f["own_spine"], f["own_neck"]) == (1, 0)
    assert f["stated_brain"] == 0 and f["rules_spine"] == 1 and f["rules_none"] == 0 and f["rules_conf"] == pytest.approx(0.8)
    none = dict(zip(header_features.NAMES, header_features.features(fp, {}, meta).tolist()))
    assert none["rules_none"] == 1 and none["rules_conf"] == 0.0
    assert header_features.flags("loc_3plane") == [0, 0, 0, 0, 0, 1]


# ------------------------------------------------------------- calibration


def test_the_cohort_is_the_first_the_calibration_names(world):
    m = fusion.load(world["inputs"])
    # gamma has no temperature of its own; alpha has
    # the batch first, as round 7 fitted the temperatures by it (p0-<cohort>)
    assert fusion.resolve_cohort({"cohorts": ["gamma", "alpha"], "batch": "import-2026-zeta"}, m) == ("zeta", "batch_part")
    assert fusion.resolve_cohort({"cohorts": ["alpha"], "batch": "p0ext-zeta"}, m) == ("zeta", "batch_part")
    # then the subject's cohorts, where the batch names none
    assert fusion.resolve_cohort({"cohorts": ["gamma", "alpha"], "batch": "import-2026"}, m) == ("alpha", "cohort")
    assert fusion.resolve_cohort({"cohorts": [], "batch": "zeta"}, m) == ("zeta", "batch")
    assert fusion.resolve_cohort({"cohorts": ["x"], "batch": "site-2025-beta"}, m) == ("beta", "batch_part")
    assert fusion.resolve_cohort({"cohorts": None, "batch": None}, m) == (None, None)
    LP = np.log(np.array([0.5, 0.1, 0.1, 0.1, 0.1, 0.1]))
    a = fusion.calibrate(m, LP, "beta")  # beta: coarse only, so fine takes the global temperature
    assert a["fine"]["temperature"] == 1.25 and a["coarse"]["temperature"] == 1.4
    assert sum(a["fine"]["probabilities"].values()) == pytest.approx(1.0) and sum(a["coarse"]["probabilities"].values()) == pytest.approx(1.0)
    # the coarse probabilities are the fine ones at the global fine temperature, summed per region, at the coarse temperature
    pf = tiny.softmax_T(LP[None, :], 1.25)[0]
    lpc = np.log([pf[0] + pf[1], pf[2] + pf[3], pf[4], pf[5]])
    assert list(a["coarse"]["probabilities"].values()) == pytest.approx(tiny.softmax_T(lpc[None, :], 1.4)[0].tolist())
    r = fusion.rounded({"a": 0.33333, "b": 0.33333, "c": 0.33334}, 2)
    assert abs(sum(r.values()) - 1) < 0.01
    # a proposal is staged by the engine at or above the card's threshold,
    # so an abstaining mode's value never rounds up to it
    below = {"probabilities": {"a": 0.8969996, "b": 0.1030004}, "value": "a", "confidence": 0.8969996, "answers": False}
    assert fusion.proposed(below)["a"] < 0.897 and fusion.rounded(below["probabilities"])["a"] == 0.897
    at = {"probabilities": {"a": 0.897, "b": 0.103}, "value": "a", "confidence": 0.897, "answers": True}
    assert fusion.proposed(at) == {"a": 0.897, "b": 0.103}


# -------------------------------------------------------------- end to end


def test_the_entry_point_scores_every_stack_it_can(world):
    out = world["root"] / "out"
    code, r = run(world, out)
    assert code == 0
    assert r["counts"] == {"succeeded": 3, "failed": 0, "skipped": 2, "total": 5}
    status = {u["unit_id"]: u for u in r["units"]}
    assert "geometry" in status["stack-14"]["error"] and status["stack-14"]["status"] == "skipped"
    assert "fingerprint" in status["stack-15"]["error"] and status["stack-15"]["status"] == "skipped"
    assert r["models"] == []
    mode = world["mode"]
    for sid in (11, 12, 13):
        doc = json.loads((out / "bodypart-fusion" / f"{sid}.json").read_text())
        assert status[f"stack-{sid}"]["derivatives"] == [f"bodypart-fusion/{sid}.json"]
        assert status[f"stack-{sid}"]["outputs"][0]["kind"] == "table"
        assert doc["stack_id"] == sid
        for mode_name, cols, threshold in (("fine", FINE_COLUMNS, 0.5), ("coarse", COARSE_COLUMNS, max(0.55, mode["threshold"]))):
            probs = [doc[c] for c in cols]
            assert sum(probs) == pytest.approx(1.0, abs=1e-5)
            assert doc[f"{mode_name}_confidence"] == pytest.approx(max(probs))
            assert doc[f"{mode_name}_value"] == doc[mode_name]["value"]
            assert doc[f"{mode_name}_answers"] == int(doc[mode_name]["confidence"] >= threshold)
            assert doc[mode_name]["threshold"] == threshold
            m = status[f"stack-{sid}"]["metrics"]
            assert m[f"{mode_name}_value"] == doc[f"{mode_name}_value"] and m[f"{mode_name}_answers"] == bool(doc[f"{mode_name}_answers"])
        assert (doc["head_digest"], doc["coarse_digest"], doc["encoder_digest"]) == (world["head"], world["coarse"], world["encoder"])
        assert doc["models"]["head"]["digest"] == world["head"] and doc["models"]["coarse"]["name"] == "bodypart-fusion-coarse"
        assert sum(doc["image"].values()) == pytest.approx(1.0, abs=1e-5)
        assert doc["geometry"]["K"] > 1
    d11 = json.loads((out / "bodypart-fusion" / "11.json").read_text())
    assert d11["calibration"] == {"cohort": "alpha", "matched_from": "cohort"}
    assert d11["fine"]["temperature"] == 0.7 and d11["coarse"]["temperature"] == 0.9
    assert d11["rules"]["value"] == "brain"
    d13 = json.loads((out / "bodypart-fusion" / "13.json").read_text())
    assert d13["calibration"] == {"cohort": None, "matched_from": None} and d13["fine"]["temperature"] == 1.25 and d13["coarse"]["temperature"] == 1.1
    # a proposal per stack and axis, by the right model, also below the threshold
    props = r["proposals"]
    assert sorted((p["stack_id"], p["axis"]) for p in props) == sorted((s, a) for s in (11, 12, 13) for a in ("body_part", "body_region"))
    for p in props:
        assert p["model_digest"] == (world["head"] if p["axis"] == "body_part" else world["coarse"])
        assert abs(sum(p["probabilities"].values()) - 1) < 0.01
        assert set(p["probabilities"]) == set(fusion.FINE if p["axis"] == "body_part" else fusion.COARSE)
        assert p["value"] == max(p["probabilities"], key=p["probabilities"].get)
        d = json.loads((out / "bodypart-fusion" / f"{p['stack_id']}.json").read_text())[p["axis"] == "body_part" and "fine" or "coarse"]
        assert (p["probabilities"][p["value"]] >= d["threshold"]) == d["answers"]
    assert r["metrics"]["no_fingerprint"] == 1 and r["metrics"]["no_geometry"] == 1


def test_without_a_coarse_model_only_fine_mode_runs(world, tmp_path):
    inputs = tmp_path / "inputs"
    shutil.copytree(world["inputs"], inputs)
    doc = json.loads((inputs / "manifest.json").read_text())
    doc["models"] = [m for m in doc["models"] if m["input"] != "coarse"]
    (inputs / "manifest.json").write_text(json.dumps(doc))
    code, r = run(world, tmp_path / "out", inputs)
    assert code == 0 and r["counts"]["succeeded"] == 3
    assert {p["axis"] for p in r["proposals"]} == {"body_part"}
    doc = json.loads((tmp_path / "out" / "bodypart-fusion" / "11.json").read_text())
    assert "coarse" not in doc and "coarse_value" not in doc and "coarse_digest" not in doc and "fine_value" in doc


def _tampered(world, tmp_path, change) -> dict:
    inputs = tmp_path / "inputs"
    shutil.copytree(world["inputs"], inputs)
    doc = json.loads((inputs / "manifest.json").read_text())
    change(doc, inputs)
    (inputs / "manifest.json").write_text(json.dumps(doc))
    code, r = run(world, tmp_path / "out", inputs)
    assert code == 1 and not r["units"] and "proposals" not in r
    return r


def _model(doc, name):
    return next(m for m in doc["models"] if m["input"] == name)


def test_a_digest_that_does_not_match_refuses_the_run(world, tmp_path):
    def change(doc, _):
        _model(doc, "head")["digest"] = "sha256:" + "0" * 64

    assert "digest" in _tampered(world, tmp_path, change)["error"]


def test_an_artifact_changed_after_registration_refuses_the_run(world, tmp_path):
    def change(doc, inputs):
        p = inputs / "coarse" / Path(_model(doc, "coarse")["artifact"]).name
        p.write_text(p.read_text() + " ")

    assert "coarse" in _tampered(world, tmp_path, change)["error"]


def test_a_head_that_names_another_encoder_refuses_the_run(world, tmp_path):
    def change(doc, _):
        _model(doc, "head")["card"]["encoders"] = [{"digest": "sha256:" + "1" * 64}]

    assert "encoder" in _tampered(world, tmp_path, change)["error"]


def test_a_mode_file_of_another_head_refuses_the_run(world, tmp_path):
    def change(doc, inputs):
        m = _model(doc, "coarse")
        p = inputs / "coarse" / Path(m["artifact"]).name
        mode = json.loads(p.read_text())
        mode["head"]["digest"] = "sha256:" + "2" * 64
        p.write_text(json.dumps(mode))
        m["digest"] = m["card"]["digest"] = fd.digest(p)

    assert "head" in _tampered(world, tmp_path, change)["error"]


def test_an_encoder_of_other_classes_refuses_the_run(world, tmp_path):
    def change(doc, inputs):
        m = _model(doc, "encoder")
        p = inputs / "encoder" / Path(m["artifact"]).name
        fd.write_encoder(p, classes=["brain", "spine", "neck", "brain-neck", "chest", "other"])
        m["digest"] = m["card"]["digest"] = fd.digest(p)
        for h in doc["models"]:
            if h["input"] in ("head", "coarse"):
                h["card"]["encoders"] = [{"digest": m["digest"]}]

    assert "classes" in _tampered(world, tmp_path, change)["error"]


# --------------------------------------------------------- numpy and torch


def test_the_numpy_encoder_is_torch_s_tiny2d(world):
    """The encoder in numpy against the same network in torch, from one state
    dict; skipped where torch is not installed (CI has none)."""
    torch = pytest.importorskip("torch")
    nn = torch.nn

    def block(i, o):
        return nn.Sequential(nn.Conv2d(i, o, 3, padding=1, bias=False), nn.BatchNorm2d(o), nn.ReLU(inplace=True), nn.MaxPool2d(2))

    class Head(nn.Module):
        def __init__(self):
            super().__init__()
            self.head = nn.Sequential(nn.Linear(64 + 15, 64), nn.ReLU(), nn.Dropout(0.2), nn.Linear(64, 6))

    class Net(nn.Module):
        def __init__(self):
            super().__init__()
            ch = (3, 16, 32, 48, 64)
            self.f = nn.Sequential(*[block(ch[i], ch[i + 1]) for i in range(4)])
            self.h = Head()

        def forward(self, x, g):
            return self.h.head(torch.cat([self.f(x).mean((2, 3)), g], 1))

    rng = np.random.default_rng(11)
    sd = fd.state_dict(rng)
    net = Net()
    net.load_state_dict({k: torch.from_numpy(np.array(v)) for k, v in sd.items()})
    net.eval()
    seed = tiny.Seed({k: v for k, v in sd.items() if not k.endswith("num_batches_tracked")}, rng.uniform(0, 3, 15).astype(np.float32), rng.uniform(0.5, 2, 15).astype(np.float32), 1.3)
    vol = volume.build(stack(world, 12).files)
    for v, g in ((vol.vol, vol.geo()), (rng.integers(0, 256, (64, 64, 64)).astype(np.uint8), rng.uniform(0, 300, 15).astype(np.float32))):
        x = tiny.planes(v.astype(np.float32) / np.float32(255.0))
        G = (tiny.geo_feats(g) - seed.geo_mean) / seed.geo_sd
        with torch.no_grad():
            want = net(torch.from_numpy(x[None]), torch.from_numpy(G[None].astype(np.float32))).numpy()[0]
        assert np.abs(seed.logits(x, g) - want).max() < 1e-4
