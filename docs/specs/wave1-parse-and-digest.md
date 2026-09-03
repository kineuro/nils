# Wave 1: parse and digest

*Specification, first draft 2026-09-02; amended the same day after review (studies
and sessions, §4.4; one key, §7.2; the toolchain, §3) and as the slices of §14 were
built (the blocks headed "Settled while building", in §4.2, §5.2, §6.2, §7, §8, §9,
§10, §11 and §13; the measurements in §8, §9.2 and §15). This is the spec the engine code
follows; the design record ([`docs/decisions/`](../decisions/)) says why each
choice was made, and this document cites it by id. It implements D2, D3, D4, D6,
D7, D13, D17, C3, C6, C36, C37 and C38 and closes at the gate in §12.*

## 1. What Wave 1 delivers

One binary, `nils`, that

1. creates a registry on SQLite or Postgres from one declared schema (`nils init`);
2. keeps the registry's key outside the database (`nils key`);
3. walks a source tree, parses the header of every file, records every path,
   quarantines what it refuses, resolves every instance to a subject under the
   registry's pseudonym scheme and the batch's identity rule, groups instances into
   stacks, and writes it all in bulk (`nils digest`), resumable by default;
4. imports existing subject codes and identifier maps as linkage records
   (`nils linkage import`);
5. answers "what state is this registry in" as queries over the data (`nils status`,
   `nils quarantine`, `nils review list`) and lists every store it keeps
   (`nils custody`);
6. reproduces v0's extraction on the live registry field for field, inside the
   budget of D6, on the baseline host of C6. That is the gate (§12).

Not in Wave 1: the fingerprint pass and the classifier (Wave 2), anonymization and
BIDS (Wave 3), the server, the catalog endpoint and the clinical importer (Wave 4).
Two things run beside it: the pack-format prototype (C11), which is judged on its
own criteria before Wave 2 opens, and the baseline measurement of v0 (C6), which
has to exist before the performance bar in §12 means anything.

## 2. Words

- **Registry**: one system of record, a directory holding `registry.db` (SQLite) or
  a Postgres database, plus config; subjects are global inside it (D4).
- **Source**: a directory root registered with the registry the first time it is
  digested. **Ingest batch**: one run of `nils digest` over a source; every row it
  creates carries its id.
- **File**: a path under a source root, recorded whatever happens to it.
  **Instance**: one DICOM SOP instance; a file that parsed and was accepted.
  **Series**, **study** (one StudyInstanceUID: one continuous run on one scanner,
  DICOM's word and v0's table), **subject**: the chain every instance hangs off.
  **Stack**: a homogeneous group of instances inside a series, defined by the
  signature in §8. **Session**: the occasion a subject came in for, which is one
  study most of the time and several when the PACS split the visit or the visit
  spanned days; not a table but the output of a session scheme (§4.4).
- **Pseudonym scheme**: the function that turns a source identifier into a subject
  code (C36). **Identity rule**: which fields carry the identifier and how they are
  parsed (C37). **Linkage store**: the separate store that maps identifiers to
  subjects and subjects to each other (D13). **Key store**: where the keys live.
- **Quarantine**: the listed output of files the digest refused, with a class per
  file (D17). **Diagnostic**: a counted, sampled observation the engine makes about
  the data (a series whose instances disagree on a field; an identity value that
  matched no pattern). **Review item**: a decision asked of a human or an agent
  (D7); Wave 1 emits very few.
- **Job**: a recorded, resumable run of a heavy verb; `digest` is Wave 1's only kind.
  **Epoch**: the registry's monotonic counter, advanced by every batch.

## 3. The shape of the code

A Cargo workspace under `engine/`:

| crate | holds |
|---|---|
| `nils-dicom` | the reader over `dicom-rs`: open, stop before Pixel Data, the field catalogue (§6), value normalization, the Enhanced MR and private-tag fallbacks, the refusal classes |
| `nils-registry` | the schema declared once, the dialect layer, migrations, the connection types for SQLite and Postgres, the linkage store, the key store |
| `nils-digest` | the walker, the pipeline, identity resolution, stack signatures, the writer, jobs, resume |
| `nils` | the binary: the CLI, config, `custody`, output formatting |

Rust 1.98.0 (the current stable at the opening of the wave), edition 2024. The
toolchain is pinned in `engine/rust-toolchain.toml`, the workspace's `rust-version`
matches it, CI reads the same file, and a new Rust release is adopted by raising
both in a pull request of its own: Rust's compatibility guarantee means the newest
toolchain costs nothing and the pin means every machine builds the same thing. Every
source file starts with `// SPDX-License-Identifier:
AGPL-3.0-only` (10; 15, R6). Dependencies are chosen for Wave 1 only: `dicom-object`
and `dicom-json` (the reader and the DICOM JSON model), `rusqlite` with the bundled
SQLite, `postgres` (synchronous, with `COPY`), `blake2`, `chacha20poly1305`, `clap`,
`serde`/`serde_json`/`serde-saphyr` (the maintained serde binding to YAML;
`serde_yaml` was archived by its author and takes no fixes), `regex`,
`crossbeam-channel`, `tracing`. DuckDB
is not linked in Wave 1: the columnar passes that need it are Wave 2's, and the
spike showed what it costs each target in build time; it enters with the
fingerprint pass. The six-target release workflow, the two-backend test matrix and
the synthetic benchmark (§12.6) are the CI skeleton of Wave 0 and exist before the
first crate compiles.

Two rules from the record that shape every module: nothing assumes MRI (the
modality column, the per-modality detail tables and the refusal classes treat MR,
CT and PT alike, and a fourth modality is a schema addition, not a rewrite); and a
stage's input is a predicate over columns, never a list carried in memory or in a
handover row (02).

## 4. The registry

### 4.1 One schema, two dialects (D3)

The schema is declared once, in Rust, as data: tables, columns with logical types,
indexes and constraints. A migration is a numbered step that emits DDL for each
dialect from that declaration; `registry_meta.schema_version` records the last
applied step; `nils init` creates, `nils doctor` checks, and every later command
refuses to run on a registry that is ahead of the binary.

| logical type | SQLite | Postgres | note |
|---|---|---|---|
| `id` | `INTEGER PRIMARY KEY` | `BIGINT GENERATED ALWAYS AS IDENTITY` | never reused |
| `text` | `TEXT` | `TEXT` | UTF-8 |
| `int` | `INTEGER` | `BIGINT` | |
| `double` | `REAL` | `DOUBLE PRECISION` | |
| `bool` | `INTEGER` 0/1 | `BOOLEAN` | |
| `date` | `TEXT` `YYYY-MM-DD` | `DATE` | |
| `time` | `TEXT` `HH:MM:SS.ffffff` | `TIME` | six fraction digits always |
| `timestamp` | `TEXT` ISO 8601 UTC | `TIMESTAMPTZ` | |
| `json` | `TEXT` | `JSONB` | |
| `bytes` | `BLOB` | `BYTEA` | |

The dialect layer is small on purpose: type names, identity columns, `INSERT ...
ON CONFLICT`, `RETURNING`, the bulk path (a multi-row insert on SQLite, `COPY` into
a temporary table followed by `INSERT ... SELECT ... ON CONFLICT` on Postgres), and
the two ways to open a connection. Anything a feature needs beyond that list is a
reason to question the feature (D3: what cannot be expressed on both does not go in
the schema). The test suite runs on both backends in CI; SQLite on every push,
Postgres 16 in a service container.

### 4.2 Tables

Registry (`registry.db`, or on Postgres the schema `nils.toml` names, `nils` by
default). Every table that the digest writes carries `first_batch_id`, the batch
that created the row; rows are never rewritten by a later batch in Wave 1 (a
changed file is a diagnostic, §5.2; a stored null is filled by the first file
that has a value, §9.1, which is not a rewrite).

- `registry_meta`: `key`, `value`. Holds `registry_id` (a UUID), `schema_version`,
  `epoch`, `created_at`, `pseudonym_scheme`, `pseudonym_key` (a key *name*,
  §7.2), `display_length`, `session_scheme` (json, §4.4).
- `job`: `id`, `kind`, `name`, `args` (json), `state` (`queued`, `running`, `done`,
  `failed`, `cancelled`), `pid`, `host`, `started_at`, `heartbeat_at`,
  `finished_at`, `progress` (json), `error`.
- `source`: `id`, `root` (as given), `root_canonical`, `first_seen_at`.
- `ingest_batch`: `id`, `source_id`, `job_id`, `name`, `config` (json: every knob
  of §11 as it was resolved, the identity rule, the file filter, workers, the
  binary's version), `started_at`, `finished_at`, `state`, `counts` (json: the
  diagnostics report of §11), `epoch_after`.
