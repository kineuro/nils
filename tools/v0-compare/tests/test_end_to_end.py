# SPDX-License-Identifier: AGPL-3.0-only
"""The tool end to end over a synthetic corpus: a v1 registry digested by
the engine, a v0-shaped export projected from it with known divergences,
and the report the compare writes about them (§12.7)."""

from __future__ import annotations

import csv
import json
import os
import re
from pathlib import Path

import duckdb
import pytest

from conftest import Synth, harvest_patient_ids, make_synth
from v0compare import cli
from v0compare.instances import SEVERAL_COHORTS
from v0compare.mapping import MULTI_STACK, MULTI_STACK_NOTE, ORDER_DEPENDENT
from v0shape import Inject, project

# what the projection cannot reproduce: v0 filled study.modality from the
# file, v1 keeps ModalitiesInStudy, which the synthetic files do not carry
CLEAN_ADJUDICATION = """
[[divergence]]
level = "study"
field = "modalities_in_study"
pattern = "value↔null"
class = "accepted"
note = "v0's study.modality is the file's modality, v1's column is the study's ModalitiesInStudy"
"""


def _codes(synth: Synth) -> list[str]:
    con = duckdb.connect()
    con.execute("INSTALL sqlite; LOAD sqlite; SET sqlite_all_varchar = true")
    con.execute(f"ATTACH '{synth.registry / 'registry.db'}' AS v1 (TYPE sqlite, READ_ONLY)")
    codes = [r[0] for r in con.execute("SELECT code FROM v1.subject ORDER BY CAST(id AS BIGINT)").fetchall()]
    con.close()
    return codes


def _compare(synth: Synth, export: Path, out: Path, *extra: str) -> tuple[int, dict]:
    v0 = out / "v0.duckdb"
    assert cli.main(["extract", "--export", str(export), "--out", str(v0)]) == 0
    args = [
        "compare",
        "--v0",
        str(v0),
        "--v1",
        str(synth.registry),
        "--root",
        str(synth.root),
        "--cohort",
        "synth",
        "--v0-files",
        "all",
        "--out",
        str(out / "report"),
        *extra,
    ]
    code = cli.main(args)
    rep = json.loads((out / "report" / "report.json").read_text(encoding="utf-8"))
    return code, rep


def _groups(rep: dict, level: str, field: str) -> dict[str, int]:
    for stat in rep["fields"]:
        if stat["level"] == level and stat["field"] == field:
            return {g["pattern"]: g["count"] for g in stat["groups"]}
    raise KeyError(f"{level}.{field}")


def _bar(rep: dict, prefix: str) -> dict:
    return next(b for b in rep["bars"] if b["name"].startswith(prefix))


def _failed(rep: dict) -> list[str]:
    return [f"{b['name']}: {b['detail']}" for b in rep["bars"] if not b["passed"]]


