# SPDX-License-Identifier: AGPL-3.0-only
"""Body part's cascade (record 55 E2, P1b): the few-slice student's input
from the manifest alone, the stages, the reasons, the scores table and the
slice cache, on synthetic stacks.

The research's answers are reproduced where they are rules: the slot-0
file, the session's top, the stack's tokens, Postgres's rounding of the
positions, the stage's features and D3's 62. The student runs as a fixed
formula here (``cascade_data.stand_in_student``), so these tests need no
torch; ``test_student.py`` tests the network itself. With
``NILS_BODYPART_FROZEN`` naming the frozen set's folder, one more test runs
the registered parts (never part of the repository) end to end.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
from decimal import Decimal
from pathlib import Path

import numpy as np
import pytest

pytest.importorskip("lightgbm")
pytest.importorskip("safetensors")

from nils_bodypart import cascade, cli, fewslice, fusion, manifest, slicecache  # noqa: E402

from . import cascade_data as cd  # noqa: E402
from . import fusion_data as fd  # noqa: E402


@pytest.fixture(scope="module")
def world(tmp_path_factory) -> dict:
    root = tmp_path_factory.mktemp("cascade")
    return {"root": root, **cd.write_stacks(root)}


def stacks(world) -> list[manifest.Stack]:
    return manifest.load(world["stacks"], source_root=world["source_root"])


def rows(world) -> dict[int, dict]:
    sts = stacks(world)
    out = [fewslice.row_of(s) for s in sts]
    fewslice.session_tops(out)
    return {g["id"]: g for g in out}


def inputs(world, tmp_path, **kw) -> dict:
    base = fd.write_inputs(tmp_path)
    return cd.add_cascade(base, **kw)


class StandIn:
    """The student as a formula, where cli loads ``student.Student``."""

    def __init__(self, path, device="cpu", threads=None):
        self.seeds = ["1", "2", "3"]

    def predict(self, tiles, sgeo, tokens, batch=64):
        return cd.stand_in_student(tiles, sgeo, tokens)


@pytest.fixture
def stand_in(monkeypatch):
    from nils_bodypart import student

    monkeypatch.setattr(student, "Student", StandIn)


def run(world, inp: dict, out: Path, *more: str) -> tuple[int, dict]:
    code = cli.main(["infer-fusion", "--stacks", str(world["stacks"]), "--source-root", str(world["source_root"]), "--inputs", str(inp["inputs"]),
                     "--output", str(out), "--device", "cpu", "--threads", "2", *more])
    return code, json.loads((out / "results.json").read_text())


def tables(out: Path) -> dict[int, dict]:
    return {int(p.stem): json.loads(p.read_text()) for p in (out / "bodypart-fusion").glob("*.json")}


# -------------------------------------------------------- the manifest's facts


def test_the_engines_stack_facts_read_in_the_readers_names(world):
    by = {s.stack_id: s for s in stacks(world)}
    sg = by[24].extra["geometry"]
    assert sg["study"] == 100 and sg["image_type"] == "ORIGINAL\\PRIMARY\\LOCALIZER" and sg["n_instances"] == 9 and sg["sealed"] is False
    assert sg["sop_class"] == cd.MR and sg["iop"] == fd.AXIAL and sg["modality"] == "MR"
    assert by[26].extra["geometry"]["sealed"] is True
    assert by[29].extra["geometry"]["sop_class"].endswith(".4.1")


# -------------------------------------------------------------- the slot-0 rule


def test_slot_zero_is_the_research_rule_per_kind(world):
    """The few-slice rule's first slot (lib.select): the inferior end of an
    axial stack whatever its numbers, the middle of a sagittal, coronal or
    thin one along the normal, a sixth of the way through a localiser by
    instance number; the inferior flag only where it is the inferior end."""
    g = rows(world)
    want = {21: 0, 22: 10, 23: 9, 24: 5, 25: 3, 31: 0}
    for sid, index in want.items():
        got = fewslice.slot0(g[sid])
        assert got is not None and got[0] == index, sid
    assert [g[s]["kind"] for s in (21, 22, 23, 24, 25)] == ["axial", "sagittal", "coronal", "localizer", "thin"]
    assert fewslice.slot0(g[21])[1][3] == 1.0 and fewslice.slot0(g[31])[1][3] == 1.0
    assert all(fewslice.slot0(g[s])[1][3] == 0.0 for s in (22, 23, 24, 25))
    # a multi-frame file is never the student's
    assert fewslice.slot0(g[29]) is None


def test_the_axial_inferior_end_follows_the_normal():
    """With the normal pointing down the lowest position along it is the
    highest slice: the rule still takes the lowest slice in z."""
    files = [{"ipp": [0.0, 0.0, 10.0 * k], "inum": k + 1, "index": k} for k in range(20)]
    up = {"iop": fd.AXIAL, "ps": [1.0, 1.0], "rows": 64, "cols": 64, "n_pos": 20}
    down = dict(up, iop=cd.AXIAL_DOWN)
    assert fewslice.select(up, files, 1)[0][0]["index"] == 0
    assert fewslice.select(down, files, 1)[0][0]["index"] == 0
    # without geometry, the middle file by instance number: round(9.5) is
    # 10 (to even), the eleventh by number, numbered 11, which is file 9
    nogeo = {"iop": None}
    assert fewslice.select(nogeo, [dict(f, inum=20 - f["index"]) for f in files], 1)[0][0]["index"] == 9


def test_a_localiser_is_found_by_protocol_name_and_image_type_only():
    """The research's quirk, kept: it read the series description under a
    name its rows never had, so only these two find a localiser."""
    base = {"iop": fd.AXIAL, "n_pos": 40}
    assert fewslice.stack_kind(dict(base, prot="3pl loc")) == "localizer"
    assert fewslice.stack_kind(dict(base, image_type="ORIGINAL\\PRIMARY\\LOCALIZER")) == "localizer"
    assert fewslice.stack_kind(dict(base, sdesc="localizer", desc=None, prot="t1 mprage")) == "axial"


# --------------------------------------------------------- the stack's registry row


def test_the_session_top_is_the_highest_unsealed_mr_box_of_the_study(world):
    g = rows(world)
    assert g[22]["z_hi_rel_top"] == 0.0  # the sagittal box reaches 130 mm
    assert (g[21]["z_hi_rel_top"], g[21]["z_lo_rel_top"]) == (-74.0, -190.0)
    assert g[23]["z_hi_rel_top"] == -2.0
    # the sealed stack and the CT stack stand higher and are no part of it
    assert g[26]["z_hi"] > 130 and "z_hi_rel_top" not in g[26]
    assert g[27]["z_hi"] > 130 and "z_hi_rel_top" not in g[27]
    # another study is its own session
    assert g[31]["z_hi_rel_top"] == 0.0


def test_the_stack_tokens_are_the_research_tokens(world):
    g = rows(world)
    t = fewslice.stack_tokens(g[21])
    assert len(t) == len(fewslice.TOKENS) == 27
    named = dict(zip(fewslice.TOKENS, t.tolist()))
    want = {"plane_axial": 1.0, "kind_axial": 1.0, "tilt": 0.0, "n_z": 1.0, "ext_x": 252 / 500, "ext_z": 116 / 500, "span_n": 116 / 500,
            "log_n_pos": math.log1p(30) / 6, "log_n_files": math.log1p(30) / 6, "fov_x": 256 / 500, "thick": 0.4, "field": 3 / 7,
            "z_lo_rel_top": -190 / 500, "z_hi_rel_top": -74 / 500, "enhanced": 0.0, "four_d": 0.0}
    for k, v in want.items():
        assert named[k] == pytest.approx(v, abs=1e-6), k
    assert sum(named[f"kind_{k}"] for k in fewslice.KINDS) == 1.0
    assert dict(zip(fewslice.TOKENS, fewslice.stack_tokens(g[29]).tolist()))["enhanced"] == 1.0


def test_positions_are_counted_as_postgres_rounds_them():
    """``count(DISTINCT round(d::numeric, 1))``: the float at 15 digits,
    half away from zero, where Python's round would halve to even."""
    assert fewslice._pg_round1(0.15) == Decimal("0.2") and round(0.15, 1) == 0.1
    assert fewslice._pg_round1(-0.25) == Decimal("-0.3")
    assert fewslice._pg_round1(2.45) == Decimal("2.5")
    st = manifest.Stack(stack_id=1, slices=[], unit="stack-1", files=[(f"/x/{i}", None) for i in range(4)],
                        file_geo=[{"ipp": [0.0, 0.0, z], "ps": [1.0, 1.0], "rows": 8, "cols": 8, "inum": i + 1, "frames": 1}
                                  for i, z in enumerate((0.15, 0.2, 0.24, 1.0))],
                        extra={"modality": "MR", "geometry": {"iop": fd.AXIAL, "n_instances": 4}})
    g = fewslice.row_of(st)
    assert g["n_pos"] == 2 and g["n_files"] == 4 and g["n_inst_reg"] == 4
    assert g["d_min"] == pytest.approx(0.15) and g["d_max"] == 1.0


