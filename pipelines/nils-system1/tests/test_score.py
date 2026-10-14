# SPDX-License-Identifier: AGPL-3.0-only
"""System 1 as a pipeline (record 55 D3): the bundle is checked before it
is read, the opinions are what the contract says, the fields that mattered
are measured by leaving each out, and a stack of a sample sealed now gets
no opinion."""

from __future__ import annotations

import json
import tarfile
from pathlib import Path

import pytest

from nils_system1 import cli
from nils_system1.bundle import BundleError, make_bundle, open_bundle

HERE = Path(__file__).resolve().parent.parent
CONTRACT = HERE.parent.parent / "contracts" / "job" / "v1"
DESCRIPTOR = HERE / "system1-score" / "nils.job.yml"

AXES = ["provenance", "technique", "modifier", "construct", "base"]

KEYWORDS = {
    "rules": [
        {"axis": "base", "field": "text_series_description", "contains": "flair", "value": "FLAIR", "p": 0.96},
        {"axis": "base", "field": "text_series_description", "contains": "t1", "value": "T1w", "p": 0.93},
        {"axis": "technique", "field": "text_sequence_name", "contains": "tse", "value": "TSE", "p": 0.9},
        {"axis": "provenance", "field": "image_type", "contains": "original", "value": "RawRecon", "p": 0.97},
    ],
    "default_p": 0.4,
    "reliability": {"missing_fields": ["text_series_description", "text_protocol_name"], "base": 0.99,
                    "per_missing": 0.3},
}


def bundle(tmp: Path, predictor=None, extra: dict | None = None) -> Path:
    folder = tmp / "bundle"
    folder.mkdir(exist_ok=True)
    (folder / "keywords.json").write_text(json.dumps(KEYWORDS))
    for name, text in (extra or {}).items():
        (folder / name).write_text(text)
    out = tmp / "system1.tar"
    make_bundle(out, folder, {"name": "kw", "version": "1", "axes": AXES,
                              "predictor": predictor or {"kind": "keywords", "file": "keywords.json"}})
    return out


def manifest(tmp: Path, sealed: int | None = None) -> Path:
    def fp(sid, desc, proto, seq, it):
        return {"stack_id": sid, "series_id": sid * 10, "study_id": 1, "subject_id": 1,
                "text_series_description": desc, "text_protocol_name": proto, "text_sequence_name": seq,
                "image_type": it, "echo_time": 90.0, "repetition_time": 9000.0}
    rows = {
        "votes": ["axis", "rule_set", "rule", "clause", "value", "tier", "restates"],
        "stacks": {
            "1": {"scored": True, "fingerprint": fp(1, "AX FLAIR", "flair", "*tse2d1", "ORIGINAL\\PRIMARY"), "votes": []},
            "2": {"scored": True, "fingerprint": fp(2, "t1_mprage", "t1", "*tfl3d1", "ORIGINAL\\PRIMARY"), "votes": []},
            "3": {"scored": True, "fingerprint": fp(3, None, None, None, "DERIVED"), "votes": []},
            "4": {"scored": False, "fingerprint": fp(4, "localizer", None, None, None), "votes": []},
        },
        "series": {"10": {"frame_of_reference": "ab" * 32, "private": None}},
    }
    stacks = [{"unit": f"stack-{i}", "stack_id": i, "files": [], "header": {"fingerprint": None, "classification": {}, "batch": None, "cohorts": []}}
              for i in (1, 2, 3)]
    if sealed:
        stacks[sealed - 1]["geometry"] = {"sealed": True}
    doc = {"contract": "job/v1", "sources": [], "stacks": stacks, "rows": rows}
    p = tmp / "stacks.json"
    p.write_text(json.dumps(doc))
    return p


def inputs(tmp: Path, archive: Path) -> Path:
    d = tmp / "inputs"
    d.mkdir(exist_ok=True)
    (d / "manifest.json").write_text(json.dumps({"models": [{"input": "model", "model_id": 7, "artifact": str(archive)}]}))
    return d


def test_a_run_writes_an_opinion_per_stack_and_axis_with_the_fields_that_mattered(tmp_path):
    archive = bundle(tmp_path)
    out = tmp_path / "out"
    assert cli.main(["score", "--stacks", str(manifest(tmp_path)), "--inputs", str(inputs(tmp_path, archive)),
                     "--output", str(out)]) == 0
    res = json.loads((out / "results.json").read_text())
    ops = {(o["stack_id"], o["axis"]): o for o in res["opinions"]}
    assert len(ops) == 3 * len(AXES)
    assert ops[(1, "base")]["value"] == "FLAIR" and ops[(1, "base")]["p"] == pytest.approx(0.96)
    assert ops[(2, "base")]["value"] == "T1w"
    assert ops[(3, "base")]["value"] is None
    # the rules cannot be trusted on a header stripped of its names
    assert ops[(1, "base")]["reliability"] == pytest.approx(0.99)
    assert ops[(3, "base")]["reliability"] == pytest.approx(0.39)
    # the series description is what mattered for base, measured by leaving it out
    top = ops[(1, "base")]["fields"][0]
    assert top["field"] == "text_series_description" and top["effect"] == pytest.approx(0.96)
    assert all(o["model_id"] == 7 for o in res["opinions"])
    assert {u["unit_id"] for u in res["units"]} == {"stack-1", "stack-2", "stack-3"}
    assert json.loads((out / "system1" / "1.json").read_text())["axes"]["base"]["value"] == "FLAIR"


