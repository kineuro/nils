# SPDX-License-Identifier: AGPL-3.0-only
"""§12.2, the instances: every instance v0 holds under the root is in v1 or
the tool says why not (the path is in v1 with a refusal, or v1 never saw
the file), and every v1 instance under the root is in v0 or is explained
(v0's extension mode skipped the name, its SOP-class filter refused the
file, its resume skipped it, or the series is one v0 never wrote)."""

from __future__ import annotations

import sys
from dataclasses import dataclass, field
from pathlib import Path

import duckdb

from .mapping import V0_MODALITIES, V0_SOP_CLASSES

#: Files checked on disk for the v0 instances v1 has no path for, at most,
#: by default (`--fs-cap`; 0 lifts the cap). A v0 path is relative to its
#: cohort's root, so a subject listed under several cohorts has instances
#: under other roots than the compared one, and the check is what tells
#: those from files v1 should have seen.
FS_CAP = 1_000_000

#: The class of a v0 path absent from disk, when the subject is listed
#: under several cohorts and the file may sit under another cohort's root.
SEVERAL_COHORTS = "path not in v1: no such file under root (subject in several cohorts)"


@dataclass
class InstanceReport:
    v0_total: int = 0
    v1_total: int = 0
    common: int = 0
    #: class -> count, for the v0 instances not in v1
    v0_only: dict[str, int] = field(default_factory=dict)
    #: class -> count, for the v1 instances not in v0
    v1_only: dict[str, int] = field(default_factory=dict)
    #: v0 subjects listed under more than one cohort
    v0_subjects_in_several_cohorts: int = 0
    fs_checked: int = 0


def _log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def _name_matches(mode: str) -> str:
    """The SQL predicate under which v0's `_matches_extension` accepted a
    file name (`name` = the last path component), per mode."""
    no_suffix = r"NOT regexp_matches(name, '^.+\.[^.]+$')"
    return {
        "all": f"(lower(name) LIKE '%.dcm' OR {no_suffix})",
        "dcm": "name LIKE '%.dcm'",
        "DCM": "name LIKE '%.DCM'",
        "all_dcm": "lower(name) LIKE '%.dcm'",
        "no_ext": no_suffix,
    }[mode]


def scope(con: duckdb.DuckDBPyConnection, cohort: str | None) -> int:
    """`w.v0_subject`: the v0 subjects compared (a cohort's, or all), and
    `w.v0_instance`: their instances with their series. The subject count."""
    if cohort is None:
        con.execute("CREATE OR REPLACE TABLE w.v0_subject AS SELECT subject_id, subject_code FROM v0db.v0.subject")
    else:
        con.execute(
            "CREATE OR REPLACE TABLE w.v0_subject AS "
            "SELECT DISTINCT s.subject_id, s.subject_code FROM v0db.v0.subject s "
            "JOIN v0db.v0.subject_cohorts sc ON sc.subject_id = s.subject_id "
            "JOIN v0db.v0.cohort c ON c.cohort_id = sc.cohort_id WHERE c.name = ?",
            [cohort],
        )
    n = con.execute("SELECT count(*) FROM w.v0_subject").fetchone()[0]
    if n == 0:
        raise LookupError("v0: no subject in scope" + (f" (cohort {cohort})" if cohort else ""))
    con.execute(
        "CREATE OR REPLACE TABLE w.v0_instance AS "
        "SELECT i.instance_id, i.sop_instance_uid, i.dicom_file_path AS path, "
        "i.series_id, i.series_stack_id, s.series_instance_uid, s.study_id, s.subject_id "
        "FROM v0db.v0.instance i JOIN v0db.v0.series s ON s.series_id = i.series_id "
        "WHERE s.subject_id IN (SELECT subject_id FROM w.v0_subject)"
    )
    con.execute("CREATE INDEX IF NOT EXISTS w_v0_instance_sop ON w.v0_instance (sop_instance_uid)")
    return n


