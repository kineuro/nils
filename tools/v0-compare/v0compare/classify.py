# SPDX-License-Identifier: AGPL-3.0-only
"""Adjudication: a TOML file that names every group of divergences and
says what it is, `v0-bug` (v1 is right), `v1-bug` (fixed before the gate
closes, with a fixture) or `accepted` (a change §6.3 or §12.3 declares).
The gate's bar is that no group is left without a class; the file is the
record of the reading, kept with the run.

    [[divergence]]            # a field's group (fields.Group)
    level = "series"
    field = "image_type"
    pattern = "list-order"    # a glob over the pattern, fnmatch style
    class = "accepted"
    note = "v0 stored the multi-valued literal, v1 the parts"

    [[partition]]             # a stack partition shape (stacks.divergent)
    pattern = "v0 1 stack(s), v1 2, *"
    class = "v0-bug"
    note = "…"

    [[axis]]                  # an axis difference (axes.Group)
    axis = "technique"
    pattern = "v0=SPACE v1=3D-TSE"
    class = "accepted"
    cause = "v0 stores the display name here and the identity in its branches"
    note = "…"

    [[instance]]              # an instance class (instances.v0_only/v1_only)
    side = "v1-only"          # or "v0-only"
    pattern = "sop class not in v0's nine"
    class = "accepted"
    note = "…"
"""

from __future__ import annotations

import tomllib
from dataclasses import dataclass
from fnmatch import fnmatchcase
from pathlib import Path

CLASSES = ("v0-bug", "v1-bug", "accepted")


@dataclass(frozen=True)
class Rule:
    kind: str
    pattern: str
    classification: str
    note: str
    level: str = "*"
    field: str = "*"
    side: str = "*"
    #: §11.5, for an axis rule: what produced each answer, and which is right
    cause: str = ""

    def matches(self, kind: str, pattern: str, level: str = "", field: str = "", side: str = "") -> bool:
        return (
            self.kind == kind
            and fnmatchcase(pattern, self.pattern)
            and fnmatchcase(level, self.level)
            and fnmatchcase(field, self.field)
            and fnmatchcase(side, self.side)
        )


@dataclass
class Adjudication:
    rules: list[Rule]

    def divergence(self, level: str, field: str, pattern: str) -> Rule | None:
        return next((r for r in self.rules if r.matches("divergence", pattern, level, field)), None)

    def partition(self, pattern: str) -> Rule | None:
        return next((r for r in self.rules if r.matches("partition", pattern)), None)

    def instance(self, side: str, pattern: str) -> Rule | None:
        return next((r for r in self.rules if r.matches("instance", pattern, side=side)), None)

    def axis(self, axis: str, pattern: str) -> Rule | None:
        return next((r for r in self.rules if r.matches("axis", pattern, field=axis)), None)


def load(path: Path | None) -> Adjudication:
    if path is None:
        return Adjudication([])
    with path.open("rb") as fh:
        parsed = tomllib.load(fh)
    rules: list[Rule] = []
    for kind in ("divergence", "partition", "instance", "axis"):
        for i, entry in enumerate(parsed.get(kind, [])):
            where = f"{path}: [[{kind}]] #{i + 1}"
            classification = entry.get("class")
            if classification not in CLASSES:
                raise ValueError(f"{where}: class must be one of {', '.join(CLASSES)}")
            if "pattern" not in entry:
                raise ValueError(f"{where}: no pattern")
            # §11.5: a class is where the reading of a difference is filed,
            # not where it ends. An axis difference says what caused it.
            if kind == "axis" and not str(entry.get("cause", "")).strip():
                raise ValueError(
                    f"{where}: no cause; an axis difference names the v0 expression or the v1 rule "
                    "that produced its answer, and says which is right"
                )
            rules.append(
                Rule(
                    kind,
                    str(entry["pattern"]),
                    classification,
                    str(entry.get("note", "")),
                    level=str(entry.get("level", "*")),
                    field=str(entry.get("axis", entry.get("field", "*"))),
                    side=str(entry.get("side", "*")),
                    cause=str(entry.get("cause", "")),
                )
            )
    return Adjudication(rules)