def test_a_sealed_stack_gets_no_opinion(tmp_path):
    archive = bundle(tmp_path)
    out = tmp_path / "out"
    assert cli.main(["score", "--stacks", str(manifest(tmp_path, sealed=2)), "--inputs",
                     str(inputs(tmp_path, archive)), "--output", str(out), "--explain", "none"]) == 0
    res = json.loads((out / "results.json").read_text())
    assert not any(o["stack_id"] == 2 for o in res["opinions"])
    unit = next(u for u in res["units"] if u["unit_id"] == "stack-2")
    assert unit["status"] == "skipped"


def test_a_bundle_is_checked_before_it_is_read(tmp_path):
    archive = bundle(tmp_path)
    # a member changed after the manifest named it is refused
    bad = tmp_path / "bad.tar"
    with tarfile.open(archive) as src, tarfile.open(bad, "w") as dst:
        for m in src.getmembers():
            data = src.extractfile(m).read()
            if m.name == "keywords.json":
                data = data.replace(b"FLAIR", b"T2w  ")
            import io
            m.size = len(data)
            dst.addfile(m, io.BytesIO(data))
    with pytest.raises(BundleError, match="names"):
        open_bundle(bad, tmp_path / "x")
    # code runs only when the run trusts it
    (tmp_path / "c").mkdir()
    code = bundle(tmp_path / "c",
                  predictor={"kind": "module", "file": "predict.py", "entry": "load"},
                  extra={"predict.py": "def load(folder, manifest):\n    raise SystemExit('ran')\n"})
    out = tmp_path / "out"
    assert cli.main(["score", "--stacks", str(manifest(tmp_path)), "--inputs", str(inputs(tmp_path, code)),
                     "--output", str(out)]) == 2


def test_a_module_predictor_runs_when_trusted(tmp_path):
    code = (
        "class P:\n"
        "    axes = ['base']\n"
        "    def predict(self, records, rows):\n"
        "        assert rows['series']\n"
        "        return [{'axes': {'base': {'value': 'PDw', 'p': 0.8, 'probabilities': {'PDw': 0.8, 'T2w': 0.2}}},"
        " 'reliability': 0.9} for r in records]\n"
        "def load(folder, manifest):\n"
        "    return P()\n"
    )
    archive = bundle(tmp_path, predictor={"kind": "module", "file": "predict.py", "entry": "load"},
                     extra={"predict.py": code})
    out = tmp_path / "out"
    assert cli.main(["score", "--stacks", str(manifest(tmp_path)), "--inputs", str(inputs(tmp_path, archive)),
                     "--output", str(out), "--trust-code", "true", "--explain", "none"]) == 0
    res = json.loads((out / "results.json").read_text())
    base = [o for o in res["opinions"] if o["axis"] == "base"]
    assert len(base) == 3 and base[0]["probabilities"]["T2w"] == pytest.approx(0.2)


def test_the_results_and_the_descriptor_hold_to_the_contract(tmp_path):
    jsonschema = pytest.importorskip("jsonschema")
    referencing = pytest.importorskip("referencing")
    yaml = pytest.importorskip("yaml")
    if not CONTRACT.is_dir():
        pytest.skip("the contracts are not in this tree")
    registry = referencing.Registry()
    for p in sorted(CONTRACT.glob("*.schema.json")):
        doc = json.loads(p.read_text())
        registry = registry.with_resource(doc["$id"], referencing.Resource.from_contents(doc))
    archive = bundle(tmp_path)
    out = tmp_path / "out"
    cli.main(["score", "--stacks", str(manifest(tmp_path)), "--inputs", str(inputs(tmp_path, archive)),
              "--output", str(out)])
    res = json.loads((out / "results.json").read_text())
    v = jsonschema.Draft202012Validator(json.loads((CONTRACT / "results.schema.json").read_text()), registry=registry)
    v.validate(res)
    d = jsonschema.Draft202012Validator(json.loads((CONTRACT / "nils.job.schema.json").read_text()), registry=registry)
    d.validate(yaml.safe_load(DESCRIPTOR.read_text()))
    s = jsonschema.Draft202012Validator(json.loads((CONTRACT / "stacks.schema.json").read_text()), registry=registry)
    s.validate(json.loads(manifest(tmp_path).read_text()))
