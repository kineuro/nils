# Wave 1: parse and digest

*Specification, first draft 2026-09-02; amended the same day after review (studies
and sessions, §4.4; one key, §7.2; the toolchain, §3). This is the spec the first
slice of engine code follows; the design record ([`docs/decisions/`](../decisions/)) says why each
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
`serde`/`serde_json`/`serde_yaml`, `regex`, `crossbeam-channel`, `tracing`. DuckDB
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

Registry (`registry.db`, or the `nils` schema on Postgres). Every table that the
digest writes carries `first_batch_id`, the batch that created the row; rows are
never rewritten by a later batch in Wave 1 (a changed file is a diagnostic, §5.2).

- `registry_meta`: `key`, `value`. Holds `registry_id` (a UUID), `schema_version`,
  `epoch`, `created_at`, `pseudonym_scheme`, `key` (a key *name*, §7.2),
  `display_length`, `session_scheme` (json, §4.4).
- `job`: `id`, `kind`, `args` (json), `state` (`queued`, `running`, `done`,
  `failed`, `cancelled`), `pid`, `host`, `started_at`, `heartbeat_at`,
  `finished_at`, `progress` (json), `error`.
- `source`: `id`, `root` (as given), `root_canonical`, `first_seen_at`.
- `ingest_batch`: `id`, `source_id`, `job_id`, `name`, `config` (json: every knob
  of §11 as it was resolved, the identity rule, the file filter, workers, the
  binary's version), `started_at`, `finished_at`, `state`, `counts` (json: the
  diagnostics report of §11), `epoch_after`.
- `source_file`: `id`, `source_id`, `batch_id` (the last batch that examined the
  path), `path` (relative to the root, forward slashes), `size`, `mtime_ns`,
  `status` (`ingested`, `duplicate`, `quarantined`, `skipped`, `gone`), `reason`
  (the quarantine class of §5.3, `symlink` for a skipped one, or null), `detail`
  (text, or null), `instance_id` (null unless `ingested` or `duplicate`),
  `seen_at`. Unique `(source_id, path)`. This is D17's "records the path of
  every file".
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
- `stack`: `id`, `series_id`, `stack_index`, `stack_key`, `modality`, the fourteen
  signature columns of §8, `orientation_confidence`, `n_instances`,
  `first_batch_id`. Unique `(series_id, stack_index)` and `(series_id, stack_key)`.
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

Linkage store (`linkage.db` beside the registry, mode 600; the `linkage` schema on
Postgres). Separate on purpose: it is the only store with identifying data, its
contents are unreadable without the registry's key (§7.2), and it can be backed
up, exported and purged on its own (D13, C38).

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
case), `no-ext`, or a glob. The default is `all` because the check for what a file
is costs 132 bytes (§6.1) and D17 forbids silent drops; v0 defaulted to ".dcm or
no extension" and the gate runs with the filter set to match each v0 digest.

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

### 5.3 Quarantine classes

A file that is not ingested gets one of these, in `source_file.reason`, and the
batch's report counts each:

| class | when |
|---|---|
| `not_dicom` | no `DICM` marker and no readable bare dataset that yields a SOPInstanceUID |
| `unreadable` | an I/O error opening or reading (permission, a vanished file, a stale NFS handle) |
| `parse_error` | the reader failed inside the header; the error text is in `detail`, classified by the reader's error chain |
| `missing_uid` | no StudyInstanceUID, SeriesInstanceUID or SOPInstanceUID |
| `unsupported_sop_class` | a SOP class outside the batch's `sop_classes` knob (default: v0's nine image storage classes for MR, CT and PT, §6.1) |
| `missing_modality` | no Modality and no ModalitiesInStudy to fall back on |
| `unsupported_modality` | a modality outside the batch's `modalities` knob (default `MR`, `CT`, `PT`; `PET` is normalized to `PT`) |

`duplicate` is not a refusal: the file parsed and its SOP instance already exists
in the registry (another path, another batch). The row keeps `instance_id` so the
second path is provenance too. `skipped` (symlink) is neither.

Each class is a listed output: `nils quarantine list [--batch <id>] [--class <c>]`
prints paths, and the batch's report carries the counts. One review item of kind
`ingest.quarantine` per batch and class groups the rows (D7, C5: one item, N
members), with the count as evidence and no path in the item body; a human or an
agent decides "accepted" (these are sidecars, this is not our data) or "retry" and
the decision is a row, not a deletion.

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

### 6.2 The catalogue

The catalogue is a table in `nils-dicom`, one row per column the digest writes:
the column name, the level (subject, study, series, series_mr, series_ct,
series_pet, stack, instance), the source (a keyword, or a fallback chain), the
converter, the sensitivity class, and a note. It is generated into the
documentation and it is the seed of Wave 4's catalog endpoint. Wave 1's rule is
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
the whole trimmed value is the identifier. A value that matches no pattern counts
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
row whose code exists under another identifier of the same type, or whose
identifier already maps to another code, is an error listed before anything is
written (the import is validate-then-apply, like everything that writes). This is
how v0's per-digest CSV maps become registry facts, and the gate checks that every
v0 subject code comes out of a v1 digest run with the v0 key and the maps imported
(§12.4).

Linkage records (`nils linkage link <a> <b> --evidence <text>`, `nils linkage
unlink`) exist from Wave 1 with the CLI as their only door; the UI and the agent
door are Wave 4's. Nothing in Wave 1 reads a linkage when it queries; the columns
exist so that Wave 4 does not retrofit them (11).

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

