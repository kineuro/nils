# SPDX-License-Identifier: AGPL-3.0-only
"""The parts that need no corpus: the normalization macros, shapes and
patterns, the catalogue against the v0 mapping, the adjudication file and
the code classes."""

from __future__ import annotations

from pathlib import Path

import duckdb
import pytest

from v0compare import catalogue, classify, fields, keys, normalize, shapes
from v0compare.mapping import LEVELS, STACK_EXTRA
from v0compare.v0 import TABLES


@pytest.fixture(scope="module")
def con() -> duckdb.DuckDBPyConnection:
    c = duckdb.connect()
    normalize.install(c)
    return c


def _one(con: duckdb.DuckDBPyConnection, expr: str, value: object) -> object:
    return con.execute(f"SELECT {expr}", [value]).fetchone()[0]


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        # a Python list literal and a backslash string are the same list
        ("['-120.5', '-100', '50.0']", "-120.5\\-100\\50.0"),
        ("-120.5\\-100\\50.0", "-120.5\\-100\\50.0"),
        ("[1, 0, 0, 0, 1, 0]", "1\\0\\0\\0\\1\\0"),
        # numbers in canonical form, but only where they are numbers
        ("12.00", "12.0"),
        ("1e-5", "1e-05"),
        ("+12", "12"),
        ("007", "007"),
        ("T1 MPRAGE", "T1 MPRAGE"),
        ("  padded  ", "padded"),
        ("padded\x00", "padded"),
        # nothing is null
        ("", None),
        ("[]", None),
        (None, None),
    ],
)
def test_norm_text(con: duckdb.DuckDBPyConnection, value: object, expected: object) -> None:
    assert _one(con, "norm_text(?)", value) == expected


@pytest.mark.parametrize(
    ("value", "expected"),
    [("20190505", "2019-05-05"), ("2019-05-05", "2019-05-05"), ("", None), (None, None)],
)
def test_norm_date(con: duckdb.DuckDBPyConnection, value: object, expected: object) -> None:
    assert _one(con, "norm_date(?)", value) == expected


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        ("123015.500000", "12:30:15.5"),
        ("123015", "12:30:15"),
        ("12:30:15.000000", "12:30:15"),
        ("12:30:15.037000", "12:30:15.037"),
        ("12:30:15", "12:30:15"),
        ("", None),
    ],
)
def test_norm_time(con: duckdb.DuckDBPyConnection, value: object, expected: object) -> None:
    assert _one(con, "norm_time(?)", value) == expected


@pytest.mark.parametrize(
    ("value", "expected"),
    [('[{"00080100": {"vr": "SH"}}]', "<json>"), ("[]", None), ("{}", None), ("None", None), ("", None)],
)
def test_norm_json_is_presence_only(con: duckdb.DuckDBPyConnection, value: object, expected: object) -> None:
    assert _one(con, "norm_json(?)", value) == expected


def test_agree_with_decimals(con: duckdb.DuckDBPyConnection) -> None:
    # v0 rounded echo times to two decimals: 12.345 agrees with 12.35 and 12.34, not with 12.4
    expr = normalize.agree("a", "b", "double", 2)
    rows = con.execute(
        f"SELECT {expr} FROM (VALUES (12.35, 12.345), (12.34, 12.345), (12.4, 12.345), (2.0, 2.0)) t(a, b)"
    ).fetchall()
    assert [r[0] for r in rows] == [True, True, False, True]


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        (None, "null"),
        ("T1 MPRAGE 3D", "A9 AAAAAA 9A"),
        ("ORIGINAL\\PRIMARY", "AAAAAAAA\\AAAAAAA"),
        (12.5, "99.9"),
        (42, "99"),
        ("é", "a"),
        ("x" * 50, "a" * 40 + "…"),
    ],
)
def test_shape(value: object, expected: str) -> None:
    assert shapes.shape(value) == expected


