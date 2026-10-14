# SPDX-License-Identifier: AGPL-3.0-only
"""``results.json``: what a run did, per unit, for the runner to read
(``contracts/job/v1``: units, derivatives, metrics, error, proposals), as
nils-bodypart writes it. A path is relative to the output folder; an error
never names a file path, since a source path may carry a subject code.
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
    # members beside the contract's, such as the sha256 of each file a
    # stack's reads took whole (``read``, record 55 E5 R3)
    extra: dict = field(default_factory=dict)

    def as_json(self) -> dict:
        return {
            "unit_id": self.unit_id,
            "work_unit": self.work_unit,
            "status": self.status,
            "derivatives": [o["path"] for o in self.outputs],
            "outputs": self.outputs,
            "metrics": self.metrics,
            "error": self.error,
            **self.extra,
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
        # what runs after the results are written (the slice cache's
        # writes, best-effort), and then the results are written again
        self.after_write = None
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
            "image": {"name": "nils-postcontrast", "version": __version__},
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
        # whole or not at all: a run writes it again after the slice cache
        tmp = p.with_name(p.name + ".partial")
        tmp.write_text(json.dumps(doc, indent=2, sort_keys=False) + "\n", encoding="utf-8")
        os.replace(tmp, p)
        return p
