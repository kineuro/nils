# SPDX-License-Identifier: AGPL-3.0-only
"""Each entry point on a three-stack synthetic manifest, on the CPU, with
the stand-in encoders: embed, embed again (all cached), seed, train, infer."""

from __future__ import annotations

import json
from pathlib import Path

from nils_bodypart import cli
from nils_bodypart import embeddings as emb

from .test_formats import write_set

PROPOSAL_KEYS = {"stack_id", "axis", "value", "probabilities", "model_id", "model_digest", "note"}


def run(*args) -> dict:
    assert cli.main([str(a) for a in args]) == 0
    out = Path(args[args.index("--output") + 1])
    return json.loads((out / "results.json").read_text())


def test_the_four_entry_points(synthetic):
    root, stacks, src = synthetic["root"], synthetic["stacks"], synthetic["source_root"]
    common = ("--standin", "--stacks", stacks, "--source-root", src)

    r = run("embed", *common, "--output", root / "e1", "--batch", 4, "--threads", 2)
    assert r["counts"] == {"succeeded": 3, "failed": 0, "skipped": 0, "total": 3}
    assert {u["unit_id"] for u in r["units"]} == {"stack-101", "stack-102", "stack-103"}
    assert len(r["models"]) == 2 and all(m["kind"] == "encoder" for m in r["models"])
    assert "proposals" not in r
    outs = [o for u in r["units"] for o in u["outputs"]]
    assert len(outs) == 6 and all(o["kind"] == "embedding" and o["sha256"].startswith("sha256:") for o in outs)
    assert all(u["derivatives"] == [o["path"] for o in u["outputs"]] for u in r["units"])
    axial = emb.decode((root / "e1" / "embeddings" / "biomedclip" / "101.emb").read_bytes())
    assert axial.slices == [2, 3, 4, 5, 6, 8] and axial.dim == 512
    siglip = emb.decode((root / "e1" / "embeddings" / "siglip2" / "103.emb").read_bytes())
    assert siglip.slices == [3, 4, 5] and siglip.dim == 768

    again = run("embed", *common, "--embeddings", root / "e1", "--output", root / "e2")
    assert again["counts"]["skipped"] == 3 and again["metrics"]["rows_written"] == 0

    s = run("seed", *common, "--embeddings", root / "e1", "--output", root / "s", "--n-target", 4)
    assert s["counts"]["succeeded"] == 3 and "proposals" not in s
    assert s["seeds"]
    assert all(p["axis"] == "body_part" and p["value"] in ("brain", "brain-neck", "spine", "chest") for p in s["seeds"])
    prior = [p for p in s["seeds"] if p["source"] == "keyword_prior"]
    assert {(p["stack_id"], p["value"]) for p in prior} == {(101, "brain"), (103, "spine")}
    for sid in s["selection"]["stacks"]:
        assert json.loads((root / "s" / "seeds" / f"{sid}.json").read_text())["stack_id"] == sid

    write_set(root / "labels", [(101, "brain"), (102, "brain"), (103, "spine")])
    t = run("train", *common, "--embeddings", root / "e1", "--labels", root / "labels", "--output", root / "t", "--min-per-class", 1)
    card = t["models"][0]
    assert card["kind"] == "head" and card["task"] == "axis:body_part" and card["artifact"]["format"] == "json"
    assert card["trained_on"]["label_set"].startswith("sha256:") and card["pack_version"] == "mri@0.4.0"
    assert json.loads((root / "t" / "head" / "card.json").read_text())["digest"] == card["digest"]
    assert t["head"]["artifact"] == "head/head.json"

    i = run("infer", *common, "--embeddings", root / "e1", "--head", root / "t" / "head", "--output", root / "i")
    assert i["counts"]["succeeded"] == 3
    assert len(i["proposals"]) == 3
    for p in i["proposals"]:
        assert set(p) <= PROPOSAL_KEYS
        assert p["model_digest"] == card["digest"]
        assert abs(sum(p["probabilities"].values()) - 1.0) < 0.01
        assert p["value"] in ("brain", "spine")
        assert json.loads((root / "i" / "bodypart" / f"{p['stack_id']}.json").read_text())["value"] == p["value"]


def test_a_run_that_cannot_be_done_says_why(synthetic, tmp_path):
    write_set(tmp_path / "labels", [(101, "brain")])
    code = cli.main(
        ["train", "--standin", "--stacks", str(synthetic["stacks"]), "--source-root", str(synthetic["source_root"]),
         "--labels", str(tmp_path / "labels"), "--output", str(tmp_path / "t")]
    )
    assert code == 1
    r = json.loads((tmp_path / "t" / "results.json").read_text())
    assert "two classes" in r["error"]


def test_an_unreadable_stack_fails_alone(synthetic, tmp_path):
    doc = json.loads(synthetic["stacks"].read_text())
    doc["stacks"].append({"unit": "stack-104", "stack_id": 104, "files": [{"source": 0, "path": "nowhere/x.dcm"}]})
    m = tmp_path / "m.json"
    m.write_text(json.dumps(doc))
    r = run("embed", "--standin", "--stacks", m, "--source-root", synthetic["source_root"], "--output", tmp_path / "e")
    by = {u["unit_id"]: u for u in r["units"]}
    assert by["stack-104"]["status"] == "failed" and "nowhere" not in by["stack-104"]["error"]
    assert r["counts"]["succeeded"] == 3
