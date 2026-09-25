# SPDX-License-Identifier: AGPL-3.0-only
"""The embedding file, the manifest and the label set."""

from __future__ import annotations

import hashlib
import json
import struct
from pathlib import Path

import numpy as np
import pytest

from nils_bodypart import embeddings as emb
from nils_bodypart import labels, manifest

D = "sha256:" + "ab" * 32


def test_an_embedding_is_the_engines_bytes():
    e = emb.Embedding(12, D, "bodypart-v1", [4, 5, 6], np.arange(6, dtype=np.float32).reshape(3, 2))
    data = emb.encode(e)
    assert data[:8] == b"NILSEMB1"
    (n,) = struct.unpack("<I", data[8:12])
    header = json.loads(data[12 : 12 + n])
    assert list(header) == sorted(header)
    assert header == {
        "dim": 2,
        "dtype": "<f4",
        "encoder": D,
        "format": "nils-embedding",
        "preprocess_version": "bodypart-v1",
        "rows": 3,
        "slices": [4, 5, 6],
        "stack_id": 12,
    }
    start = len(data) - 3 * 2 * 4
    assert start % 64 == 0 and not any(data[12 + n : start])
    back = emb.decode(data)
    assert back.slices == [4, 5, 6] and np.array_equal(back.matrix, e.matrix)


def test_a_broken_embedding_is_refused():
    e = emb.Embedding(1, D, "v", [0], np.ones((1, 3), dtype=np.float32))
    data = bytearray(emb.encode(e))
    with pytest.raises(ValueError):
        emb.decode(bytes(data[:-1]))
    (n,) = struct.unpack("<I", data[8:12])
    assert (12 + n) % 64 != 0  # there is padding to spoil
    data[12 + n] = 1
    with pytest.raises(ValueError):
        emb.decode(bytes(data))
    with pytest.raises(ValueError):
        emb.encode(emb.Embedding(1, "not-a-digest", "v", [0], np.ones((1, 3), dtype=np.float32)))
    with pytest.raises(ValueError):
        emb.encode(emb.Embedding(1, D, "v", [0, 0], np.ones((2, 3), dtype=np.float32)))


def test_the_store_finds_embeddings_anywhere_and_joins_slices(tmp_path):
    a = emb.Embedding(7, D, "v", [1, 2], np.ones((2, 3), dtype=np.float32))
    b = emb.Embedding(7, D, "v", [3], np.zeros((1, 3), dtype=np.float32))
    emb.write(tmp_path / "x" / "7.emb", a)
    emb.write(tmp_path / "y" / "anything", b)
    (tmp_path / "noise.txt").write_text("hello")
    (tmp_path / "bad.emb").write_bytes(b"NILSEMB1garbage")
    s = emb.Store.scan(tmp_path)
    got = s.get(D, "v", 7)
    assert got.slices == [1, 2, 3] and s.unreadable == 1
    assert s.get(D, "other", 7) is None


def test_a_stack_s_slice_count_spares_reading_a_header():
    doc = {
        "sources": [{"id": 0, "mount": "/source/0"}],
        "stacks": [{"unit": "stack-5", "stack_id": 5, "slices": 7, "orientation": "axial",
                    "body_part": None, "technique": "TSE", "files": [{"source": 0, "path": "a/mf.dcm", "frames": None}]}],
    }
    asked = []
    st = manifest.parse(doc, frames_of=lambda p: asked.append(p) or 1)
    assert st[0].num_slices == 7 and not asked
    assert "slices" not in st[0].extra
    doc["stacks"][0].pop("slices")
    st = manifest.parse(doc, frames_of=lambda p: 4)
    assert st[0].num_slices == 4


