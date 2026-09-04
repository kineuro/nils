# SPDX-License-Identifier: AGPL-3.0-only
"""A v0-shaped export projected from a v1 SQLite registry, so the compare
tool can be run end to end on a synthetic corpus without a v0 database: one
CSV per v0 table, the columns of `export.sh`, v0's spellings of the values
(Python list literals for multi-valued fields, padded time fractions,
rounded stack values), and the divergences the tests expect, injected on
request. Nothing here reads a real registry; the fixture is built from the
synthetic corpus of `nils-dicom`'s `corpus` example."""

from __future__ import annotations

import argparse
import csv
import sys
from dataclasses import dataclass, field
from pathlib import Path

import duckdb

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from v0compare import catalogue  # noqa: E402
from v0compare.mapping import LEVELS  # noqa: E402
from v0compare.v0 import TABLES  # noqa: E402
from v0compare.v1 import quote  # noqa: E402

#: The UID root of everything synthetic (the DICOM example root).
ROOT = "1.2.826.0.1.3680043.8.498"

#: Multi-valued fields v0 stored as the `str()` of pydicom's MultiValue.
LIST_LITERAL = {
    ("series", "image_type"),
    ("series", "image_orientation_patient"),
    ("series", "image_position_patient"),
    ("instance", "window_center"),
    ("instance", "window_width"),
    ("instance", "pixel_spacing"),
}


@dataclass
class Inject:
    """What the v0 side gets wrong, on purpose."""

    # fields: v0's value differs from v1's on this many rows
    upper_series_description: int = 0  # pattern `case`
    null_sar: int = 0  # `null↔value` on series_mr.sar
    other_institution: int = 0  # `A↔A` shapes on study.institution_name
    # instances
    drop_max_sop: int = 0  # v0 lacks the greatest SOP of n series: `unexplained: series known to v0`
    drop_lower_sop: int = 0  # v0 lacks a lower SOP of n series: `resume skip`
    phantom_missing: int = 0  # v0 rows whose path is nowhere: `path not in v1: no such file under root`
    phantom_unwalked: int = 0  # v0 rows pointing at README.txt files v1 never walked: `file under root`
    phantom_quarantined: int = 0  # v0 rows pointing at files v1 quarantined: `path in v1, quarantined: …`
    # stacks
    split_stack: int = 0  # v0 splits n single-stack series in two: `v0 2 stack(s), v1 1, 0 matched`
    # v0's series_mr row carries another instance's echo_time in n MR series,
    # the split ones first: `rounded (multi-stack)` there, accepted by the
    # tool itself; plain `rounded` in a single-stack series
    other_first_echo: int = 0
    # subjects
    recode: int = 0  # n v0 subjects carry a code v1 does not have
    # the subject the phantoms hang on is listed under a second cohort too:
    # its missing paths class as `no such file under root (subject in several cohorts)`
    second_cohort: int = 0
    #: v0 subject code -> PatientID-type identifier written to subject_other_identifiers
    identifiers: dict[str, str] = field(default_factory=dict)


def _open(registry: Path) -> duckdb.DuckDBPyConnection:
    """The registry by its declared types, so that a double reaches the CSV
    as Python writes it (the shortest spelling that reads back to the same
    value, as v0's `str()` did), not through SQLite's 15-digit text."""
    con = duckdb.connect()
    con.execute("INSTALL sqlite; LOAD sqlite; SET sqlite_all_varchar = false")
    con.execute(f"ATTACH {quote(str(registry / 'registry.db'))} AS v1 (TYPE sqlite, READ_ONLY)")
    return con


def _rows(con: duckdb.DuckDBPyConnection, sql: str) -> list[dict[str, object]]:
    cur = con.execute(sql)
    names = [d[0] for d in cur.description]
    return [dict(zip(names, r)) for r in cur.fetchall()]


def _list_literal(value: object) -> object:
    if value is None or not isinstance(value, str) or "\\" not in value:
        return value
    return "[" + ", ".join(repr(p) for p in value.split("\\")) + "]"


