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
