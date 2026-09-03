# SPDX-License-Identifier: AGPL-3.0-only
"""The v1 side: a registry home (`nils.toml` next to `registry.db`, or a DSN)
attached read-only through DuckDB's SQLite or Postgres scanner, and the rows
under the compared root copied into the work database, typed by the
catalogue's converters. Nothing here writes to the registry."""

from __future__ import annotations

import os
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

import duckdb

from .catalogue import Field

CONFIG_FILE = "nils.toml"
REGISTRY_DB = "registry.db"
DSN_ENV = "NILS_DSN"
DEFAULT_SCHEMA = "nils"

#: The DuckDB type of a column, by the catalogue's converter (§6.3): dates,
#: times and JSON stay text and are normalized before comparing.
DUCK_TYPES: dict[str, str] = {
    "text": "VARCHAR",
    "int": "BIGINT",
    "double": "DOUBLE",
    "date": "VARCHAR",
    "time": "VARCHAR",
    "json": "VARCHAR",
}


@dataclass(frozen=True)
class Registry:
    backend: str
    #: `registry.db` on SQLite
    path: Path | None = None
    #: the DSN on Postgres; never printed
    dsn: str | None = None
    schema: str = DEFAULT_SCHEMA

    @property
    def prefix(self) -> str:
        """The qualified prefix of the registry's tables once attached."""
        return "v1" if self.backend == "sqlite" else f"v1.{self.schema}"

    def describe(self) -> str:
        """The registry for a report: the home or the backend, never a DSN."""
        if self.backend == "sqlite":
            return str(self.path)
        return f"postgres schema {self.schema}"


def from_home(home: Path) -> Registry:
    """The registry `home` names, as `nils.toml` describes it; `NILS_DSN`
    overrides the DSN as it does for `nils` itself."""
    config = home / CONFIG_FILE
    if not config.is_file():
        raise FileNotFoundError(f"{home}: no {CONFIG_FILE}; is this a registry home?")
    with config.open("rb") as fh:
        parsed = tomllib.load(fh)
    backend = parsed.get("backend")
    if backend not in ("sqlite", "postgres"):
        raise ValueError(f"{config}: backend must be sqlite or postgres, not {backend!r}")
    schema = parsed.get("schema", DEFAULT_SCHEMA)
    if backend == "sqlite":
        path = home / REGISTRY_DB
        if not path.is_file():
            raise FileNotFoundError(f"{home}: no {REGISTRY_DB}")
        return Registry("sqlite", path=path, schema=schema)
    dsn = os.environ.get(DSN_ENV) or parsed.get("dsn")
    if not dsn:
        raise ValueError(f"{config}: a postgres registry needs a dsn (or {DSN_ENV})")
    return Registry("postgres", dsn=dsn, schema=schema)


def from_dsn(dsn: str, schema: str = DEFAULT_SCHEMA) -> Registry:
    return Registry("postgres", dsn=dsn, schema=schema)


def attach(con: duckdb.DuckDBPyConnection, registry: Registry) -> None:
    """Attach the registry as `v1`, read-only. A SQLite registry is read by
    its declared types (INTEGER, REAL, TEXT), not as text: the writer binds
    every value typed by the catalogue, and a text read would render every
    REAL through SQLite's 15-significant-digit conversion, so a double that
    needs 16 or 17 digits (a float32 widened, as B1rms is) would no longer
    meet the same value spelled by v0."""
    if registry.backend == "sqlite":
        con.execute("INSTALL sqlite; LOAD sqlite")
        con.execute("SET sqlite_all_varchar = false")
        con.execute(f"ATTACH {quote(str(registry.path))} AS v1 (TYPE sqlite, READ_ONLY)")
    else:
        con.execute("INSTALL postgres; LOAD postgres")
        con.execute(f"ATTACH {quote(registry.dsn)} AS v1 (TYPE postgres, READ_ONLY)")


def quote(value: str) -> str:
    """A SQL string literal; ATTACH takes no parameters."""
    return "'" + value.replace("'", "''") + "'"


def _log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def _cast(column: str, converter: str) -> str:
    return f"CAST({column} AS {DUCK_TYPES[converter]}) AS {column}"


def _fields(fields: list[Field]) -> str:
    return ", ".join(_cast(f.column, f.converter) for f in fields)


