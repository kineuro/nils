# SPDX-License-Identifier: AGPL-3.0-only
"""The field catalogue, read from `docs/reference/catalogue.md`, which the
engine renders from its own table and checks with a test: one row per column
the digest writes, with its converter (§6.3) and its sensitivity class
(§4.3). The tool compares by converter and reports by class."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

LEVELS = ("subject", "study", "series", "series_mr", "series_ct", "series_pet", "stack", "instance")
CONVERTERS = ("text", "int", "double", "date", "time", "json")
CLASSES = ("technical", "quasi-identifying", "identifying")

_HEADING = re.compile(r"^## (\w+) \((\d+)")
_ROW = re.compile(r"^\| `(\w+)` \| (.*?) \| (\w+) \| ([\w-]+) \| (.*?) \|$")


@dataclass(frozen=True)
class Field:
    level: str
    column: str
    converter: str
    sensitivity: str
    note: str

    @property
    def classed(self) -> bool:
        """Whether a value of this field may never appear in a report."""
        return self.sensitivity != "technical"


def default_path() -> Path:
    """The catalogue of the checkout this file sits in."""
    return Path(__file__).resolve().parents[3] / "docs" / "reference" / "catalogue.md"


def load(path: Path | None = None) -> dict[str, list[Field]]:
    """The fields per level, in the catalogue's order."""
    path = path or default_path()
    levels: dict[str, list[Field]] = {}
    declared: dict[str, int] = {}
    level = None
    for line in path.read_text(encoding="utf-8").splitlines():
        m = _HEADING.match(line)
        if m:
            level = m.group(1)
            if level not in LEVELS:
                raise ValueError(f"{path}: unknown level {level!r}")
            declared[level] = int(m.group(2))
            levels[level] = []
            continue
        m = _ROW.match(line)
        if m and level:
            column, _source, converter, sensitivity, note = m.groups()
            if converter not in CONVERTERS:
                raise ValueError(f"{path}: {level}.{column}: unknown converter {converter!r}")
            if sensitivity not in CLASSES:
                raise ValueError(f"{path}: {level}.{column}: unknown class {sensitivity!r}")
            levels[level].append(Field(level, column, converter, sensitivity, note))
    for name in LEVELS:
        if name not in levels:
            raise ValueError(f"{path}: no section for {name}")
        if len(levels[name]) != declared[name]:
            raise ValueError(
                f"{path}: {name} declares {declared[name]} columns, {len(levels[name])} rows read"
            )
    return levels
