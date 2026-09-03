# SPDX-License-Identifier: AGPL-3.0-only
"""Normal forms (§6.3) as DuckDB macros, applied to both sides before a value
is compared, so that what v0 stored as `str()` of a pydicom value and what v1
stored as the converter's text meet on the same form:

* text: a multi-valued literal (`['a', 'b']`, v0) or a backslash-joined
  string (`a\\b`, v1) becomes its parts, each trimmed of spaces and NULs;
  a part that reads as a number is written the way DuckDB writes it (`1.0`
  and `1.00` meet, `01` stays `01`, a plain integer stays text so that long
  digit strings never collide); the parts are joined by a backslash again;
  the empty string is null;
* date: `YYYYMMDD` becomes `YYYY-MM-DD`;
* time: `HHMMSS[.f]` becomes `HH:MM:SS[.f]`, trailing zeros of a fraction
  and a trailing dot dropped;
* numbers stay numbers (`int`, `double`) and compare exactly, except where
  v0 stored a rounded value (`mapping.Level.decimals`), where either
  rounding of v1's value counts as agreement;
* json: compared for presence only; the two representations (Python's
  `str()` of a sequence in v0, JSON in v1) do not meet.
"""

from __future__ import annotations

import duckdb

MACROS = r"""
CREATE OR REPLACE MACRO canon_token(x) AS
    CASE
        WHEN x IS NULL THEN NULL
        WHEN regexp_matches(x, '^[+-]?[0-9]+$') THEN regexp_replace(x, '^\+', '')
        WHEN regexp_matches(x, '^[+-]?([0-9]+\.[0-9]*|\.[0-9]+)([eE][+-]?[0-9]+)?$')
             OR regexp_matches(x, '^[+-]?[0-9]+[eE][+-]?[0-9]+$')
            THEN coalesce(CAST(TRY_CAST(x AS DOUBLE) AS VARCHAR), x)
        ELSE x
    END;

CREATE OR REPLACE MACRO norm_list(v) AS
    CASE
        WHEN v IS NULL THEN NULL
        WHEN regexp_matches(v, '^\[.*\]$')
            THEN list_transform(
                regexp_split_to_array(v[2:-2], ',\s*'),
                x -> trim(x, ' ' || chr(39) || chr(34)))
        ELSE string_split(v, '\')
    END;

CREATE OR REPLACE MACRO norm_text(v) AS
    NULLIF(
        array_to_string(
            list_transform(norm_list(v), x -> canon_token(trim(x, ' ' || chr(0)))),
            '\'),
        '');

CREATE OR REPLACE MACRO norm_date(v) AS
    CASE
        WHEN v IS NULL THEN NULL
        WHEN regexp_matches(v, '^[0-9]{8}$') THEN v[1:4] || '-' || v[5:6] || '-' || v[7:8]
        ELSE NULLIF(trim(v), '')
    END;

CREATE OR REPLACE MACRO norm_time(v) AS
    CASE
        WHEN v IS NULL THEN NULL
        WHEN regexp_matches(v, '^[0-9]{6}(\.[0-9]+)?$')
            THEN regexp_replace(regexp_replace(
                v[1:2] || ':' || v[3:4] || ':' || v[5:6] || v[7:],
                '(\.[0-9]*?)0+$', '\1'), '\.$', '')
        WHEN regexp_matches(v, '^[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?$')
            THEN regexp_replace(regexp_replace(v, '(\.[0-9]*?)0+$', '\1'), '\.$', '')
        ELSE NULLIF(trim(v), '')
    END;

CREATE OR REPLACE MACRO norm_json(v) AS
    CASE WHEN v IS NULL OR trim(v) IN ('', '[]', '{}', 'None') THEN NULL ELSE '<json>' END;
"""

#: The macro that normalizes a column of a converter.
BY_CONVERTER: dict[str, str | None] = {
    "text": "norm_text",
    "date": "norm_date",
    "time": "norm_time",
    "json": "norm_json",
    "int": None,
    "double": None,
}


def install(con: duckdb.DuckDBPyConnection) -> None:
    for statement in MACROS.split(";\n"):
        if statement.strip():
            con.execute(statement)


def expression(column: str, converter: str) -> str:
    """The SQL that yields the normal form of `column`."""
    macro = BY_CONVERTER[converter]
    return column if macro is None else f"{macro}({column})"


def agree(a: str, b: str, converter: str, decimals: int | None) -> str:
    """The SQL predicate under which two normalized non-null values agree."""
    if converter == "double" and decimals is not None:
        # v0 rounded with Python's round (half to even), DuckDB rounds half
        # away from zero: either rounding of v1's value counts.
        tolerance = 0.5 * 10 ** (-decimals) + 1e-9
        return f"(round({b}, {decimals}) = {a} OR abs({b} - {a}) <= {tolerance!r})"
    return f"{a} = {b}"