def test_the_lowest_position_ties_on_the_lower_number_and_a_missing_number_last():
    files = [{"ipp": [1.0, 0.0, 0.0], "inum": None}, {"ipp": [2.0, 0.0, 0.0], "inum": 7}, {"ipp": [3.0, 0.0, 0.0], "inum": 3},
             {"ipp": [9.0, 0.0, 5.0], "inum": None}, {"ipp": [8.0, 0.0, 5.0], "inum": 9}]
    st = manifest.Stack(stack_id=1, slices=[], unit="stack-1", files=[(f"/x/{i}", None) for i in range(5)],
                        file_geo=[dict(f, ps=[1.0, 1.0], rows=8, cols=8, frames=1) for f in files], extra={"geometry": {"iop": fd.AXIAL}})
    g = fewslice.row_of(st)
    assert g["ipp_lo"] == [3.0, 0.0, 0.0] and g["ipp_hi"] == [8.0, 0.0, 5.0]


# ------------------------------------------------------ the stage and its deferral


def test_the_stage_table_and_d3s_62_features(tmp_path):
    inp = fd.write_inputs(tmp_path)
    model = fusion.load(inp["inputs"])
    rng = np.random.default_rng(1)
    P = rng.dirichlet(np.ones(6), (5, 3)).astype(np.float32)
    h44 = rng.normal(0, 1, (5, 44)).astype(np.float32)
    h44[0, 3] = np.nan
    table = cascade.stage_table(model, P, h44, ["alpha", "", "zeta", "", ""])
    assert len(table) == 5 and all(len(t["feats"]) == 14 for t in table)
    t0 = table[0]
    mean = fusion.calibrate(model, fusion.head_logp(model, P[0].mean(0), h44[0]), "alpha")
    assert t0["ans"]["fine"]["confidence"] == pytest.approx(mean["fine"]["confidence"], abs=1e-12)
    seeds = [fusion.calibrate(model, fusion.head_logp(model, P[0, s], h44[0]), "alpha") for s in range(3)]
    assert t0["feats"][:6] == pytest.approx([seeds[s][m]["confidence"] for s in range(3) for m in ("fine", "coarse")])
    assert t0["feats"][12] == float(all(cascade.eff(s[m]) == cascade.eff(mean[m]) for s in seeds for m in ("fine", "coarse")))
    assert t0["feats"][13] == pytest.approx(float(P[0].std(0).max()))
    x = cascade.d3_features(t0["feats"], h44[0], {"plane": "axial", "kind": "axial", "tilt": 3.5, "fp_slices": 40})
    assert len(x) == cascade.D3_FEATURES
    assert x[14 + 3] == -1.0  # a missing header feature
    assert x[-4:] == [0, 42, 3.5, 40.0]  # axial, sum(ord("axial")) % 97, the tilt, the slices
    assert cascade.d3_features(t0["feats"], h44[0], {})[-4:] == [3, 0, 0.0, 0.0]