def test_the_manifest_expands_frames_in_order():
    counted = []

    def frames_of(path):
        counted.append(path)
        return 3

    doc = {
        "contract": "job/v1",
        "sources": [{"id": 0, "mount": "/source/0"}, {"id": 1, "mount": "/source/1"}],
        "stacks": [
            {
                "unit": "stack-1",
                "stack_id": 1,
                "files": [{"source": 0, "path": "a/1.dcm", "frames": None}, {"source": 1, "path": "a/mf.dcm", "frames": "3-5,1"}],
                "orientation": "axial",
            },
            {"stack_id": 2, "files": [], "body_part": ""},
            {"stack_id": 3, "files": [{"source": 0, "path": "b/enhanced.dcm", "frames": None}]},
        ],
    }
    stacks = manifest.parse(doc, source_root=Path("/tmp/x"), frames_of=frames_of)
    assert [(s.path, s.frame) for s in stacks[0].slices] == [
        ("/tmp/x/0/a/1.dcm", 0),
        ("/tmp/x/1/a/mf.dcm", 2),
        ("/tmp/x/1/a/mf.dcm", 3),
        ("/tmp/x/1/a/mf.dcm", 4),
        ("/tmp/x/1/a/mf.dcm", 0),
    ]
    assert stacks[0].unit == "stack-1" and stacks[0].orientation == "axial"
    assert stacks[1].unit == "stack-2" and stacks[1].num_slices == 0 and stacks[1].body_part is None
    # Only a stack of one file whose frames are null is asked how many it has.
    assert counted == ["/tmp/x/0/b/enhanced.dcm"] and stacks[2].num_slices == 3
    assert manifest.parse_frames("1-4,9") == [0, 1, 2, 3, 8]
    src = [{"id": 0, "mount": "/source/0"}]
    for bad in (
        {"sources": src, "stacks": [{"stack_id": 1, "files": [{"source": 0, "path": "/etc/passwd"}]}]},
        {"sources": src, "stacks": [{"stack_id": 1, "files": [{"source": 0, "path": "../x"}]}]},
        {"sources": src, "stacks": [{"stack_id": 1, "files": [{"source": 5, "path": "x"}]}]},
        {"sources": src, "stacks": [{"stack_id": 1}, {"stack_id": 1}]},
        {"sources": src, "stacks": [{"stack_id": 1, "files": [{"source": 0, "path": "a", "frames": "0-3"}]}]},
        {"sources": src, "stacks": [{"stack_id": 1, "files": [{"source": 0, "path": "a", "frames": "x"}]}]},
        {"sources": src, "stacks": [{"stack_id": "twelve"}]},
    ):
        with pytest.raises(manifest.ManifestError):
            manifest.parse(bad)


def write_set(d, rows, **prov):
    d.mkdir(parents=True, exist_ok=True)
    cols = ["stack_id", "subject_id", "session_day", "what", "value", "derivative_id", "author_kind", "author", "decision_id", "campaign_id", "model_id", "answer_id"]
    text = "\t".join(cols) + "\n" + "".join(f"{s}\t1\t\tbody_part\t{v}\t\tperson\tp\t1\t\t\t\n" for s, v in rows)
    (d / "labels.tsv").write_text(text)
    p = {"name": "bp", "version": 1, "pack_version": "mri@0.4.0", "sealed": False, "digest": {"sha256": hashlib.sha256(text.encode()).hexdigest(), "of": "labels.tsv"}}
    p.update(prov)
    (d / "provenance.json").write_text(json.dumps(p))
    return text


def test_a_label_set_is_held_to_its_digest_and_its_seal(tmp_path):
    write_set(tmp_path / "ok", [(1, "brain"), (2, "spine"), (3, "brain"), (3, "spine"), (4, "cant_tell"), (5, "cant_tell"), (5, "brain")])
    ls = labels.load(tmp_path / "ok")
    got, conflicted = ls.stack_labels("body_part")
    # a rater's can't tell is never a label: stack 4 has none, stack 5 its value
    assert got == {1: "brain", 2: "spine", 5: "brain"} and conflicted == 1
    assert ls.digest.startswith("sha256:") and ls.pack_version == "mri@0.4.0"
    write_set(tmp_path / "tampered", [(1, "brain")], digest={"sha256": "0" * 64})
    with pytest.raises(labels.LabelSetError):
        labels.load(tmp_path / "tampered")
    write_set(tmp_path / "sealed", [(1, "brain")], sealed=True)
    with pytest.raises(labels.LabelSetError):
        labels.load(tmp_path / "sealed")
