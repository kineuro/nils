# SPDX-License-Identifier: AGPL-3.0-only
"""§12.3, the fields: for every level, the rows both sides hold are paired
on their UID (the subject code for subjects, the matched stacks for stacks),
every catalogue column is normalized on both sides into a pair table, and
per field the tool counts rows compared, both null, one null, equal and
different. The rows that differ are read back as shapes and grouped by the
pattern they follow."""

from __future__ import annotations

import sys
from dataclasses import dataclass, field

import duckdb

from . import normalize, v1
from .catalogue import Field
from .mapping import LEVELS, STACK_EXTRA, Level
from .shapes import pattern, shape
from .v0 import TABLES

#: Residual pairs read back per field, at most; beyond it a deterministic
#: sample of that size is classified and the counts say so.
SAMPLE_CAP = 200_000

_NUMERIC = {"BIGINT", "DOUBLE"}

# How the v0 side of each level is joined to the v1 side in `w`.
_JOINS: dict[str, str] = {
    "subject": "FROM v0db.v0.subject a JOIN w.subject b ON b.code = a.subject_code",
    "study": "FROM v0db.v0.study a JOIN w.study b ON b.study_instance_uid = a.study_instance_uid",
    "series": "FROM v0db.v0.series a JOIN w.series b ON b.series_instance_uid = a.series_instance_uid",
    "series_mr": (
        "FROM v0db.v0.mri_series_details a JOIN v0db.v0.series a0 ON a0.series_id = a.series_id "
        "JOIN w.series b0 ON b0.series_instance_uid = a0.series_instance_uid "
        "JOIN w.series_mr b ON b.series_id = b0.id"
    ),
    "series_ct": (
        "FROM v0db.v0.ct_series_details a JOIN v0db.v0.series a0 ON a0.series_id = a.series_id "
        "JOIN w.series b0 ON b0.series_instance_uid = a0.series_instance_uid "
        "JOIN w.series_ct b ON b.series_id = b0.id"
    ),
    "series_pet": (
        "FROM v0db.v0.pet_series_details a JOIN v0db.v0.series a0 ON a0.series_id = a.series_id "
        "JOIN w.series b0 ON b0.series_instance_uid = a0.series_instance_uid "
        "JOIN w.series_pet b ON b.series_id = b0.id"
    ),
    "stack": (
        "FROM v0db.v0.series_stack a JOIN w.stack_pair p ON p.v0_id = a.series_stack_id "
        "JOIN w.stack b ON b.id = p.v1_id"
    ),
    "instance": "FROM v0db.v0.instance a JOIN w.instance b ON b.sop_instance_uid = a.sop_instance_uid",
}

_KEYS: dict[str, str] = {
    "subject": "a.subject_code",
    "study": "a.study_instance_uid",
    "series": "a.series_instance_uid",
    "series_mr": "a0.series_instance_uid",
    "series_ct": "a0.series_instance_uid",
    "series_pet": "a0.series_instance_uid",
    "stack": "a.series_stack_id",
    "instance": "a.sop_instance_uid",
}


@dataclass
class Group:
    """Divergences of one field that follow one pattern."""

    level: str
    field: str
    pattern: str
    count: int
    #: up to three (v0 shape, v1 shape) pairs; none for a classed field
    samples: list[tuple[str, str]] = field(default_factory=list)
    #: set by the adjudication
    classification: str | None = None
    note: str | None = None

    @property
    def key(self) -> tuple[str, str, str]:
        return (self.level, self.field, self.pattern)


@dataclass
class FieldStat:
    level: str
    field: str
    converter: str
    sensitivity: str
    #: how the two sides were compared: `double`, `int`, `text`, `date`,
    #: `time`, `json` (presence only)
    kind: str
    compared: int = 0
    both_null: int = 0
    one_null: int = 0
    equal: int = 0
    differ: int = 0
    #: the residual was sampled down to this many rows (0: read whole)
    sampled: int = 0
    groups: list[Group] = field(default_factory=list)
    #: the v0 column the field was read from; none when v0 has no counterpart
    v0_column: str | None = None
    #: rows whose divergence the adjudication excuses (accepted, or v0's
    #: bug); scaled up from the sample when the residual was sampled
    excused: int = 0

    @property
    def residual(self) -> int:
        return self.one_null + self.differ

    @property
    def agreement(self) -> float | None:
        """equal + both null + excused over compared; none when nothing was
        compared."""
        if self.compared == 0:
            return None
        return (self.equal + self.both_null + self.excused) / self.compared

    def excuse(self, classes: tuple[str, ...]) -> None:
        """Count the rows of the groups classed as one of `classes`."""
        n = sum(g.count for g in self.groups if g.classification in classes)
        if self.sampled and self.sampled < self.residual:
            n = round(n * self.residual / self.sampled)
        self.excused = min(n, self.residual)


@dataclass
class Plan:
    """How one field is read from both sides."""

    fld: Field
    v0_column: str
    kind: str
    a_expr: str
    b_expr: str
    decimals: int | None