def test_clean_projection_passes(synth: Synth, tmp_path: Path) -> None:
    """A v0 export that says what v1 says, in v0's spellings, passes every
    bar once the one systematic difference is adjudicated."""
    export = tmp_path / "export"
    counts, done = project(synth.registry, export, root=synth.root)
    assert counts["instance"] == synth.manifest["instances"]
    assert not any(done.values())
    adj = tmp_path / "adj.toml"
    adj.write_text(CLEAN_ADJUDICATION, encoding="utf-8")
    code, rep = _compare(synth, export, tmp_path, "--adjudication", str(adj))
    assert code == 0, _failed(rep)
    assert rep["passed"]
    inst = rep["instances"]
    assert inst["common"] == inst["v0_total"] == inst["v1_total"] == synth.manifest["instances"]
    assert inst["v0_only"] == {} and inst["v1_only"] == {}
    # every field agrees once normalized: list literals, padded times,
    # rounded stack values; the one adjudicated field is the exception
    for stat in rep["fields"]:
        if (stat["level"], stat["field"]) == ("study", "modalities_in_study"):
            assert stat["one_null"] == synth.manifest["studies"]
            assert [g["classification"] for g in stat["groups"]] == ["accepted"]
            continue
        assert stat["differ"] == 0 and stat["one_null"] == 0, (stat["level"], stat["field"], stat["groups"])
    st = rep["stacks"]
    assert st["series"] == synth.manifest["series"] and st["divergent"] == {}
    assert st["v0_stacks"] == st["v1_stacks"] == synth.manifest["series"]
    su = rep["subjects"]
    assert su["v0_subjects"] == su["codes_in_v1"] == synth.manifest["subjects"]
    assert su["studies"] == su["studies_same_code"] == synth.manifest["studies"]
    # one event per study; same-day studies are several events on one day,
    # one session in v1
    assert su["v0_events"] == synth.manifest["studies"]
    assert su["v0_events_surplus"] == synth.manifest["same_day_studies"]
    assert su["v1_extra_sessions"] == 0
    assert su["v0_extra_events"] == su["v0_events_surplus"]
    assert su["v1_sessions"] + su["v0_events_surplus"] == su["v0_events"]
    assert rep["unclassified"] == 0
    # no key, but a cohort: the code classes are computed without the key
    assert sum(rep["subjects"]["code_classes"].values()) == synth.manifest["subjects"]
    md = (tmp_path / "report" / "report.md").read_text(encoding="utf-8")
    assert "## Verdict: PASS" in md
    # nothing from the corpus in either report: no path, no UID, no code
    for text in (md, json.dumps(rep)):
        assert "sub-0" not in text and "1.2.826" not in text and "IM_" not in text
        assert not any(c in text for c in _codes(synth))


def test_injected_divergences_are_found(synth: Synth, tmp_path: Path) -> None:
    export = tmp_path / "export"
    inject = Inject(
        upper_series_description=3,
        null_sar=2,
        other_institution=2,
        drop_max_sop=1,
        drop_lower_sop=2,
        phantom_missing=2,
        phantom_unwalked=1,
        phantom_quarantined=2,
        split_stack=1,
        other_first_echo=3,
        recode=1,
        second_cohort=1,
    )
    _counts, done = project(synth.registry, export, root=synth.root, inject=inject)
    for name in ("drop_max_sop", "drop_lower_sop", "phantom_missing", "phantom_unwalked", "phantom_quarantined",
                 "split_stack", "other_first_echo", "recode", "second_cohort"):
        assert done[name] == getattr(inject, name), (name, done)
    assert done["upper_series_description"] >= 1 and done["null_sar"] >= 1 and done["other_institution"] >= 1
    code, rep = _compare(synth, export, tmp_path)
    assert code == 1 and not rep["passed"]

    # §12.2, both directions; the phantoms' subject sits in two cohorts, so
    # its paths absent from disk say so
    inst = rep["instances"]
    assert inst["v0_subjects_in_several_cohorts"] == 1
    assert inst["v0_only"][SEVERAL_COHORTS] == done["phantom_missing"]
    assert "path not in v1: no such file under root" not in inst["v0_only"]
    assert inst["v0_only"]["path not in v1: file under root (v1 missed it)"] == done["phantom_unwalked"]
    quarantined = {k: v for k, v in inst["v0_only"].items() if k.startswith("path in v1, quarantined")}
    assert sum(quarantined.values()) == done["phantom_quarantined"]
    assert inst["v1_only"] == {
        "resume skip": done["drop_lower_sop"],
        "unexplained: series known to v0": done["drop_max_sop"],
    }
    assert inst["fs_checked"] == done["phantom_missing"] + done["phantom_unwalked"]
    assert not _bar(rep, "12.2 every v0 instance")["passed"]
    assert not _bar(rep, "12.2 every v1 instance")["passed"]

    # §12.3 fields: the pattern names the shape of the difference
    assert _groups(rep, "series", "series_description") == {"case": done["upper_series_description"]}
    assert _groups(rep, "series_mr", "sar") == {"null↔value": done["null_sar"]}
    # institution_name is quasi-identifying: the pattern collapses to
    # `other` and the group carries no shapes
    assert _groups(rep, "study", "institution_name") == {"other": done["other_institution"]}
    # echo_time is a stack-signature column: the row of the split series is
    # grouped apart and is the tool's own to accept, the single-stack rows
    # keep their plain pattern and stay unclassified
    echo = _groups(rep, "series_mr", "echo_time")
    assert sum(echo.values()) == done["other_first_echo"]
    assert [n for p, n in echo.items() if p.endswith(MULTI_STACK)] == [1]
    assert sum(n for p, n in echo.items() if not p.endswith(MULTI_STACK)) == done["other_first_echo"] - 1
    auto = []
    for stat in rep["fields"]:
        for g in stat["groups"]:
            if g["pattern"].endswith(MULTI_STACK):
                auto.append(g)
                assert (g["classification"], g["note"]) == ("accepted", MULTI_STACK_NOTE)
            else:
                assert g["classification"] is None
            if stat["sensitivity"] != "technical":
                assert g["samples"] == []
    assert len(auto) == 1
    assert not _bar(rep, "12.3 every other field")["passed"]

    # §12.3 stacks: the split series is the one multi-stack series in v0
    assert rep["stacks"]["divergent"] == {"v0 2 stack(s), v1 1, 0 matched": done["split_stack"]}
    assert rep["stacks"]["multi"] == done["split_stack"] and rep["stacks"]["multi_identical"] == 0
    assert rep["partition_classes"] == {"v0 2 stack(s), v1 1, 0 matched": None}
    assert not _bar(rep, "12.3 the stack partition")["passed"]

    # §12.4 subjects: the recoded subject's studies hang off a code v0 never had
    su = rep["subjects"]
    assert su["v0_subjects"] - su["codes_in_v1"] == done["recode"]
    assert su["studies"] > su["studies_same_code"]
    assert su["studies_other_code"] == {"v1 code unknown to v0": su["studies"] - su["studies_same_code"]}
    assert su["v0_subjects_touched"] == done["recode"] and su["v1_subjects_touched"] == done["recode"]
    assert not _bar(rep, "12.4 every v0 subject code")["passed"]
    assert not _bar(rep, "12.4 every common study")["passed"]

    # nothing is adjudicated: every group counts, but the tool's own
    assert rep["instance_classes"] == {
        "v0-only": {k: None for k in inst["v0_only"]},
        "v1-only": {k: None for k in inst["v1_only"]},
    }
    n_groups = sum(len(stat["groups"]) for stat in rep["fields"])
    assert rep["unclassified"] == n_groups - len(auto) + len(inst["v0_only"]) + len(inst["v1_only"]) + 1
    assert not _bar(rep, "12.3 every divergence is classified")["passed"]


