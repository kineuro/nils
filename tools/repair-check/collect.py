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
    out.commit()
    out.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