def materialize(
    con: duckdb.DuckDBPyConnection,
    registry: Registry,
    catalogue: dict[str, list[Field]],
    root: Path | None,
) -> dict[str, int]:
    """Copy the registry's rows under `root` (every source when none) into
    schema `w` of the work database: the sources, their files, the instances
    those files hold, and the series, studies, subjects and stacks of those
    instances; plus every subject code the registry has, for §12.4."""
    p = registry.prefix
    con.execute("CREATE SCHEMA IF NOT EXISTS w")
    if root is None:
        con.execute(
            f"CREATE OR REPLACE TABLE w.source AS "
            f"SELECT CAST(id AS BIGINT) AS id, root, root_canonical FROM {p}.source"
        )
    else:
        wanted = str(root)
        canonical = str(root.resolve()) if root.exists() else wanted
        con.execute(
            f"CREATE OR REPLACE TABLE w.source AS "
            f"SELECT CAST(id AS BIGINT) AS id, root, root_canonical FROM {p}.source "
            f"WHERE root_canonical = ? OR root = ?",
            [canonical, wanted],
        )
    counts: dict[str, int] = {}
    counts["source"] = con.execute("SELECT count(*) FROM w.source").fetchone()[0]
    if counts["source"] == 0:
        raise LookupError(f"{registry.describe()}: no source matches the root")
    _log(f"v1: {counts['source']} source(s) in scope")

    con.execute(
        f"CREATE OR REPLACE TABLE w.source_file AS "
        f"SELECT CAST(f.id AS BIGINT) AS id, CAST(f.source_id AS BIGINT) AS source_id, "
        f"f.path, f.status, f.reason, CAST(f.instance_id AS BIGINT) AS instance_id "
        f"FROM {p}.source_file f WHERE CAST(f.source_id AS BIGINT) IN (SELECT id FROM w.source)"
    )
    con.execute("CREATE INDEX IF NOT EXISTS w_source_file_path ON w.source_file (path)")
    counts["source_file"] = con.execute("SELECT count(*) FROM w.source_file").fetchone()[0]
    _log(f"v1: {counts['source_file']:,} file(s)")

    con.execute(
        f"CREATE OR REPLACE TABLE w.instance AS "
        f"SELECT CAST(i.id AS BIGINT) AS id, i.sop_instance_uid, "
        f"CAST(i.series_id AS BIGINT) AS series_id, CAST(i.stack_id AS BIGINT) AS stack_id, "
        f"CAST(i.source_file_id AS BIGINT) AS source_file_id, {_fields(catalogue['instance'])} "
        f"FROM {p}.instance i WHERE CAST(i.id AS BIGINT) IN "
        f"(SELECT instance_id FROM w.source_file WHERE instance_id IS NOT NULL)"
    )
    con.execute("CREATE INDEX IF NOT EXISTS w_instance_sop ON w.instance (sop_instance_uid)")
    counts["instance"] = con.execute("SELECT count(*) FROM w.instance").fetchone()[0]
    _log(f"v1: {counts['instance']:,} instance(s)")

    con.execute(
        f"CREATE OR REPLACE TABLE w.series AS "
        f"SELECT CAST(s.id AS BIGINT) AS id, s.series_instance_uid, "
        f"CAST(s.study_id AS BIGINT) AS study_id, CAST(s.subject_id AS BIGINT) AS subject_id, "
        f"CAST(s.n_instances AS BIGINT) AS n_instances, CAST(s.n_stacks AS BIGINT) AS n_stacks, "
        f"{_fields(catalogue['series'])} "
        f"FROM {p}.series s WHERE CAST(s.id AS BIGINT) IN (SELECT DISTINCT series_id FROM w.instance)"
    )
    con.execute("CREATE INDEX IF NOT EXISTS w_series_uid ON w.series (series_instance_uid)")
    counts["series"] = con.execute("SELECT count(*) FROM w.series").fetchone()[0]
    for level in ("series_mr", "series_ct", "series_pet"):
        con.execute(
            f"CREATE OR REPLACE TABLE w.{level} AS "
            f"SELECT CAST(d.series_id AS BIGINT) AS series_id, {_fields(catalogue[level])} "
            f"FROM {p}.{level} d WHERE CAST(d.series_id AS BIGINT) IN (SELECT id FROM w.series)"
        )
        counts[level] = con.execute(f"SELECT count(*) FROM w.{level}").fetchone()[0]
    _log(
        f"v1: {counts['series']:,} series ({counts['series_mr']:,} MR, "
        f"{counts['series_ct']:,} CT, {counts['series_pet']:,} PET)"
    )

    con.execute(
        f"CREATE OR REPLACE TABLE w.stack AS "
        f"SELECT CAST(k.id AS BIGINT) AS id, CAST(k.series_id AS BIGINT) AS series_id, "
        f"CAST(k.stack_index AS BIGINT) AS stack_index, k.stack_key, k.modality, k.orientation, "
        f"CAST(k.n_instances AS BIGINT) AS n_instances, {_fields(catalogue['stack'])} "
        f"FROM {p}.stack k WHERE CAST(k.series_id AS BIGINT) IN (SELECT id FROM w.series)"
    )
    counts["stack"] = con.execute("SELECT count(*) FROM w.stack").fetchone()[0]
    _log(f"v1: {counts['stack']:,} stack(s)")

    con.execute(
        f"CREATE OR REPLACE TABLE w.study AS "
        f"SELECT CAST(t.id AS BIGINT) AS id, t.study_instance_uid, "
        f"CAST(t.subject_id AS BIGINT) AS subject_id, {_fields(catalogue['study'])} "
        f"FROM {p}.study t WHERE CAST(t.id AS BIGINT) IN (SELECT DISTINCT study_id FROM w.series)"
    )
    con.execute("CREATE INDEX IF NOT EXISTS w_study_uid ON w.study (study_instance_uid)")
    counts["study"] = con.execute("SELECT count(*) FROM w.study").fetchone()[0]

    con.execute(
        f"CREATE OR REPLACE TABLE w.subject AS "
        f"SELECT CAST(u.id AS BIGINT) AS id, u.code, {_fields(catalogue['subject'])} "
        f"FROM {p}.subject u WHERE CAST(u.id AS BIGINT) IN (SELECT DISTINCT subject_id FROM w.study)"
    )
    counts["subject"] = con.execute("SELECT count(*) FROM w.subject").fetchone()[0]
    # every code the registry knows, whatever root it came from
    con.execute(
        f"CREATE OR REPLACE TABLE w.subject_all AS SELECT CAST(id AS BIGINT) AS id, code FROM {p}.subject"
    )
    counts["subject_all"] = con.execute("SELECT count(*) FROM w.subject_all").fetchone()[0]
    _log(f"v1: {counts['study']:,} studies, {counts['subject']:,} subjects in scope, {counts['subject_all']:,} in all")
    return counts


def session_scheme(con: duckdb.DuckDBPyConnection, registry: Registry) -> str | None:
    """The registry's session scheme, as written in `registry_meta`."""
    row = con.execute(
        f"SELECT value FROM {registry.prefix}.registry_meta WHERE key = 'session_scheme'"
    ).fetchone()
    return None if row is None else row[0]