def test_fs_cap_leaves_the_rest_unchecked(synth: Synth, tmp_path: Path) -> None:
    """`--fs-cap` bounds the paths checked on disk; what lies beyond it is
    reported as unchecked, and `0` lifts the cap."""
    export = tmp_path / "export"
    _counts, done = project(synth.registry, export, root=synth.root, inject=Inject(phantom_missing=3))
    assert done["phantom_missing"] == 3
    _code, rep = _compare(synth, export, tmp_path, "--fs-cap", "2")
    inst = rep["instances"]
    assert inst["fs_checked"] == 2
    assert inst["v0_only"]["path not in v1: no such file under root"] == 2
    assert inst["v0_only"]["path not in v1: unchecked"] == 1
    _code, rep = _compare(synth, export, tmp_path, "--fs-cap", "0")
    inst = rep["instances"]
    assert inst["fs_checked"] == 3
    assert inst["v0_only"]["path not in v1: no such file under root"] == 3
    assert "path not in v1: unchecked" not in inst["v0_only"]


def test_adjudication_classifies_the_groups(synth: Synth, tmp_path: Path) -> None:
    """A classified divergence is still a divergence: the class settles who
    is wrong, the bar it fails stays failed."""
    export = tmp_path / "export"
    _counts, done = project(
        synth.registry, export, root=synth.root, inject=Inject(upper_series_description=2, drop_lower_sop=1)
    )
    adj = tmp_path / "adj.toml"
    adj.write_text(
        CLEAN_ADJUDICATION
        + """
[[divergence]]
level = "series"
field = "series_description"
pattern = "case"
class = "v0-bug"
note = "v0 upper-cased it"

[[instance]]
side = "v1-only"
pattern = "resume skip*"
class = "accepted"
note = "v0's resume rule skipped it"
""",
        encoding="utf-8",
    )
    code, rep = _compare(synth, export, tmp_path, "--adjudication", str(adj))
    assert rep["unclassified"] == 0
    assert _bar(rep, "12.3 every divergence is classified")["passed"]
    assert rep["instance_classes"] == {"v0-only": {}, "v1-only": {"resume skip": "accepted"}}
    groups = [g for stat in rep["fields"] for g in stat["groups"] if stat["field"] == "series_description"]
    assert [(g["pattern"], g["count"], g["classification"], g["note"]) for g in groups] == [
        ("case", done["upper_series_description"], "v0-bug", "v0 upper-cased it")
    ]
    # a v1-only instance v0 skipped on resume is explained, so §12.2 holds;
    # the description bar depends on how many series the fixture has
    assert _bar(rep, "12.2 every v1 instance")["passed"]
    assert rep["instances"]["v1_only"] == {"resume skip": done["drop_lower_sop"]}
    md = (tmp_path / "report" / "report.md").read_text(encoding="utf-8")
    assert "v0 upper-cased it" in md and "**unclassified**" not in md
    assert code == (0 if rep["passed"] else 1)