@pytest.mark.parametrize(
    ("a", "b", "converter", "expected"),
    [
        (None, None, "text", "equal"),
        (None, "x", "text", "null↔value"),
        ("x", None, "text", "value↔null"),
        ("abc", "abc", "text", "equal"),
        ("ABC", "abc", "text", "case"),
        ("a b", "ab", "text", "whitespace"),
        ("abcdef", "abc", "text", "prefix"),
        ("1.50", "1.5", "text", "number-format"),
        ("1.5", "1.499", "text", "rounded"),
        ("1.5", "150", "text", "scale"),
        ("a\\b\\c", "c\\b\\a", "text", "list-order"),
        ("a\\b", "a\\b\\c", "text", "subset"),
        ("1.0\\2.0", "1.0000001\\2.0", "text", "rounded"),
        ("T1", "T2", "text", "A9↔A9"),
        (12.0, 12.0, "double", "number-format"),
        (12.35, 12.345, "double", "rounded"),
        (12.0, 1200.0, "double", "scale"),
        (12.0, 17.0, "double", "99.9↔99.9"),
    ],
)
def test_pattern(a: object, b: object, converter: str, expected: str) -> None:
    assert shapes.pattern(a, b, converter) == expected


def test_catalogue_maps_onto_v0() -> None:
    """Every catalogue column has a v0 column of a compatible type, or is
    declared absent; the comparison plan builds for all of them."""
    cat = catalogue.load()
    assert set(cat) == set(catalogue.LEVELS)
    for level_name, fs in cat.items():
        level = LEVELS[level_name]
        for f in [*fs, *(STACK_EXTRA if level_name == "stack" else ())]:
            v0 = level.v0_column(f.column)
            if v0 is None:
                assert f.column in level.absent
                continue
            assert v0 in TABLES[level.table], f"{level_name}.{f.column}: v0 column {v0} unknown"
        ps = fields.plans(level_name, fs)
        assert len(ps) == sum(1 for f in [*fs, *(STACK_EXTRA if level_name == "stack" else ())] if level.v0_column(f.column))
        for p in ps:
            assert p.kind in ("text", "int", "double", "date", "time", "json")
        # a field the mapping names must exist in the catalogue
        for column in [*level.renames, *level.absent, *level.decimals]:
            assert any(f.column == column for f in fs) or (level_name == "stack" and column == "orientation"), (
                f"{level_name}.{column}: in the mapping, not in the catalogue"
            )


def test_v0_tables_cover_the_levels() -> None:
    for level in LEVELS.values():
        assert level.table in TABLES


def test_adjudication(tmp_path: Path) -> None:
    toml = tmp_path / "adj.toml"
    toml.write_text(
        """
[[divergence]]
level = "series"
field = "image_type"
pattern = "list-*"
class = "accepted"
note = "v0 kept the literal"

[[divergence]]
pattern = "null↔value"
class = "v1-bug"

[[partition]]
pattern = "v0 1 stack(s), v1 *"
class = "v0-bug"

[[instance]]
side = "v1-only"
pattern = "resume skip*"
class = "accepted"
""",
        encoding="utf-8",
    )
    adj = classify.load(toml)
    assert adj.divergence("series", "image_type", "list-order").classification == "accepted"
    assert adj.divergence("series", "image_type", "case") is None
    assert adj.divergence("instance", "rows", "null↔value").classification == "v1-bug"
    assert adj.partition("v0 1 stack(s), v1 3, 0 matched").classification == "v0-bug"
    assert adj.partition("v0 2 stack(s), v1 1, 0 matched") is None
    assert adj.instance("v1-only", "resume skip: legacy token").classification == "accepted"
    assert adj.instance("v0-only", "resume skip") is None
    assert classify.load(None).rules == []


def test_adjudication_rejects_unknown_class(tmp_path: Path) -> None:
    toml = tmp_path / "bad.toml"
    toml.write_text('[[divergence]]\npattern = "*"\nclass = "maybe"\n', encoding="utf-8")
    with pytest.raises(ValueError, match="class must be one of"):
        classify.load(toml)