## 9. The pipeline

### 9.1 Stages and bounds

```
walker pool (8) ──files──▶ parsers (workers) ──batches──▶ writer (1) ──▶ registry
                  cap 16k            cap 2 × workers                     linkage store
```

- **Parsers**: `--workers` threads, default the number of cores. Each reads a
  file (§6.1), extracts the catalogue, computes the stack signature, applies the
  identity rule (the value only; resolution is the writer's), and appends one
  row set to its current batch. A batch closes at 2,000 instances or 64 MB, whichever
  first, and goes to the writer. Refusals are rows too (`source_file` with a
  reason), so quarantine is written in the same transactions as the data.
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
  parsers, two batches per worker between the parsers and the writer), so a slow
  writer stops the parsers and a slow disk stops the walker. The only structures that grow with the corpus are
  the subject map (a few hundred bytes per subject) and the LRU caches; per-subject
  materialization, id lists and pickled batches do not exist (02). The Wave 1
  target for peak RSS is 4 GB at any corpus size, against D6's ceiling of 16.

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

The diagnostics are counted per batch and kind, with `scope` and `ref_id` where
one row is the subject and a `sample` of at most ten shapes:

`walk_error`, `charset_unknown`, `charset_lossy`, `value_invalid`,
`field_disagreement` (per series or study, per field), `identity_unparsed`,
`identity_fallback`, `subject_field_disagreement` (birth date or sex),
`file_changed`, `orientation_oblique`, `series_multi_study` (a
SeriesInstanceUID seen under two StudyInstanceUIDs: the instances are ingested
under the first study and the disagreement is counted, because a series
belongs to one study by the standard, and the file disagrees with the
standard, not with the digest).

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

### 12.2 Instances

Every instance v0 holds under a digested root is in v1, and every v1 instance
under that root is in v0 or is explained (v0's extension filter skipped it; v0's
SOP-class filter refused it; v0's resume skipped it: on a resumed digest,
`plan_subject_series` dropped a file whose SOP UID sorted at or below its series'
resume token, or below the legacy token when the series had none, whether or not
it had been written). The gate runs with `files` matched to each v0 digest's
extension mode so that the first case is measured, not assumed, and it counts the
third.

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

### 12.7 Corpus hygiene

Every adjudicated divergence becomes a fixture in `nils-dicom`'s tests: a
synthetic file that reproduces the shape (never the file, never a value from it),
with the expected values. The fixtures are the parser's regression suite and the
start of the verified corpus (C12).

## 13. CLI reference for Wave 1

```
nils init [--backend sqlite|postgres] [--dsn ...] [--scheme blake2b-32|blake2b-8]
          --key <name> [--display-length 12] [--session-scheme <file>]
nils key add <name> [--from-file <path>]      # otherwise from stdin
nils key list | remove <name>
nils digest <root> [--name <label>] [--workers N] [--files all|dcm|no-ext|<glob>]
            [--identity-rule <file>] [--retry-quarantine] [--restart] [--dry-run]
            [--describe] [--json]
nils status [--batch <id>] [--json]
nils quarantine list [--batch <id>] [--class <c>] [--json]
nils review list [--kind <k>] [--status open] | show <id> | apply <id> --decision <d>
nils linkage import <csv> --id-type <t> --id-column <c> --code-column <c>
nils linkage id-type add <name> [--description ...] | list
nils linkage link <code-a> <code-b> --evidence <text> | unlink <id>
nils linkage show <code>                       # decrypts; writes a read_audit row
nils linkage purge --subject <code> | --all    # confirm; listed in custody
nils custody [--json]
nils doctor
```

`--dry-run` walks and parses and prints the report without writing: the way to
see what a tree contains before digesting it. `nils custody` prints, for the
registry, the linkage store, the key store, the quarantine list, the job records
and the logs: where it lives, which classes it holds, how long it is kept, and
the command that exports, changes or deletes it (C38); every command it names
exists in this list, and nothing is retained that the table does not show.

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
   the partitions on the spike's corpora equal v0's for those series.
6. **Jobs, resume, status, custody.** *Done when:* a digest killed at any point
   resumes to the same counts as an uninterrupted one, and `nils custody` lists
   every file the tests created.
7. **The compare tool and the gate runs** on the baseline host: the spike's
   corpora first, then the live corpus study by study, each run's report in the
   record; the session check of §12.4 is part of the tool. *Done when:* §12.2 to
   §12.4 hold and every divergence is classified.
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
- **The Postgres bulk path.** `COPY` plus merge is the plan; slice 3 measures it
  against multi-row inserts and the spec is amended with the numbers.
- **Identity rule grammar.** Named groups over one field cover the two cases C37
  named; a rule that combines fields, or normalizes case and separators before
  hashing, waits for a source that needs it, and is then a knob, not code.
- **Session schemes** are fixed in their shape here (§4.4) and first applied in
  Wave 3. What Wave 3 decides with the exporter in hand: the window the group
  wants as its registry default (zero reproduces v0; the live registry's
  fourteen-day pairs are the argument for more), where overrides are written and
  how they are reviewed, and whether `months` under a clinical anchor waits for
  Wave 4 or comes earlier.
- **The baseline host** is described in the record with its deviations from the
  VM that C6 asked for.