def test_the_middle_stages_margin_is_in_log_odds_at_six_places():
    th = {"fine": 0.897, "coarse": 0.9152}
    a = {"fine": {"confidence": 0.95}, "coarse": {"confidence": 0.93}}
    lo = lambda p: math.log(p / (1 - p))  # noqa: E731
    assert cascade.margin(a, th) == pytest.approx(min(abs(lo(0.95) - lo(0.897)), abs(lo(0.93) - lo(0.9152))))
    assert cascade.margin({"fine": {"confidence": 0.8970004}, "coarse": {"confidence": 0.99}}, th) == 0.0


# ----------------------------------------------------------------- end to end


def test_every_stack_carries_a_region_or_a_reason(world, tmp_path, stand_in):
    inp = inputs(world, tmp_path, deferral="all")
    code, r = run(world, inp, tmp_path / "out")
    assert code == 0, r.get("error")
    t = tables(tmp_path / "out")
    assert sorted(t) == [21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31]
    status = {u["unit_id"]: u for u in r["units"]}
    # D3 agrees with every stack the student read: one file each
    for sid in (21, 22, 23, 24, 25, 31):
        assert (t[sid]["stage"], t[sid]["files_read"]) == ("s1", 1), sid
        assert t[sid]["deferral"] >= 0.99
        assert status[f"stack-{sid}"]["status"] == "succeeded"
    # the multi-frame file goes to the planes
    assert t[29]["stage"] in ("b24", "touched") and t[29]["files_read"] == 1
    reasons = {26: "sealed", 27: "not-mr", 28: "no-geometry", 30: "unreadable"}
    for sid, why in reasons.items():
        assert t[sid]["reason"] == why and t[sid]["stage"] is None and t[sid]["fine_value"] is None, sid
    assert (status["stack-26"]["status"], status["stack-30"]["status"]) == ("skipped", "failed")
    for sid, d in t.items():
        answered = d.get("coarse_answers") == 1
        assert answered != bool(d["reason"]), sid
        assert d["reason"] in (None, *cascade.REASONS)
        assert (d["cascade_digest"], d["student_digest"], d["deferral_digest"]) == (inp["cascade"], inp["student"], inp["deferral"])
        assert (d["head_digest"], d["coarse_digest"], d["encoder_digest"]) == (inp["head"], inp["coarse"], inp["encoder"])
    # each body_part decision names the cascade, each body_region one the
    # cascade's own coarse part; no proposal for a reason
    props = r["proposals"]
    assert {p["stack_id"] for p in props} == {21, 22, 23, 24, 25, 29, 31}
    for p in props:
        assert p["model_digest"] == (inp["cascade"] if p["axis"] == "body_part" else inp["coarse"])
    # results.json and its proposals are the contract's, and so is the cascade's card
    jsonschema = pytest.importorskip("jsonschema")
    from .test_contract import MODEL, schema, validator

    validator("results.schema.json").validate(r)
    jsonschema.validate(r["proposals"], schema("proposals.schema.json"))
    card = json.loads((inp["inputs"] / "manifest.json").read_text())["models"][-1]["card"]
    validator("card.schema.json", MODEL).validate(card)
    m = r["metrics"]
    assert m["stages"]["s1"] == 6 and sum(m["stages"].values()) == 7
    assert m["reasons"]["sealed"] == 1 and m["reasons"]["not-mr"] == 1 and m["reasons"]["unreadable"] == 1
    assert r["params"]["models"]["cascade"]["digest"] == inp["cascade"]