def test_code_classes() -> None:
    key = "not-a-real-key"
    code = keys.blake2b8("SYN0000001", key)
    assert len(code) == 16
    assert keys.classify_code(code, ["SYN0000001"], key, "synth") == "key-consistent"
    assert keys.classify_code(code, ["SYN0000002"], key, "synth") == "other"
    assert keys.classify_code(code, [], key, "synth") == "no identifier"
    cohort_code = keys.blake2b8("SYN0000001", keys.cohort_seed("synth"))
    assert keys.classify_code(cohort_code, ["SYN0000001"], key, "synth") == "cohort-hashed"
    assert keys.classify_code(cohort_code, ["SYN0000001"], None, None) == "other"
    assert keys.cohort_seed("  ") == "DEFAULT-SEED"


def test_read_key(tmp_path: Path) -> None:
    f = tmp_path / "k"
    f.write_text("abc\n", encoding="utf-8")
    f.chmod(0o600)
    assert keys.read_key(f) == "abc"
    f.write_text("", encoding="utf-8")
    with pytest.raises(ValueError, match="empty key"):
        keys.read_key(f)


def test_excused_rows_lift_the_agreement(tmp_path: Path) -> None:
    """A group classed accepted or v0-bug is excused from the floor; a v1
    bug and an unclassified group are not; a sampled residual scales."""
    from v0compare import report
    from v0compare.fields import FieldStat, Group

    stat = FieldStat("series", "series_description", "text", "quasi-identifying", "text", compared=1000, equal=900,
                     differ=100)
    stat.groups = [
        Group("series", "series_description", "case", 60, classification="v0-bug"),
        Group("series", "series_description", "whitespace", 30, classification="accepted"),
        Group("series", "series_description", "other", 10),
    ]
    stat.excuse(report.EXCUSED)
    assert stat.excused == 90 and stat.agreement == 0.99
    stat.groups[0].classification = "v1-bug"
    stat.excuse(report.EXCUSED)
    assert stat.excused == 30
    # sampled: 20 of 100 residual rows were read, 15 of them excused
    stat.sampled = 20
    stat.groups = [Group("series", "series_description", "case", 15, classification="accepted"),
                   Group("series", "series_description", "other", 5)]
    stat.excuse(report.EXCUSED)
    assert stat.excused == 75

    rep = report.Report()
    rep.fields = [
        FieldStat("series", "media_storage_sop_instance_uid", "text", "technical", "text", compared=8, equal=1, differ=7,
                  groups=[Group("series", "media_storage_sop_instance_uid", "9.9↔9.9", 7)]),
        FieldStat("series", "sop_class_uid", "text", "technical", "text", compared=8, equal=7, differ=1,
                  groups=[Group("series", "sop_class_uid", "9.9↔9.9", 1)]),
    ]
    rep.stacks.multi, rep.stacks.multi_identical = 10, 9
    rep.stacks.divergent = {"v0 2 stack(s), v1 1, 0 matched": 1}
    rep.instances.v1_only = {"unexplained: series known to v0": 3}
    toml = tmp_path / "adj.toml"
    toml.write_text(
        '[[partition]]\npattern = "v0 2 stack(s), v1 *"\nclass = "v0-bug"\n'
        '[[instance]]\nside = "v1-only"\npattern = "unexplained*"\nclass = "accepted"\nnote = "v0 skipped them"\n',
        encoding="utf-8",
    )
    report.adjudicate(rep, classify.load(toml))
    report.verdict(rep)
    # the order-dependent column is classed by the tool, the exact field is not
    assert rep.fields[0].groups[0].classification == "accepted" and rep.fields[0].excused == 7
    assert rep.fields[1].groups[0].classification is None and rep.fields[1].excused == 0
    assert rep.unclassified == 1
    bars = {b.name: b for b in rep.bars}
    assert not bars["12.3 the exact fields agree on every row"].passed
    assert bars["12.3 every other field agrees on 99.9% of rows"].passed
    assert rep.stacks.excused == 1 and bars["12.3 the stack partition is identical for 99.9% of multi-stack series"].passed
    assert bars["12.2 every v1 instance is in v0 or explained"].passed
    assert "3 excused" in bars["12.2 every v1 instance is in v0 or explained"].detail
    assert not bars["12.3 every divergence is classified"].passed
    assert '"passed": false' in report.to_json(rep)