def test_code_classes_with_key(synth: Synth, tmp_path: Path) -> None:
    """With v0's identifiers and the key, every v0 code is the keyed hash of
    the subject's PatientID, except the subjects identified by their study."""
    codes = _codes(synth)
    harvest_patient_ids(synth, codes)
    identifiers = {c: p for c, p in synth.patient_ids.items() if p}
    assert len(identifiers) == synth.manifest["subjects"] - synth.manifest["subjects_without_patient_id"]
    export = tmp_path / "export"
    project(synth.registry, export, root=synth.root, inject=Inject(identifiers=identifiers))
    key_file = tmp_path / "key"
    key_file.write_text(synth.key + "\n", encoding="utf-8")
    key_file.chmod(0o600)
    _code, rep = _compare(synth, export, tmp_path, "--key-file", str(key_file))
    expected = {"key-consistent": len(identifiers)}
    if len(codes) > len(identifiers):
        expected["no identifier"] = len(codes) - len(identifiers)
    assert rep["subjects"]["code_classes"] == expected
    # the wrong key: the same identifiers hash elsewhere
    key_file.write_text("not-the-key\n", encoding="utf-8")
    _code, rep = _compare(synth, export, tmp_path, "--key-file", str(key_file))
    assert rep["subjects"]["code_classes"].get("other", 0) == len(identifiers)
    # neither the key nor an identifier reaches the reports
    for text in (json.dumps(rep), (tmp_path / "report" / "report.md").read_text(encoding="utf-8")):
        assert synth.key not in text
        assert not any(p in text for p in identifiers.values())


def test_linkage_csv_round_trip(synth: Synth, tmp_path: Path) -> None:
    """The CSV `linkage-csv` writes is what `nils linkage import` reads; the
    pairs v0 held are the pairs the digest filed, so the import changes
    nothing, and a collision is counted or dropped."""
    codes = _codes(synth)
    harvest_patient_ids(synth, codes)
    identifiers = {c: p for c, p in synth.patient_ids.items() if p}
    assert len(identifiers) >= 1
    export = tmp_path / "export"
    project(synth.registry, export, root=synth.root, inject=Inject(identifiers=identifiers))
    v0 = tmp_path / "v0.duckdb"
    assert cli.main(["extract", "--export", str(export), "--out", str(v0)]) == 0
    out = tmp_path / "ids.csv"
    assert cli.main(["linkage-csv", "--v0", str(v0), "--cohort", "synth", "--out", str(out)]) == 0
    assert oct(out.stat().st_mode & 0o777) == "0o600"
    with out.open(newline="", encoding="utf-8") as fh:
        rows = list(csv.DictReader(fh))
    assert {r["code"]: r["identifier"] for r in rows} == identifiers
    result = synth.run("linkage", "import", str(out), "--id-type", "patient-id")
    m = re.search(
        r"imported (\d+) row\(s\) as patient-id: (\d+) subject\(s\) created, (\d+) identifier\(s\) filed, "
        r"(\d+) already filed",
        result.stdout,
    )
    assert m and [int(x) for x in m.groups()] == [len(identifiers), 0, 0, len(identifiers)], result.stdout
    out.unlink()

    # an identifier under two codes is counted, and dropped on request
    first, second = codes[0], codes[-1]
    assert first != second and first in identifiers
    con = duckdb.connect()
    con.execute(f"ATTACH '{v0}' AS v0db")
    con.execute(
        "INSERT INTO v0db.v0.subject_other_identifiers "
        "SELECT 9999, s.subject_id, 1, ? FROM v0db.v0.subject s WHERE s.subject_code = ?",
        [identifiers[first], second],
    )
    con.close()
    assert cli.main(["linkage-csv", "--v0", str(v0), "--out", str(out), "--drop-collisions"]) == 0
    with out.open(newline="", encoding="utf-8") as fh:
        kept = {r["code"] for r in csv.DictReader(fh)}
    assert first not in kept and second not in kept
    assert kept == set(identifiers) - {first, second}
    assert cli.main(["linkage-csv", "--v0", str(v0), "--list-id-types"]) == 0
    assert cli.main(["linkage-csv", "--v0", str(v0), "--out", str(out), "--id-type", "nope"]) == 2


