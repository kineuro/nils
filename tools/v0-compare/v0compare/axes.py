# SPDX-License-Identifier: AGPL-3.0-only
"""§11.1 of Wave 2, the axes: v0's classification cache against v1's
`classification_axis`, joined on the stacks both sides partitioned the same
way, compared axis by axis.

Every difference is grouped by what it is, `axis: v0=<value> v1=<value>`, and
a group is either agreed or classified by the adjudication. Two classes are
never allowed, whatever the file says: an axis v1 leaves unresolved that v0
resolved, and a stack v1 excludes that v0 classified. A value here is a class
name from a vocabulary, never a description, a path or an identifier, so the
groups can go into the record.
"""

from __future__ import annotations

from dataclasses import dataclass, field

import duckdb

#: v0's column for each axis of the pack, and v1's axis name.
AXES = (
    ("provenance", "provenance"),
    ("technique", "technique"),
    ("modifier", "modifier_csv"),
    ("construct", "construct_csv"),
    ("base", "base"),
    ("body_part", "body_part"),
    ("post_contrast", "post_contrast"),
    ("directory_type", "directory_type"),
)

#: what v0 writes when it has decided nothing
NOTHING = ("", "Unknown", "unknown", "None", "none")


@dataclass
class Group:
    """Differences of one axis that read the same way."""

    axis: str
    pattern: str
    count: int
    classification: str | None = None
    note: str | None = None

    @property
    def key(self) -> tuple[str, str]:
        return (self.axis, self.pattern)


@dataclass
class AxisStat:
    axis: str
    compared: int = 0
    agreed: int = 0
    #: v1 says nothing where v0 said something, which no class excuses
    v1_silent: int = 0
    #: and the other way, which is a fill and is allowed
    v0_silent: int = 0
    groups: list[Group] = field(default_factory=list)

    @property
    def differed(self) -> int:
        return self.compared - self.agreed


@dataclass
class AxesReport:
    stacks: int = 0
    #: stacks v1 classified as excluded that v0 gave a directory type
    excluded_by_v1: int = 0
    axes: list[AxisStat] = field(default_factory=list)
    #: v0 had a verdict, v1 has no classification row at all
    unclassified_by_v1: int = 0

    @property
    def unclassified_groups(self) -> int:
        return sum(1 for a in self.axes for g in a.groups if g.classification is None)


def _norm(value: str | None) -> str:
    """v0 spells an empty answer four ways; they are one answer."""
    if value is None:
        return ""
    v = value.strip()
    return "" if v in NOTHING else v


def compare(con: duckdb.DuckDBPyConnection, cap: int = 12) -> AxesReport:
    rep = AxesReport()
    if not _has(con, "series_classification_cache"):
        return rep

    con.execute("DROP TABLE IF EXISTS w.axis_pair")
    con.execute(
        "CREATE TABLE w.axis_pair AS "
        "SELECT p.v0_id AS v0_stack, p.v1_id AS v1_stack, "
        + ", ".join(f'c."{v0}" AS v0_{name}' for name, v0 in AXES)
        + " FROM w.stack_pair p "
        "JOIN v0db.v0.series_classification_cache c ON c.series_stack_id = p.v0_id"
    )
    rep.stacks = con.execute("SELECT count(*) FROM w.axis_pair").fetchone()[0]
    if rep.stacks == 0:
        return rep

    con.execute("DROP TABLE IF EXISTS w.v1_axis")
    con.execute(
        "CREATE TABLE w.v1_axis AS SELECT stack_id, axis, value FROM v1.classification_axis"
    )
    rep.unclassified_by_v1 = con.execute(
        "SELECT count(*) FROM w.axis_pair a WHERE NOT EXISTS "
        "(SELECT 1 FROM v1.classification c WHERE c.stack_id = a.v1_stack)"
    ).fetchone()[0]

    for name, _v0 in AXES:
        stat = AxisStat(axis=name)
        rows = con.execute(
            f"SELECT a.v0_{name}, x.value, count(*) FROM w.axis_pair a "
            "LEFT JOIN w.v1_axis x ON x.stack_id = a.v1_stack AND x.axis = ? "
            f"GROUP BY a.v0_{name}, x.value",
            [name],
        ).fetchall()
        counts: dict[str, int] = {}
        for v0_value, v1_value, n in rows:
            left, right = _norm(v0_value), _norm(v1_value)
            stat.compared += n
            if left == right:
                stat.agreed += n
                continue
            if right == "":
                stat.v1_silent += n
            elif left == "":
                stat.v0_silent += n
            pattern = f"v0={left or '(nothing)'} v1={right or '(nothing)'}"
            counts[pattern] = counts.get(pattern, 0) + n
        stat.groups = [
            Group(axis=name, pattern=p, count=c)
            for p, c in sorted(counts.items(), key=lambda kv: -kv[1])[:cap]
        ]
        # Anything past the cap is one group, so nothing goes uncounted.
        rest = sum(c for p, c in counts.items() if p not in {g.pattern for g in stat.groups})
        if rest:
            stat.groups.append(Group(axis=name, pattern="(the rest)", count=rest))
        rep.axes.append(stat)

    rep.excluded_by_v1 = con.execute(
        "SELECT count(*) FROM w.axis_pair a "
        "JOIN w.v1_axis x ON x.stack_id = a.v1_stack AND x.axis = 'directory_type' "
        "WHERE x.value = 'excluded' AND a.v0_directory_type IS NOT NULL "
        "AND a.v0_directory_type <> 'excluded'"
    ).fetchone()[0]
    return rep


def _has(con: duckdb.DuckDBPyConnection, table: str) -> bool:
    return (
        con.execute(
            "SELECT count(*) FROM duckdb_tables() WHERE database_name = 'v0db' AND table_name = ?",
            [table],
        ).fetchone()[0]
        > 0
    )
