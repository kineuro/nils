<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# v0-compare

I compare a v1 registry against the v0 (0.x) one it replaces, the way the
Wave 1 spec asks for it (`docs/specs/wave1-parse-and-digest.md`, §12): every
v0 instance found in v1 or refused by name, every field agreeing on 99.9% of
the rows, the stack partitions identical, every v0 subject code a v1 code,
sessions meeting v0's events one for one, and every divergence classified.
The report holds counts and shapes only. No value of any field, no code, no
identifier, no path and no UID appears in it, so it can go into the record.

Python 3.11 or later and DuckDB; nothing else.

```sh
python -m pip install -e "tools/v0-compare[test]"
```

## The run

1. Export v0. `export.sh` copies the tables I read out of v0's database with
   the session forced read-only, one zstd-compressed CSV per table. The
   subject table comes without names; everything else comes as stored,
   identifiers included, so the export is as sensitive as the database.

   ```sh
   tools/v0-compare/export.sh /path/to/v0-export
   ```

2. Load the export into one DuckDB file. `--dsn` reads the database
   directly instead, read-only, when it is reachable; the DSN is never
   printed.

   ```sh
   v0-compare extract --export /path/to/v0-export --out v0.duckdb
   ```

3. Carry v0's identifiers into v1 before the gate digest, so that v1 files
   the same subject under the same code (§7.2). The CSV holds identifiers:
   import it, then delete it.

   ```sh
   v0-compare linkage-csv --v0 v0.duckdb --cohort COHORT --out ids.csv
   nils linkage import ids.csv --id-type patient-id --registry HOME
   rm ids.csv
   ```

   `--list-id-types` shows v0's id types with their row counts; an
   identifier under two codes, or a code under two identifiers, is counted
   and, with `--drop-collisions`, left out (the import would refuse it).

4. Digest the same tree with v1, in the file mode v0 used, then compare.

   ```sh
   nils digest ROOT --registry HOME --files dcm,no-ext
   v0-compare compare --v0 v0.duckdb --v1 HOME --root ROOT --cohort COHORT \
       --v0-files all --key-file KEY --adjudication adjudication.toml --out report/
   ```

   The exit code is 0 when every bar passes, 1 otherwise. `report/` holds
   `report.md`, `report.json` and `work.duckdb`; the last one holds values
   from both registries and stays where the registries are.

`--v0-files` names v0's file-name mode (`all`: `.dcm` in any case or no
suffix; `dcm`, `DCM`, `all_dcm`, `no_ext`), which selects the same names in
v1 (the `--files` knob). `--root` scopes v1 to the source that holds the
tree and lets me check on disk whether a v0 path v1 never saw exists;
`--fs-cap` bounds that check (a million paths by default, `0` for all of
them) and `--no-fs` skips it. `--key-file` names the v0 subject-code key,
one line, which classifies how v0 derived each code (`key-consistent`,
`cohort-hashed`, `no identifier`, `other`); the key is read and dropped,
never written.

## What the report says

Instances (§12.2) are paired on their SOP Instance UID. A v0 instance
missing from v1 is classed by what v1 knows about its path: quarantined
under a refusal class, ingested under another SOP, present on disk but
never walked, or absent from disk. A v0 path is relative to its cohort's
root, so a path absent from the compared root whose subject is listed under
several cohorts is reported apart from one whose subject is in a single
cohort: only the second is a file v0 holds and nobody has. A v1 instance missing from v0 is `in v0
under another subject or cohort`, `name outside v0 mode`, `sop class not
in v0's nine`, `modality not in v0's`, `resume skip` when v0's resume rule
would have skipped it (its SOP sorts below the highest one v0 holds for the
series), `series absent from v0` with what v0 knew (the study, the subject,
neither), or `unexplained: series known to v0`. Only the last one fails the
bar; the others are explained, and the adjudication says whether they are
accepted.

Fields (§12.3) are compared per catalogue row after both sides are brought
to one normal form: multi-valued strings and Python list literals to the
backslash form, numbers to a canonical spelling, dates and times to ISO,
JSON to presence. A SQLite registry is read by its declared types, not as
text: SQLite writes a REAL out with 15 significant digits, and a float32
widened to a double (B1rms) needs 17, so a text read would make the two
sides differ on the spelling alone. Rows that still differ are read back as shapes (`A` for
a letter, `9` for a digit) and grouped by pattern: `case`, `whitespace`,
`number-format`, `rounded`, `scale`, `list-order`, `subset`, `prefix`,
`null↔value`, or the shapes on both sides. A quasi-identifying or
identifying field collapses its shapes to `other`.

Stacks (§12.3) are matched by membership on the common instances of each
series; a series whose partition differs is reported by its shape (`v0 2
stack(s), v1 1, 0 matched`).

Subjects (§12.4) are compared by code, and studies by the code they hang
off on each side. Sessions are v1's (subject, study date) groups against
v0's events; v0 wrote an event per modality on a day, so the surplus on
days with several events is accepted.

## Adjudication

Every group of divergences must carry a class before the gate passes:
`v0-bug` (v1 is right), `v1-bug` (fixed before the gate closes, with a
fixture) or `accepted` (a change the spec declares). The TOML names the
groups; patterns are globs.

```toml
[[divergence]]
level = "series"
field = "image_type"
pattern = "list-*"
class = "accepted"
note = "v0 stored the literal, v1 the parts"

[[partition]]
pattern = "v0 1 stack(s), v1 *"
class = "v0-bug"

[[instance]]
side = "v1-only"
pattern = "resume skip*"
class = "accepted"
```

A group classed `accepted` or `v0-bug` is excused from the bar it would
fail; a `v1-bug` and an unclassified group count in full. Two series
columns carry the first instance's value on both sides
(`media_storage_sop_instance_uid`, `image_position_patient`), and which
instance is first follows the walk order, so I class their divergences
`accepted` myself unless the file says otherwise. The thirteen
stack-signature columns of the series tables (§8) carry the first
instance's value the same way; there the instances differ only where the
series has several stacks, so a divergence of one of them in a series with
more than one stack on either side is grouped apart, under the pattern with
` (multi-stack)` appended, and is `accepted` the same way. The same field
in a single-stack series keeps its plain pattern and needs a rule.

## Tests

`tests/` runs the tool over a synthetic corpus the engine's `corpus`
example writes and the `nils` binary digests (both built in release mode
under `engine/target`, or named by `NILS_BIN` and `NILS_CORPUS_BIN`).
`tests/v0shape.py` projects a v0-shaped export out of that registry and
injects known divergences; the tests assert that each lands in its class,
and that a clean projection passes. `NILS_TEST_POSTGRES_DSN` adds the same
run against a v1 registry on Postgres.

```sh
cd tools/v0-compare && python -m pytest -q
```

Nothing here touches a real registry: the corpus is invented, its UIDs
live under `1.2.826.0.1.3680043.8.498`, and the fixture key comes from
`secrets.token_hex`.
