# SPDX-License-Identifier: AGPL-3.0-only
"""A label set as the engine exports it (record 42): ``labels.tsv`` with
``provenance.json`` beside it.

The reader holds the set to what the engine says of it: the sha256 of
``labels.tsv`` must be the digest ``provenance.json`` names, and a set the
registry marked ``sealed`` (it holds items of a certification sample) is
refused as training data, as the registry refuses it (record 40 R3).
"""

from __future__ import annotations

import csv
import hashlib
import io
import json
from dataclasses import dataclass
from pathlib import Path


# The engine's word for a rater's can't tell on an axis (record 48): an
# answer, never a value of the pack.
CANT_TELL = "cant_tell"


class LabelSetError(ValueError):
    pass


@dataclass
class LabelSet:
    digest: str  # sha256:<hex> of labels.tsv
    name: str | None
    version: int | None
    pack_version: str | None
    rows: list[dict]
    provenance: dict

    def stack_labels(self, axis: str) -> tuple[dict[int, str], int]:
        """{stack: value} for one axis, and how many stacks were dropped
        because their rows disagree. A rater's can't tell (record 48) is no
        value and never a label: its rows are left out."""
        seen: dict[int, set[str]] = {}
        for r in self.rows:
            if r.get("what") != axis or not r.get("stack_id") or not r.get("value"):
                continue
            if r["value"] == CANT_TELL:
                continue
            seen.setdefault(int(r["stack_id"]), set()).add(r["value"])
        out = {s: next(iter(v)) for s, v in seen.items() if len(v) == 1}
        return out, sum(1 for v in seen.values() if len(v) > 1)


def load(directory: Path) -> LabelSet:
    d = Path(directory)
    tsv_path, prov_path = d / "labels.tsv", d / "provenance.json"
    if not tsv_path.is_file():
        raise LabelSetError("the label set has no labels.tsv")
    data = tsv_path.read_bytes()
    digest = "sha256:" + hashlib.sha256(data).hexdigest()
    prov: dict = {}
    if prov_path.is_file():
        prov = json.loads(prov_path.read_text(encoding="utf-8"))
        named = (prov.get("digest") or {}).get("sha256")
        if named:
            named = named if named.startswith("sha256:") else "sha256:" + named
            if named != digest:
                raise LabelSetError("labels.tsv is not the file its provenance.json names")
        if prov.get("sealed"):
            raise LabelSetError("the label set holds items of a sealed sample and is not training data")
    rows = list(csv.DictReader(io.StringIO(data.decode("utf-8")), delimiter="\t", quoting=csv.QUOTE_NONE))
    return LabelSet(
        digest=digest,
        name=prov.get("name"),
        version=prov.get("version"),
        pack_version=prov.get("pack_version"),
        rows=rows,
        provenance=prov,
    )