def test_where_d3_defers_the_certified_reader_answers_as_it_does_alone(world, tmp_path, stand_in):
    """The planes stages are the certified path: with D3 deferring every
    stack and a margin no answer reaches, each stack's answer is the
    touched reader's run alone, number for number; with margin 0 it is the
    24-frame reader's."""
    for m3, policy in ((1e9, "touched"), (0.0, "b24")):
        inp = inputs(world, tmp_path / f"m{m3}", deferral="none", m3=m3)
        code, _ = run(world, inp, tmp_path / f"cas-{policy}")
        assert code == 0
        code, _ = run(world, inp, tmp_path / f"alone-{policy}", "--cascade", "off", "--reader", f"reduced:{policy}")
        assert code == 0
        a, b = tables(tmp_path / f"cas-{policy}"), tables(tmp_path / f"alone-{policy}")
        scored = [s for s in a if a[s]["stage"] is not None]
        assert sorted(scored) == [21, 22, 23, 24, 25, 29, 31]
        for sid in scored:
            assert a[sid]["stage"] == policy, sid
            for k in ("fine", "coarse", "image", "calibration", "geometry"):
                assert a[sid][k] == b[sid][k], (sid, k)


def test_without_the_engines_study_the_student_reads_nothing(world, tmp_path):
    """A manifest of an engine before P1b names no study: no stack has a
    session to be measured against, so the planes answer every one."""
    doc = json.loads(world["stacks"].read_text())
    for st in doc["stacks"]:
        st["geometry"].pop("study")
    old = tmp_path / "old.json"
    old.write_text(json.dumps(doc))
    inp = inputs(world, tmp_path, deferral="all")
    model = fusion.load(inp["inputs"])
    cas = cascade.load(inp["inputs"], model)
    outs = cascade.score(model, cas, inp["inputs"], manifest.load(old, source_root=world["source_root"]), workers=1,
                         student_factory=cd.stand_in_student)
    assert all(o.d3 is None for o in outs)
    assert {o.result.stage for o in outs if o.result is not None} <= {"b24", "touched"}


