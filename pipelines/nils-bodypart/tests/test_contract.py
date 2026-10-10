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
ENTRIES = ("bodypart-embed", "bodypart-seed", "bodypart-train", "bodypart-infer", "bodypart-infer-fusion")


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


def entrypoint() -> list[str]:
    """The image's ENTRYPOINT in its exec form, or none."""
    words: list[str] = []
    for line in (HERE / "Dockerfile").read_text().splitlines():
        if line.startswith("ENTRYPOINT"):
            words = json.loads(line.split(None, 1)[1])
    return words


def container_argv(doc: dict) -> list[str]:
    """What the container runs for a descriptor, as the engine writes it:
    the image's entry point, then the command line with the engine's
    folders in place and every parameter at its default (one without a
    default left out)."""
    import shlex

    line = doc["command-line"]
    for key, at in {"[Manifest]": "/input/stacks.json", "[Inputs]": "/inputs", "[OutputLocation]": "/output"}.items():
        line = line.replace(key, at)
    for p in doc.get("inputs", []):
        given = p.get("default-value")
        word = "" if given is None else f"{p.get('command-line-flag', '')} {given}".strip()
        line = line.replace(p["value-key"], word)
    return entrypoint() + shlex.split(line)


@pytest.mark.parametrize("entry", ENTRIES)
def test_each_command_line_runs_against_the_image_s_entry_point(entry):
    """Wave 43's proof: the image's ENTRYPOINT and a command line that
    named the program too ran ``nils-bodypart nils-bodypart embed``, which
    argparse refused. The program is named once, and what follows it is a
    command this package's parser takes."""
    yaml = pytest.importorskip("yaml")
    doc = yaml.safe_load((HERE / entry / "nils.job.yml").read_text())
    argv = container_argv(doc)
    assert argv[0] == "nils-bodypart" and argv.count("nils-bodypart") == 1, argv
    parsed = cli.parser().parse_args(argv[1:])
    assert parsed.entry == entry.split("-", 1)[1]


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


def test_the_train_and_infer_descriptors_expose_what_a_small_set_and_a_pickle_need():
    """Wave 43's proof: train could not be given a PCA size, and infer could
    not be told to trust a pickled head whose card checks."""
    yaml = pytest.importorskip("yaml")
    train = yaml.safe_load((HERE / "bodypart-train" / "nils.job.yml").read_text())
    params = {p["id"]: p for p in train["inputs"]}
    assert params["pca_components"]["command-line-flag"] == "--pca-components"
    assert params["pca_components"]["value-key"] in train["command-line"]
    assert params["auto_tune"]["command-line-flag"] == "--auto-tune"
    a = cli.parser().parse_args(container_argv(train)[1:] + ["--pca-components", "32", "--auto-tune", "false"])
    assert (a.pca_components, a.auto_tune) == (32, False)
    infer = yaml.safe_load((HERE / "bodypart-infer" / "nils.job.yml").read_text())
    params = {p["id"]: p for p in infer["inputs"]}
    assert params["allow_pickle"]["default-value"] == "false"
    assert params["allow_pickle"]["value-key"] in infer["command-line"]
    assert cli.parser().parse_args(container_argv(infer)[1:]).allow_pickle is False


def test_the_descriptors_pin_one_published_image_and_real_encoder_weights():
    """A catalog takes a descriptor as it is: all five name the same image
    by its registry manifest digest, and the embeddings name the encoders'
    weights as the image reports them (``python -m nils_bodypart.bake
    verify``), never the placeholders the descriptors started with. A
    release that publishes a new image repins them."""
    import re

    yaml = pytest.importorskip("yaml")
    docs = {e: yaml.safe_load((HERE / e / "nils.job.yml").read_text()) for e in ENTRIES}
    images = {doc["container-image"]["image"] for doc in docs.values()}
    assert len(images) == 1, images
    (image,) = images
    m = re.fullmatch(r"ghcr\.io/kineuro/nils-bodypart@(sha256:[0-9a-f]{64})", image)
    assert m, image
    placeholder = re.compile(r"sha256:0{62}[0-9a-f]{2}")
    assert not placeholder.fullmatch(m.group(1)), image
    encoders = [
        d for o in docs["bodypart-embed"]["x-nils"]["outputs"] for d in o.get("encoders", [])
    ]
    assert len(encoders) == 2 and len(set(encoders)) == 2, encoders
    for d in encoders:
        assert re.fullmatch(r"sha256:[0-9a-f]{64}", d) and not placeholder.fullmatch(d), d


