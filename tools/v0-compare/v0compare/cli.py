# SPDX-License-Identifier: AGPL-3.0-only
"""`v0-compare`: `extract` a v0 registry into a DuckDB file, write the
`linkage-csv` that carries v0's identifiers into v1 before the gate digest,
and `compare` the two registries into a report."""

from __future__ import annotations

import argparse
import csv
import sys
from pathlib import Path

import duckdb

from . import catalogue, classify, fields, instances, keys, normalize, report, stacks, subjects, v0, v1
from .mapping import V0_FILE_MODES


def _log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def _threads(con: duckdb.DuckDBPyConnection, threads: int | None) -> None:
    if threads:
        con.execute(f"SET threads = {int(threads)}")


def cmd_extract(args: argparse.Namespace) -> int:
    out = Path(args.out)
    if args.export:
        counts = v0.from_export(Path(args.export), out, threads=args.threads)
    else:
        counts = v0.from_dsn(args.dsn, out, threads=args.threads)
    _log(f"wrote {out}: " + ", ".join(f"{t} {n:,}" for t, n in counts.items()))
    return 0


def cmd_linkage_csv(args: argparse.Namespace) -> int:
    con = v0.open_readonly(Path(args.v0))
    types = con.execute(
        "SELECT t.id_type_name, count(o.subject_other_identifier_id) FROM v0db.v0.id_types t "
        "LEFT JOIN v0db.v0.subject_other_identifiers o ON o.id_type_id = t.id_type_id GROUP BY 1 ORDER BY 2 DESC"
    ).fetchall()
    if args.list_id_types or not types:
        for name, n in types:
            print(f"{name}\t{n:,}")
        return 0 if types else 1
    id_type = args.id_type or types[0][0]
    if id_type not in {t[0] for t in types}:
        _log(f"no v0 id type named {id_type!r}; --list-id-types shows them")
        return 2
    cohort_filter = ""
    params: list[object] = [id_type]
    if args.cohort:
        cohort_filter = (
            " AND s.subject_id IN (SELECT sc.subject_id FROM v0db.v0.subject_cohorts sc "
            "JOIN v0db.v0.cohort c ON c.cohort_id = sc.cohort_id WHERE c.name = ?)"
        )
        params.append(args.cohort)
    rows = con.execute(
        "SELECT o.other_identifier, s.subject_code FROM v0db.v0.subject_other_identifiers o "
        "JOIN v0db.v0.subject s ON s.subject_id = o.subject_id "
        "JOIN v0db.v0.id_types t ON t.id_type_id = o.id_type_id "
        f"WHERE t.id_type_name = ?{cohort_filter} "
        "ORDER BY s.subject_id",
        params,
    ).fetchall()
    con.close()
    # an identifier under two codes, or a code under two identifiers,
    # would fault the import; count them, drop them when asked
    by_identifier: dict[str, set[str]] = {}
    by_code: dict[str, set[str]] = {}
    for identifier, code in rows:
        identifier = (identifier or "").strip()
        code = (code or "").strip()
        if not identifier or not code:
            continue
        by_identifier.setdefault(identifier, set()).add(code)
        by_code.setdefault(code, set()).add(identifier)
    colliding_ids = {i for i, codes in by_identifier.items() if len(codes) > 1}
    colliding_codes = {c for c, ids in by_code.items() if len(ids) > 1}
    kept = 0
    written: set[tuple[str, str]] = set()
    out = Path(args.out)
    with out.open("w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh)
        w.writerow(["identifier", "code"])
        for identifier, code in rows:
            identifier = (identifier or "").strip()
            code = (code or "").strip()
            if not identifier or not code or (identifier, code) in written:
                continue
            if args.drop_collisions and (identifier in colliding_ids or code in colliding_codes):
                continue
            written.add((identifier, code))
            w.writerow([identifier, code])
            kept += 1
    try:
        out.chmod(0o600)
    except OSError:
        pass
    _log(
        f"{out}: {kept:,} row(s) of id type {id_type}; {len(colliding_ids):,} identifier(s) under several codes, "
        f"{len(colliding_codes):,} code(s) under several identifiers"
        + (" (dropped)" if args.drop_collisions else " (kept; the import will refuse them)")
    )
    _log("the file holds identifiers: import it, then delete it")
    return 0


def cmd_compare(args: argparse.Namespace) -> int:
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    cat = catalogue.load(Path(args.catalogue) if args.catalogue else None)
    adj = classify.load(Path(args.adjudication) if args.adjudication else None)
    key = keys.read_key(Path(args.key_file)) if args.key_file else None
    if args.v1_dsn:
        registry = v1.from_dsn(args.v1_dsn, args.v1_schema or v1.DEFAULT_SCHEMA)
    else:
        registry = v1.from_home(Path(args.v1))
    root = Path(args.root) if args.root else None

    work = out_dir / "work.duckdb"
    if work.exists():
        work.unlink()
    con = duckdb.connect(str(work))
    _threads(con, args.threads)
    normalize.install(con)
    con.execute(f"ATTACH {v1.quote(str(Path(args.v0)))} AS v0db (READ_ONLY)")
    origin = con.execute("SELECT kind FROM v0db.v0.origin").fetchone()
    v1.attach(con, registry)

    rep = report.Report()
    rep.v0_origin = origin[0] if origin else "unknown"
    rep.v1_backend = registry.backend
    rep.cohort = args.cohort
    rep.v0_files = args.v0_files
    rep.root_given = root is not None
    rep.v1_counts = v1.materialize(con, registry, cat, root)
    rep.instances = instances.compare(con, args.cohort, args.v0_files, root, not args.no_fs)
    rep.stacks = stacks.compare(con)
    for level in catalogue.LEVELS:
        n, stats = fields.compare_level(con, level, cat[level], args.sample_cap)
        rep.pairs[level] = n
        rep.fields += stats
    rep.subjects = subjects.compare(con, args.cohort, key, classify=key is not None or args.cohort is not None)
    del key
    report.adjudicate(rep, adj)
    report.verdict(rep)
    (out_dir / "report.json").write_text(report.to_json(rep), encoding="utf-8")
    (out_dir / "report.md").write_text(report.to_markdown(rep), encoding="utf-8")
    con.close()
    for b in rep.bars:
        _log(f"{'pass' if b.passed else 'FAIL'}  {b.name}: {b.detail}")
    _log(f"wrote {out_dir / 'report.md'} ({'PASS' if rep.passed else 'FAIL'}); work.duckdb holds values, keep it private")
    return 0 if rep.passed else 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="v0-compare", description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("extract", help="load a v0 registry into a DuckDB file")
    src = p.add_mutually_exclusive_group(required=True)
    src.add_argument("--export", help="the directory export.sh wrote")
    src.add_argument("--dsn", help="the v0 database, read-only; never printed")
    p.add_argument("--out", required=True, help="the DuckDB file to write (replaced)")
    p.add_argument("--threads", type=int)
    p.set_defaults(run=cmd_extract)

    p = sub.add_parser("linkage-csv", help="write identifier,code rows for nils linkage import")
    p.add_argument("--v0", required=True, help="the DuckDB file extract wrote")
    p.add_argument("--out", help="the CSV to write (mode 600; holds identifiers)")
    p.add_argument("--cohort", help="only the subjects of this v0 cohort")
    p.add_argument("--id-type", help="the v0 id type to export (default: the one with most rows)")
    p.add_argument("--list-id-types", action="store_true", help="list v0's id types with their row counts")
    p.add_argument("--drop-collisions", action="store_true", help="leave out rows the import would refuse")
    p.set_defaults(run=cmd_linkage_csv)

    p = sub.add_parser("compare", help="compare a v1 registry against the v0 one")
    p.add_argument("--v0", required=True, help="the DuckDB file extract wrote")
    dst = p.add_mutually_exclusive_group(required=True)
    dst.add_argument("--v1", help="the v1 registry home (nils.toml)")
    dst.add_argument("--v1-dsn", help="a v1 Postgres registry, read-only; never printed")
    p.add_argument("--v1-schema", help="the v1 schema on Postgres (default nils)")
    p.add_argument("--root", help="the digested root; scopes v1 to its source and checks paths on disk")
    p.add_argument("--cohort", help="the v0 cohort whose subjects are compared (default: all)")
    p.add_argument(
        "--v0-files",
        default="all",
        choices=sorted(V0_FILE_MODES),
        help="the extension mode of the v0 digest (default all: .dcm any case, or no suffix)",
    )
    p.add_argument("--key-file", help="the v0 subject-code key, one line; classifies how v0 derived codes")
    p.add_argument("--adjudication", help="the TOML that classifies the divergences")
    p.add_argument("--no-fs", action="store_true", help="do not check paths on disk")
    p.add_argument("--catalogue", help="the catalogue.md to read (default: this checkout's)")
    p.add_argument("--sample-cap", type=int, default=fields.SAMPLE_CAP, help="residual rows classified per field")
    p.add_argument("--threads", type=int)
    p.add_argument("--out", required=True, help="the directory for report.md, report.json and work.duckdb")
    p.set_defaults(run=cmd_compare)

    args = parser.parse_args(argv)
    if args.command == "linkage-csv" and not args.list_id_types and not args.out:
        parser.error("linkage-csv needs --out (or --list-id-types)")
    return args.run(args)


if __name__ == "__main__":
    sys.exit(main())
