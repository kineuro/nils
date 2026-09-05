# SPDX-License-Identifier: AGPL-3.0-only
"""Fold one scenario's registry into the gate's combined view.

    collect.py REGISTRY.db SCENARIO MERGED.db FIRST

Each scenario of the awkward corpus is digested on its own, with the identity
rule its manifest declares, so the registries are separate. The checker reads
one table, so this copies out of each what the checker needs: the study a file
belongs to, the subject that study belongs to, and the date the study ended up
with. Paths are re-prefixed with the scenario, since each registry saw only its
own subtree.

Subject and study ids are made unique across scenarios by prefixing the
scenario, because two registries both number from one and the checker counts
distinct subjects.
"""

from __future__ import annotations

import sqlite3
import sys


def main() -> int:
    registry, scenario, merged, first = sys.argv[1:5]
    report = sys.argv[5] if len(sys.argv) > 5 else None
    out = sqlite3.connect(merged)
    if first == "1":
        out.execute("DROP TABLE IF EXISTS rows")
        out.execute(
            "CREATE TABLE rows (path TEXT, study TEXT, subject TEXT, study_date TEXT)"
        )
        out.execute("DROP TABLE IF EXISTS said")
        out.execute("CREATE TABLE said (scenario TEXT, kind TEXT, count INTEGER)")
        out.execute("DROP TABLE IF EXISTS fp")
        out.execute(
            "CREATE TABLE fp (scenario TEXT, field_strength_tesla TEXT,"
            " field_strength_normalized TEXT, field_strength_unit TEXT,"
            " acquisition_type_filled TEXT, acquisition_type_source TEXT, image_role TEXT,"
            " dwi_b_value TEXT, dwi_b_values TEXT, dwi_b_value_source TEXT,"
            " dwi_pe_direction TEXT, dwi_pe_direction_source TEXT,"
            " dwi_directions TEXT, dwi_directions_source TEXT)"
        )

    src = sqlite3.connect(registry)
    got = src.execute(
        """
        SELECT sf.path, st.id, st.subject_id,
               COALESCE(st.date_filled, st.study_date)
        FROM source_file sf
        JOIN instance i ON i.id = sf.instance_id
        JOIN series se ON se.id = i.series_id
        JOIN study st ON st.id = se.study_id
        """
    ).fetchall()
    src.close()

    out.executemany(
        "INSERT INTO rows (path, study, subject, study_date) VALUES (?, ?, ?, ?)",
        [
            (f"{scenario}/{path}", f"{scenario}:{study}", f"{scenario}:{subject}", date)
            for path, study, subject, date in got
        ],
    )
    # A scenario whose point is that the reader must speak is checked by what
    # it said, not only by what it counted.
    src = sqlite3.connect(registry)
    spoke = src.execute("SELECT kind, sum(count) FROM diagnostic GROUP BY kind").fetchall()
    src.close()
    # Two sinks: what the writer recorded per batch is in the registry, and
    # what the run concluded once every file had been seen is in the report.
    if report:
        import json

        with open(report, encoding="utf-8") as fh:
            spoke += [(d["kind"], d["count"]) for d in json.load(fh).get("diagnostics", [])]
    out.executemany(
        "INSERT INTO said (scenario, kind, count) VALUES (?, ?, ?)",
        [(scenario, kind, n) for kind, n in spoke],
    )

    # The derived columns of the scenario's own stacks. The mess is excluded by
    # path: it is a duplicate and a truncated file under the study, and a
    # scenario is a statement about the study it declared.
    src = sqlite3.connect(registry)
    derived = src.execute(
        """
        SELECT DISTINCT f.field_strength_tesla, f.field_strength_normalized,
               f.field_strength_unit, f.acquisition_type_filled,
               f.acquisition_type_source, f.image_role,
               f.dwi_b_value, f.dwi_b_values, f.dwi_b_value_source,
               f.dwi_pe_direction, f.dwi_pe_direction_source,
               f.dwi_directions, f.dwi_directions_source
        FROM stack_fingerprint f
        JOIN instance i ON i.series_id = f.series_id
        JOIN source_file sf ON sf.instance_id = i.id
        WHERE sf.path NOT LIKE '%/_mess/%'
        """
    ).fetchall()
    src.close()
    out.executemany(
        "INSERT INTO fp (scenario, field_strength_tesla, field_strength_normalized,"
        " field_strength_unit, acquisition_type_filled, acquisition_type_source, image_role,"
        " dwi_b_value, dwi_b_values, dwi_b_value_source, dwi_pe_direction,"
        " dwi_pe_direction_source, dwi_directions, dwi_directions_source)"
        " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        [(scenario, *row) for row in derived],
    )
    out.commit()
    out.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