def test_the_fusion_descriptor_declares_its_models_table_and_axes():
    """Record 50: the certified model's descriptor opts in to the stacks'
    headers, takes the encoder, the head and the optional coarse mode file
    as models, declares the table the ask reads and both proposal axes."""
    yaml = pytest.importorskip("yaml")
    doc = yaml.safe_load((HERE / "bodypart-infer-fusion" / "nils.job.yml").read_text())
    x = doc["x-nils"]
    assert x["input"] == {"layout": "stacks", "header": True, "geometry": True}
    assert [(t["id"], t["type"], t.get("optional", False)) for t in x["inputs"]] == [
        ("encoder", "model", False), ("head", "model", False), ("coarse", "model", True)
    ]
    assert {p["axis"] for p in x["proposals"]} == {"body_part", "body_region"}
    assert x["needs"] == {"gpu": "none", "memory-gb": 8, "cores": 16, "cores-input": "threads"}
    (table,) = x["outputs"]
    assert (table["kind"], table["format"], table["path-template"]) == ("table", "json", "bodypart-fusion/{stack}.json")
    cols = {c["name"]: c.get("type", "number") for c in table["columns"]}
    fine = [f"fine_{v}" for v in ("brain", "brain_neck", "neck", "spine", "chest", "other")]
    coarse = [f"coarse_{v}" for v in ("head", "spine", "chest", "other")]
    assert list(cols) == fine + ["fine_value", "fine_confidence", "fine_answers"] + coarse + [
        "coarse_value", "coarse_confidence", "coarse_answers", "head_digest", "coarse_digest", "encoder_digest"
    ]
    assert all(cols[c] == "number" for c in fine + coarse + ["fine_confidence", "coarse_confidence"])
    assert cols["fine_answers"] == cols["coarse_answers"] == "integer"
    assert all(cols[c] == "text" for c in ("fine_value", "coarse_value", "head_digest", "coarse_digest", "encoder_digest"))
    assert doc["tool-version"] == "0.4.0"
    # the image offline: the command names no host, and the entry point
    # parses what the engine writes: since 0.4.0 the reduced reader, from
    # each file's geometry, four stacks ahead
    a = cli.parser().parse_args(container_argv(doc)[1:])
    assert (a.entry, str(a.inputs), a.threads) == ("infer-fusion", "/inputs", 16)
    assert (a.reader, a.readahead) == ("reduced:touched", 4)
    reader = next(p for p in doc["inputs"] if p["id"] == "reader")
    assert set(reader["value-choices"]) == {"reduced:touched", "full"}


def test_the_fusion_results_and_its_tables_are_the_contracts(tmp_path):
    """results.json and its proposals validate, and each stack's table holds
    every declared column by its own name (the engine folds a key and
    compares it with the column's name)."""
    pytest.importorskip("lightgbm")
    pytest.importorskip("safetensors")
    jsonschema = pytest.importorskip("jsonschema")
    yaml = pytest.importorskip("yaml")
    from . import fusion_data as fd

    s = fd.write_stacks(tmp_path)
    i = fd.write_inputs(tmp_path)
    out = tmp_path / "out"
    assert cli.main(["infer-fusion", "--stacks", str(s["stacks"]), "--source-root", str(s["source_root"]), "--inputs", str(i["inputs"]), "--output", str(out)]) == 0
    r = json.loads((out / "results.json").read_text())
    validator("results.schema.json").validate(r)
    jsonschema.validate(r["proposals"], schema("proposals.schema.json"))
    card = validator("card.schema.json", MODEL)
    for c in i["cards"].values():
        card.validate(c)
    doc = yaml.safe_load((HERE / "bodypart-infer-fusion" / "nils.job.yml").read_text())
    declared = {c["name"]: c.get("type", "number") for c in doc["x-nils"]["outputs"][0]["columns"]}
    import re

    fold = lambda k: re.sub(r"[^a-z0-9]+", "_", k.lower()).strip("_")  # noqa: E731
    tables = sorted((out / "bodypart-fusion").glob("*.json"))
    assert len(tables) == 3
    for t in tables:
        row = json.loads(t.read_text())
        assert isinstance(row, dict)
        found = {fold(k): v for k, v in row.items()}
        for name, ty in declared.items():
            v = found[name]
            if ty == "text":
                assert isinstance(v, str) and v
            elif ty == "integer":
                assert v in (0, 1) and not isinstance(v, bool)
            else:
                assert isinstance(v, float) and 0 <= v <= 1