def _drop_schemas(dsn: str, *schemas: str) -> None:
    """Leave the test database as it was: the registry schema and its
    linkage schema go."""
    con = duckdb.connect()
    con.execute("INSTALL postgres; LOAD postgres")
    con.execute(f"ATTACH '{dsn}' AS pg (TYPE postgres)")
    for schema in schemas:
        con.execute(f"CALL postgres_execute('pg', 'DROP SCHEMA IF EXISTS {schema} CASCADE')")
    con.close()


@pytest.mark.skipif(not os.environ.get("NILS_TEST_POSTGRES_DSN"), reason="NILS_TEST_POSTGRES_DSN is not set")
def test_postgres_registry(tmp_path: Path, nils_bin: Path, corpus_bin: Path) -> None:
    """The same run against a v1 registry on Postgres: DATE, TIME and JSONB
    columns come through the scanner and normalize like SQLite's text."""
    dsn = os.environ["NILS_TEST_POSTGRES_DSN"]
    schema = "v0cmp_test"
    _drop_schemas(dsn, schema, f"{schema}_linkage")
    pg = make_synth(tmp_path, nils_bin, corpus_bin, backend="postgres", dsn=dsn, schema=schema, instances=600)
    try:
        # the projection reads a SQLite registry: digest the same corpus
        # into one under the same key, then compare v1-on-Postgres with it
        lite = tmp_path / "lite"
        lite.mkdir()
        key_file = tmp_path / "k"
        key_file.write_text(pg.key + "\n", encoding="utf-8")
        key_file.chmod(0o600)
        sqlite = Synth(root=pg.root, registry=lite, key=pg.key, manifest=pg.manifest, nils=nils_bin)
        sqlite.run("key", "add", "test", "--from-file", str(key_file))
        key_file.unlink()
        sqlite.run("init", "--backend", "sqlite", "--scheme", "blake2b-8", "--key", "test")
        sqlite.run("digest", str(pg.root), "--files", "dcm,no-ext")
        export = tmp_path / "export"
        project(lite, export, root=pg.root)
        adj = tmp_path / "adj.toml"
        adj.write_text(CLEAN_ADJUDICATION, encoding="utf-8")
        code, rep = _compare(pg, export, tmp_path, "--adjudication", str(adj))
        assert rep["v1_backend"] == "postgres"
        assert code == 0, _failed(rep)
        assert rep["instances"]["common"] == pg.manifest["instances"]
        # the two digests walked in their own order: the series columns that
        # carry the first instance's value differ, and are excused as such
        for stat in rep["fields"]:
            key = (stat["level"], stat["field"])
            if key == ("study", "modalities_in_study"):
                continue
            if key in ORDER_DEPENDENT:
                assert stat["excused"] == stat["differ"] + stat["one_null"]
                assert all(g["classification"] == "accepted" for g in stat["groups"])
                continue
            assert stat["differ"] == 0 and stat["one_null"] == 0, (*key, stat["groups"])
    finally:
        _drop_schemas(dsn, schema, f"{schema}_linkage")