def test_stack_defining_pins_the_thirteen() -> None:
    """The series-level columns a stack signature is also made of, derived
    from the catalogue as the engine derives them (spec §8): thirteen."""
    cols = catalogue.stack_defining(catalogue.load())
    assert len(cols) == 13
    assert {("series", "image_type"), ("series", "image_orientation_patient")} <= cols
    assert sum(1 for level, _ in cols if level == "series_mr") == 7
    assert {("series_ct", "kvp"), ("series_ct", "x_ray_tube_current"), ("series_ct", "exposure")} <= cols
    assert ("series_pet", "series_type") in cols
    assert not any(level in ("study", "subject", "instance", "stack") for level, _ in cols)


def test_multi_stack_groups_are_the_tools_own(tmp_path: Path) -> None:
    """A stack-signature column that differs in a multi-stack series is
    `accepted` by the tool itself; the same pattern in a single-stack series
    is not, and the file's rule wins over both."""
    from v0compare import report
    from v0compare.fields import FieldStat, Group
    from v0compare.mapping import MULTI_STACK, MULTI_STACK_NOTE

    def fresh() -> report.Report:
        rep = report.Report()
        rep.fields = [
            FieldStat("series_mr", "echo_time", "double", "technical", "double", compared=10, equal=7, differ=3,
                      groups=[Group("series_mr", "echo_time", "rounded" + MULTI_STACK, 2),
                              Group("series_mr", "echo_time", "rounded", 1)]),
        ]
        return rep

    rep = fresh()
    report.adjudicate(rep, classify.load(None))
    multi, single = rep.fields[0].groups
    assert (multi.classification, multi.note) == ("accepted", MULTI_STACK_NOTE)
    assert single.classification is None
    assert rep.fields[0].excused == 2 and rep.unclassified == 1

    toml = tmp_path / "adj.toml"
    toml.write_text(
        '[[divergence]]\nlevel = "series_mr"\nfield = "echo_time"\npattern = "rounded*"\nclass = "v1-bug"\n'
        'note = "not this time"\n',
        encoding="utf-8",
    )
    rep = fresh()
    report.adjudicate(rep, classify.load(toml))
    assert [g.classification for g in rep.fields[0].groups] == ["v1-bug", "v1-bug"]
    assert rep.fields[0].excused == 0 and rep.unclassified == 0


def test_sqlite_registry_is_read_typed(tmp_path: Path) -> None:
    """A REAL column reaches DuckDB as the double SQLite holds, not through
    its 15-digit text: a float32 widened to f64 (B1rms) keeps its 17
    digits, and so meets v0's `str()` of the same value."""
    import sqlite3

    from v0compare import v1

    path = tmp_path / "registry.db"
    db = sqlite3.connect(path)
    db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, x REAL, n INTEGER, s TEXT)")
    value = 0.30000001192092896  # float32 0.3, widened
    db.execute("INSERT INTO t VALUES (1, ?, 7, 'a')", (value,))
    db.commit()
    db.close()
    con = duckdb.connect()
    v1.attach(con, v1.Registry("sqlite", path=path))
    x, n, s = con.execute("SELECT x, n, s FROM v1.t").fetchone()
    assert x == value and isinstance(n, int) and s == "a"
    assert con.execute("SELECT CAST(x AS VARCHAR) FROM v1.t").fetchone()[0] == repr(value)
    con.close()
    # what the text read would have made of it
    con = duckdb.connect()
    con.execute("INSTALL sqlite; LOAD sqlite; SET sqlite_all_varchar = true")
    con.execute(f"ATTACH '{path}' AS t2 (TYPE sqlite, READ_ONLY)")
    assert con.execute("SELECT x FROM t2.t").fetchone()[0] == "0.300000011920929"
    con.close()


