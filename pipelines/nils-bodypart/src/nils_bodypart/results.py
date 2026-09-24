# SPDX-License-Identifier: AGPL-3.0-only
"""``results.json``: what a run did, per unit, for the runner to read.

v0's schema (``analysis_pipeline/results.py``) with what record 43 adds:

    {"schema_version": "1",
     "pipeline": "bodypart-infer", "image": {...}, "device": "cpu",
     "params": {...},
     "units": [{"unit_id": "stack-12", "work_unit": "stack",
                "status": "succeeded" | "failed" | "skipped",
                "derivatives": ["embeddings/biomedclip/12.emb"],
                "outputs": [{"kind": "embedding", "path": "...", "sha256": ...,
                             "model": <encoder digest>, ...}],
                "metrics": {...}, "error": null}],
     "metrics": {...},
     "models": [<model card>, ...],
     "proposals": [{"stack_id": 12, "axis": "body_part", "value": "brain",
                    "probabilities": {...}, "model_digest": "sha256:..."}]}

``units``, ``derivatives``, ``metrics``, ``error`` and ``proposals`` are
the contract's (``contracts/job/v1``, results and proposals schemas);
``outputs`` says what each file is, and ``models`` carries the cards of the
encoders an embed used and of the head a train fitted. A path is relative
to the output folder. An error names what went wrong and never a file path,
since a source path may carry a subject code.
"""

from __future__ import annotations

import hashlib
import json
import os
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from . import __version__

SUCCEEDED, FAILED, SKIPPED = "succeeded", "failed", "skipped"


@dataclass
class Unit:
    unit_id: str
    work_unit: str = "stack"
    status: str = SUCCEEDED
    outputs: list[dict] = field(default_factory=list)
    metrics: dict = field(default_factory=dict)
    error: str | None = None

    def as_json(self) -> dict:
        return {
            "unit_id": self.unit_id,
            "work_unit": self.work_unit,
            "status": self.status,
            "derivatives": [o["path"] for o in self.outputs],
            "outputs": self.outputs,
            "metrics": self.metrics,
            "error": self.error,
        }


class Run:
    def __init__(self, pipeline: str, output: Path, params: dict, device: str) -> None:
        self.pipeline = pipeline
        self.output = Path(output)
        self.params = params
        self.device = device
        self.units: list[Unit] = []
        self.metrics: dict[str, Any] = {}
        self.models: list[dict] = []
        self.proposals: list[dict] = []
        self.extra: dict[str, Any] = {}
        self.started = time.time()
        self.output.mkdir(parents=True, exist_ok=True)

    def output_file(self, rel: str, data: bytes, kind: str, media_type: str, **more: Any) -> dict:
        p = self.output / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        tmp = p.with_name(p.name + ".partial")
        tmp.write_bytes(data)
        os.replace(tmp, p)
        return {
            "kind": kind,
            "path": rel,
            "media_type": media_type,
            "bytes": len(data),
            "sha256": "sha256:" + hashlib.sha256(data).hexdigest(),
            **more,
        }

    def counts(self) -> dict[str, int]:
        c = {SUCCEEDED: 0, FAILED: 0, SKIPPED: 0}
        for u in self.units:
            c[u.status] = c.get(u.status, 0) + 1
        c["total"] = len(self.units)
        return c

    def write(self) -> Path:
        doc = {
            "schema_version": "1",
            "pipeline": self.pipeline,
            "image": {"name": "nils-bodypart", "version": __version__},
            "device": self.device,
            "params": self.params,
            "counts": self.counts(),
            "seconds": round(time.time() - self.started, 3),
            "units": [u.as_json() for u in self.units],
            "metrics": self.metrics,
            "models": self.models,
            **self.extra,
        }
        # A results file with no proposals leaves the member out (the
        # contract's proposals schema).
        if self.proposals:
            doc["proposals"] = self.proposals
        p = self.output / "results.json"
        p.write_text(json.dumps(doc, indent=2, sort_keys=False) + "\n", encoding="utf-8")
        return p