def test_the_cascade_passes_gpu_workers_on(world, tmp_path, stand_in, monkeypatch):
    """--gpu-workers reaches the planes stages' pool. With a card and 0, the
    student alone takes the card and every frame is decoded on the CPU, where
    the cascade used to leave the pool at scoring's default of 2."""
    from nils_bodypart import gpu, scoring

    seen = []
    real = scoring.score

    def spy(*args, **kw):
        seen.append((kw.get("device"), kw.get("gpu_workers")))
        return real(*args, **{**kw, "device": "cpu"})  # no card in a test

    monkeypatch.setattr(gpu, "cuda_available", lambda: True)
    monkeypatch.setattr(scoring, "score", spy)
    inp = inputs(world, tmp_path, deferral="all")
    code, r = run(world, inp, tmp_path / "out", "--device", "cuda", "--gpu-workers", "0")
    assert code == 0, r.get("error")
    assert seen and set(seen) == {("cuda", 0)}
    assert (r["params"]["gpu_workers"], r["params"]["cascade"]["student_device"]) == (0, "cuda")


def test_d3_decides_at_its_threshold(world, tmp_path):
    inp = inputs(world, tmp_path, deferral="mixed")
    model = fusion.load(inp["inputs"])
    cas = cascade.load(inp["inputs"], model)
    outs = cascade.score(model, cas, inp["inputs"], stacks(world), workers=1, student_factory=cd.stand_in_student)
    seen = [o for o in outs if o.d3 is not None]
    assert len(seen) == 6
    for o in seen:
        assert (o.result.stage == "s1") == (o.d3 >= cas.d3_threshold), o.stack.stack_id


def test_a_cascade_that_does_not_name_the_runs_parts_is_refused(world, tmp_path):
    cases = {
        "student": {"name_parts": {"student": "sha256:" + "0" * 64}},
        "threshold": {"thresholds": {"fine": 0.9, "coarse": 0.6}},
    }
    for name, kw in cases.items():
        inp = inputs(world, tmp_path / name, **kw)
        code, r = run(world, inp, tmp_path / f"out-{name}")
        assert code == 1 and r["error"], name
    # a cascade without its deferral model
    inp = inputs(world, tmp_path / "no-deferral")
    mf = json.loads((inp["inputs"] / "manifest.json").read_text())
    mf["models"] = [m for m in mf["models"] if m["input"] != "deferral"]
    (inp["inputs"] / "manifest.json").write_text(json.dumps(mf))
    code, r = run(world, inp, tmp_path / "out-no-deferral")
    assert code == 1 and "deferral" in r["error"]
    # --cascade on with none given
    code, r = run(world, fd.write_inputs(tmp_path / "plain"), tmp_path / "out-plain", "--cascade", "on")
    assert code == 1 and "cascade" in r["error"]


def test_a_coarse_mode_naming_the_cascade_is_refused(world, tmp_path):
    """A coarse mode names the head it sums. One naming the cascade (the
    coarse twin ruling R1 (a) described) can never be the part the cascade's
    file names, since each would hold the other's digest, so it is refused;
    the cascade answers body_region with its own coarse part."""
    inp = inputs(world, tmp_path)
    root = inp["inputs"]
    mf = json.loads((root / "manifest.json").read_text())
    entry = next(m for m in mf["models"] if m["input"] == "coarse")
    path = root / "coarse" / Path(entry["artifact"]).name
    mode = json.loads(path.read_text())
    mode["head"] = {"name": "bodypart-cascade", "version": "test", "digest": inp["cascade"]}
    path.write_text(json.dumps(mode))
    entry["digest"] = entry["card"]["digest"] = fd.digest(path)
    (root / "manifest.json").write_text(json.dumps(mf))
    with pytest.raises(fusion.ModelError, match="another head"):
        fusion.load(root)


# ------------------------------------------------------------- the slice cache