def compare(
    con: duckdb.DuckDBPyConnection,
    cohort: str | None,
    v0_files: str,
    root: Path | None,
    check_fs: bool,
    fs_cap: int = FS_CAP,
) -> InstanceReport:
    rep = InstanceReport()
    scope(con, cohort)
    con.execute(
        "CREATE OR REPLACE TABLE w.v0_subject_several AS "
        "SELECT subject_id FROM v0db.v0.subject_cohorts "
        "WHERE subject_id IN (SELECT subject_id FROM w.v0_subject) GROUP BY subject_id HAVING count(*) > 1"
    )
    rep.v0_subjects_in_several_cohorts = con.execute("SELECT count(*) FROM w.v0_subject_several").fetchone()[0]
    rep.v0_total = con.execute("SELECT count(*) FROM w.v0_instance").fetchone()[0]
    rep.v1_total = con.execute("SELECT count(*) FROM w.instance").fetchone()[0]

    # v0 -> v1
    con.execute(
        "CREATE OR REPLACE TABLE w.v0_instance_class AS "
        "SELECT v.instance_id, v.path, "
        "v.subject_id IN (SELECT subject_id FROM w.v0_subject_several) AS several_cohorts, "
        "CASE WHEN b.id IS NOT NULL THEN 'matched' "
        "     WHEN f.id IS NULL THEN 'path not in v1' "
        "     WHEN f.status = 'ingested' THEN 'path in v1, ingested under another sop' "
        "     ELSE 'path in v1, ' || f.status || coalesce(': ' || f.reason, '') END AS class "
        "FROM w.v0_instance v "
        "LEFT JOIN w.instance b ON b.sop_instance_uid = v.sop_instance_uid "
        "LEFT JOIN w.source_file f ON f.path = v.path"
    )
    rep.common = con.execute("SELECT count(*) FROM w.v0_instance_class WHERE class = 'matched'").fetchone()[0]
    for cls, n in con.execute(
        "SELECT class, count(*) FROM w.v0_instance_class WHERE class <> 'matched' GROUP BY class ORDER BY 2 DESC"
    ).fetchall():
        rep.v0_only[cls] = n
    missing = rep.v0_only.pop("path not in v1", 0)
    if missing:
        if check_fs and root is not None:
            limit = f" LIMIT {int(fs_cap)}" if fs_cap else ""
            rows = con.execute(
                "SELECT path, several_cohorts FROM w.v0_instance_class WHERE class = 'path not in v1' "
                f"ORDER BY several_cohorts, instance_id{limit}"
            ).fetchall()
            exists = 0
            absent = {False: 0, True: 0}
            for path, several in rows:
                if (root / path).is_file():
                    exists += 1
                else:
                    absent[bool(several)] += 1
            rep.fs_checked = len(rows)
            if exists:
                rep.v0_only["path not in v1: file under root (v1 missed it)"] = exists
            if absent[False]:
                rep.v0_only["path not in v1: no such file under root"] = absent[False]
            if absent[True]:
                rep.v0_only[SEVERAL_COHORTS] = absent[True]
            if missing > len(rows):
                rep.v0_only["path not in v1: unchecked"] = missing - len(rows)
        else:
            rep.v0_only["path not in v1: unverified"] = missing
    _log(f"instances: v0 {rep.v0_total:,}, v1 {rep.v1_total:,}, common {rep.common:,}")

    # v1 -> v0
    con.execute(
        "CREATE OR REPLACE TABLE w.v0_series_max AS "
        "SELECT s.series_instance_uid, max(i.sop_instance_uid) AS max_sop "
        "FROM v0db.v0.instance i JOIN v0db.v0.series s ON s.series_id = i.series_id GROUP BY 1"
    )
    con.execute(
        "CREATE OR REPLACE TABLE w.v0_subject_max AS "
        "SELECT u.subject_code, max(i.sop_instance_uid) AS max_sop "
        "FROM v0db.v0.instance i JOIN v0db.v0.series s ON s.series_id = i.series_id "
        "JOIN v0db.v0.subject u ON u.subject_id = s.subject_id GROUP BY 1"
    )
    nine = ", ".join(f"'{u}'" for u in sorted(V0_SOP_CLASSES))
    modalities = ", ".join(f"'{m}'" for m in sorted(V0_MODALITIES))
    con.execute(
        f"CREATE OR REPLACE TABLE w.v1_only_class AS "
        f"SELECT b.id, CASE "
        f"  WHEN o.sop_instance_uid IS NOT NULL THEN 'in v0 under another subject or cohort' "
        f"  WHEN f.path IS NOT NULL AND NOT {_name_matches(v0_files)} THEN 'name outside v0 mode {v0_files}' "
        f"  WHEN s.sop_class_uid IS NULL OR s.sop_class_uid NOT IN ({nine}) THEN 'sop class not in v0''s nine' "
        f"  WHEN s.modality IS NULL OR s.modality NOT IN ({modalities}) THEN 'modality not in v0''s' "
        f"  WHEN m.max_sop IS NOT NULL AND b.sop_instance_uid <= m.max_sop THEN 'resume skip' "
        f"  WHEN m.max_sop IS NOT NULL THEN 'unexplained: series known to v0' "
        f"  WHEN t0.study_id IS NOT NULL THEN 'series absent from v0: study known' "
        f"  WHEN um.max_sop IS NOT NULL AND b.sop_instance_uid <= um.max_sop THEN 'resume skip: legacy token' "
        f"  WHEN um.max_sop IS NOT NULL THEN 'series absent from v0: subject known' "
        f"  ELSE 'series absent from v0: subject new' END AS class "
        f"FROM w.instance b "
        f"JOIN w.series s ON s.id = b.series_id "
        f"JOIN w.study t ON t.id = s.study_id "
        f"JOIN w.subject u ON u.id = t.subject_id "
        f"LEFT JOIN (SELECT sop_instance_uid FROM v0db.v0.instance) o ON o.sop_instance_uid = b.sop_instance_uid "
        f"LEFT JOIN (SELECT id, regexp_extract(path, '[^/]*$') AS name, path FROM w.source_file) f "
        f"  ON f.id = b.source_file_id "
        f"LEFT JOIN w.v0_series_max m ON m.series_instance_uid = s.series_instance_uid "
        f"LEFT JOIN (SELECT study_instance_uid, study_id FROM v0db.v0.study) t0 "
        f"  ON t0.study_instance_uid = t.study_instance_uid "
        f"LEFT JOIN w.v0_subject_max um ON um.subject_code = u.code "
        f"WHERE b.sop_instance_uid NOT IN "
        f"(SELECT sop_instance_uid FROM w.v0_instance WHERE sop_instance_uid IS NOT NULL)"
    )
    for cls, n in con.execute(
        "SELECT class, count(*) FROM w.v1_only_class GROUP BY class ORDER BY 2 DESC"
    ).fetchall():
        rep.v1_only[cls] = n
    _log(f"instances: v0-only {sum(rep.v0_only.values()):,}, v1-only {sum(rep.v1_only.values()):,}")
    return rep
