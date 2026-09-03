# SPDX-License-Identifier: AGPL-3.0-only
"""§12.3, the stacks: within every series both sides hold, the instances
both sides hold are partitioned into stacks by each side; the partitions
are compared as sets of SOP UIDs, a v0 stack and a v1 stack with the same
members are a matched pair (`w.stack_pair`, the stack level of `fields`
compares their values), and a series' partition is identical when every
stack on either side has its pair."""

from __future__ import annotations

import sys
from dataclasses import dataclass, field

import duckdb


@dataclass
class StackReport:
    #: series both sides hold with at least one common instance
    series: int = 0
    #: of those, series with more than one stack on either side
    multi: int = 0
    multi_identical: int = 0
    #: single-stack series on both sides that are not identical (a
    #: partition of one stack on one side, several on the other, counts
    #: as multi above)
    single_identical: int = 0
    #: matched stack pairs
    pairs: int = 0
    v0_stacks: int = 0
    v1_stacks: int = 0
    #: multi-stack series with identical partitions whose stacks are
    #: numbered in the same order on both sides
    multi_same_order: int = 0
    #: common instances v0 left without a stack
    v0_unstacked: int = 0
    v1_unstacked: int = 0
    #: how the non-identical partitions differ: pattern -> series
    divergent: dict[str, int] = field(default_factory=dict)
    #: divergent series whose pattern the adjudication excuses
    excused: int = 0

    @property
    def multi_agreement(self) -> float | None:
        return None if self.multi == 0 else (self.multi_identical + self.excused) / self.multi


def _log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def compare(con: duckdb.DuckDBPyConnection) -> StackReport:
    rep = StackReport()
    con.execute(
        "CREATE OR REPLACE TABLE w.common AS "
        "SELECT v.sop_instance_uid, v.series_instance_uid, v.series_stack_id AS v0_stack, "
        "b.stack_id AS v1_stack "
        "FROM w.v0_instance v JOIN w.instance b ON b.sop_instance_uid = v.sop_instance_uid"
    )
    rep.v0_unstacked = con.execute("SELECT count(*) FROM w.common WHERE v0_stack IS NULL").fetchone()[0]
    rep.v1_unstacked = con.execute("SELECT count(*) FROM w.common WHERE v1_stack IS NULL").fetchone()[0]
    for side in ("v0", "v1"):
        con.execute(
            f"CREATE OR REPLACE TABLE w.{side}_set AS "
            f"SELECT series_instance_uid, {side}_stack AS stack, "
            f"md5(string_agg(sop_instance_uid, ',' ORDER BY sop_instance_uid)) AS members, count(*) AS n "
            f"FROM w.common WHERE {side}_stack IS NOT NULL GROUP BY 1, 2"
        )
    con.execute(
        "CREATE OR REPLACE TABLE w.stack_pair AS "
        "SELECT a.series_instance_uid, a.stack AS v0_id, b.stack AS v1_id, a.n "
        "FROM w.v0_set a JOIN w.v1_set b ON b.series_instance_uid = a.series_instance_uid AND b.members = a.members"
    )
    con.execute(
        "CREATE OR REPLACE TABLE w.series_partition AS "
        "SELECT s.series_instance_uid, "
        "  coalesce(a.n, 0) AS v0_stacks, coalesce(b.n, 0) AS v1_stacks, coalesce(p.n, 0) AS pairs, "
        "  coalesce(a.n, 0) = coalesce(p.n, 0) AND coalesce(b.n, 0) = coalesce(p.n, 0) AS identical, "
        "  coalesce(a.n, 0) > 1 OR coalesce(b.n, 0) > 1 AS multi "
        "FROM (SELECT DISTINCT series_instance_uid FROM w.common) s "
        "LEFT JOIN (SELECT series_instance_uid, count(*) AS n FROM w.v0_set GROUP BY 1) a USING (series_instance_uid) "
        "LEFT JOIN (SELECT series_instance_uid, count(*) AS n FROM w.v1_set GROUP BY 1) b USING (series_instance_uid) "
        "LEFT JOIN (SELECT series_instance_uid, count(*) AS n FROM w.stack_pair GROUP BY 1) p USING (series_instance_uid)"
    )
    row = con.execute(
        "SELECT count(*), "
        "count(*) FILTER (WHERE multi), count(*) FILTER (WHERE multi AND identical), "
        "count(*) FILTER (WHERE NOT multi AND identical), "
        "sum(v0_stacks), sum(v1_stacks), sum(pairs) FROM w.series_partition"
    ).fetchone()
    rep.series, rep.multi, rep.multi_identical, rep.single_identical = row[0], row[1], row[2], row[3]
    rep.v0_stacks, rep.v1_stacks, rep.pairs = int(row[4] or 0), int(row[5] or 0), int(row[6] or 0)
    for v0n, v1n, pairs, n in con.execute(
        "SELECT v0_stacks, v1_stacks, pairs, count(*) FROM w.series_partition "
        "WHERE NOT identical GROUP BY 1, 2, 3 ORDER BY 4 DESC"
    ).fetchall():
        rep.divergent[f"v0 {v0n} stack(s), v1 {v1n}, {pairs} matched"] = n
    # the order of the stacks, where the partitions are identical
    rep.multi_same_order = con.execute(
        "SELECT count(*) FROM ("
        "  SELECT p.series_instance_uid, "
        "    string_agg(p.v0_id::VARCHAR, ',' ORDER BY a.stack_index) = "
        "    string_agg(p.v0_id::VARCHAR, ',' ORDER BY b.stack_index) AS same "
        "  FROM w.stack_pair p "
        "  JOIN v0db.v0.series_stack a ON a.series_stack_id = p.v0_id "
        "  JOIN w.stack b ON b.id = p.v1_id "
        "  WHERE p.series_instance_uid IN (SELECT series_instance_uid FROM w.series_partition WHERE multi AND identical) "
        "  GROUP BY 1) WHERE same"
    ).fetchone()[0]
    _log(
        f"stacks: {rep.series:,} common series, {rep.multi:,} multi-stack, "
        f"{rep.multi_identical:,} identical, {rep.pairs:,} matched stack(s)"
    )
    return rep