def _log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def _plan(level: Level, fld: Field) -> Plan | None:
    v0_column = level.v0_column(fld.column)
    if v0_column is None:
        return None
    v0_type = TABLES[level.table][v0_column]
    v1_type = v1.DUCK_TYPES[fld.converter]
    a = f"a.{v0_column}"
    b = f"b.{fld.column}"
    decimals = level.decimals.get(fld.column)
    if fld.converter in ("int", "double") and v0_type in _NUMERIC:
        if fld.converter == "double" or v0_type == "DOUBLE":
            return Plan(fld, v0_column, "double", f"CAST({a} AS DOUBLE)", f"CAST({b} AS DOUBLE)", decimals)
        return Plan(fld, v0_column, "int", a, b, None)
    if fld.converter in ("int", "double") or v0_type in _NUMERIC:
        # numeric on one side, text on the other: both as text, canonical
        return Plan(
            fld,
            v0_column,
            "text",
            normalize.expression(f"CAST({a} AS VARCHAR)", "text"),
            normalize.expression(f"CAST({b} AS VARCHAR)", "text"),
            None,
        )
    if v1_type != v0_type:
        raise AssertionError(f"{level.name}.{fld.column}: v0 {v0_type}, v1 {fld.converter}")
    return Plan(
        fld,
        v0_column,
        fld.converter,
        normalize.expression(a, fld.converter),
        normalize.expression(b, fld.converter),
        None,
    )


def plans(level_name: str, fields: list[Field]) -> list[Plan]:
    level = LEVELS[level_name]
    extra = STACK_EXTRA if level_name == "stack" else ()
    out = []
    for fld in [*fields, *extra]:
        p = _plan(level, fld)
        if p is not None:
            out.append(p)
    return out


def pair(con: duckdb.DuckDBPyConnection, level_name: str, fields: list[Field]) -> tuple[int, list[Plan]]:
    """Build `w.pair_<level>`: one row per pair of rows, the key and every
    field in normal form on both sides; the number of pairs."""
    ps = plans(level_name, fields)
    columns = ", ".join(f"{p.a_expr} AS a_{p.fld.column}, {p.b_expr} AS b_{p.fld.column}" for p in ps)
    con.execute(
        f"CREATE OR REPLACE TABLE w.pair_{level_name} AS "
        f"SELECT {_KEYS[level_name]} AS key, {columns} {_JOINS[level_name]}"
    )
    n = con.execute(f"SELECT count(*) FROM w.pair_{level_name}").fetchone()[0]
    _log(f"{level_name}: {n:,} pair(s)")
    return n, ps


def _agree(p: Plan) -> str:
    return normalize.agree(f"a_{p.fld.column}", f"b_{p.fld.column}", p.kind, p.decimals)


def stats(con: duckdb.DuckDBPyConnection, level_name: str, ps: list[Plan]) -> list[FieldStat]:
    """Per field: compared, both null, one null, equal, differ."""
    if not ps:
        return []
    aggregates = ["count(*)"]
    for p in ps:
        a, b = f"a_{p.fld.column}", f"b_{p.fld.column}"
        both = f"{a} IS NOT NULL AND {b} IS NOT NULL"
        aggregates += [
            f"count(*) FILTER (WHERE {a} IS NULL AND {b} IS NULL)",
            f"count(*) FILTER (WHERE ({a} IS NULL) <> ({b} IS NULL))",
            f"count(*) FILTER (WHERE {both} AND {_agree(p)})",
            f"count(*) FILTER (WHERE {both} AND NOT ({_agree(p)}))",
        ]
    row = con.execute(f"SELECT {', '.join(aggregates)} FROM w.pair_{level_name}").fetchone()
    out = []
    compared = row[0]
    for i, p in enumerate(ps):
        both_null, one_null, equal, differ = row[1 + 4 * i : 5 + 4 * i]
        out.append(
            FieldStat(
                level_name,
                p.fld.column,
                p.fld.converter,
                p.fld.sensitivity,
                p.kind,
                compared,
                both_null,
                one_null,
                equal,
                differ,
                v0_column=p.v0_column,
            )
        )
    return out


def residual(con: duckdb.DuckDBPyConnection, level_name: str, p: Plan, stat: FieldStat, cap: int = SAMPLE_CAP) -> None:
    """Read back the pairs of one field that do not agree and group them by
    pattern; a residual beyond `cap` rows is sampled deterministically."""
    total = stat.one_null + stat.differ
    if total == 0:
        return
    a, b = f"a_{p.fld.column}", f"b_{p.fld.column}"
    where = f"NOT ({a} IS NULL AND {b} IS NULL) AND NOT ({a} IS NOT NULL AND {b} IS NOT NULL AND {_agree(p)})"
    if total > cap:
        every = -(-total // cap)
        where += f" AND hash(key) % {every} = 0"
        stat.sampled = cap
    rows = con.execute(f"SELECT {a}, {b} FROM w.pair_{level_name} WHERE {where}").fetchall()
    if stat.sampled:
        stat.sampled = len(rows)
    classed = p.fld.classed
    groups: dict[str, Group] = {}
    for va, vb in rows:
        name = pattern(va, vb, p.kind)
        if classed and "↔" in name and name not in ("null↔value", "value↔null"):
            name = "other"
        g = groups.get(name)
        if g is None:
            g = groups[name] = Group(level_name, p.fld.column, name, 0)
        g.count += 1
        if not classed and len(g.samples) < 3:
            sample = (shape(va), shape(vb))
            if sample not in g.samples:
                g.samples.append(sample)
    stat.groups = sorted(groups.values(), key=lambda g: -g.count)


def compare_level(
    con: duckdb.DuckDBPyConnection, level_name: str, fields: list[Field], cap: int = SAMPLE_CAP
) -> tuple[int, list[FieldStat]]:
    """Pair, count and group one level; the pair count and the field stats."""
    n, ps = pair(con, level_name, fields)
    out = stats(con, level_name, ps)
    for p, stat in zip(ps, out):
        residual(con, level_name, p, stat, cap)
    return n, out