def test_the_cache_keeps_each_input_by_its_files_identity(world, tmp_path, stand_in):
    inp = inputs(world, tmp_path, deferral="mixed")
    cache = tmp_path / "slice-cache"
    code, r = run(world, inp, tmp_path / "out", "--cache", str(cache), "--cache-fill", "touched")
    assert code == 0, r.get("error")
    t = tables(tmp_path / "out")
    status = {u["unit_id"]: u for u in r["units"]}
    by = {s.stack_id: s for s in stacks(world)}
    # each file a stack read whole reports its sha256 by its place in the manifest (R3)
    for sid in (21, 31):
        read = status[f"stack-{sid}"]["read"]
        assert read and len(read) >= t[sid]["files_read"]  # and the touched planes filled for the cache
        for x in read:
            path = by[sid].files[x["file"]][0]
            assert x["sha256"] == "sha256:" + hashlib.sha256(Path(path).read_bytes()).hexdigest()
    # the student's slices, and the touched planes of every scored stack (filled at arrival)
    index, arrays = slicecache.read(cache, slicecache.SLICE, cd.study_uid(100))
    assert len(index) == 5  # stacks 21 to 25; 30's slice did not decode
    for key, rec in index.items():
        assert arrays[f"{key}.tile"].shape == (192, 192) and arrays[f"{key}.tile"].dtype == np.uint8
        assert key == slicecache.entry_key(slicecache.SLICE, [tuple(f) for f in rec["files"]])
        assert rec["made_by"]["cascade"] == inp["cascade"]
    touched, ta = slicecache.read(cache, slicecache.PLANES["touched"], cd.study_uid(100))
    assert len(touched) == 6  # 21 to 25 and 29
    for key, rec in touched.items():
        assert ta[f"{key}.planes"].shape == (6, 64, 64) and ta[f"{key}.geo"].shape == (15,) and ta[f"{key}.header44"].shape == (44,)
        assert rec["encoder"] == inp["encoder"]
    assert len(slicecache.read(cache, slicecache.SLICE, cd.study_uid(200))[0]) == 1
    names = [p.name for p in cache.rglob("*.npz")]
    assert all(cd.study_uid(100) not in n and cd.study_uid(200) not in n for n in names)
    # a second run merges into the same entries
    code, _ = run(world, inp, tmp_path / "out2", "--cache", str(cache), "--cache-fill", "touched")
    assert code == 0
    assert slicecache.read(cache, slicecache.SLICE, cd.study_uid(100))[0].keys() == index.keys()
    assert slicecache.read(cache, slicecache.PLANES["touched"], cd.study_uid(100))[0].keys() == touched.keys()
    # the touched planes are the encoder's input: the encoder gives the stored probabilities from them
    model = fusion.load(inp["inputs"])
    for key in touched:
        v = np.zeros((64, 64, 64), np.uint8)
        p = ta[f"{key}.planes"]
        v[31], v[32], v[:, 31], v[:, 32], v[:, :, 31], v[:, :, 32] = p
        assert np.allclose(model.encoder.probabilities(v, ta[f"{key}.geo"]), ta[f"{key}.image"], atol=1e-6)


# ------------------------------------------------------- the frozen set (opt in)


@pytest.mark.skipif(not os.environ.get("NILS_BODYPART_FROZEN"), reason="NILS_BODYPART_FROZEN names no frozen set (site weights stay out of the repository)")
def test_the_frozen_cascade_runs_end_to_end(world, tmp_path):
    pytest.importorskip("torch")
    pytest.importorskip("timm")
    root = cd.frozen_inputs(Path(os.environ["NILS_BODYPART_FROZEN"]), tmp_path / "inputs")
    code, r = run(world, {"inputs": root}, tmp_path / "out")
    assert code == 0, r.get("error")
    t = tables(tmp_path / "out")
    cas = json.loads((root / "cascade" / "bodypart-cascade-r7.json").read_text())
    for d in t.values():
        assert d["reason"] or d.get("coarse_answers") == 1
        assert (d["student_digest"], d["deferral_digest"]) == (cas["parts"]["student"], cas["parts"]["deferral"])
    assert {p["model_digest"] for p in r["proposals"] if p["axis"] == "body_part"} == {fd.digest(root / "cascade" / "bodypart-cascade-r7.json")}