def _pad_time(value: object) -> object:
    if isinstance(value, str) and len(value) == 8 and value[2] == ":":
        return value + ".000000"
    return value


def _round(value: object, decimals: int) -> object:
    if value is None or value == "":
        return None
    x = round(float(value), decimals)
    return int(x) if decimals == 0 else x


def _level_columns(fields: dict[str, list[catalogue.Field]], level: str) -> list[tuple[str, str, str]]:
    """(v1 column, v0 column, converter) of a level's catalogue fields that
    v0 has a column for."""
    out = []
    for f in fields[level]:
        v0 = LEVELS[level].v0_column(f.column)
        if v0 is not None:
            out.append((f.column, v0, f.converter))
    return out


def project(
    registry: Path, out: Path, *, cohort: str = "synth", root: Path | None = None, inject: Inject | None = None
) -> tuple[dict[str, int], dict[str, int]]:
    """Write the export under `out`; the rows per table, and what was
    actually injected per `Inject` field (a request can exceed what the
    registry offers)."""
    inject = inject or Inject()
    fields = catalogue.load()
    con = _open(registry)
    tables: dict[str, list[dict[str, object]]] = {}

    tables["schema_version"] = [{"id": 1, "version": "0.5.3", "applied_at": "2026-01-01 00:00:00"}]
    tables["cohort"] = [{"cohort_id": 1, "name": cohort, "path": f"/data/{cohort}", "is_active": True}]
    tables["id_types"] = [{"id_type_id": 1, "id_type_name": "PatientID", "description": "DICOM PatientID"}]
    tables["observation_types"] = [
        {"observation_type_id": i + 1, "name": m} for i, m in enumerate(("MR", "CT", "PT"))
    ]

    # subjects
    subjects = _rows(con, "SELECT id, code, birth_date, sex FROM v1.subject ORDER BY CAST(id AS BIGINT)")
    tables["subject"] = [
        {
            "subject_id": int(s["id"]),
            "subject_code": s["code"],
            "patient_birth_date": s["birth_date"],
            "patient_sex": s["sex"],
            "has_patient_name": True,
            "is_active": True,
        }
        for s in subjects
    ]
    tables["subject_cohorts"] = [{"subject_id": int(s["id"]), "cohort_id": 1} for s in subjects]
    identifiers = []
    for s in subjects:
        ident = inject.identifiers.get(s["code"])
        if ident:
            identifiers.append(
                {
                    "subject_other_identifier_id": len(identifiers) + 1,
                    "subject_id": int(s["id"]),
                    "id_type_id": 1,
                    "other_identifier": ident,
                }
            )
    tables["subject_other_identifiers"] = identifiers

    # studies, one event each; v0's study.modality is the first series'
    first_modality = {
        r["study_id"]: r["modality"]
        for r in _rows(
            con,
            "SELECT study_id, arg_min(modality, id) AS modality FROM v1.series GROUP BY study_id",
        )
    }
    obs = {"MR": 1, "CT": 2, "PT": 3}
    studies = _rows(
        con,
        "SELECT id, study_instance_uid, subject_id, "
        + ", ".join(v1 for v1, _, _ in _level_columns(fields, "study") if v1 != "modalities_in_study")
        + " FROM v1.study ORDER BY CAST(id AS BIGINT)",
    )
    events = []
    study_rows = []
    for st in studies:
        modality = first_modality.get(st["id"], "MR")
        events.append(
            {
                "event_id": int(st["id"]),
                "subject_id": int(st["subject_id"]),
                "observation_type_id": obs.get(modality, 1),
                "event_date": st["study_date"],
                "event_time": _pad_time(st["study_time"]),
            }
        )
        row: dict[str, object] = {
            "study_id": int(st["id"]),
            "study_instance_uid": st["study_instance_uid"],
            "subject_id": int(st["subject_id"]),
            "event_id": int(st["id"]),
            "modality": modality,
        }
        for v1, v0, conv in _level_columns(fields, "study"):
            if v1 == "modalities_in_study":
                continue
            value = st[v1]
            row[v0] = _pad_time(value) if conv == "time" else value
        study_rows.append(row)
    tables["event"] = events
    tables["study"] = study_rows

    # series and the modality details
    series_cols = _level_columns(fields, "series")
    series = _rows(
        con,
        "SELECT id, series_instance_uid, study_id, subject_id, "
        + ", ".join(v1 for v1, _, _ in series_cols)
        + " FROM v1.series ORDER BY CAST(id AS BIGINT)",
    )
    series_rows = []
    for se in series:
        row = {
            "series_id": int(se["id"]),
            "series_instance_uid": se["series_instance_uid"],
            "study_id": int(se["study_id"]),
            "subject_id": int(se["subject_id"]),
        }
        for v1, v0, conv in series_cols:
            value = se[v1]
            if ("series", v1) in LIST_LITERAL:
                value = _list_literal(value)
            elif conv == "time":
                value = _pad_time(value)
            row[v0] = value
        series_rows.append(row)
    tables["series"] = series_rows
    for level, table in (("series_mr", "mri_series_details"), ("series_ct", "ct_series_details"), ("series_pet", "pet_series_details")):
        cols = _level_columns(fields, level)
        rows = _rows(
            con,
            f"SELECT d.series_id, s.series_instance_uid, " + ", ".join(f"d.{v1}" for v1, _, _ in cols)
            + f" FROM v1.{level} d JOIN v1.series s ON s.id = d.series_id ORDER BY CAST(d.series_id AS BIGINT)",
        )
        out_rows = []
        for r in rows:
            row = {"series_id": int(r["series_id"]), "series_instance_uid": r["series_instance_uid"]}
            for v1, v0, conv in cols:
                row[v0] = _pad_time(r[v1]) if conv == "time" else r[v1]
            out_rows.append(row)
        tables[table] = out_rows

    # stacks: v0's `stack_*` columns, rounded where v0 rounded
    stack_cols = _level_columns(fields, "stack")
    stacks = _rows(
        con,
        "SELECT id, series_id, stack_index, stack_key, modality, orientation, orientation_confidence, n_instances, "
        + ", ".join(v1 for v1, _, _ in stack_cols)
        + " FROM v1.stack ORDER BY CAST(id AS BIGINT)",
    )
    stack_rows = []
    for sk in stacks:
        row = {
            "series_stack_id": int(sk["id"]),
            "series_id": int(sk["series_id"]),
            "stack_modality": sk["modality"],
            "stack_index": int(sk["stack_index"]),
            "stack_key": sk["stack_key"],
            "stack_image_orientation": sk["orientation"],
            "stack_orientation_confidence": sk["orientation_confidence"],
            "stack_n_instances": int(sk["n_instances"]),
        }
        for v1, v0, _conv in stack_cols:
            value = sk[v1]
            decimals = LEVELS["stack"].decimals.get(v1)
            if decimals is not None:
                value = _round(value, decimals)
            row[v0] = value
        stack_rows.append(row)
    tables["series_stack"] = stack_rows
    # v0's verdicts. The projection carries none: a fixture that classified
    # nothing is what a registry looks like before anything was sorted, and
    # the axes bar has nothing to compare, which is what it should say.
    tables["series_classification_cache"] = []

    # instances, with v1's path
    inst_cols = _level_columns(fields, "instance")
    instances = _rows(
        con,
        "SELECT i.id, i.sop_instance_uid, i.series_id, i.stack_id, s.series_instance_uid, f.path, "
        + ", ".join(f"i.{v1}" for v1, _, _ in inst_cols)
        + " FROM v1.instance i JOIN v1.series s ON s.id = i.series_id "
        "JOIN v1.source_file f ON f.id = i.source_file_id ORDER BY CAST(i.id AS BIGINT)",
    )
    inst_rows = []
    for it in instances:
        row = {
            "instance_id": int(it["id"]),
            "series_id": int(it["series_id"]),
            "series_instance_uid": it["series_instance_uid"],
            "sop_instance_uid": it["sop_instance_uid"],
            "dicom_file_path": it["path"],
            "series_stack_id": int(it["stack_id"]) if it["stack_id"] is not None else None,
        }
        for v1, v0, conv in inst_cols:
            value = it[v1]
            if ("instance", v1) in LIST_LITERAL:
                value = _list_literal(value)
            elif conv == "time":
                value = _pad_time(value)
            row[v0] = value
        inst_rows.append(row)
    tables["instance"] = inst_rows

    done = _inject(con, tables, inject, root)
    con.close()

    out.mkdir(parents=True, exist_ok=True)
    counts = {}
    for table, columns in TABLES.items():
        rows = tables[table]
        with (out / f"{table}.csv").open("w", newline="", encoding="utf-8") as fh:
            w = csv.DictWriter(fh, fieldnames=list(columns), extrasaction="ignore")
            w.writeheader()
            for r in rows:
                w.writerow({c: _csv(r.get(c)) for c in columns})
        counts[table] = len(rows)
    return counts, done