def _axes_fixture(con: duckdb.DuckDBPyConnection, rows: list[tuple]) -> None:
    """A pair table, v0's verdicts and v1's rows, in the shape `axes` reads."""
    con.execute("CREATE SCHEMA IF NOT EXISTS w")
    con.execute("CREATE SCHEMA IF NOT EXISTS v1")
    con.execute("ATTACH ':memory:' AS v0db")
    con.execute("CREATE SCHEMA IF NOT EXISTS v0db.v0")
    con.execute("CREATE TABLE w.stack_pair (series_instance_uid VARCHAR, v0_id BIGINT, v1_id BIGINT, n BIGINT)")
    con.execute(
        "CREATE TABLE v0db.v0.series_classification_cache ("
        "series_stack_id BIGINT, directory_type VARCHAR, base VARCHAR, technique VARCHAR, "
        "modifier_csv VARCHAR, construct_csv VARCHAR, provenance VARCHAR, acceleration_csv VARCHAR, "
        "body_part VARCHAR, post_contrast VARCHAR, localizer VARCHAR, spinal_cord VARCHAR, "
        "manual_review_required VARCHAR, manual_review_reasons_csv VARCHAR, dicom_origin_cohort VARCHAR)"
    )
    con.execute("CREATE TABLE v1.classification (stack_id BIGINT)")
    con.execute("CREATE TABLE v1.classification_axis (stack_id BIGINT, axis VARCHAR, value VARCHAR)")
    for i, (v0_base, v0_dt, v1_base, v1_dt) in enumerate(rows, start=1):
        con.execute("INSERT INTO w.stack_pair VALUES (?, ?, ?, 1)", [f"1.2.{i}", i, i])
        con.execute(
            "INSERT INTO v0db.v0.series_classification_cache "
            "(series_stack_id, base, directory_type) VALUES (?, ?, ?)",
            [i, v0_base, v0_dt],
        )
        con.execute("INSERT INTO v1.classification VALUES (?)", [i])
        con.execute("INSERT INTO v1.classification_axis VALUES (?, 'base', ?)", [i, v1_base])
        con.execute("INSERT INTO v1.classification_axis VALUES (?, 'directory_type', ?)", [i, v1_dt])


def test_axes_group_their_differences_and_name_the_two_that_are_never_allowed() -> None:
    """§11.1: a difference is a group to be classified, but an axis v1 leaves
    unresolved and a stack v1 excludes are bars of their own."""
    from v0compare import axes

    con = duckdb.connect()
    _axes_fixture(
        con,
        [
            ("T1w", "anat", "T1w", "anat"),  # agrees
            ("T2w", "anat", "PDw", "anat"),  # a difference to classify
            ("T2w", "anat", "PDw", "anat"),  # the same one, so one group
            ("T1w", "anat", "", "anat"),  # v1 silent where v0 spoke
            ("T1w", "anat", "T1w", "excluded"),  # v1 excludes what v0 kept
            ("", "anat", "T2w", "anat"),  # v1 filled a gap, which is allowed
        ],
    )
    rep = axes.compare(con)
    assert rep.stacks == 6
    base = next(a for a in rep.axes if a.axis == "base")
    assert base.compared == 6
    assert base.agreed == 2
    assert base.v1_silent == 1
    assert base.v0_silent == 1
    assert [(g.pattern, g.count) for g in base.groups][0] == ("v0=T2w v1=PDw", 2)
    assert rep.excluded_by_v1 == 1
    con.close()


def test_an_axis_difference_without_a_cause_is_refused(tmp_path: Path) -> None:
    """§11.5: a class is where the reading of a difference is filed, not where
    it ends."""
    from v0compare import classify as adjudication

    path = tmp_path / "a.toml"
    path.write_text(
        '[[axis]]\naxis = "base"\npattern = "v0=T2w v1=PDw"\nclass = "accepted"\n',
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="no cause"):
        adjudication.load(path)

    path.write_text(
        '[[axis]]\naxis = "base"\npattern = "v0=T2w v1=PDw"\nclass = "accepted"\n'
        'cause = "v0 reads the echo train length, the pack reads the sequence variant"\n',
        encoding="utf-8",
    )
    rule = adjudication.load(path).axis("base", "v0=T2w v1=PDw")
    assert rule is not None and rule.classification == "accepted"
