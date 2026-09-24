# SPDX-License-Identifier: AGPL-3.0-only
"""The descriptors and the results against the job contract's schemas
(``contracts/job/v1``), where the repository has them."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from nils_bodypart import cli

from .test_formats import write_set

HERE = Path(__file__).resolve().parent.parent
CONTRACT = HERE.parent.parent / "contracts" / "job" / "v1"
MODEL = HERE.parent.parent / "contracts" / "model" / "v1"
ENTRIES = ("bodypart-embed", "bodypart-seed", "bodypart-train", "bodypart-infer")


def schema(name: str, where: Path = CONTRACT) -> dict:
    p = where / name
    if not p.is_file():
        pytest.skip(f"{p.relative_to(HERE.parent.parent)} is not in this tree")
    return json.loads(p.read_text())


def validator(name: str, where: Path = CONTRACT):
    """A validator of one schema that resolves the contract's other
    documents from this tree, never from the network."""
    jsonschema = pytest.importorskip("jsonschema")
    referencing = pytest.importorskip("referencing")
    registry = referencing.Registry()
    for p in sorted(where.glob("*.schema.json")):
        doc = json.loads(p.read_text())
        registry = registry.with_resource(doc["$id"], referencing.Resource.from_contents(doc))
    return jsonschema.Draft202012Validator(schema(name, where), registry=registry)


@pytest.mark.parametrize("entry", ENTRIES)
def test_each_descriptor_is_the_contracts(entry):
    jsonschema = pytest.importorskip("jsonschema")
    yaml = pytest.importorskip("yaml")
    doc = yaml.safe_load((HERE / entry / "nils.job.yml").read_text())
    jsonschema.validate(doc, schema("nils.job.schema.json"))
    assert doc["name"] == entry
    assert doc["x-nils"]["input"]["layout"] == "stacks" and doc["x-nils"]["analysis-level"] == "stack"
    # Every value-key the command line uses is a parameter's or the engine's.
    own = {p["value-key"] for p in doc.get("inputs", [])}
    engine = {"[Manifest]", "[Inputs]", "[OutputLocation]", "[InputDataset]"}
    import re

    used = set(re.findall(r"\[[A-Za-z0-9_]+\]", doc["command-line"]))
    assert used <= own | engine, used - own - engine
    assert doc["command-line"].split()[:2] == ["nils-bodypart", entry.split("-")[1]]


def test_the_results_and_proposals_are_the_contracts(synthetic):
    jsonschema = pytest.importorskip("jsonschema")
    results = schema("results.schema.json")
    root, stacks, src = synthetic["root"], synthetic["stacks"], synthetic["source_root"]
    common = ["--standin", "--stacks", str(stacks), "--source-root", str(src)]
    assert cli.main(["embed", *common, "--output", str(root / "e")]) == 0
    write_set(root / "labels", [(101, "brain"), (102, "brain"), (103, "spine")])
    assert cli.main(["train", *common, "--embeddings", str(root / "e"), "--labels", str(root / "labels"), "--output", str(root / "t"), "--min-per-class", "1"]) == 0
    assert cli.main(["infer", *common, "--embeddings", str(root / "e"), "--head", str(root / "t" / "head"), "--output", str(root / "i")]) == 0
    assert results["properties"]["proposals"]["$ref"] == "proposals.schema.json"
    check = validator("results.schema.json")
    assert cli.main(["seed", *common, "--embeddings", str(root / "e"), "--output", str(root / "s"), "--n-target", "2"]) == 0
    for out in ("e", "s", "t", "i"):
        doc = json.loads((root / out / "results.json").read_text())
        check.validate(doc)
    doc = json.loads((root / "i" / "results.json").read_text())
    jsonschema.validate(doc["proposals"], schema("proposals.schema.json"))
    # the seeds are apart from the proposals, each with its value and margin
    seeded = json.loads((root / "s" / "results.json").read_text())
    assert seeded["seeds"] and "proposals" not in seeded
    assert all({"stack_id", "axis", "value", "margin"} <= set(s) for s in seeded["seeds"])
    # every card the runs carry is a model card: the encoders an embed
    # used, and the head a train fitted with its encoders and threshold
    card = validator("card.schema.json", MODEL)
    for out in ("e", "t"):
        for m in json.loads((root / out / "results.json").read_text())["models"]:
            card.validate(m)
    head = json.loads((root / "t" / "results.json").read_text())["models"][0]
    assert [e["digest"] for e in head["encoders"]] == [m["digest"] for m in json.loads((root / "e" / "results.json").read_text())["models"]]
    assert head["threshold"] == 0.7