def _csv(value: object) -> object:
    if value is None:
        return ""
    if isinstance(value, bool):
        return "t" if value else "f"
    return value


def _inject(
    con: duckdb.DuckDBPyConnection,
    tables: dict[str, list[dict[str, object]]],
    inject: Inject,
    root: Path | None,
) -> dict[str, int]:
    done = {name: 0 for name in Inject.__dataclass_fields__ if name != "identifiers"}
    series_rows = tables["series"]
    study_rows = tables["study"]
    inst_rows = tables["instance"]
    stack_rows = tables["series_stack"]

    for row in series_rows:
        if done["upper_series_description"] >= inject.upper_series_description:
            break
        text = row.get("series_description")
        if text and str(text).upper() != text:
            row["series_description"] = str(text).upper()
            done["upper_series_description"] += 1
    for row in tables["mri_series_details"]:
        if done["null_sar"] >= inject.null_sar:
            break
        if row.get("sar") not in (None, ""):
            row["sar"] = None
            done["null_sar"] += 1
    for row in study_rows:
        if done["other_institution"] >= inject.other_institution:
            break
        if row.get("institution_name") != "Somewhere Else":
            row["institution_name"] = "Somewhere Else"
            done["other_institution"] += 1

    # instances: drop by SOP order within a series
    by_series: dict[int, list[dict[str, object]]] = {}
    for row in inst_rows:
        by_series.setdefault(int(row["series_id"]), []).append(row)
    dropped: set[int] = set()
    multi = [sid for sid, rows in sorted(by_series.items()) if len(rows) >= 3]
    # v0's resume rule compares SOP UIDs as strings, as `max()` does in SQL
    for sid in multi[: inject.drop_max_sop]:
        rows = sorted(by_series[sid], key=lambda r: str(r["sop_instance_uid"]))
        dropped.add(int(rows[-1]["instance_id"]))
        done["drop_max_sop"] += 1
    for sid in multi[inject.drop_max_sop : inject.drop_max_sop + inject.drop_lower_sop]:
        rows = sorted(by_series[sid], key=lambda r: str(r["sop_instance_uid"]))
        dropped.add(int(rows[0]["instance_id"]))
        done["drop_lower_sop"] += 1
    if dropped:
        inst_rows[:] = [r for r in inst_rows if int(r["instance_id"]) not in dropped]
        for row in stack_rows:
            row["stack_n_instances"] = sum(
                1 for r in inst_rows if r["series_stack_id"] == row["series_stack_id"]
            )

    # phantoms: v0 rows v1 has no instance for, hung on the last series so
    # their SOP UIDs (which sort high) leave the dropped series' maxima alone
    next_id = max(int(r["instance_id"]) for r in inst_rows) + 1
    template = by_series[multi[-1]][0] if multi else inst_rows[-1]
    unwalked = sorted(str(p.relative_to(root)) for p in root.rglob("README.txt")) if root else []
    quarantined = [
        r[0]
        for r in con.execute(
            "SELECT path FROM v1.source_file WHERE status = 'quarantined' ORDER BY path"
        ).fetchall()
    ]

    def phantom(n: int, path: str) -> dict[str, object]:
        row = dict(template)
        row.update(
            {
                "instance_id": next_id + n,
                "sop_instance_uid": f"{ROOT}.999.{n}",
                "dicom_file_path": path,
                "series_stack_id": None,
            }
        )
        return row

    n = 0
    for _ in range(inject.phantom_missing):
        inst_rows.append(phantom(n, f"sub-999999/st-1/se-01-MR/IM_{n:04}.dcm"))
        n += 1
        done["phantom_missing"] += 1
    for path in unwalked[: inject.phantom_unwalked]:
        inst_rows.append(phantom(n, path))
        n += 1
        done["phantom_unwalked"] += 1
    for path in quarantined[: inject.phantom_quarantined]:
        inst_rows.append(phantom(n, path))
        n += 1
        done["phantom_quarantined"] += 1

    # stacks: split the first n single-stack MR series with enough instances
    next_stack = max(int(r["series_stack_id"]) for r in stack_rows) + 1
    split: list[int] = []
    for row in stack_rows:
        if len(split) >= inject.split_stack:
            break
        if row["stack_modality"] != "MR":
            continue
        members = [r for r in inst_rows if r["series_stack_id"] == row["series_stack_id"]]
        if len(members) < 4:
            continue
        second = dict(row)
        second.update({"series_stack_id": next_stack, "stack_index": int(row["stack_index"]) + 1})
        half = members[len(members) // 2 :]
        for r in half:
            r["series_stack_id"] = next_stack
        second["stack_n_instances"] = len(half)
        row["stack_n_instances"] = len(members) - len(half)
        stack_rows.append(second)
        next_stack += 1
        split.append(int(row["series_id"]))
    done["split_stack"] = len(split)

    # a stack-signature column that names another first instance: the split
    # series first, so the tool's own multi-stack class is exercised, then
    # single-stack series, which stay unclassified
    mr_rows = tables["mri_series_details"]
    ordered = [r for r in mr_rows if int(r["series_id"]) in split] + [
        r for r in mr_rows if int(r["series_id"]) not in split
    ]
    for row in ordered:
        if done["other_first_echo"] >= inject.other_first_echo:
            break
        if row.get("echo_time") in (None, ""):
            continue
        row["echo_time"] = float(row["echo_time"]) + 7.0
        done["other_first_echo"] += 1

    # subjects: another code
    for i, row in enumerate(tables["subject"][: inject.recode]):
        row["subject_code"] = f"deadbeef{i:08x}"
        done["recode"] += 1

    # the phantoms' subject under a second cohort too
    if inject.second_cohort:
        subject_of_series = {int(r["series_id"]): int(r["subject_id"]) for r in series_rows}
        tables["cohort"].append({"cohort_id": 2, "name": "other", "path": "/data/other", "is_active": True})
        tables["subject_cohorts"].append(
            {"subject_id": subject_of_series[int(template["series_id"])], "cohort_id": 2}
        )
        done["second_cohort"] = 1
    return done


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--registry", required=True, help="the v1 registry home (SQLite)")
    parser.add_argument("--out", required=True, help="the export directory to write")
    parser.add_argument("--cohort", default="synth")
    parser.add_argument("--root", help="the digested corpus, for the README.txt files v1 never walked")
    for name, f in Inject.__dataclass_fields__.items():
        if f.type == "int":
            parser.add_argument(f"--{name.replace('_', '-')}", type=int, default=0)
    args = parser.parse_args(argv)
    inject = Inject(**{k: v for k, v in vars(args).items() if k in Inject.__dataclass_fields__})
    counts, done = project(
        Path(args.registry),
        Path(args.out),
        cohort=args.cohort,
        root=Path(args.root) if args.root else None,
        inject=inject,
    )
    print(", ".join(f"{t} {n:,}" for t, n in counts.items()), file=sys.stderr)
    print("injected: " + ", ".join(f"{k} {v}" for k, v in done.items() if v), file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