- `source_file`: `id`, `source_id`, `batch_id` (the last batch that examined the
  path), `dir` (the path's directory, what the resume check queries by, §5.2),
  `path` (relative to the root, forward slashes), `size`, `mtime_ns`,
  `status` (`ingested`, `duplicate`, `quarantined`, `skipped`, `gone`), `reason`
  (the quarantine class of §5.3, `symlink` or `special` for a skipped one, or
  null), `detail` (text, or null), `instance_id` (null unless `ingested` or
  `duplicate`), `seen_at`. Unique `(source_id, path)`; indexed on
  `(source_id, dir)`, `(batch_id, status)` and `instance_id`. This is D17's
  "records the path of every file".
- `subject`: `id`, `code` (unique), `code_digest` (bytes, the full digest under the
  scheme), `birth_date`, `sex`, `first_batch_id`, `created_at`. Nothing else: names
  never enter (D13), and the demographics v0 collected through its importer arrive
  with the clinical layer in Wave 4 (D30).
- `study`: `id`, `study_instance_uid` (unique), `subject_id`, `study_date`,
  `study_time`, `study_description`, `study_comments`, `modalities_in_study`,
  `manufacturer`, `manufacturer_model_name`, `station_name`, `institution_name`,
  `first_batch_id`. v0's table and DICOM's word; there is no `session` table
  (§4.4).
- `series`: `id`, `series_instance_uid` (unique), `study_id`, `subject_id`,
  `modality`, the series columns of the catalogue (§6.2), `n_instances`,
  `n_stacks`, `first_batch_id`.
- `series_mr`, `series_ct`, `series_pet`: `series_id` (primary key) and the
  modality columns of the catalogue. Present from Wave 1 for all three (11,
  "nothing may assume MRI").
- `stack`: `id`, `series_id`, `stack_index`, `stack_key`, `modality`,
  `orientation` (the class of §8), the fourteen signature columns of §8 as read,
  `orientation_confidence`, `n_instances`, `first_batch_id`. Unique
  `(series_id, stack_index)` and `(series_id, stack_key)`.
  The progress columns of later passes (`fingerprinted_at`, `classified_at`, pack
  version, evidence ref) are added by the migrations of the waves that fill them;
  the pattern is fixed now: a pass is a nullable timestamp and a version on its
  unit, and its input is `WHERE <previous> IS NOT NULL AND <mine> IS NULL`.
- `instance`: `id`, `sop_instance_uid` (unique), `series_id`, `stack_id`, the
  instance columns of the catalogue, `charset` (the SpecificCharacterSet as
  written), `source_file_id` (the first path), `first_batch_id`.
- `diagnostic`: `id`, `batch_id`, `kind`, `scope` (`file`, `instance`, `series`,
  `study`, `subject`, `batch`), `ref_id`, `count`, `sample` (json), `created_at`.
- `review_item`: `id`, `kind`, `scope`, `ref` (json), `evidence` (json), `status`
  (`open`, `accepted`, `rejected`, `superseded`), `actor`, `created_at`,
  `decided_at`, `decision` (json). The full shape (grouping by evidence signature,
  staged results, precedence) is Wave 4's; Wave 1 creates the table with these
  columns so that its items have a home and are not a log line.

Linkage store (`linkage.db` beside the registry, mode 600; on Postgres the schema
`<schema>_linkage`, `nils_linkage` by default). Separate on purpose: it is the
only store with identifying data, its contents are unreadable without the
registry's key (§7.2), and it can be backed up, exported and purged on its own
(D13, C38).

- `linkage_meta`: `key`, `value`. Holds the store's own `schema_version`, so the
  two stores migrate separately; slice 4 adds the `registry_id` of the registry
  it belongs to, so that a linkage store copied next to the wrong registry is
  refused, not joined.
- `id_type`: `id`, `name`, `description`. Seeded with `patient-id` (DICOM
  PatientID) and `study-instance-uid` (the fallback of §7.3); sites add their own
  (`personal-number`, a study's own scheme) with `nils linkage id-type add`.
- `identity`: `id`, `subject_id`, `id_type_id`, `lookup` (bytes: the keyed
  BLAKE2b-256 of `id_type || 0x00 || value` under the lookup subkey of §7.2),
  `ciphertext` (bytes: the value under XChaCha20-Poly1305 with the encryption
  subkey, a fresh 24-byte nonce prefixed), `source` (`dicom`, `csv`, `manual`), `first_batch_id`,
  `created_at`. Unique `(id_type_id, lookup)`. The identifier itself is in no
  column in clear: a copied file needs the key.
- `linkage`: `id`, `subject_a`, `subject_b`, `kind` (`same-person`), `evidence`
  (json), `actor`, `created_at`, `reversed_at`, `reversed_by`. Merges are logical:
  `subject_a` is canonical, `subject_b` an alias, and reversing is a column, never
  row surgery (03).
- `date_shift`: `subject_id`, `offset_days`. Created now, filled by Wave 3's
  anonymizer (D13).
- `read_audit`: `id`, `at`, `actor`, `identity_id`, `why`. Every command that
  decrypts an identifier writes a row.

Settled while building identity (slice 4):

- `subject.code_digest` and `subject.first_batch_id` are nullable: a subject an
  import created has its code as given and no digest, and no batch created it
  (§7.4). `identity.first_batch_id` is null for the same rows.
- `linkage_meta` holds `registry_id` from this slice on: a registry refuses a
  linkage store whose `registry_id` is another's, with the two ids in the error,
  and stamps its own into a store that has none (one made before this slice).
- A subject holds **one identity per type**: the unique `(id_type_id, lookup)`
  stands, and neither the writer nor the import files a second identifier of
  one type on a subject; that is a collision (§7.4) or a refused import row.
  Two subjects that are one person are joined by `linkage link`, never by a
  second row.
- The first `review_item` rows are `identity.collision`: `scope` `subject`,
  `ref` `{subject_id, code}` (the subject that holds the code, or none when the
  collision is inside one batch), `evidence` `{id_type, reason, scheme,
  display_length, batch_id}` with `reason` one of `identity`, `display-code`,
  `batch` (§7.4). No identifier and no lookup is in the item.
- `identity.source` is `dicom` or `csv` in Wave 1; `manual` waits for the door
  that would write it.
- The schema version of both stores stays 1 through the alpha: a registry made
  by an earlier alpha is re-created from its sources, not migrated. Migrations
  begin at 1.0.0, and the version column is there so that they can.

Settled while building stacks (slice 5): the `stack` row holds `orientation`
(the class) beside `orientation_confidence`, and its fourteen signature columns
hold the values as the first instance of the stack read them, not rounded: the
rounding is the key's business (§8), the columns are the catalogue's. The
`stack_id` of an instance is set by the same transaction that files it; there
is no instance without a stack, and `series.n_stacks` and `stack.n_instances`
are incremented per batch beside `series.n_instances`.

### 4.3 Sensitivity classes

The field catalogue (§6) declares a class for every column the digest writes:
`identifying` (none in the registry; the linkage store is the only place),
`quasi-identifying` (birth date, sex, every date and time, station and institution
names, free text: descriptions, comments, protocol and sequence names, image
comments, derivation descriptions, and `source_file.path`, since a path can hold
a name), `clinical` (nothing the digest writes; the clinical layer arrives in Wave
4), `technical` (everything else). Wave 4 serves the classes through the catalog
and enforces them in results, evidence and MCP responses (D13, C27); Wave 1 uses
them in one place already: a diagnostic sample of a classed value is a *shape*,
never the value (§11).

### 4.4 Studies and sessions

A study is DICOM's unit, one StudyInstanceUID for one continuous run on one
scanner, and it is a row because it is a fact in the file. A session is the
occasion the subject came in for, and no file records it: the PACS splits one
visit into a brain study and a spine study, a scan that stopped and started again
is a new study, and a visit that was too much for one day continues a few days
later. In the live registry, 4,443 of 30,665 occasions (14.5 percent) hold more
than one study on the same day; 4,280 of those are different exams and 4,323 begin
within an hour of each other; a further 331 pairs of consecutive visits, about one
percent, fall within fourteen days, half of them the same exam again. v0 keeps
`study` and derives the occasion three times in three places (an `event` per
subject, modality and date; the QC services' grouping by subject and date; the
anonymizer's M00, M06, M12 labels from the first study date), none of them stored,
reviewable or able to join two days.

v1 keeps the table `study` and makes the session the output of a **session
scheme**, applied by whatever needs sessions (the BIDS exporter and the anonymizer
in Wave 3, the `session` grain of the query language in Wave 4) and never stored
as a fact, so a changed scheme never rewrites the registry. The registry declares a
default scheme at `nils init` (`registry_meta.session_scheme`) and a selection
carries its own (03, "Sessions and timepoints"), so two projects can cut the same
subject's studies differently without a conflict. A scheme has four parts:

- `window_days` (default 0). A subject's studies are taken in date and time order,
  whatever their modality; a study joins the open session when its date is within
  `window_days` of that session's first study, otherwise it opens a new one. Zero
  is the same calendar day, which is v0's `event` and its QC grouping; fourteen
  joins the brain-on-Monday, spine-on-Wednesday visit.
- `label` (default `date`). `date`: the first study's date as `YYYYMMDD`, v0's
  `ses-` label. `months`: v0's anonymizer function, calendar months from the
  anchor plus the remaining days over 30.44, rounded; zero is `M00`; a value within
  one month of a multiple of six snaps to it, so M05, M07, M11 and M13 become M06
  and M12 while M03 stays M03; `M` and two digits. `ordinal`: `01`, `02`, in order.
- `anchor`, for `months`: the subject's first session in scope, which is what v0
  did (its anchor was the first study in the export, and the export is the
  selection's scope). A clinical event as the anchor (months since treatment start)
  is Wave 4's, when the timeline exists (D30).
- `overrides`: explicit assignments by study UID, this study belongs to that
  session and this session is labelled so; they win over the rule, and they are the
  door for the case the rule cannot know.

Wave 1 stores what every scheme reads (`study_date`, `study_time`, `subject_id`,
`modality`) and applies the default scheme in one place: the compare tool groups
v1's studies under it and checks the groups against v0's `event` rows (§12.4).

## 5. The walker (D17)

### 5.1 Traversal

`nils digest <root>` registers the root as a source (canonical path; the same root
given twice is one source) and walks it with a pool of `walk_threads` (default 8)
threads, one directory listing per task, breadth on the pool and depth inside a
directory. Directory listing over NFS is the metadata-bound part of a digest, and
the pool is what keeps the parsers fed. The walk has no opinion about layout:
subject folders, session folders, flat dumps and mixed trees are all "files under
a root", and grouping comes from the header alone.

Every regular file is a candidate. Symbolic links are not followed and are
recorded (`skipped`, reason `symlink`); hidden files are candidates like any
other; a directory that cannot be listed is a diagnostic (`walk_error`, with the
path) and the rest of the walk continues; a root that cannot be listed fails the
job. Files are handed to the parsers in directory order, per directory: no global
sort, no per-subject planning pass. v0 read every file twice (a planning pass for
UIDs, then the extraction); v1 reads once.

### 5.2 Filter and resume

The `files` knob selects candidates by name: `all` (default), `dcm` (`.dcm`, any
case), `no-ext`, a glob, or a comma-separated union of those (`dcm,no-ext`: a name
matching any part is a candidate). The default is `all` because the check for what
a file is costs 132 bytes (§6.1) and D17 forbids silent drops; v0's own "all" mode
meant ".dcm (any case) or no extension" and the gate runs with `--files dcm,no-ext`
so that both sides see the same candidates.

Before parsing, every candidate is checked against `source_file` for its source,
one query per directory by path prefix (an index on `(source_id, path)` makes it a
range scan; there is no in-memory index of the source, which at thirty million
paths would be the memory budget). A path with an unchanged `(size, mtime_ns)` and
a status of `ingested` or `duplicate` is skipped; `quarantined` is skipped unless
`--retry-quarantine` is given (a newer binary may read what an older one refused);
a changed file is parsed again and, if its SOP instance is already known, recorded
as a diagnostic (`file_changed`) rather than rewritten. When the walk completes
with no `walk_error`, paths of the source that were not seen are marked `gone`
(their instances stay; a gone file is provenance, not a deletion).

This is what "resume by default" means: the second `nils digest <root>` after an
interrupted first one does the remaining work, and a `nils digest` over a tree that
grew since last month ingests only what is new. `--restart` ignores `source_file`
for the run and re-parses everything; it does not delete rows.

Settled while building the writer (slice 3):

- The check is one query per directory on `(source_id, dir)`, with the paths of
  that directory held in a small LRU while its files are in flight; a source
  with no rows at all is not asked. An unchanged file is counted (`unchanged` in
  the report) and its row is touched (`batch_id`, `seen_at`), because
  `batch_id` is what says a path was examined by this batch, and that is what
  gone-marking reads. A quarantined file left alone is `quarantine_kept`.
  Settled in slice 6: an unchanged file has nothing to parse, so it never
  enters the parsers' queue; the resume stage collects the unchanged files
  into batches of its own (closing at eight times `batch_rows`, as a parser's
  would) and hands them to the writer directly. §9.2 has the measurement that
  decided it.
- Only an instance's own file carries its instance forward (the file
  `instance.source_file_id` points back to). A duplicate is filed afresh against
  whatever it holds now; a changed duplicate is not the instance changing.
- A changed file is parsed again and, when its SOP instance is known, filed
  under it: `ingested` when the instance is its own, `duplicate` otherwise, and
  in both cases a `file_changed` diagnostic whose subject says what the new
  content was (`new_sop`, an instance nobody had; `same_sop`, its own instance
  as before; `other_sop`, another instance's). A file that was `gone` and is
  back unchanged is its instance's own file again, not a duplicate of it.
- Paths are marked `gone` only by a run that could have seen everything: the
  `files` knob at `all` and no `walk_error`. A filtered run or one with an
  unlistable directory leaves the absent paths as they were.
- `--restart` reads every file again; a file whose instance is its own is
  refiled as that instance (`same_sop` when the file changed), and nothing is
  deleted.
- Special files (sockets, devices, pipes) are recorded as `skipped` with reason
  `special`, beside `symlink`.

### 5.3 Quarantine classes

A file that is not ingested gets one of these, in `source_file.reason`, and the
batch's report counts each:

| class | when |
|---|---|
| `not_dicom` | no `DICM` marker and no readable bare dataset that yields a SOPInstanceUID |
| `unreadable` | an I/O error opening or reading (permission, a vanished file, a stale NFS handle) |
| `parse_error` | the reader failed inside the header; the error text is in `detail`, classified by the reader's error chain |
| `missing_uid` | no StudyInstanceUID, SeriesInstanceUID, SOPInstanceUID or SOPClassUID, checked in that order; `detail` names the first one missing (the SOP class may come from the file meta, §6.1) |
| `unsupported_sop_class` | a SOP class outside the batch's `sop_classes` knob (default: v0's nine image storage classes for MR, CT and PT, §6.1) |
| `missing_modality` | no Modality and no ModalitiesInStudy to fall back on |
| `unsupported_modality` | a modality outside the batch's `modalities` knob (default `MR`, `CT`, `PT`; `PET` is normalized to `PT`) |

`duplicate` is not a refusal: the file parsed and its SOP instance already exists
in the registry (another path, another batch). The row keeps `instance_id` so the
second path is provenance too. `skipped` (symlink) is neither.

The first four classes are the reader's, decided before any policy applies; the
spike's harness counted exactly these, and the dry run of slice 2 over the nmosd
corpus refused the same 134 files (124 `not_dicom`, 10 `missing_uid`). The other
three are the batch's knobs at work.

Each class is a listed output: `nils quarantine list [--batch <id>] [--class <c>]`
prints paths, and the batch's report carries the counts. One review item of kind
`ingest.quarantine` per batch and class groups the rows (D7, C5: one item, N
members), with the count as evidence and no path in the item body; a human or an
agent decides "accepted" (these are sidecars, this is not our data) or "retry" and
the decision is a row, not a deletion.

Settled while building custody (slice 6): `nils quarantine list` prints each
refused path joined to its root, with the batch, the class and the detail,
oldest batch first; `--json` is the same as one document. The review item per
batch and class is filed in the run's finish transaction, with `ref`
`{"batch_id", "class"}` and evidence `{"count"}`, by a cancelled run too; a
quarantine that a later run keeps (the file unchanged and not retried) files
nothing, because it belongs to the item of the batch that refused it. `nils
review list [--kind <k>] [--status <s>]` and `review show <id>` read the items;
the decision, `review apply`, is Wave 4's.

## 6. The reader and the field catalogue

### 6.1 Opening a file

The reader reads the first 132 bytes. With `DICM` at offset 128 the file is Part
10 and is opened with the preamble; without it, as a bare dataset (implicit VR
little endian, then explicit, the way the spike's fallback did). Either way the
read stops in front of Pixel Data (`read_until(PixelData)`), which is what makes
a header read a header read; the spike measured 63,900 files/s on eight workers
with it. Everything after Pixel Data is never read in Wave 1: there is no hash, no
pixel check and no burned-in text detection here (the last is Wave 3's).

Character sets: values are decoded per SpecificCharacterSet, including the
variants written without their underscore that the spike met (`ISO IR 100`); an
unknown charset decodes as ISO-8859-1 and counts a `charset_unknown` diagnostic;
bytes that do not decode become U+FFFD and count `charset_lossy`. Nothing is
refused for its charset.

SOP classes accepted by default, exactly v0's set: CT Image Storage, Enhanced CT,
Legacy Converted Enhanced CT, MR Image Storage, Enhanced MR, MR Spectroscopy,
Legacy Converted Enhanced MR, PET Image Storage, Legacy Converted Enhanced PET.
The SOP class is read from the dataset and, when absent there, from the file meta
(MediaStorageSOPClassUID), as v0 did.

Settled while reading the mix corpus (slice 7): an element may declare a length
its own VR cannot hold, and one vendor's private block does it in every file it
writes (a `UL` of six bytes, where a `UL` takes four). `dicom-rs` reads the
values that fit and counts the whole declared length as consumed, so the stream
is two bytes behind from there on: the next tag it reads is the middle of a
text value, its length is nonsense, and the file is reported truncated when
nothing is truncated at all. It cost 1,996 of the mix corpus's 196,086 files,
one percent, and with them 69 series, 47 studies and 32 subjects. The reader
therefore repairs such a file in memory when, and only when, the first read
failed as truncated: it walks the header itself, finds every element whose
length its VR cannot hold, rounds the length down to the values that fit and
drops the surplus bytes (which any reader ignores anyway), and reads the
repaired copy. The file on disk is never touched, the repair is bounded to the
first 8 MB of the header, a file it cannot follow keeps the first verdict, and
every repaired file counts a `ragged_length` diagnostic, so the archive's
malformed corner stays visible in the report rather than becoming silent.

A bare dataset's transfer syntax is the one it was read with (implicit or
explicit VR little endian) and `instance.transfer_syntax_uid` records it. When
the file meta lacks (0002,0012), `dicom-rs` substitutes its own implementation
class UID and version name; the reader treats that substitute as absent, so
`implementation_class_uid` and `implementation_version_name` stay null rather
than naming the reader.

### 6.2 The catalogue

The catalogue is a table in `nils-dicom`, one row per column the digest writes:
the column name, the level (subject, study, series, series_mr, series_ct,
series_pet, stack, instance), the source (a keyword, or a fallback chain), the
converter, the sensitivity class, and a note. It is generated into the
documentation (`docs/reference/catalogue.md`, rendered from the code by
`cargo run -p nils-dicom --example catalogue -- --write` and checked against it
by a test) and it is the seed of Wave 4's catalog endpoint. Wave 1's rule is
**v0's field set, v0's fallbacks, v0's converters, then additions**: the gate
compares field by field (§12), and a field that v0 wrote and v1 dropped would
have to be argued as an accepted change, in writing.

The levels and their fields, as v0 extracts them (`extract/dicom_mappings.py` in
the 0.5.3 source):

- **study** (9): StudyDate, StudyTime, StudyDescription, StudyComments,
  ModalitiesInStudy, Manufacturer, ManufacturerModelName, StationName,
  InstitutionName.
- **series** (29): FrameOfReferenceUID; ImplementationClassUID,
  MediaStorageSOPInstanceUID, SOPClassUID and ImplementationVersionName with their
  file-meta fallbacks; SequenceName, ProtocolName, SeriesDate, SeriesTime,
  SeriesDescription, BodyPartExamined; ScanningSequence and SequenceVariant with
  the private per-frame fallback; ScanOptions, SeriesComments, ImageType;
  SliceThickness and SpacingBetweenSlices with the PixelMeasures fallback;
  ImagesInAcquisition; ImageOrientationPatient with the PlaneOrientation fallback;
  ImagePositionPatient, PatientPosition; the seven ContrastBolus and ContrastFlow
  fields. Plus `modality`, from Modality with ModalitiesInStudy as the fallback.
- **instance** (23): InstanceNumber, AcquisitionNumber, AcquisitionDate and Time,
  ContentDate and Time, SliceLocation, PixelSpacing (PixelMeasures fallback), Rows,
  Columns, BitsAllocated, BitsStored, HighBit, PixelRepresentation, WindowCenter,
  WindowWidth, RescaleIntercept, RescaleSlope, NumberOfFrames,
  LossyImageCompression, DerivationDescription, ImageComments, TransferSyntaxUID
  (file-meta fallback).
- **stack-defining** (14, read per instance, stored on the stack, §8):
  InversionTime, EchoTime, EchoNumbers, EchoTrainLength, RepetitionTime, FlipAngle,
  ReceiveCoilName, ImageOrientationPatient, ImageType, Exposure, KVP,
  XRayTubeCurrent, NumberOfSlices (v0's `pet_bed_index`), SeriesType (v0's
  `pet_frame_type`).
- **series_mr** (33 + 6): MRAcquisitionType, AngioFlag, RepetitionTime, EchoTime,
  InversionTime, InversionTimes, FlipAngle, PhaseContrast, NumberOfAverages,
  ImagingFrequency, ImagedNucleus, EchoNumbers, MagneticFieldStrength,
  NumberOfPhaseEncodingSteps, EchoTrainLength, PercentSampling,
  PercentPhaseFieldOfView, PixelBandwidth, ReceiveCoilName, TransmitCoilName,
  AcquisitionMatrix, PhaseEncodingDirection, SAR, dBdt, B1rms,
  TemporalPositionIdentifier, NumberOfTemporalPositions, TemporalResolution,
  DiffusionBValue, DiffusionGradientOrientation, DiffusionDirectionality,
  ParallelAcquisitionTechnique, ParallelReductionFactorInPlane, each with the
  Enhanced MR fallbacks v0 has; and the six DWI private values: Siemens b-value
  (0019,100C), Siemens directionality (0019,100D), Siemens
  PhaseEncodingDirectionPositive from the CSA image header (0029,1010, the SV10
  format only), GE b-value (0043,1039) and number of directions (0043,1030),
  Philips b-value number (2001,1003).
- **series_ct** (24): KVP, DataCollectionDiameter, ReconstructionDiameter,
  GantryDetectorTilt, TableHeight, RotationDirection, ExposureTime,
  XRayTubeCurrent, Exposure, FilterType, GeneratorPower, FocalSpots,
  ConvolutionKernel, RevolutionTime, SingleCollimationWidth,
  TotalCollimationWidth, TableSpeed, TableFeedPerRotation, SpiralPitchFactor,
  ExposureModulationType, CTDIvol, CTDIPhantomTypeCodeSequence (as DICOM JSON),
  CalciumScoringMassFactorDevice and Patient.
- **series_pet** (29): Radiopharmaceutical and its dose, half-life, positron
  fraction, start and stop time, volume and route; DecayCorrection, DecayFactor,
  ReconstructionMethod, the three correction methods, DoseCalibrationFactor,
  ActivityConcentrationScale, SUVType, SUVbw, SUVlbm, SUVbsa, CountsSource, Units
  (as `units` and, once more, as `units_type`, the way v0 has it),
  FrameReferenceTime, ActualFrameDuration,
  PatientGantryRelationshipCodeSequence (as DICOM JSON),
  SliceProgressionDirection, SeriesType, CountsIncluded.
- **subject** (2): PatientBirthDate and PatientSex, written when the subject is
  created and never overwritten; a later instance that disagrees counts a
  diagnostic. This is an addition: v0 left both empty at ingest and filled them
  through its importer (C35 puts them in the registry as quasi-identifying fields).

Settled while building the table (slice 2):

- Six keywords v0 mapped do not exist in the standard dictionary (SeriesComments,
  PhaseEncodingDirection, SUVbw, SUVlbm, SUVbsa, ActivityConcentrationScale);
  v0's `getattr` found nothing for them, so those columns were always null. They
  stay as columns and stay null, except `phase_encoding_direction`, which v1 reads
  from InPlanePhaseEncodingDirection (0018,1312): an addition for the gate's
  list of accepted changes.
- The eight radiopharmaceutical and radionuclide fields sit inside
  RadiopharmaceuticalInformationSequence in every real PET file; v0 read the top
  level only and wrote null. v1 reads the top level, then the first item of the
  sequence: an addition, listed the same way.
- The six DWI private values are read by creator block: the creator element is
  checked and the block is shifted to where the creator sits, with v0's fixed
  block (`0x10`) only when the group declares no creator at all. The two private
  per-frame sequences are read at their fixed tags without a creator check, as
  v0 read them.
- A fallback chain moves on when the element is absent or empty and stops at the
  first present value, even one the converter refuses (null, `value_invalid`).

Additions in Wave 1 beyond that list: `instance.charset`, `source_file.size` and
`mtime_ns`, `stack.stack_key`, the counts on series and stacks. Nothing removed.
PatientName and PatientID are read for identity (§7) and stored nowhere in the
registry.

The Enhanced MR fallback is v0's, in v0's order: the standard functional group
sequences first (SharedFunctionalGroupsSequence, then the first item of
PerFrameFunctionalGroupsSequence), then the private per-frame sequences, Philips
(2005,140F) and Siemens (0021,1201). A multi-frame object is one instance with
`number_of_frames`, its stack fields taken from the first frame, as in v0;
per-frame stacks are a question Wave 2 answers with the fingerprint pass in hand
(§15).

### 6.3 Normalization

One converter per logical type, and the normal form is written down because the
gate compares against it:

- **text**: the decoded string, trailing spaces and NULs trimmed, leading kept.
  A multi-valued element is stored as its values joined by a backslash, always.
  v0 did that only for the fields it mapped with its backslash converter and
  stored a Python list literal for a multi-valued value elsewhere; the compare
  tool maps that literal to the backslash form before comparing, and the
  difference is an accepted change, listed once.
- **int**: the integer value of IS, US, SS, UL, SL; a decimal string with no
  fraction; otherwise null and a `value_invalid` diagnostic (v0 wrote null
  silently).
- **double**: DS and FD/FL as parsed; a multi-valued DS where a single is expected
  is null (as v0) and a diagnostic.
- **date**: DA `YYYYMMDD` to `YYYY-MM-DD`; an already ISO value passes; anything
  else null and a diagnostic.
- **time**: TM `HHMMSS[.ffffff]` to `HH:MM:SS.ffffff`, the fraction padded to six
  digits; `HH:MM:SS` passes; anything else null and a diagnostic. v0 kept the
  fraction as written; Postgres normalized it for v0 and v1 normalizes it itself.
- **json**: the DICOM JSON model of the element (`dicom-json`), which is the shape
  pydicom's `to_json_dict` produced for v0.
- **PN** is never converted: no person-name element is stored.
- An **empty** element (zero length, or a sequence with no item) is null for every
  converter, with no diagnostic, and does not stop a fallback chain (§6.2).

## 7. Identity (C36, C37, D13, C3)

### 7.1 The pseudonym scheme

A registry declares its scheme once, at `nils init`, and it cannot change without
re-deriving every subject from the sources, which `nils` refuses to do implicitly.

- `blake2b-8`: v0's function byte for byte. `code = hex(BLAKE2b(key = the key
  bytes, data = UTF-8 of the identifier, digest 8 bytes))`, sixteen lowercase hex
  characters. The key bytes are the UTF-8 of the string v0 used, without a
  trailing newline. The registry that continues v0 is created with `--scheme
  blake2b-8 --key <name>` where the named key holds that string, so that every
  known person lands on the known subject and every existing BIDS tree, export and
  collaboration keeps its codes (C36).
- `blake2b-32`: the default for new registries (D13). The full 32-byte keyed
  digest is stored in `subject.code_digest`; the display code in `subject.code`
  is the first 60 bits of the digest in Crockford base32, twelve lowercase
  characters (`display_length` in `registry_meta`, 12 by default). A new subject
  whose display code exists under a different digest is a collision: the job
  stops with a review item `identity.collision`, and the operator chooses a longer
  display length; it is never resolved silently, and at 60 bits it will not happen
  at registry scale.

Fixture, with a throwaway key that appears only here (`nils-fixture-key`,
identifier `PID-0001`): `blake2b-8` gives `771c4326c89c082c`; `blake2b-32` gives
`ec0b67a602077942a174a5c8d1683043e58e1b18c44e83769a20be0f4dd43927` and the display
code `xg5pf9g20xwm`. The unit tests pin both, and a Python one-liner with `hashlib`
reproduces them.

### 7.2 The key store

Keys live outside the database: by default in `keys/` beside the registry
(directory mode 700, files mode 600), or at the path `keys.dir` in the config, or
later in a KMS behind the same interface. `nils key add <name>` reads the key from
standard input or `--from-file`, strips one trailing newline and says so, refuses
a key longer than 64 bytes (BLAKE2b's limit, and so v0's), and writes the file;
`nils key list` shows names, lengths and fingerprints (the first eight hex
characters of an unkeyed BLAKE2b of the key), never bytes; `nils key remove`
refuses while `registry_meta` names the key.

A registry names **one key** at `init` (`--key <name>`). It is the pseudonym key
of §7.1, used as it is, so that the registry that continues v0 is created with the
v0 key and nothing else; and it is the root of the linkage store's two subkeys,
derived once per process and never stored: `k_lookup = BLAKE2b-256(key = the key,
data = "nils/linkage/lookup")` and `k_encrypt = BLAKE2b-256(key = the key, data =
"nils/linkage/encrypt")`. One secret to set, back up and guard. Whoever holds it
can derive codes and read the linkage store, which is the same power stated twice;
the store's file mode and its `read_audit` table are the controls on it. The key
is referred to by name in `registry_meta` and in a batch's config, and appears
nowhere else: not in a log line, not in an error, not in a diagnostic, not in a
document. Re-keying is written down as what it is: a re-derivation from the
sources, and the reason the key is backed up under the custody of §13.

Fixture, under the throwaway key of §7.1: `k_lookup` is
`d7d3eeb7a8fb4fc9c1cdd83c215c93fabef487366ee678717f8edd0935336fa0`, `k_encrypt` is
`1313a85029438352d9ebb2b8f4b03f32390dfd160355b1ace070bb40f87aabc2`, and the lookup
of `PID-0001` under `patient-id` is
`a548a6fa8cf22772d1de1ee342ff8bd7460c15b1c01e0e189f297cf8a168bd0c`.

### 7.3 The identity rule

The rule is data in the batch's config (`--identity-rule <file>` or the
registry's default), the ingest case of C37:

```yaml
identity:
  id_type: patient-id           # the type the identifier is filed under
  from:                         # tried in order; the first value wins
    - field: PatientName        # a field that carries more than one thing
      pattern: '^(?<id>\d{12})[-_ ](?<date>\d{8})$'
    - field: PatientID
  fallback: StudyInstanceUID    # v0's behaviour when nothing above yields
```

`field` names a DICOM keyword; `pattern` is a regex with a named group `id` (other
named groups are recorded as diagnostics, never as identity); without a pattern
the whole trimmed value is the identifier. `code: verbatim` beside `from` says
that the value the rule reads **is** the subject code, not an identifier to
derive one from: data pseudonymized before it reaches us, where whoever holds
the key decided the code and wrote it into the file. It needs a pattern on every
field, so that a value which is not shaped like a code is never filed as one; a
file whose value does not match takes the fallback, whose study UID is derived
under the scheme as always. The default is `code: derived`. A value that matches no pattern counts
an `identity_unparsed` diagnostic whose sample is the value's *shape* (its length
and character classes, `dddddddddddd-dddddddd`), so the report can say "a
thousand names carry an identifier and a date" without carrying either. The
fallback files the study UID under `study-instance-uid`, so a PatientID-less
study digested again lands on the same subject; it counts `identity_fallback`.
The default rule is the two-line one (PatientID, then the fallback), which is v0.

### 7.4 Resolution

For each instance, in the writer, cached by identifier:

1. Apply the rule to get `(id_type, value)`.
2. `lookup = BLAKE2b-256(key = k_lookup, data = id_type || 0x00 || value)` (§7.2).
3. An `identity` row with that lookup gives the subject. Done; this is the common
   case, and the reason a returning person is one subject.
4. Otherwise derive the code under the scheme. If no subject has it, create the
   subject (birth date and sex from this instance), the `identity` row
   (ciphertext under `k_encrypt`) and continue.
5. If a subject already has that code: when it has no `identity` row of this type,
   it came from an import without an identifier or from a run that stopped between
   the two stores (§9.3), and the identity is attached; when it has one with a
   different lookup, the scheme mapped two identifiers to one code, and that is a
   collision (§7.1): the job stops with the review item.

Imported codes (C3): `nils linkage import <csv> --id-type <t> --id-column <c>
--code-column <c>` creates or finds the subject per code exactly as given, never
re-derived, and files the identifier as an `identity` row with source `csv`. A
row whose identifier already maps to another code is an error listed before
anything is written (the import is validate-then-apply, like everything that
writes). Several identifiers of one type on one code are not an error but the
point: **many identifiers map to one subject**, and the number is not two. A
personnummer is not for life. Someone here on a temporary number is given a
permanent one when residency is granted, may carry more than one temporary
number before that, and the permanent one itself changes when a legal sex change
does, since the number says which. A project reissues its own numbers besides.
The import files each as its own `identity` row on the one subject and counts
the ones that joined a code another identifier already named, in one file or in
a later one; a digest that meets any of them lands on the one subject and the
one code, which is what "the code stays what it was" means in practice. Two
identifiers that *derive* one code are a different thing and stay a collision
(§7.1); this is one code that several identifiers were told to share. This is how v0's per-digest CSV
maps become registry facts, and the gate checks that every v0 subject code comes
out of a v1 digest run with the v0 key and the maps imported (§12.4).

Linkage records (`nils linkage link <a> <b> --evidence <text>`, `nils linkage
unlink`) exist from Wave 1 with the CLI as their only door; the UI and the agent
door are Wave 4's. Nothing in Wave 1 reads a linkage when it queries; the columns
exist so that Wave 4 does not retrofit them (11).

Settled while building identity (slice 4):

- The rule file (§7.3) is `identity: {id_type, from: [{field, pattern?}, ...],
  fallback?, code?}`. `fallback` may be left out and, when written, can only be
  `StudyInstanceUID`: there is one fallback and it is v0's. A pattern is read
  for its `id` group alone; other named groups are not recorded (the
  "diagnostics" of §7.3 wait for a rule that needs them). A value that is
  empty or whitespace is no value. The file is parsed before anything runs and
  a fault is a usage error (exit 2); the rule is recorded in the batch's config
  with the path it came from. The `identity_fallback` diagnostic names the
  rule's fields as its subject and carries no sample.
- Resolution runs per batch, not per instance: the lookups of a batch are
  matched against the writer's cache, the misses against the `identity` table
  in one keyed select (step 3), and the rest are grouped by their code. A group
  whose code no subject holds becomes a subject in the batch's insert; a group
  whose code exists is attached to the subject (step 5) unless the subject's
  stored digest is another (reason `display-code`, `blake2b-32` only) or the
  subject already holds an identifier of that type (reason `identity`); two
  identifiers of one type in one batch that derive one code are a collision
  before any row is written (reason `batch`). The same value under two id
  types derives the one code, as in v0, and attaches: it is not a collision.
- Settled while reading the first gate run (slice 7), on how a person with more
  than one original identifier is kept whole. A personnummer is not for life:
  someone here on a temporary number is given a permanent one when residency is
  granted, may carry several temporary ones before that, and the permanent one
  itself changes when a legal sex change does, since the number encodes it. The
  mapping is many to one and stays open-ended. v0 kept the code constant by
  hashing the *main* number, which meant carrying a map of every other number to
  it outside the tool. In v1 the map is a registry fact: `nils linkage import`
  files any number of identifiers of one type on one subject and counts them,
  instead of refusing the second, so every number resolves to the one subject
  and the one code. Only an identifier that maps to *two* codes is refused. Two
  identifiers that derive one code by the scheme's function remain a collision
  (§7.1): that is a hash accident, not a person. The order matters and is worth
  writing down: the identifiers must reach the linkage store before the digest
  meets them, since a digest that sees an unknown number derives a code of its
  own and creates a second subject, which no later import can undo (the row now
  maps that identifier to another code and is refused). A person split that way
  is a `linkage link` record in Wave 1 and a merge in Wave 4. Where the
  anonymizer writes the subject code into `PatientID` (the shape Wave 3 takes),
  `code: verbatim` (§7.3) files it as the code and the registry derives nothing,
  so the map lives where the key is, the code survives a change of the main
  number by construction, and the linkage store holds no identifying value at
  all for such a cohort.
- A collision rolls the batch back, opens the `identity.collision` item in a
  transaction of its own, marks the job and the batch `failed`, and exits 1 with
  a message that names the type, the code and the item, never an identifier. A
  `blake2b-32` registry is re-created with a longer `--display-length`; under
  `blake2b-8` two identifiers on one code are a fact of v0's function, and the
  review decides them (`review apply` is Wave 4's; in Wave 1 the item is a row
  and the batch is digested again once the operator has acted).
- The identity rows of a batch are filed in the linkage store after the
  registry's transaction commits (§9.3); a subject whose identity row that
  order lost is repaired on the next run by step 5.
- `linkage import` reads a CSV with a header row; `--id-column` and
  `--code-column` name the columns (`identifier` and `code` by default), values
  are trimmed, and the whole file is checked before a row is written: an empty
  identifier or code, an identifier that appears twice with two codes, a code
  that appears twice with two identifiers, an identifier the store already maps
  to another code, and a code that exists under another identifier of the type
  are each listed by line, and nothing is written. The registry's subjects come
  first, then the identity rows (§9.3). The subject's birth date and sex are
  filled by the first file that carries them (§9.1).
- `linkage show <code> [--why <text>]` decrypts every identifier of the subject
  and prints each with its type, its identity id and its source, then the
  subject's linkages, open and reversed; it writes one `read_audit` row per
  identifier with `why` as given. The actor of the audit and of `link` and
  `unlink` is the operating-system user (`USER`, else `USERNAME`, else
  `unknown`) until the doors of Wave 4 bring their own. A linkage's evidence is
  stored as `{"text": ...}`; `unlink` reverses an open linkage and refuses one
  already reversed.
- The report's `written` block gained `subjects_matched` (identifiers the
  store had met, step 3) and `identities_attached` (step 5).
- `linkage purge` arrives with `custody` in slice 6, so that what it deletes
  is listed before it can be deleted.

Settled while building custody (slice 6): `nils linkage purge --subject <code>
| --all` says what it would delete, asks at a terminal, and refuses without
`--yes` anywhere else. It deletes the subject's (or every subject's) `identity`
and `linkage` rows in one transaction and keeps the id types, the read audit
(the record that a read happened, not what was read) and the registry's
subjects; the purge is recorded as a `linkage-purge` job row that `status` lists.
A purged identifier is filed again only when its file is parsed again (changed,
or new): a run that finds the file unchanged does not read it (§5.2), so the
next digest does not undo a purge.

## 8. Stacks

Stack membership is decided per instance, without seeing the rest of the series,
from the signature v0 defined (`extract/stack_utils.py`): the SeriesInstanceUID
and fourteen values,

| field | normal form in the signature |
|---|---|
| EchoTime | rounded to 2 decimals |
| InversionTime | rounded to 1 |
| EchoNumbers | as text (backslash-joined) |
| EchoTrainLength | integer |
| RepetitionTime | rounded to 1 |
| FlipAngle | rounded to 1 |
| ReceiveCoilName | text |
| Exposure | as read |
| KVP | rounded to 0 |
| XRayTubeCurrent | rounded to 0 |
| NumberOfSlices | integer (v0's PET bed index) |
| SeriesType | text (v0's PET frame type) |
| orientation | `Axial`, `Coronal` or `Sagittal` from ImageOrientationPatient |
| ImageType | text (backslash-joined) |

with a null wherever the file has no value: a CT instance has null MR fields and
they do not affect grouping. Rounding is Python's `round`, half to even on the
exact binary value; Rust's `format!("{:.n}")` does the same and the tests pin the
tie cases (2.125 rounds to 2.12, 2.675 to 2.67, 2.5 to 2).

The orientation is v0's function: the normal of the image plane by the cross
product of the row and column cosines, normalized; the confidence is the largest
absolute component; the class follows the dominant axis, X to Sagittal, Y to
Coronal, Z to Axial, with ties resolved in that order; a missing, short or
degenerate orientation gives Axial with confidence 0.5. The confidence is stored
on the stack (`orientation_confidence`) and a value under 0.9 counts an
`orientation_oblique` diagnostic, which is the seed of a review kind Wave 2 may
emit.

`stack_key` is the unkeyed BLAKE2b-8 of the signature's canonical string (the
fourteen values in the order above, joined by `|`, a null as the empty string,
rounded floats written with their fixed number of decimals, without the series
UID, which is the row's parent). `stack_index` is the order of first appearance
within the series, continuing across batches, as in v0. The gate compares
partitions (which instances share a stack), not indexes (§12.3), and later waves
refer to a stack by id and key.

Settled while building stacks (slice 5):

- The canonical string is the fourteen values in v0's tuple order (EchoTime,
  InversionTime, EchoNumbers, EchoTrainLength, RepetitionTime, FlipAngle,
  ReceiveCoilName, Exposure, KVP, XRayTubeCurrent, NumberOfSlices, SeriesType,
  the orientation class, ImageType), joined by `|`, with `|` and `\` inside a
  value escaped by `\`; a rounded value that comes out as negative zero is
  written without its sign, as Python prints it. A text value is the
  catalogue's normal form (§6.3: trimmed, multi-valued joined by `\`), so a
  trailing space that v0 kept and an empty string that v0 kept as `''` are the
  only ways a key can differ from v0's tuple, and neither occurred on the
  spike's corpus. The key of the empty signature is `e4a6a0577479b2b4`, pinned
  in the tests beside a full MR signature.
- The series row and its detail row carry one instance's values of the
  thirteen series-level columns a stack signature is also made of (`image_type`
  and `image_orientation_patient` on `series`; the seven MR timing, echo and
  coil columns on `series_mr`; `kvp`, `x_ray_tube_current` and `exposure` on
  `series_ct`; `series_type` on `series_pet`), as in v0, and those columns are
  left out of the `field_disagreement` check (§9.1): instances that differ on
  them are the series' stacks, which the `stack` table records, not a
  disagreement. The list is derived from the catalogue (a series-level column
  whose source a stack column reads too), and a test pins the thirteen. Which
  instance's values those are was v0's answer, the first one walked, until
  slice 7: the row now keeps the smallest value of each column (§9.1), so a
  multi-stack series says the same thing whatever the walk order. The raw
  `image_orientation_patient` is in the thirteen but not in the signature
  itself, which carries the derived orientation class, so instances of one
  stack may still spell it differently; the same rule decides it.
- The stack row is written once, from the instance that created the stack, and
  is not decided the way the rows above are: its fourteen signature values are
  what defines the stack, so they agree by construction, except in the last
  decimals of the six the signature rounds and in the raw orientation. A hash
  per stack row, to decide those too, is Wave 2's if the fingerprint reads
  them.
- `orientation_oblique` is counted once per stack created, not per instance,
  when the class is known and the confidence is under 0.9; the unknown
  orientation (`Axial`, 0.5, from a missing or degenerate ImageOrientationPatient)
  is not oblique, or every non-image series would be. The sample is the class
  and the confidence to two decimals.
- The dry run counts `stacks` (distinct series and key pairs) beside `series`
  and `subjects`, and the report's `written` block counts `stacks_created`.
- On the nmosd corpus (508,045 files; 493,708 instances ingested, 10,539
  duplicate SOP instances, 3,798 quarantined: 124 not DICOM, 10 without a
  StudyInstanceUID, 3,664 of the non-image SOP classes §5.3 refuses, of which
  3,530 Secondary Capture, 102 Enhanced SR, 17 Encapsulated PDF, 13 Grayscale
  Softcopy Presentation State and two others) v1 makes 2,534 stacks over 2,165
  series (212 series with more than one stack, at most five), and the partition
  equals v0's on every one of the 2,165 series both hold, over every one of the
  493,708 instances both hold: v0's signature computed with its own code and
  pydicom 3.0.2 gives 2,534 groups, one per v1 stack. The 3,180 instances only
  v0 holds are the non-image SOP classes. 34 stacks are oblique (confidence
  0.81 to 0.89); none has an unknown orientation. The digest takes 32.5 s
  (15,600 files/s, 32 workers, 1.7 GB peak) and the same tree again 25.8 s,
  creating nothing. The mix corpus is compared the same way when it lands
  (`spikes/stacks/`).

## 9. The pipeline

### 9.1 Stages and bounds

```
walker pool (8) ──files──▶ parsers (workers) ──batches──▶ writer (1) ──▶ registry
                  cap 16k          cap 2 × workers, at most 16           linkage store
```

- **Parsers**: `--workers` threads, default the number of cores. Each reads a
  file (§6.1), extracts the catalogue, computes the stack signature, applies the
  identity rule (the value only; resolution is the writer's), and appends one
  row set to its current batch. A batch closes at `batch_rows` parsed files
  (2,000) or at eight times that many items of any kind, whichever first, and
  goes to the writer; the second bound is what keeps a tree of unchanged files
  moving through the writer in transactions of a size it can commit quickly.
  Refusals are rows too (`source_file` with a reason), so quarantine is written
  in the same transactions as the data.
- **Writer**: one thread, one transaction per batch, in this order: subjects
  (resolution, §7.4, with an in-memory map of identifier lookup to subject id),
  studies, series and their detail tables (`ON CONFLICT DO NOTHING`; the
  conflicting rows' ids fetched after; first instance wins, and a later
  instance's disagreement on a series or study field counts a
  `field_disagreement` diagnostic per field with the two shapes as its sample;
  the cache keeps an 8-byte hash of each row's values beside its id, so the check
  costs one hash per instance and no values in memory), stacks (a per-series map
  of key to id, an LRU of 200,000 series in memory, the registry behind it),
  instances (`ON CONFLICT DO NOTHING` on the SOP UID; a conflict is a `duplicate`
  file), `source_file` rows, diagnostic increments, batch counts. Then commit, then
  the linkage store's rows for the subjects created (§9.3).
- **Bounds**: every channel is bounded (16,384 paths between the walker and the
  parsers, two batches per worker between the parsers and the writer, and never
  more than sixteen), so a slow writer stops the parsers and a slow disk stops
  the walker. The only structures that grow with the corpus are the subject map
  (a few hundred bytes per subject) and the LRU caches; per-subject
  materialization, id lists and pickled batches do not exist (02). The Wave 1
  target for peak RSS is 4 GB at any corpus size, against D6's ceiling of 16.

Settled while building the writer (slice 3):

- The writer keeps, beside each cached subject, study and series id, one 32-bit
  hash per catalogue column of that row. A later file of a known row is
  compared hash by hash: a stored null against a value is a fill (the null was
  no value, and the first file that has one supplies it, as an `UPDATE` of that
  column alone); two values that differ are a `field_disagreement` counted per
  batch and kind, whose sample is `<table>.<field>=<shape>`. The columns that
  vary per instance by nature (`VARIES_PER_INSTANCE` in the catalogue) are left
  out of the series comparison, and a file of another modality than the series
  row is compared on the series columns only, never on the other modality's
  detail table.
- Settled while reading the first gate run (slice 7): a field two files
  disagree on is **decided by value**, not by which file arrived first. The row
  keeps the smaller of the two in the catalogue's canonical text order, on
  every column of the subject, study and series rows, whether the column is one
  the comparison above counts or one it leaves out. A row is therefore the same
  whatever order the walk and the workers gave the files, which "the first
  record stands" was not: the first gate run found the instances of single
  series carrying up to twenty-six spellings of `sequence_name`, two
  acquisition matrices, two slice spacings and two orientations, and each of
  them made the registry depend on a race. The rule needs nothing stored, since
  `min` does not care in what order it is applied: a run that resumes, a batch
  that arrives late and a second digest of the same tree reach the same row.
  The cost is one read of the stored value the first time a field of a row is
  decided (kept with the cached row afterwards) and an `UPDATE` of that column
  when the smaller value is the new one; both are rare, because almost no field
  of almost any row is ever disagreed about. A file that carries a null decides
  nothing: a null is no value. The fill rule covers every column the same way
  now, the per-instance and stack-signature ones included; only the
  disagreement *diagnostic* still leaves them out, since an instance that
  differs there is the series' stacks, not a disagreement (§8).
- Cache misses are one keyed select per batch and table (`WHERE uid IN (...)`),
  never a query per file; each of the three caches holds 200,000 rows.
- The batch queue was two batches per worker with no ceiling. On a 64-core host
  that is 128 closed batches and, with the open batch each parser holds, a
  quarter of a million parsed rows in flight: the million digested at 4.1 GB
  peak RSS on SQLite and Postgres alike, and the dry run alone at 2.9 GB. Capped
  at sixteen, the same runs peak at 2.0 to 2.1 GB and the SQLite digest is a
  third faster (§9.2), since a full queue means the writer is the wall either
  way. On eight workers the dry run peaks at 0.4 GB. What remains is the open
  batches, one per parser; the budget slice (§14, 8) decides whether
  `batch_rows` should shrink with the worker count.
- The epoch advances once per committed batch, including a batch that held only
  unchanged files: the counter says "the registry was examined", not "the
  registry grew", and a consumer that polls it sees every run.

Settled while building stacks (slice 5): the stack step sits between the
series and the instances. Its cache is keyed by series id and stack key, holds
200,000 entries like the other three, and a miss loads every stack of the
series in one keyed select per batch (`WHERE series_id IN (...)`), which also
yields the next `stack_index`; the batch's new stacks are then inserted with
`ON CONFLICT DO NOTHING` on `(series_id, stack_key)` in the order their first
instances came, and the rows the insert returned are the ones this batch
created, which is where `stacks_created` and `orientation_oblique` are counted.

### 9.2 Backends

SQLite: WAL, `synchronous=NORMAL`, one connection for the writer, a 2,000-row
transaction lands in the low milliseconds; the spike wrote 500,000 instance rows
in a few seconds through `rusqlite`. Postgres: a transaction per batch, rows sent
through `COPY` into temporary tables and merged with `INSERT ... SELECT ... ON
CONFLICT`, one round trip per table per batch, so that a 1 ms network does not
multiply by the row count. The dialect layer hides both behind `insert_many`.
v0's writer committed every hundred rows through one connection with the
database's parallelism turned off; the budget (§12.5) is what shows whether the
writer is the wall, and slice 3 of §14 measures both paths before the parser is
tuned.

Measured while building the writer (slice 3), on the synthetic million of §12.6
(1,011,783 files: 1,000,000 instances, 9,783 duplicate paths, 2,000 refused; 862
subjects, 1,711 studies, 7,622 series; 5.7 GB at 4 KB of pixel data per file) on
the development container (64 cores, 256 GB, the corpus on local NVMe and warm in
the ZFS cache, PostgreSQL 17 on the same host with 8 GB of shared buffers), 64
workers, `batch_rows` 2,000, the binary at the end of the slice:

| run | elapsed | files/s | peak RSS | batches |
|---|---|---|---|---|
| dry run | 5.8 s | 175,000 | 2.0 GB | |
| SQLite, first digest | 32.3 s | 31,300 | 2.1 GB | 506 |
| Postgres, `COPY` and merge | 65.3 s | 15,500 | 2.0 GB | 505 |
| Postgres, multi-row `INSERT` | 70.0 s | 14,500 | 2.0 GB | 505 |
| SQLite, the same tree again | 65.9 s | 15,400 | 0.3 GB | 82 |
| Postgres, the same tree again | 37.3 s | 27,100 | 0.2 GB | 82 |

Every count is identical across the three backends and equal to the dry run's
and the generator's manifest (§12.6). The registry is 538 MB on SQLite and
1,015 MB on Postgres for the million. The Postgres wall is the server: during
the digest one backend sits at ninety percent of a core merging the temporary
tables into the indexed ones, while the parsers idle on the full queue, and the
two bulk paths differ by seven percent because the transfer is not where the
time goes. `COPY` stays the default; `NILS_PG_BULK=insert` selects the other
path, kept so that the comparison can be repeated on the baseline host and over
a real network, where the round trips the multi-row insert makes (a statement per 1,000 rows,
or fewer when the row is wide, under the protocol's 65,535 parameters) may
weigh more than they do on localhost.

Measured while building identity (slice 4), same host and knobs: the million on
SQLite takes 35.3 s (28,600 files/s) against 32.3 s before, the resolution
through the linkage store costing a tenth, with identical counts and a linkage
store of 212 KB for 862 identities. The nmosd corpus (508,045 files, 44
subjects, 82 studies) under `blake2b-8` with a throwaway key digests in 32 s and
again with `--restart` in 26 s, with 44 identifiers matched, none created and
none attached: every returning identifier lands on its subject.
The temporary tables are created once per connection and truncated before each
`COPY`. The second pass over an
unchanged tree is bound by the writer touching a million `source_file` rows
(`batch_id` and `seen_at`, §5.2), which on SQLite costs more than ingesting
them did; the resume slice (§14, 6) owns that number. What the table does not
say: a real digest walks NFS from a cold cache, and there the reader, not the
writer, is expected to be the wall, which is the budget's measurement (§12.5).

Measured while building jobs and custody (slice 6), same host and knobs: the
touch was never the wall. The second pass over nmosd took 25 s with 32 workers
and 5 s with one, and on a copy of the registry the pieces of the SQLite side
add up to four seconds (the lookup of 508,045 records in 3,178 directory
queries 2.2 s, the touch of every row 2.0 s, a bare `find` with a stat of
every file 1.7 s). The rest was the parsers' queue: half a million unchanged
files sent through it one by one, each waking a worker with nothing to do,
and the workers' contention for the next slowing the stage that fed them. With
the unchanged files batched by the resume stage (§5.2) the second pass over
nmosd takes 4.7 s (108,000 files/s) on 32 workers and the same on one, against
25.8 s before; the million's second pass takes 5.4 s on SQLite and 18.2 s on
Postgres, against 66 s and 37 s, with identical counts on both (862 subjects,
1,711 studies, 7,622 series and stacks, 1,000,000 instances). The first pass
is what it was: 33.1 s on nmosd, 35.5 s for the million on SQLite and 75.5 s
on Postgres `COPY`, whose second pass is now the round trips of the lookup
and the touch, a number for the baseline host (§14, 8).

### 9.3 Two stores, one order

The registry commits first, then the linkage store. A crash between the two leaves
a subject without its `identity` row, which the next run repairs by attaching
(§7.4, step 5); the reverse order would leave an identity pointing at a subject
that does not exist. There is no two-phase commit and no need for one.

## 10. Jobs, resume and cancellation

`nils digest` opens a `job` row (`kind = digest`, the batch's config as args) and
heartbeats every ten seconds. A second `digest` on the same registry refuses while
a job's heartbeat is fresh (`< 60 s`); a job whose heartbeat is stale is taken over
by marking it `failed` and starting the batch that resumes it (§5.2). One SIGINT
finishes the batch in flight, commits, and marks the job `cancelled`; a second
SIGINT aborts the transaction, which rolls back cleanly because every write is in
one. Nothing half-written ever becomes a row.

Progress is columns and counts, not a ledger: the job row's `progress` (files
seen, parsed, ingested, duplicate, quarantined, rate over the last minute, elapsed,
remaining when the walk has finished and the count is known) is updated every ten
seconds and printed to stderr on a TTY as one updating line, or as one JSON line
per interval with `--json`. `nils status` reads the job rows and the batch counts,
and `nils status --batch <id>` prints the full report of §11.

Settled while building the writer (slice 3): the job row and its heartbeat, the
refusal while another job's heartbeat is fresh (exit code 3), the takeover of a
stale one (marked `failed` with the reason in `error`, then the new batch
resumes, §5.2), the batch row with its resolved config and its counts, and
`nils status` (the registry's metadata, the running jobs, the last batches; with
`--batch`, one batch's counts). The heartbeat carries the same JSON the
progress line prints, so a `status` from another shell sees where a digest is.
SIGINT handling is slice 6's, with the kill-and-resume test that proves it.

Settled while building jobs and custody (slice 6):

- One SIGINT (or SIGTERM) asks the run to stop: nothing new is read, everything
  already parsed is written and committed, the batch and the job are marked
  `cancelled`, the report says `stopped`, and the exit code is 130. A second
  signal asks for an abort: the transaction in flight rolls back, the batch is
  marked `cancelled` with what was committed before it, the report says
  `aborted`, exit code 130 again. Either way the next `digest` resumes (§5.2)
  and a cancelled run marks nothing `gone`.
- A job whose process is gone (this host, no such pid) is taken over at once,
  without the 60 s heartbeat window; a stale heartbeat of a process that may
  still be alive waits the window as before.
- A batch marked `failed` records `reparse_from`, the last `seen_at` of its
  files; the run that resumes it parses the files of that last second again,
  which repairs the §9.3 window (a registry commit without its linkage commit):
  their subjects are matched or created and their identities attached (§7.4,
  step 5).
- The kill-and-resume tests script the moment with
  `NILS_DEBUG_STOP=<stop|abort|interrupt|terminate|kill|kill-inside>:<n>`, a
  test hook and not a knob: act once `n` batches have committed, `kill-inside`
  ending the process inside the next transaction with its rows written. A run
  killed after a commit, killed inside a transaction, stopped, aborted or
  interrupted by a real SIGINT resumes to the same subjects, studies, series,
  stacks, instances, source-file statuses and identity rows as an uninterrupted
  run, on SQLite and on Postgres.
- On CT 110 the same holds over nmosd (508,045 files, 32 workers, SQLite):
  a run killed after 40 commits (5 s in), one killed inside the 61st
  transaction (8 s), one stopped by SIGINT after 80 commits (18 s, exit 130)
  and one aborted after 20 (3 s) each resume to the uninterrupted run's 44
  subjects, 82 studies, 2,165 series, 2,534 stacks, 493,708 instances,
  493,708 ingested, 10,539 duplicate and 3,798 quarantined files and 44
  identities; the resumes take 29 s, 25 s, 18 s and 30 s, the killed runs'
  batches end `failed` with `reparse_from` set and the signalled ones
  `cancelled`, and a third pass over any of them takes 5 s. What differs is
  the review items: a run files one `ingest.quarantine` item per class it
  quarantined itself, so a resumed tree has between one and six, never one
  per file and never twice for a file.

## 11. Knobs, diagnostics and the report (C37, D7)

The digest declares its knobs as data; `nils digest --describe` prints them with
their defaults and types, and they are recorded, resolved, in `ingest_batch.config`:

| knob | default | note |
|---|---|---|
| `files` | `all` | §5.2 |
| `sop_classes` | v0's nine | §6.1 |
| `modalities` | `MR`, `CT`, `PT` | |
| `identity` | PatientID, then StudyInstanceUID | §7.3 |
| `workers` | cores | |
| `walk_threads` | 8 | |
| `batch_rows` | 2,000 | |
| `charset_fallback` | `iso-8859-1` | §6.1 |
| `retry_quarantine` | false | |
| `name` | the root's basename and the date | the batch's label |

This is the seed of the affordance API (`describe` and `diagnose`, C20): in Wave
4 the same declaration is served over HTTP and an agent proposes changes to it as
review items; in Wave 1 a human edits a file and runs the digest again.

Settled while building the writer (slice 3): `workers`, `walk_threads`,
`batch_rows`, `files`, `retry_quarantine` and `name` are flags of `nils digest`
and `restart` and the binary's version are recorded beside them in the batch's
config (a dry run writes no batch row);
`sop_classes`, `modalities` and `charset_fallback` are declared with their
defaults and not yet settable from the command line, and `identity` is slice
4's, with the rule file. The report gained `unchanged`, `skipped` by reason,
`walk_errors` and, when writing, a `written` block (the batch id, the epoch
after, batches committed, instances ingested and duplicate, files changed,
quarantine kept, paths gone, subjects, studies, series and stacks created).

Settled while building identity (slice 4): `identity` is set with
`--identity-rule <file>` (§7.3) and recorded in the batch's config as the rule
it resolved to, with the file's path; the `written` block gained
`subjects_matched` and `identities_attached` (§7.4). The `identity.collision`
review item exists from this slice; the `ingest.quarantine` items are slice
6's, with the quarantine list they summarize.

Settled while building stacks (slice 5): the report counts `stacks` beside
`studies`, `series` and `subjects`, in the dry run too, and the `written` block's
`stacks_created` is filled; `orientation_oblique` is counted per stack created
(§8) and its samples are the class and the confidence (`Coronal 0.71`).

The diagnostics are counted per batch and kind, with `scope` and `ref_id` where
one row is the subject and a `sample` of at most ten shapes:

`walk_error`, `charset_unknown`, `charset_lossy`, `value_invalid`,
`field_disagreement` (per series or study, per field), `identity_unparsed`,
`identity_fallback`, `subject_field_disagreement` (birth date or sex),
`file_changed`, `orientation_oblique`, `series_multi_study` (a
SeriesInstanceUID seen under two StudyInstanceUIDs: the instances are ingested
under the first study and the disagreement is counted, because a series
belongs to one study by the standard, and the file disagrees with the
standard, not with the digest), `ragged_length` (§6.1: a length a fixed-size
VR cannot hold, repaired in memory so the header could be read).

The report (`ingest_batch.counts`, printed at the end and by `nils status --batch`)
is what C37 calls "the diagnostics report": counts per quarantine class, per
diagnostic kind, subjects created and matched, studies, series and stacks
created, instances ingested and duplicate, the rate, the elapsed time and the
peak RSS. Without an agent, that is where the digest stops, which is v0 today made
legible; with one, in Wave 4, the report is what the agent reads.

Review items in Wave 1: `ingest.quarantine` (one per batch and class, §5.3) and
`identity.collision` (§7.1). Everything else is a diagnostic, because D7's rule is
that no-evidence is a column, not an item, and a queue of thirty thousand parse
failures is a list, not thirty thousand decisions.

## 12. The gate

The gate is the parse-and-compare of 11: v1's digest of the sources behind the
live registry against v0's extraction of the same files, on the baseline host of
C6, with every divergence classified. Its numbers land in the design record when
they exist; the bars are these.

### 12.1 The compare tool

`tools/v0-compare/`, Python over DuckDB, reads v0's registry through the Postgres
scanner (read-only role, the live database untouched, F1) and v1's through the
SQLite or Postgres scanner, joins on the UIDs at each level and on the subject
code, and emits per field: rows compared, agreeing, both null, one null, both
present and different; per series: whether the stack partition is identical;
per subject: whether the code and its studies match. Values are normalized to
§6.3's forms on both sides before comparing. Divergences are grouped by field and
pattern and written as a report with counts and shapes only; the classed fields
show no value. The tool is public (nothing in it is about our data); the runs are
private and their reports are summarized in the record.

Settled while building the compare tool (slice 7): v0 is read either through the
Postgres scanner with a DSN that is never printed, or from the zstd CSVs that
`tools/v0-compare/export.sh` takes in a session forced read-only
(`default_transaction_read_only`), and either lands in one DuckDB file
(`v0-compare extract`); `v0-compare compare` then reads v1 through the SQLite or
the Postgres scanner. A SQLite registry is read by its declared types, not as
text: the writer binds every value typed by the catalogue, and SQLite's text
conversion of a REAL keeps 15 significant digits where a float32 widened to a
double (B1rms) needs 17, so a text read made hundreds of series differ on
nothing but that in the first real run. Each v0 digest's file-name mode (`--v0-files all | dcm | DCM
| all_dcm | no_ext`) names the union `files` filter v1 is digested with (§5.2),
so that both sides see the same candidates. Normalization is symmetric: v0's
Python list literals and multi-valued strings to the backslash form, numbers to
one canonical spelling, dates and times to ISO, JSON to presence; the six stack
columns v0 stored rounded are compared at v0's decimals. Rows that still differ
are read back as shapes and grouped by field and pattern (`case`, `whitespace`,
`number-format`, `rounded`, `scale`, `list-order`, `subset`, `prefix`,
`null↔value`, or the shapes on both sides; a quasi-identifying or identifying
field collapses its shapes to `other`), a residual beyond 200,000 rows sampled
deterministically and the report saying so. An adjudication file (TOML:
`[[divergence]]`, `[[partition]]`, `[[instance]]`, glob patterns, a class and a
note each) classes every group, and the bars apply one rule to the classes: a
group classed `accepted` or `v0-bug` is excused from the bar it would otherwise
fail, a `v1-bug` and an unclassified group count in full. The floor of §12.3 is
therefore measured net of what the record has explained, and the bar that
matters, the classification, is unchanged. The report (`report.md`,
`report.json`) holds counts, shapes, classes and notes; the exit code is 0 only
when every bar passes; the `work.duckdb` beside it holds values from both
registries and stays where the registries are.

### 12.2 Instances

Every instance v0 holds under a digested root is in v1, and every v1 instance
under that root is in v0 or is explained (v0's extension filter skipped it; v0's
SOP-class filter refused it; v0's resume skipped it: on a resumed digest,
`plan_subject_series` dropped a file whose SOP UID sorted at or below its series'
resume token, or below the legacy token when the series had none, whether or not
it had been written). The gate runs with `files` matched to each v0 digest's
extension mode so that the first case is measured, not assumed, and it counts the
third.

Settled in slice 7: instances are paired on the SOP Instance UID. A v0 instance
v1 does not hold is classed by what v1 knows of its path: refused under a
quarantine class, ingested under another SOP, or never seen, in which case the
tool looks on disk under the root (`--root`) and says whether the file is there
(v1 missed it) or not (v0 holds a file that is gone). A v0 path is relative to
its cohort's root, so a subject listed under several cohorts has instances under
roots other than the compared one: an absent path whose subject is in several
cohorts is reported apart from one whose subject is in a single cohort, since
only the second is a file v0 holds and nobody has. The check is bounded by
`--fs-cap` (a million paths by default, `0` for all of them) and what lies
beyond the cap is reported as unchecked, never as absent. Only the two classes
where v1 saw the path pass the bar by themselves; the rest need the adjudication. A
v1 instance v0 does not hold is `in v0 under another subject or cohort`, `name
outside v0 mode`, `sop class not in v0's nine`, `modality not in v0's`, `resume
skip` (its SOP sorts at or below the highest one v0 holds for the series, or
below the legacy token when the series has none), `series absent from v0` with
what v0 knew (the study, the subject, neither), or `unexplained: series known to
v0`, and only the last one fails.

### 12.3 Fields and stacks

Per field, after normalization: 100 percent agreement on the UIDs, modality, the
SOP class, instance number, rows, columns, number of frames, the fourteen
stack-defining values and the orientation class; at least 99.9 percent on every
other field of the catalogue; and every group of divergences classified as **v0
bug** (v1 is right; the record lists the v0 behaviour), **v1 bug** (fixed before
the gate closes, with a fixture) or **accepted change** (the normalization of
§6.3, the multi-valued literals, the padded time fraction, birth date and sex at
ingest, names not stored; each listed once with its count). Per series: the stack
partition identical for at least 99.9 percent of series with more than one stack,
the rest classified the same way. "At least 99.9 percent" is a floor for the
comparison to be worth reading; the bar is the classification, which has to be
complete.

Settled in slice 7: the stack partition of a series is matched by membership on
the instances both sides hold, and a series whose partition differs is reported
by its shape (`v0 2 stack(s), v1 1, 0 matched`) and classed through the
adjudication's `[[partition]]` rules. Two series columns carry the first
instance's value on both sides (`media_storage_sop_instance_uid` and
`image_position_patient`, the engine's `VARIES_PER_INSTANCE`), and which instance
is first follows the walk order, so two digests of one corpus disagree on them by
construction; the tool compares and reports them and classes their divergences
`accepted` itself unless the adjudication says otherwise. The thirteen
stack-signature columns of the series tables (§8) carry the first instance's
value the same way, but only where a series has several stacks do the instances
differ on them by definition: the tool derives the thirteen from the catalogue
as the engine does, groups a divergence of one of them apart when either side
gives the series more than one stack (the pattern with ` (multi-stack)`
appended) and classes that group `accepted` itself; the same field in a
single-stack series keeps its plain pattern and needs the adjudication, because
there the two sides read one value from files that agree. v0's `study.modality`
is the first file's modality where v1's `modalities_in_study` is the study's, so
that pair is expected to show `subset` or `null↔value` and is declared accepted
in the adjudication of each run.

### 12.4 Subjects

With the registry created under `blake2b-8` and the v0 key, and v0's CSV maps
imported through `nils linkage import`, every subject code in v0 is a subject code
in v1 and every v0 study hangs off the same code. This is C36's promise and it
is measured, not assumed. The two places v0 could not keep a person whole (a
digest with an empty key; a study re-linked under a second cohort) are expected to
show up as v0 bugs here, and the report says how many people they touched.

Sessions, the same way: the compare tool groups v1's studies under the default
scheme of §4.4 and checks the groups against v0's `event` rows, one for one. The
one known divergence is declared now as an accepted change: v0 opens an event per
modality, so a subject scanned on MR and CT the same day has two, and v1's default
scheme has one, since one occasion is one session whatever the scanners; the live
registry holds exactly one such day.

Settled in slice 7: `v0-compare linkage-csv` writes the CSV `nils linkage import`
takes (a code and its identifier per row, mode 600, deleted after the import),
lists v0's id types with their counts, and counts an identifier under two codes
or a code under two identifiers, leaving both out with `--drop-collisions` since
the import would refuse them. With the v0 key in a file (`--key-file`, read and
dropped) the report classes how v0 derived each code: `key-consistent` (the key
and the identifier reproduce it), `cohort-hashed` (the hash under v0's fallback
seed, the cohort's name, for a digest whose key was empty), `no identifier`, or
`other` (a CSV-mapped code, or an identifier v0 overwrote); the counts say how
many people the two v0 bugs of this section touched. Sessions are v1's (subject,
study date) groups against v0's events, and the surplus on days with several
events is the per-modality case above, counted and accepted.

### 12.5 Performance (D6, C6)

On the baseline host (8 cores, 64 GB, sources over NFS, v0 measured on it first),
a full digest of the live corpus sustains at least 1,000 files/s from a cold cache
with peak RSS under 16 GB; the Wave 1 target is 4 GB. The budget is restated in
the record from the measurement (files/s and RSS on that host), and the record's
"thirty million instances in a working day" is what those numbers mean. The
spike's corpora (one study's raw tree of 508,045 files; the mix corpus of 2,568
series) are the development runs on the way there, and every run's numbers are
recorded with the binary's version.

### 12.6 Small-machine CI and the six targets

The synthetic generator of the spike moves to `tools/synth/` and grows to a
one-million-instance corpus generated at CI time from a fixed seed (C10: nothing
real goes public). A benchmark job digests it end to end on SQLite on a standard
runner with a hard regression gate: files/s not below 80 percent of the recorded
baseline for that runner class, RSS not above a fixed cap; the baseline is a file
in the repository that a deliberate commit updates. The release workflow builds
`nils` for the six targets of the spike and runs `nils init` and `nils digest` on
a small synthetic tree on each. The test suite is green on SQLite and on
Postgres 16.

Settled while building the writer (slice 3): the generator is the `corpus`
example of `nils-dicom` (`cargo run --release -p nils-dicom --example corpus`),
written on the crate's own Part 10 writer, so that what the reader reads is
what the writer wrote and no second implementation of the file format exists.
From a seed it writes subjects with one to three studies of one to eight series
each, MR, Enhanced MR, CT and PT in v0's proportions, in the three file forms
and both VR encodings, three character sets, a share of subjects without a
PatientID, one duplicate path per hundred instances and one refused file per
five hundred, cycling through the classes of §5.3; it prints a manifest with the
counts a digest must reproduce. A million instances take 26 s to write and
5.7 GB at 4 KB of pixel data per file (`tools/synth/README.md`). The CI-time
generation and the regression gate are slice 8's, when the baseline is known.

### 12.7 Corpus hygiene

Every adjudicated divergence becomes a fixture in `nils-dicom`'s tests: a
synthetic file that reproduces the shape (never the file, never a value from it),
with the expected values. The fixtures are the parser's regression suite and the
start of the verified corpus (C12).

Settled in slice 7: the compare tool has a gate of its own, synthetic and in CI
(`v0-compare` job). `tools/v0-compare/tests/v0shape.py` projects a v0-shaped
export out of a v1 registry digested from the generator's corpus and injects
known divergences (a case change, a null, another institution, dropped and
phantom instances in each of v1's classes, a split stack, another first
instance's echo time in a multi-stack and in single-stack series, a subject
listed under a second cohort, a recoded subject, an identifier per subject); the
tests assert that a clean projection passes and
that every injected divergence lands in its class, the linkage CSV round trip
through `nils linkage import` files every code as already known, the key
classes come out right with the right key and `other` with a wrong one, and the
same holds against a v1 registry on Postgres. The generator gained
`--same-day-percent` (a later study on the day of the previous one; zero by
default, so existing seeds write what they wrote) and its manifest
`same_day_studies`, which is what the session check counts against.

## 13. CLI reference for Wave 1

```
nils [--registry <dir>] <command>              # else NILS_REGISTRY, else the working directory
nils init [--backend sqlite|postgres] [--dsn ...] [--schema nils]
          [--scheme blake2b-32|blake2b-8] --key <name> [--display-length 12]
          [--session-scheme <file>]
nils key add <name> [--from-file <path>]      # otherwise from stdin
nils key list | remove <name>
nils digest <root> [--name <label>] [--workers N] [--walk-threads N] [--batch-rows N]
            [--files all|dcm|no-ext|<glob>[,...]] [--identity-rule <file>]
            [--retry-quarantine] [--restart] [--dry-run] [--describe] [--json]
nils status [--batch <id>] [--json]
nils quarantine list [--batch <id>] [--class <c>] [--json]
nils review list [--kind <k>] [--status <s>] [--json] | show <id> [--json]
nils review apply <id> --decision <d>          # Wave 4
nils linkage import <csv> --id-type <t> --id-column <c> --code-column <c>
nils linkage id-type add <name> [--description ...] | list
nils linkage link <code-a> <code-b> --evidence <text> | unlink <id>
nils linkage show <code>                       # decrypts; writes a read_audit row
nils linkage purge --subject <code> | --all [--yes]   # says what it deletes; listed in custody
nils custody [--json | --markdown]
nils doctor
```

`--dry-run` walks and parses and prints the report without writing: the way to
see what a tree contains before digesting it. `nils custody` prints, for the
registry, the linkage store, the key store, the quarantine list, the job records
and the logs: where it lives, which classes it holds, how long it is kept, and
the command that exports, changes or deletes it (C38); every command it names
exists in this list, and nothing is retained that the table does not show.

Settled while building the writer (slice 3):

- A registry is a **home**: the directory `--registry` names (else
  `NILS_REGISTRY`, else the working directory), holding `nils.toml` (backend,
  DSN, Postgres schema, key store path), the key store `keys/` and, on SQLite,
  `registry.db` and `linkage.db`. On Postgres the two stores are the schemas
  `<schema>` and `<schema>_linkage` of the database the DSN names; `NILS_DSN`
  in the environment overrides the DSN in `nils.toml`, so a file checked into a
  project need not hold a password. `nils init` refuses a home that is already
  one and a schema that already holds a registry.
- The key is added before `init` (`nils key add`), `init` names it and reads it
  once to prove it is there, and every later command reads it from the store by
  that name; `key remove` refuses the key the registry uses.
- Exit codes: 0 done; 1 the command failed; 2 the arguments or the
  configuration are wrong; 3 another job holds the registry (§10).
- `--identity-rule` is listed and not yet accepted (slice 4); `custody`,
  `quarantine`, `review`, `linkage` and `doctor` arrive with the slices that
  give them something to do.

Settled while building identity (slice 4): `--identity-rule <file>` is accepted
(§7.3); `nils linkage import`, `id-type add | list`, `show`, `link` and `unlink`
exist as listed, `show` takes `--why <text>` for the audit row, and `import`'s
column flags default to `identifier` and `code` (§7.4). `linkage purge` comes
with `custody` (slice 6); `quarantine`, `review` and `doctor` still wait for
their slices.

Settled while building jobs and custody (slice 6):

- Exit code 130: the run was stopped or aborted by a signal; what was read is
  written, and the report says which (§10).
- `nils custody` lists the configuration file beside the six stores named
  above: for each, where it lives (on SQLite the files with their mode and
  size, on Postgres the schema and the DSN with its password replaced), the
  classes it holds (§4.3), the counts of the moment, how long it is kept, and
  the commands that read, change, export and delete it. The CLI test walks the
  home and checks that every file under it is listed and that every command
  the table names exists. `--markdown` renders the deployment's record without
  the live counts; [`docs/reference/custody.md`](../reference/custody.md) is
  that page for a SQLite home, and a test keeps it current.
- `quarantine list`, `review list | show` and `linkage purge` exist as listed
  (§5.3, §7.4); `review apply` is Wave 4's and `doctor` still waits.
- `status` lists the purges under "other jobs" (`other_jobs` in `--json`)
  beside the running jobs and the last batches; the job kinds so far are
  `digest` and `linkage-purge`.

## 14. Order of work

Each slice is done when its "done when" holds; the slices are sequential where
they share the schema.

1. **Skeleton.** The workspace, the SPDX headers, the CI: lint and test on SQLite
   and Postgres, the six-target build from the spike's matrix, the synthetic
   generator moved. *Done when:* `nils --version` builds on six targets and the
   empty test suite runs on both backends.
2. **Reader and walker.** `nils-dicom` with the catalogue, the fallbacks and the
   normalization; the walker with its classes; `nils digest --dry-run` prints the
   report. *Done when:* the dry run over the spike's two corpora refuses only what
   the spike's harness refused, and the mix corpus (sixteen manufacturers,
   twenty-six years, six transfer syntaxes) parses with every failure classified.
3. **Schema and writer.** The declaration, both dialects, the migrations, the bulk
   paths, the writer with its caches. *Done when:* the synthetic million digests
   on SQLite and Postgres with the same counts, and the bulk path on each is
   measured and recorded.
4. **Identity.** The two schemes, the key store and the derived subkeys, the
   linkage store, the identity rule, `linkage import`. *Done when:* the fixtures
   of §7.1 and §7.2 pass, a CSV round
   trip reproduces its codes, and a digest of one study's tree under `blake2b-8`
   with a test key lands every returning identifier on one subject.
5. **Stacks.** The signature, the key, the index, the orientation. *Done when:*
   the partitions on the spike's corpora equal v0's for those series. Landed
   with the nmosd half of the check equal on every series (§8); the mix corpus
   is checked the same way when its copy completes, and the pack-format
   prototype (C11) may start.
6. **Jobs, resume, status, custody.** *Done when:* a digest killed at any point
   resumes to the same counts as an uninterrupted one, and `nils custody` lists
   every file the tests created. Landed: the kill-and-resume tests of §10 hold
   on SQLite and Postgres and over nmosd on CT 110, the CLI test finds every
   file under the home in `custody --json`, and the second pass over an
   unchanged tree went from 25.8 s to 4.7 s on nmosd (§9.2).
7. **The compare tool and the gate runs** on the baseline host: the spike's
   corpora first, then the live corpus study by study, each run's report in the
   record; the session check of §12.4 is part of the tool. *Done when:* §12.2 to
   §12.4 hold and every divergence is classified. Landed so far: the tool
   (`tools/v0-compare/`, §12.1 to §12.4 as amended) with its synthetic gate in
   CI (§12.7) and the walker's union filter (§5.2); the runs on the baseline
   host follow as v0's export and the cohorts' trees arrive there, nmosd first,
   and the slice closes with their reports in the record.
8. **The budget.** v0 measured on the baseline host (C6); v1 tuned against it;
   the CI benchmark's baseline recorded. *Done when:* §12.5 and §12.6 hold and
   the record's D6 numbers are restated.

The pack-format prototype (C11) starts when slice 5 is done and runs beside 6 to
8, on the fingerprint fields the mix corpus yields; it has its own criteria in the
record and does not gate Wave 1.

## 15. Open questions carried into the wave

- **Per-frame stacks.** A multi-frame object is one instance with first-frame
  values here, as in v0. Whether Enhanced MR frames with different echo times
  should be stacks of their own is Wave 2's to decide with the fingerprint pass
  in hand; Wave 1 records `number_of_frames` and the charset so that the
  question can be counted.
- **The default file filter.** `all` is the honest default; the gate's runs say
  whether the sidecars in real trees make it expensive or noisy.
- **Hashing.** No content hash in Wave 1. The anonymizer and the custody page
  may want one; if so it is a knob (`--hash`) that reads the whole file, and the
  cost is measured before it is on by default.
- **The Postgres bulk path.** Answered by slice 3 (§9.2): `COPY` plus merge
  digests the million in 65 s against 70 s for multi-row inserts on the same
  host, both bound by the server's merge on one core, so `COPY` stays the
  default and the insert path stays selectable for the measurement over a real
  network. What the wave still has to learn is whether the Postgres registry
  should be digested with more than one writer connection, which is the
  budget's question (§14, 8) if the gate's runs show the server as the wall
  on the baseline host too.
- **Identity rule grammar.** Named groups over one field cover the two cases C37
  named; a rule that combines fields, or normalizes case and separators before
  hashing, waits for a source that needs it, and is then a knob, not code.
  Slice 4 built the grammar of §7.3 with one group read (`id`), one fallback
  and no normalization beyond trimming; the mix corpus and the gate's runs say
  which source asks for more.
- **Session schemes** are fixed in their shape here (§4.4) and first applied in
  Wave 3. What Wave 3 decides with the exporter in hand: the window the group
  wants as its registry default (zero reproduces v0; the live registry's
  fourteen-day pairs are the argument for more), where overrides are written and
  how they are reviewed, and whether `months` under a clinical anchor waits for
  Wave 4 or comes earlier.
- **The baseline host** is described in the record with its deviations from the
  VM that C6 asked for.
