# SPDX-License-Identifier: AGPL-3.0-only
"""How far a run is, as the engine reads it while the container works
(``contracts/job/v1/progress.schema.json``, 2026-11-05): written only where
the engine names the file, replaced whole, at most every few seconds, the
last count always."""

from __future__ import annotations

import json
from pathlib import Path

from nils_bodypart.progress import ENV, Progress

CONTRACT = Path(__file__).resolve().parents[3] / "contracts" / "job" / "v1" / "progress.schema.json"


def test_nothing_is_written_where_the_engine_names_no_file(tmp_path, monkeypatch):
    monkeypatch.delenv(ENV, raising=False)
    p = Progress(3)
    p.add()
    p.finish()
    assert p.path is None and list(tmp_path.iterdir()) == []


def test_the_count_is_written_whole_now_and_then_and_last(tmp_path, monkeypatch):
    path = tmp_path / ".nils-progress.json"
    monkeypatch.setenv(ENV, str(path))
    p = Progress(870, every_s=3600)
    p.set(1)
    assert json.loads(path.read_text()) == {"done": 1, "of": 870, "what": "stacks"}
    # too soon: kept for the next write
    p.set(312)
    assert json.loads(path.read_text())["done"] == 1
    p.set(312, force=True)
    assert json.loads(path.read_text())["done"] == 312
    # never past what it was given, and the last count is every stack
    p.set(900, force=True)
    assert json.loads(path.read_text())["done"] == 870
    p.finish()
    assert json.loads(path.read_text()) == {"done": 870, "of": 870, "what": "stacks"}
    # replaced whole: nothing half written is left beside it
    assert sorted(f.name for f in tmp_path.iterdir()) == [".nils-progress.json"]


def test_an_unknown_total_is_left_out_and_a_failed_write_is_let_go(tmp_path):
    p = Progress(None, path=str(tmp_path / "p.json"), every_s=0)
    p.add(4)
    assert json.loads((tmp_path / "p.json").read_text()) == {"done": 4, "what": "stacks"}
    gone = Progress(2, path=str(tmp_path / "nowhere" / "p.json"), every_s=0)
    gone.add()
    gone.finish()


def test_the_file_and_its_variable_are_the_contract_s():
    doc = json.loads(CONTRACT.read_text())
    assert doc["x-nils-env"] == ENV
    assert doc["required"] == ["done"]
    assert set(doc["properties"]) == {"done", "of", "what"}
