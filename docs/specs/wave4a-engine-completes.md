<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Wave 4a: the engine completes

The specification of the first of three waves that the record's Wave 4 became
(`docs/decisions/17-wave4-reframed.md`, rulings R1 to R8). It follows
`wave3-anonymize-and-bids.md` and cites the record by id.

It is the wave in which the engine finishes being an engine: the debt Wave 3
measured is paid, the private elements the archives carry are read rather than
dropped, the faults an eighteen-agent reading found in the tree are fixed, the
registry layer v0 keeps in a second database is carried across, selection is
made exactly as simple as it should be, and the engine gains the doors a second
process needs. Nothing in it is an AST, a catalog or an agent. Those are the
next two waves, and each of them opens with a conversation rather than with
code.

## 1. What Wave 4a delivers

Sixteen slices in four groups, in this order because the record says so (R3:
the debt and the faults first).

**The debt, the ingestion and the faults** (§4 to §6): the release's
bookkeeping at the grain of a stack, as a current state and a change log;
private elements read by creator into a catalogue the pack declares, and an
allowlist chosen from three measured surveys; five faults; and the `contracts/`
directory, which has been empty since the repository was made.

**The registry layer** (§7): what v0 keeps in its metadata database, cohorts,
diseases, observation types and events, carried into the one registry with the
vocabulary as pack data; one declarative importer in place of thirteen; the
clinical anchor the session scheme declared and refused; and the clinical
values a released tree has never carried, which closes the bar Wave 3 deferred.

**Selection, jobs, the principal, review** (§8 to §10): an enumeration and
nothing more; one job model; a principal on every judgement and an audit log
that is a table; the review spine measured before it is built.

**The doors** (§11): `nils serve`, one door per operation, and three
authentication modes, so that the two apps of Waves 4b and 4c have an engine
to be built against.

And the gate (§12), which closes the wave on a registry built from an archive
and a clinical file with nothing copied out of v0, because there is no
migration (R4).

## 2. What it rests on

Three rulings of the record shape every section:

- **The apps are separate and optional** (R1, R2, and the vision's third
  sentence). Present, the engine acknowledges them; absent, nothing is missing.
  So this wave has no AST, no catalog, no MCP and no notebook, and it gates on
  a second *process* driving the engine through its doors, never on a UI. The
  UI is its own product and always will be.
- **There is no migration** (R4). At the end, v1 makes the new registry by
  running over the archives and the clinical files. Nothing is copied out of
  v0's databases. So `nils digest` and the importer of §7.2 are the migration,
  and they are proved on the real inputs rather than on fixtures alone.
- **Selection stays simple** (R5). A cohort, subjects by code or by any
  identifier the registry resolves, subject-sessions, stacks by id or by their
  classification axes. What a study means by inclusion and exclusion is a
  question, and questions are Wave 4b's.

And three lessons Wave 3 paid for, now binding:

1. **A release takes an enumeration; a query computes one.** No filter flag is
   added to `nils release`, ever. When a scenario cannot express what it wants
   to release, that is evidence for the question wave, not a reason to widen
   the release.
2. **Versioning is a current state plus a change log**, never a row per thing
   per version. Wave 3 stored a row per stack per version and a row per file
   per version, measured at 1.4 KB of memory and of disk per file, and the
   same shape is waiting in every versioned object to come.
3. **Per-site knowledge needs an editing workflow from day one.** Three
   cohorts of one department share three private-element creators out of
   dozens. Whatever is knowledge about a clinic, a scanner or a vocabulary is
   pack data with a way to amend it, and the tool that measures it ships beside
   it.

## 3. What Wave 4a does not do

- **No AST, catalog, result handle, affordance, sync path or notebook.** Wave
  4b, after its talk and its research (17 §4).
- **No MCP, agent, provider, knob contract or evals.** Wave 4c, after its talk
  and its research (17 §5). Where MCP lives, in the engine as a veneer or in
  the agent app, is that talk's first question.
- **No web UI.** After 4b, when the contracts have stopped moving.
- **No migration of v0's databases** (R4).
- **No pixel or DICOM viewing door.** Wave 6, with a declared absence: the
  visual review kinds stay in v0's UI until then, which extends the dual-run
  window, and the wave says so rather than discovering it.
- **No general temporal window.** The engine gains the one temporal function a
  release and a gate need, the nearest event to a date (§7.3). `within`,
  `pairs`, `age_at` and set algebra are clauses of the question language
  (C18).

## 4. Versioning at the right grain

### 4.1 What Wave 3 built, and what it costs

A release records, per version, a row per stack (`release_stack`) and a row per
file (`release_file`), and a re-run holds the previous version's file rows in
memory to compare. Measured: 196 MB for a first version of 150,000 files, 232 MB
for the next, linear, so tens of gigabytes at the archive's 37.5M instances, and
the same again on disk **for every version**. Ten versions of the archive would
be 375 million manifest rows.

### 4.2 The three changes

**The stack is the grain.** It is the unit of everything else: the comparison,
the route, the pick, the conversion. 37.5M instances are ~518k stacks. A file's
path is derivable from its stack's place plus its instance id (DICOM) or its
extension (converted), so paths are not stored.

**The state is current, and the history is a log.** `release_stack` holds one
row per stack per dataset, updated in place; `release_move`, which already
records only what changed with the old and new place, is the history. "What is
in the tree now" is a lookup, "what did version 4 do" is a query on the log,
and "what was in version 3" is a replay of the log backwards, which is exact and
rare. Rows per dataset: ~518k total, whatever the number of versions.

**The diff is computed in SQL.** This version's per-stack plan is written to
a table (`release_plan`, one subject at a time, emptied when the version
closes) and the five outcomes (unchanged, moved, rewritten, added, removed)
are a paged join of the plan against the current state, not two maps in
memory. A stack's instances are read only when the stack is written. Memory
becomes a function of the largest subject and of one page of stacks, and not
of the number of files.

### 4.3 What is lost, and what replaces it

A per-file digest becomes a per-stack roll-up: the digest of the file digests,
with the count and the bytes. A handover verifies at stack granularity and
names the stack rather than the file; a per-file check is a recomputation. The
handover's own record already carries a checksum per archive, which is the
right place to spend bytes on verification.

`release_file` is dropped, with no opt-in. The reason is not the bytes: a
per-file manifest is a second description of the tree, kept beside the state
that already describes it, and two descriptions disagree. What the state knows
is enough to find every file again: a DICOM stack owns its directory and its
files are its instances, a converted stack is its stem plus its extensions,
and the dataset's own files sit at the levels no stack occupies. The handover
reads its file list from exactly that, so what it packs is what the release
recorded and not what happens to be in the directory. A registry made before
this folds its manifest into the state on upgrade (migration 18), summing the
bytes it knew and leaving the digest empty, since a digest the old shape held
per file cannot be made into one per stack without reading the tree.

### 4.4 Measured, not asserted

The slice lands with a benchmark of the release and its re-run at 150,000 and
1,000,000 files, today's design against the new one, and the numbers go in the
pull request and in §12's budget bar. Nima asked for exactly this: test which
is more efficient rather than assume.

**Measured, 2026-09-06**, on the baseline host, synthetic corpora from the
`corpus` example (seeds 1 and 2, 2 KB of pixels a file), the descriptive
layout, one process per release, peak resident set of that process alone,
with `engine/benches/release-manifest.py`, which is how the numbers are made
again. The
digest is the same code in both columns and peaks at 600 MB at either size,
which is Wave 1's budget and not this slice's.

| | files | first version | re-run, nothing changed | registry after v1, after v2 |
|---|---|---|---|---|
| Wave 3 | 150,000 | 176 MB, 18.6 s | 209 MB, 0.9 s | 110 MB, 137 MB |
| **slice 1** | 150,000 | **84 MB**, 17.7 s | **68 MB**, 0.5 s | **85 MB, 85 MB** |
| Wave 3 | 1,000,000 | 765 MB, 135 s | 977 MB, 6.2 s | 740 MB, 918 MB |
| **slice 1** | 1,000,000 | **172 MB**, 121 s | **84 MB**, 3.9 s | **567 MB, 567 MB** |

The registry no longer grows with a version that changed nothing: Wave 3 added
27 MB per version at 150,000 files and 178 MB at 1,000,000, and the new shape
adds a plan that is emptied and a log that is empty. The re-run's memory is
flat. The first version's grows from 84 MB to 172 MB over a 6.7 times larger
corpus, against 176 MB to 765 MB before; the store's SQLite page cache alone is
capped at 64 MB, and nothing in the release holds a row per file. The BIDS
layout at 150,000 files (81,124 written, the rest routed away by the synthetic
corpus's classification) measures the same way: 85 MB and 75 MB against 146 MB
and 161 MB, registry 86 MB flat against 103 MB then 119 MB.

## 5. Private elements

### 5.1 What the archives carry

`nils private` (Wave 3 §8.4, `docs/decisions/17`) was run over 40,000 files of
each of three cohorts on the private hosts. It reports shapes and never values:
a creator, a block, an element, how many files carry it, its VR, its length,
whether it is printable and how many distinct values it takes.

| | files with private elements | distinct elements | orphaned |
|---|---|---|---|
| an MS cohort, legacy scanners | 39,997 of 39,997 | 228 | 0 |
| nmosd | 39,999 of 40,000 | 158 | 0 |
| mix, multi-vendor | 28,503 of 39,963 | 1,377 | 484 |

Five facts decide the design:

- **Only three creators appear in all three**: `SIEMENS MR HEADER`,
  `SIEMENS CSA HEADER`, `SIEMENS MEDCOM HEADER2`. A list tuned on one cohort
  covers almost nothing of another.
- **The pack's six entries cover 16, 17 and 21 elements** of 228, 158 and
  1,377. They address `SIEMENS MR HEADER` in group 0019, and that creator also
  reserves group **0051**, where eight of the strongest candidates live.
- **484 of the mix's elements are orphaned**, in blocks no creator reserved.
  No allowlist can ever keep them, and the cause is not established.
- **Philips spells one creator two ways in one corpus.** Matching is
  case-insensitive, and this is the evidence it had to be.
- **`GEMS_PARM_01` has 32 per-acquisition elements**, the most of any block.
  A pack that handles one vendor silently drops the next vendor's information.

### 5.2 Two lists, one grain, one rule

Reading a private element into the registry and letting it leave in a release
are different risks, so they are different lists.

**`ingest`** is broad: everything the pack names is read at digest time into a
`private_element` catalogue, by creator and offset, and becomes a field the
fingerprint and the classifier can use. The registry is the private thing, and
a private element that carries a diffusion direction or a slice-timing table is
worth as much to a classifier as a standard one.

**`release`** is narrow: only what the pack names may survive
de-identification, and only by name. The rule that stays whatever a survey
says: **a block is never kept whole.** Siemens CSA headers have carried the
patient name, the operator and the institution in shipping firmware, so an
element comes back by name or not at all.

Both lists are at the grain of **creator and element offset**, addressed by the
creator's name and not by the block's position, because the same vendor lands
at a different offset from file to file (Wave 3 §8.4).

### 5.3 The sweet spot: a measured list and a repeatable method

Nima's instruction was to make this as useful as possible without making it
narrow. A hand-picked list of 139 offsets is narrow, and a rule that keeps
"whatever varies" is unsafe. The answer is both halves:

- **The list is measured.** The initial entries are the surveys' candidates
  under one stated rule, *varies per acquisition, printable, short*, because
  that shape is a parameter, and a constant across forty thousand files is a
  property of the scanner or the site and not worth the risk. The pack groups
  them by vendor block (`SIEMENS MR HEADER` 0019 and 0051, `SIEMENS CSA
  HEADER`, `SIEMENS MR SDS 01` and `SDI 02`, `GEMS_ACQU_01`, `GEMS_PARM_01`,
  `Philips MR Imaging DD 001`, `Philips Imaging DD 001`, `ELSCINT1`), each
  entry with its VR, its dictionary name, a `kind` (parameter, geometry,
  diffusion, timing, reconstruction) and the evidence that put it there.
- **The method is repeatable.** `nils private --suggest` turns a survey into
  pack entries under the same rule, so a site with a vendor we have never seen
  grows the list by measurement rather than by hand, and a pull request that
  adds entries carries the survey that justified them.
- **The names come from a private dictionary**, shipped as pack data and not
  as code (principle 6), so that `(0051,xx0C)` is `AcquisitionDuration` or
  whatever its vendor calls it, with a VR, and values stop arriving as `UN`.
  The dictionary is generated from a public source; which one, and under what
  licence it may be redistributed, is settled inside slice 2 and written down
  in the pack's `PROVENANCE.md`.
- **The pack states its vendor coverage**, because coverage is the honest
  measure of readiness: "Siemens, GE and Philips MR; nothing else", rather
  than a count of elements.

### 5.4 What `nils private` learns

Orphans reported by group, so the 484 can be understood; the dictionary
applied, so the report names elements; `--suggest`; and a JSON output the pull
request can carry.

### 5.5 As built (slice 2)

**Where an ingested element lives.** Per series, in `series_private`, as one
JSON object keyed by the element's address (`0019xx0C SIEMENS MR HEADER`),
with the addresses two files disagreed on listed under `varied`. A series and
not a column per element, because which elements are read is pack data and
changes without the schema changing; a series and not an instance, because an
element worth reading is a parameter of the acquisition, and the writer's rule
for a field two files disagree on applies unchanged: the first value fills,
the smaller in text order stays, and the row is the same whatever order the
files came in. The per-image diffusion values Wave 1 reads stay where they
are, per instance, because a b value is per image.

**How a rule reads one.** An ingested element is a field of the pack under
the `name` its entry gives it, numbered past the fingerprint's fields and the
pack's derived text, so `{field: siemens_b_value, gt: 0}` and `{text:
siemens_coil_string, substring: hea}` work without a new atom. The classifier
reads `series_private` in the window's join, beside the fingerprint and never
copied into it, and hands the values to the evaluator. A corpus case sets an
ingested field by the pack's name. A pass may not name one, because the
reference corpus has no private columns, and is refused as naming no field
rather than reading an empty one.

**The digest and the pack.** The digest reads the ingest list of the pack it
finds (`--pack`, `--pack-dir`, `--no-private`). A pack that cannot be found is
not an error unless one was asked for by name or by directory, because a
digest has never needed a pack; the report's `private elements` line says
what was read and from which pack, and `ingest_batch.config` records the
addresses. The bytes of an implicit VR file are read as the dictionary's VR
says: a little-endian number for `SS`, `US`, `SL`, `UL`, `FL` and `FD`, text
otherwise. A value longer than 256 bytes or not printable is not ingested,
whatever the pack asked, because a header blob is not a parameter.

**The dictionary.** `packs/mri/private-dictionary.tsv`, generated by
`tools/private-dict/extract.py` from pydicom's private dictionary (MIT), which
pydicom generates from GDCM's (BSD 3-clause): 449 creators, 10,507 elements,
38 wildcard entries left out. The pack's `PROVENANCE.md` carries the notices
and the two places the dictionary and the pack disagree.

**The starter list.** Twenty-two `ingest` entries over Siemens (groups 0019
and 0051), GE and Philips, from the three surveys' strongest candidates and
what Wave 1 already reads, each with a `kind` and a `why`; the `release` list
is Wave 3's six, unchanged. Slice 3 settles both lists against the surveys
with `nils private --suggest` as the method.

### 5.6 As built (slice 3): the lists, settled by measurement

**The method has two halves, and both are verbs.** A survey (`nils private`)
says what varies across an archive; only a digest can say whether an element
varies *inside* a series, and that is the question that separates a
parameter of the acquisition from a per-image value. So each cohort was
surveyed with the pack at hand, every candidate the survey proposed was put
on a broad ingest list, each cohort was digested with it, and
`nils private --measured` read back, per address, in how many series it
varied within the series and how many distinct values it took across them.
The reading is mechanical and the judgement is a person's: an identifier
(`Scanner Study ID`, a `UID` in group `0009`) varies exactly like a
parameter, and only a name tells them apart, which is why every `why` in
the pack was written by hand and carries the numbers.

**Measured, three cohorts** (varied within / series that hold it; distinct
across series). Blank where the cohort has no such element.

| address | reading | nmosd | mix | legacy MS |
|---|---|---|---|---|
| `0051xx0C` FoV text | parameter, kept | 0/984, 16 | 5/945, 152 | 0/921, 29 |
| `0051xx0A` TA text | parameter, kept | 0/984, 33 | 44/824, 348 | 28/911, 88 |
| `0051xx0B` matrix text | parameter, kept | 0/358, 14 | 0/624, 201 | 0/921, 60 |
| `0051xx0E` orientation text | mostly parameter, kept | 30/358, 118 | 130/633, 486 | 10/921, 302 |
| `0051xx0F` coil string | parameter, kept | 0/358, 17 | 0/566, 89 | 0/911, 20 |
| `0051xx11` PAT mode | parameter, kept | 0/311, 3 | 0/350, 6 | 0/619, 2 |
| `0019xx14` table position | parameter, kept | 0/358, 93 | 4/485, 150 | 0/869, 96 |
| `0019xx0F` gradient mode | near constant, kept | 0/277, 2 | 0/471, 3 | 0/869, 3 |
| `0021xx1C` `xx2C` `xx2D` SDS 01 | parameter by shape, kept, unnamed | 4/578, 64 to 74 | 2/222, 173 to 184 | |
| `0021xx77` SDI 02 | parameter by shape, kept, unnamed | 0/578, 112 | 0/222, 56 | |
| `0043xx39` GE b values | parameter, kept | | 12/573, 37 | |
| `0019xxE0` GE directions | parameter, kept | | 0/554, 7 | |
| `0019xx84` `xx93` GE SAR, frequency | parameter, kept | | 0/483, 463; 0/482, 446 | |
| `2001xx03` `xx04` Philips diffusion | parameter, kept | | 11/409, 2; 7/58, 4 | |
| `2001xx13` `xx14` `xx20` `xx21` `xx83` Philips | parameter, kept | | 0/409 each | |
| `0051xx0D` slice position text | per image, out | 984/984 | 921/942 | 921/921 |
| `0019xx0B` slice duration | per image, out | 38/358 | 294/566 | 648/911 |
| `0019xx16` time after start | per image, out | 103/103 | 294/295 | 530/530 |
| `0021xx04` `xx06` `xx88` `xx8A` SDI 02 | per image, out | all | all | |
| `2001xx0A` slice number, `2005xx0F` `xx10` window | per image, out | | all; 307/406 | |
| `0051xx13` PCS directions | constant, out | 1 value | 1 value | 1 value |
| `0029xx09` `xx19` CSA versions | the software's, out | 0/358, 9 | 0/691, 496 | 0/966, 133 |
| `0043xx62` Scanner Study ID | identifier, never | | 0/532, 431 | |
| VA0 COAD `0019xx16` `xx60` `xxA2` | calibration, out | | | 2 to 8/124, 83 to 99 |

Twenty-three entries are ingested, all of them parameters of the acquisition
by this measure; the release list is Wave 3's six, unchanged. The
per-image diffusion values Wave 1 reads (`0019xx0C`, `xx0D`, `xx0E`) stay in
the catalogue, per instance, and in the release list, and are no longer on
the ingest list: the smallest b value of a series is `0` for every series
that played a b0, which is what Wave 3 §6 found when it read them per
series. The legacy cohort's own blocks (Numaris VA) are per-image values,
calibration state or administrative identifiers, and none is ingested; its
`SIEMENS MR HEADER` elements measure the same as the other cohorts'. The
`coverage` list in `private.yml` says so.

**What the gate asserts.** The reference corpus's diffusion series carries a
Siemens block: a b value and a directionality, which the release list
keeps, and a coil string, which the pack ingests and a release drops. Bar 8b
runs `nils private` over every released tree and holds what it finds against
the release list: nothing outside the list, no block without a creator, the
kept elements present where the series is DICOM, the dropped one absent
everywhere. Negative-tested by taking the b value off the list: the bar names
it in both DICOM trees.

## 6. The faults, and the contracts

### 6.1 Five faults

Found by reading the tree, each with a file and a line, each in code that has
tests and passes them:

1. **A raw DATE read on the session verb.** `nils session --anchors` projects
   `COALESCE(st.date_filled, st.study_date)` without the dialect's cast and
   reads it as text. Postgres hands a `date` back in a type the store reads only
   as text, which is the third occurrence of the failure Wave 3 fixed twice.
2. **The CLI tests have no Postgres half.** 1,872 lines of `crates/nils/tests/
   cli.rs` and no reference to the test DSN: session, pick, review, custody and
   status are untested on the backend the server will run on.
3. **The cast is a discipline, not a mechanism.** `Dialect::text_of_qualified`
   already exists with a doc comment naming the exact failure. It becomes
   unavoidable: a projection resolves through a `&Column`, and a debug
   assertion fires when a Date, Time, Timestamp or JSON column is selected
   raw.
4. **Multi-valued axes are comma-joined strings.** `classification_axis.value`
   stores several values joined by commas, "exactly as v0 wrote them", which
   is v0's `classification_string` mistake ported deliberately. It is why a
   role match in the release is four interpolated `LIKE` patterns, and it sits
   directly under the pick clause the question wave needs. **One row per
   value**: the unique key becomes `(stack_id, axis, value)`, a single-valued
   axis has one row, and every reader that split on a comma reads rows.
5. **The released tree carries no clinical value.** `participants.tsv` is
   written with an empty extras map. Named here; fixed in §7.4 when there are
   values to write.

**As built (slice 4).** Faults 1 and 3 are one fix, and it is structural
rather than a discipline: the Postgres store reads a `date`, `time`,
`timestamp`, `timestamptz`, `json` or `jsonb` column selected raw as the text
`Dialect::text_of` would have rendered (`YYYY-MM-DD`, `HH:MM:SS.ffffff`,
`YYYY-MM-DDTHH:MM:SSZ`, the JSON itself), decoded off the wire, so a
projection that forgets the cast reads the same on both backends instead of
failing the first time a row of that shape exists. The casts stay where they
are and stay right; forgetting one stops being a fault. A both-backend test
selects the four shapes raw, and the session verb's anchor projection, fault
1's site, is exercised on Postgres unchanged. Fault 2: the CLI suite has a
Postgres half, one round of the verbs the server will run (init, digest,
fingerprint, classify, status, session with an anchor file, pick, review,
custody, release, the private survey) on that backend, gated on the test DSN
like every other suite's. Fault 4: `classification_axis` holds one row per
value, its key is `(stack_id, axis, value)`, migration 20 splits what was
joined, and every reader reads rows: the release matches a role by equality,
a pick reads its roles as rows, the passes and the naming grammar join the
rows back into the one string they were written to read. Fault 5 waits for
§7.4, as it says.

### 6.2 `contracts/`

The directory holds a licence and a README. Three contracts are overdue:
`pack/` (the pack format is data and has been since C11; a pack's `contract:`
field already names a version nothing checks), `review-item/` (the schema of
D7), and `openapi/` (empty until §11 fills it). Each gets a skeleton, a version,
and the rule that a version changes only by a pull request that says so. A
contract test exists and is green, even if it tests little, because a test that
exists is amended and one that does not is never written.

**As built (slice 5).** Three directories under `contracts/`, each with a
`README.md`, a `VERSION` and one `vN/` directory per version, and the rule
they share written at the top of `contracts/README.md`: a version changes
only by a pull request that says so in its title. `pack/` is at version 1,
the manifest as the loader reads it, as a JSON Schema with a description on
every property; the engine's `CONTRACT` constant and the file's `VERSION` are
kept equal by a test, every key the loader reads is kept on the schema and
every key on the schema is kept read, and the shipped MRI pack's manifest is
held to it key by key. `review-item/` is at version 1, the item as `nils
review list --json` prints it, with the kinds so far tabled in its README; a
test digests a tree with one refused file and holds the printed item to the
schema property by property. `openapi/` is at version 0 and empty, a
skeleton a test reads, so that the first route of `nils serve` lands in a
file already versioned and tested. The query AST, MCP, job and federation
contracts wait for their waves, and the table in `contracts/README.md` says
which.

## 7. The registry layer

### 7.1 The clinical schema

What v0 keeps in a second database, in the one registry. The live archive
holds 139,033 events over fifteen observation types spanning seven decades,
2,695 diagnoses, 2,981 onsets, 17,792 EDSS observations, 10,048 treatments; the
counts are the shape the schema has to hold, not a claim about the fixture.

| table | holds | note |
|---|---|---|
| `cohort` | a name, an owner, a description | a membership fact (D4); whether a saved selection can also be one is Wave 4b's talk |
| `cohort_member` | `(cohort_id, subject_id)` with joined-at | one subject in many cohorts is the norm, a third of them in the live archive |
| `disease_type`, `disease` | the vocabulary and the instance | vocabulary is pack data |
| `subject_disease` | `(subject_id, disease_id)` with onset and diagnosis dates | |
| `observation_type` | the fifteen kinds, as pack data | EDSS, treatment, delivery, transition and the rest |
| `event` | `(subject_id, observation_type, event_date, value, unit, source)` | v0's one event-attribute-value shape, kept, because every clinical import is one |
| `subject` | gains `birth_date`, `sex`, `deceased_at` | already present: code, identifiers, id types, linkage (Wave 1) |

The date is the join key of the clinical layer, as Nima ruled in Wave 3: the
clinical import matches on `(subject, event_date)`, the session anchors are
themselves event dates, and age, disease duration and the nearest EDSS are date
arithmetic. Nothing here is ever rewritten by a date policy; that is a release's
concern.

**Corrections** (settled here, §13.2): a corrected row supersedes the old one,
the old one stays with `superseded_by`, and a retraction is a supersede by
nothing. Nothing about a person's record is deleted, which is the same rule the
`decision` table already lives by.

**As built (slice 6).** The eight tables of the table above, in migration
21: `cohort` and `cohort_member` (a membership fact, with `joined_at` and a
`left_at` that is never a deletion), `disease`, `disease_type`,
`observation_type`, `subject_disease`, `subject_disease_type` and `event`,
the last three with `actor` and `superseded_by`, which is the correction
rule of §13.2 built in. `event` is the one event-attribute-value shape:
the value as text, and as a number beside it when the kind is numeric, so a
nearest-of is arithmetic and not a parse. One thing the table above had
wrong: the subject's birth date and sex are catalogue columns already, read
from the files since Wave 1, so the subject gains only `deceased_at`; the
importer fills the two where the files are silent and a disagreement is a
review item (§13.3). The vocabulary is pack data in `packs/clinical/
vocabulary.yml`, carried from v0's seed (eight diseases with seventeen types,
sixteen observation kinds counting the delivery its archive recorded without
the seed naming it, EDSS the primary one), and `nils clinical
vocabulary load` upserts it by name: what exists is updated, what is new is
added, and nothing is ever removed by a load, because an event already
recorded against a kind must keep its kind; a second load says it changed
nothing. `nils clinical vocabulary list` reads it back, and the custody table
has a `clinical layer` store with its counts and its rule (C38). The id
types stay where Wave 1 put them.

### 7.2 One declarative importer

v0 has thirteen importers, 5,807 lines, each a preview-then-apply CSV importer
with its own field parsers and validation. They differ in their targets and not
in their shape, so v1 has **one**: a mapping file names the target table, the
columns and their parsers, the key the row is idempotent on, and what to do
with a row whose key exists. Preview shows what would change and applies
nothing; apply writes under a principal; a re-run changes nothing.

This is the migration under R4, so it is proved on the real clinical files
rather than on fixtures alone, on the private hosts, with counts in the pull
request.

**Demographics conflict** (settled here, §13.3): birth date and sex arrive only
by file, and when a later file disagrees with what the registry holds, the
conflict becomes a review item and the registry's value stands until a person
decides. That is what the spine is for (D7).

**As built (slice 7).** `nils clinical import --mapping FILE --file CSV
[--apply]`, one importer for six targets: `event`, `subject`, `cohort`,
`cohort_member`, `subject_disease` and `subject_disease_type`, which are v0's
thirteen import shapes less the four that are vocabulary (loaded by §7.1's
verb) and the identifiers (Wave 1's linkage import). A mapping names the
target, how a row names its subject (by the registry's code, or by an
identifier through the linkage store, which is the way that works without
the key that made the codes), the reference the target needs as a constant
or a column, the columns with their parsers, the key, and what to do with a
row that exists. A date is read under a declared format and never guessed;
v0 accepted twelve formats, three of which cannot be told apart on most days
of the month. A preview reads every row and writes nothing; an apply writes
in one transaction under the caller's name; a row whose key exists is
skipped, updated or superseded as the mapping says, and a re-run under the
default changes nothing. A demographics row fills a blank, skips an equal
value, and raises `subject.demographics` on a different one, the registry's
value standing (§13.3). A diagnosis row's onset and diagnosis dates become
events of their kinds, made if absent, so the dates are where every other
date is and `Anchor::Event` can find them. Refusals are counted by reason,
not listed by row, so a file with a thousand bad dates is one line. The
example mappings and the format are in `packs/clinical/imports/`.

**Proved on the real rows.** On the v0 host, read-only against v0's clinical
database, the nmosd cohort's rows were exported keyed by the cohort's own
identifier, the cohort's raw DICOM was digested into a scratch registry with
the same binary (493,708 files, 44 subjects, 82 studies), and every file went
through the importer, previewed, applied, and applied again: the cohort (1),
its members (43 of 43), the diagnoses (40 of 40), the demographics (14 fields
of the 7 subjects v0 holds them for, the 3 that disagreed by spelling settled
by the standard's letter), and the events (298 of 298: 135 scans, 98 heights,
65 weights). The second apply of each changed nothing. The first run found
two faults, both fixed with a test: a key field the row did not carry was
compared with `= NULL`, which is never true, so every event without a time
was added again; and v0 stores a sex as a word where the scanner wrote a
letter. Nothing left the host, and the scratch registry and the exported
files were deleted afterwards.

### 7.3 `Anchor::Event`, and the nearest event

Wave 3 §5 carried the session scheme whole and declared the clinical anchor
without being able to serve it. With events in the registry it is served: a
scheme may anchor on a diagnosis, an onset or a treatment start, and the
labels follow. The one temporal function the engine needs beyond that is
**the nearest event of a type to a date**, which the release of §7.4 and the
gate's bar both use. It is a function, not a clause: the general windows are
Wave 4b's.

**As built (slice 8).** A scheme anchored on a kind of event says which:
`anchor: event` with `event: Diagnosis` (or `Disease Onset`, `Treatment`,
whatever the vocabulary names), refused without the kind, and the kind
refused without the anchor. Month zero per subject is the earliest event of
that kind the subject has, among those not superseded; a subject with none
is unanchored, as an `explicit` subject with no row is, and the resolver
stays a pure function that is handed a day. `nils session list --scheme`
serves it, and refuses a kind the registry does not hold by name. The
nearest event of a kind to a day is one function, `clinical::nearest`, with
its tie rule written down: of two events the same distance away, the earlier
one, because an observation made before a scan describes the state the scan
saw and one made after may describe what the scan changed. It returns the
event, its date, the signed distance in days, the value and the number, and
it is what §7.4's release and the gate's bar read.

### 7.4 Clinical export in the release

`participants.tsv` carries the sex and the age at first session; `sessions.tsv`
carries the age at session and the nearest observation the release names,
under the date policy (an age is computed before a birth date goes, Wave 3
§8.3; under `shift` the observation's date moves with the subject's offset;
under `year` no date is written at all). Sensitivity is enforced here: a field
the pack marks sensitive never reaches the tree.

This closes **Wave 3's deferred bar 10**: the EDSS nearest each scan is the
same computed from the registry and from the tree, under every date policy.

**As built (slice 9, 2026-09-06).** `participants.tsv` carries `age` (whole
years at the subject's first session in the release) and `sex`; each
`_sessions.tsv` carries `age` at the session and, for every kind the release
names, three columns from the kind's name in lower case with underscores:
`<kind>` (the number, else the value, else `yes` for a kind that is a date
and nothing else), `<kind>_days` (the signed distance in days from the
session's earliest study day to the observation, written under every policy
because it names no day), and `<kind>_date` (under `keep` the date; under
`shift` the date moved by the subject's offset, so the distance holds; under
`year` not written at all). The nearest is `clinical::nearest`, the earlier
of two equidistant. Kinds come from `--observation KIND`, repeatable, or by
default the vocabulary's primary kinds. Sensitivity is a mark on the kind
in the vocabulary (`sensitive: true`; the delivery carries it): a sensitive
kind is refused by name and left out of the default, before anything is
planned and whatever the layout, and so is a name the registry does not
hold. Migration 22 adds `observation_type.is_sensitive`. The release report
counts what it wrote (`sex`, `age`, `session age`, `nearest <kind>`). The
gate loads the vocabulary, imports two EDSS scores by subject code, releases
a shifted BIDS tree beside the kept one, and bar 10 now compares each
session's row with the registry's own nearest and checks that the shifted
tree's date moved while its distance held.

## 8. Selection

Exactly R5, and no more:

| grain | how it is named | resolved by |
|---|---|---|
| a cohort | its name | `cohort_member` |
| subjects | the code, **or any identifier of any type the registry resolves** (v0's manifest resolver did this across every id type and it is the one thing of v0's resolver worth carrying) | `identity` and `linkage` |
| subject-sessions | `<subject>:<label>` under the release's scheme | derived on read, matched after the sessions are computed, because a session is never a column (Wave 3 §5) |
| stacks | by id, **or by their classification axes**: equality on pack axes, which is what a stack *is* | `classification_axis`, one row per value (§6.1) |

`--select` takes them from a file or standard input, as JSON or one item a
line, which Wave 3 built; this wave adds the cohort, the identifier resolution
and the axis form, and `nils select` prints what a file would select without
releasing it, so a person can see a cohort before it leaves.

Nothing else is added. A field strength, a date range, a diagnosis, a slice
count: each is a predicate, and predicates are the question wave's.

**As built (slice 10, 2026-09-06).** One grammar, in `nils-release`'s
`select` module, shared by `nils release` and `nils select`: a line is a
stack (a number), a cohort (`@name`), an axis value (`<axis>=<value>`), a
session (`<subject>:<label>`) or a subject (anything else); the JSON form
carries `cohorts`, `subjects`, `sessions`, `stacks` and `axes` (a map of
axis to value or values, or a list of `axis=value`). The flags `--cohort`
and `--axis` join `--subject`, `--session`, `--stack` and `--select`. A
subject that is not a code is tried as an identifier under every type the
linkage store knows, and resolves with the type named; a name that is an
identifier of two subjects is refused as ambiguous, and one that is neither
is refused. A cohort resolves to its current members (no `left_at`),
folded on case. An axis value is checked against the pack's axes when a
pack is at hand, so a name that would match no row is refused rather than
selecting nothing; values of one axis are alternatives, every axis named has
to hold, and the match is equality on the one-value-per-row table of §6.1.
Everything is resolved before a release plans anything, and a release with
an unresolved item refuses, naming each one, rather than releasing less
than it was asked for. `nils select` prints each item with how it resolved
and what the selection reaches (subjects, studies, stacks, files, bytes;
sessions are named, not counted, because they are derived under the
release's scheme), exits 1 when anything did not resolve, and has `--json`.
The release record carries the axes and the cohorts beside the codes.
`nils clinical cohort list` names the cohorts with their member counts.

## 9. Jobs, the principal and the audit

### 9.1 One job model

Two verbs claim work today through two ~60-line paths of their own. They become
one: per-kind claims, a queue, a worker, a pool. Every verb that runs longer
than a second is a job row with progress; `nils jobs` lists, cancels and
resumes; a job is resumable because nothing is in flight (principle 4). The
doors of §11 answer 202 with this job's id, and nothing else, for anything
heavy.

**As built (slice 11, 2026-09-06).** The one model is `nils_registry::job`:
`claim` (per kind: a fresh running job of the same kind refuses, a stale
one of any kind is failed on the way, a job of this host whose process is
gone is over however fresh its last beat; the claim records the process's
command line under `args.argv`), `beat` (the heartbeat with the progress
record beside it, returning whether a cancel was asked meanwhile), `finish`,
`request_cancel` (a running job becomes `cancelling` and stops at its next
heartbeat the way the first signal stops it, so what is written stays
written; a queued one is cancelled outright), `list`, `show`, and the
queue: `enqueue` (a command line as a row in state `queued`, its kind the
verb), `next_queued`, `take`. The digest's and the classifier's claim paths
are gone; the digest, fingerprint, classify, pick, release, handover,
clinical import (apply, not preview) and linkage purge all claim through
it, and the release's, the handover's, the pick's and the import's rows are
new. `nils jobs list [--all] | show <id> | cancel <id> | resume <id> |
enqueue [--name] -- <command> | work [--once] [--every]`. A worker is a
job of kind `worker`; it takes the oldest queued row, runs the command
line in its own registry as a child process with `NILS_JOB_ID` set, and
the verb's claim **adopts** that row instead of making a second one, so a
queued job is one row from the queue to the outcome and the id a door
answers 202 with is the id whose progress is read; the worker writes the
outcome only when the verb did not. A resume runs the recorded command
line again as a child process, which is enough because every verb resumes
by design (nothing is in flight). The pool of §13's default belongs to
`nils serve` (slice 14).

### 9.2 The principal and the audit log

Today `actor` is a string read from the environment. It becomes a principal on
every decision, import, release and handover, and an **audit log that is a
table**: who did what, to which scope, when, under which policy. The doors map
an OIDC subject onto it; until then it is the local user, recorded as such.
`user@node` is the principal's shape from the start (C30), so federation adds
no column later.

**Acknowledgement** (settled here, §13.4): "the machine was right and I
checked" does not write a `decision` row, because that would inflate the count
of human-authored values; it sets `review_item.accepted_by` and `accepted_at`,
which is its own home and its own count.

**As built (slice 12, 2026-09-06).** The principal is
`nils_registry::principal::Principal { user, node }`, shown as `user@node`:
at the command line the operating system's user on this host, or
`NILS_PRINCIPAL` when a door sets it; a bare name given to `--actor` is a
user on this host. Every `actor` the command line writes (decisions, picks,
releases, handovers, clinical imports, linkage reveal, link, unlink, purge)
is that string now. The audit log is the `audit` table (migration 23): `at`,
`principal`, `action` (dotted where a verb has several acts: `decision`,
`review.accept`, `clinical.import`, `vocabulary.load`, `linkage.import`,
`linkage.link`, `linkage.unlink`, `linkage.purge`, `linkage.reveal`,
`release`, `handover`), `scope` (ids, names, counts; never an identifier),
`policy` (a release's), `job_id`, `epoch` and `details`. `audit::record`
writes the row and advances the epoch for every act that changes a
judgement (§13.5), which an acknowledgement and a reveal do not; the row
carries the epoch it made. The release, the handover and the clinical
import write theirs in the engine beside their job; the command line writes
the rest. `nils audit list [--principal] [--action] [--since] [--limit]
[--json]` reads them, newest first, an action given with a trailing dot
matching every act under it. `nils review accept <id> [--why]` is the
acknowledgement: `status` becomes `accepted`, `accepted_by` and
`accepted_at` are set, no `decision` row is written and the epoch does not
move; a second acknowledgement is refused. The custody page lists the audit
log as a store of its own.

## 10. The review spine

### 10.1 Measure first

v0 flags 435k of 518k stacks, most for "no keyword evidence" rather than for
doubt, and a queue that long is read by nobody. The pack already declares
emission thresholds per axis (Wave 2 §8.2). **The first thing this slice does
is turn them up and count what survives**, on the reference corpus and on nmosd,
before a line of grouping is written. The number goes in the pull request.

### 10.2 Then the spine

v0's eight review surfaces come in three shapes: per-item draft-then-confirm;
cohort snapshot with stage, commit, undo and a drift signature; write-through.
One spine serves all three (C5, C15):

- **grouped items**: one question about a rule, an origin or a study is one
  item with n members, not n items;
- **bulk decisions**: a decision applied to a group is one row with its scope;
- **staged result versions with a commit**: the snapshot shape is a batch
  decision on a result set, staged, then committed, and undone by withdrawal;
- **decision precedence**, human over agent over rule, and a decision that
  survives re-classification (Wave 3 §10.1 gave it an author; this gives it a
  rank).

`nils review apply` is the one verb, and the doors expose the same queue.

**Measured first (slice 13, 2026-09-06).** One classification per threshold,
in a copy of each registry, on the work host; `--review-below` overrides
every axis at once, so `body_part`, which the pack decides at 0.65, floods
above it. A group is one (kind, value, tier).

| corpus | stacks | threshold | items | stacks asked | groups |
|---|---|---|---|---|---|
| nmosd | 2,484 | the pack's (0.70, body_part 0.65) | 3, all `ingest.quarantine` | 0 | 1 |
| nmosd | 2,484 | 0.75 | 2,528 | 2,433 | 4 |
| nmosd | 2,484 | 0.85 | 7,286 | 2,475 | 16 |
| nmosd | 2,484 | 0.95 | 10,791 | 2,483 | 34 |
| mixed | 4,120 | the pack's | 77 | 45 | 9 |
| mixed | 4,120 | 0.75 | 1,822 | 1,553 | 23 |
| mixed | 4,120 | 0.85 | 7,114 | 3,024 | 45 |
| mixed | 4,120 | 0.95 | 10,725 | 3,076 | 108 |

The largest groups at 0.75 on nmosd are body_part brain by keywords
(2,140), body_part spine by keywords (293) and base T1w by physics (92); at
the pack's thresholds on the mixed corpus they are the votes on base T1w
(30) and technique FLASH (25), base PDw by physics (9) and body_part spine
by physics (4). The number that decided the shape: at every threshold the
queue is tens of questions, not thousands of items. 10,791 items on nmosd
at 0.95 are 34 questions; 10,725 on the mixed corpus are 108.

**As built (slice 13, 2026-09-06).** The spine is `nils_registry::review`,
with migration 24. **Grouped items:** after a classification run the
per-stack axis questions it raised (they carry the run's `job_id` now)
collapse into one `review_item` of scope `group` per (kind, value, tier),
`members` counted and `group_key` naming the three, with a `review_member`
row per stack holding the evidence the question was raised on; the run's
report says `review_items` (the members) and `review_groups` (the queue a
person reads). A re-classification supersedes the open grouped questions
its stacks belonged to and asks again as new items (C15). **Bulk
decisions:** `nils review apply <item> --value V | --nothing` on a grouped
item writes one `decision` row of scope `group` whose `ref` is the item,
marks every member decided and closes the item; `--member <stack>` decides
one member alone, at `--scope stack | series | subject | origin` from that
stack, and the item closes when its last member is decided. `decide` is the
same verb under Wave 2's name. **Staged versions with a commit:** `--stage`
writes the decision with `staged_at` and the epoch it was staged at and
leaves it out of force, the item in status `staged`; `nils review commit
<id> | --all` sets `committed_at` and accepts the items, refused when the
epoch moved since staging unless `--anyway` (v0's drift signature);
`nils review withdraw <id>` takes a decision out of force, staged or
committed, and reopens the items it closed; nothing is deleted. Items
point at their decision by `decision_id`, because the two backends spell
JSON apart. **Precedence with a rank:** person 3, agent 2, model 1
(`review::rank`); the classifier reads only decisions in force (not
withdrawn, and committed, where a decision written without staging is
committed as written), and of every decision naming a stack at any scope
the highest rank wins, then the narrowest scope (stack, group, series,
subject, origin); on one key a lower rank does not override a higher one
and is refused with who decided. Every apply, commit and withdrawal is an
audit row and moves the epoch. The review-item contract is at version 2
for the grouped shape.

## 11. The doors

### 11.1 `nils serve`

One door per operation, resource-shaped, versioned (05 §1). A POST that does
anything heavy answers 202 with a job id; progress and results are read from
the job. Server-sent events carry progress for display and are never the
execution context. The route inventory is `contracts/openapi/` filled in, and
what it exposes in this wave is exactly what the CLI has: selection, release,
handover, review, jobs, custody, status, and `GET /api/capabilities` reporting
the contract versions, the loaded pack versions and the registry epoch (C26).
No AST endpoint: that is the question wave's.

The store is blocking and stays so in this wave; the server runs it behind a
pool (§13.6). Whether it goes async is a question for the wave that builds a
compiler on it.

**As built (slice 14, 2026-09-06).** `nils serve [--bind ADDR] [--auth
off|token] [--token TOKEN=user@node ...] [--workers N] [--pack-dir DIR]`,
in the binary's `serve` module on a small synchronous HTTP server; the
store stays blocking and the pool is one registry connection per handler
thread, a request handled by whichever thread receives it. Nineteen doors
under `/api`, the OpenAPI contract at version 1 naming each: `GET
capabilities` (the engine version; the contract versions, read from the
checked-in `VERSION` files at build time so the door and the document
cannot drift; the loaded packs with versions; the registry id, epoch and
schema version; the auth mode; the caller's principal; the node; the list
of doors), `GET status`, `GET custody`, `GET audit`, `GET jobs`, `POST
jobs` (a command line for a worker, the verb one of digest, fingerprint,
classify, pick, release, handover; 202 with the job's id; the caller's
principal rides on the queued row and the worker hands it to the verb as
`NILS_PRINCIPAL`), `GET jobs/{id}`, `POST jobs/{id}/cancel`, `GET
releases`, `POST releases` and `POST handovers` (queued command lines,
202), `POST select` (the bounded synchronous path: the resolution and what
it reaches), `GET review`, `GET review/{id}` (with the members), `POST
review/{id}/apply`, `POST review/{id}/accept`, `POST decisions/{id}/commit`,
`POST decisions/{id}/withdraw`, and `GET events` (server-sent events with
the open jobs and the epoch every second, for display only). Errors are
`{"error"}` with 400, 401, 404, 409 (a refusal: a job of the kind running,
a higher rank holding, the registry moved on) or 500. `off` records the
local user as the principal; `token` takes `Authorization: Bearer` and the
token names the principal; `oidc` is slice 15's and says so. The status,
custody and releases documents are built by functions the command line and
the door share. Tested as a second process would use it, over plain HTTP.

### 11.2 Three modes

`off`, `token`, `oidc` (D8). Groups map to roles; the engine stores no
password, mints no session and owns no user table beyond a cache of claims. In
`oidc` mode the audit principal is the subject. The federation primitives that
cost nothing now ride here (C26, C27, C30, C33): the principal's `user@node`
shape, the epoch in capabilities, and `local`, `federated` and `sensitive` as an
attribute the pack may put on a field, read by nothing yet.

**As built (slice 15, 2026-09-06).** The three modes are `nils serve
--auth off | token | oidc`. `off` records the local user; `token` names the
caller by a bearer token. `oidc` takes `--oidc-issuer URL`, `--oidc-audience
AUD` and `--oidc-jwks FILE` (the issuer's JWKS document as a file the
deployment keeps current; a shared-secret key in it is ignored, because the
engine holds no secrets), validates every bearer token's signature (RSA, EC
or EdDSA, by `kid` where the token names one) against the issuer and the
audience and requires `exp`, `iss`, `aud` and `sub`, and maps the groups
claim (`--oidc-groups-claim`, `groups` by default) to roles through `--role
GROUP=reader|reviewer|operator|admin`. A reader reads and previews; a
reviewer applies, accepts, commits and withdraws; an operator queues work
and cancels it; an admin reads the audit log and the custody; a role
implies the ones below it, a caller with no mapped group is a reader, and
a door beyond the caller's role answers 403 naming the role it asks for.
Under `off` and `token` every caller holds every role. The audit principal
in `oidc` mode is the subject at the issuer's host, `sub@issuer`, and it
rides on a queued job to the verb the worker runs. The engine stores no
password, mints no session and owns no user table: the only state is an
in-memory cache of a token's principal, roles and expiry, dropped when the
token expires. The capabilities report the auth mode and the caller's
roles. The riders (C26, C27, C30, C33): the principal's `user@node` shape
and the registry epoch in the capabilities were built in slices 12 and
14; this slice adds the pack's `fields` key, an optional visibility
(`local`, `federated`, `sensitive`) the pack puts on a catalogue field,
which is the pack contract's version 2 (`nils_pack::CONTRACT` = 2, the
MRI pack declares it and names its free text, paths, exact dates and
station as `local`, the patient comments as `sensitive`, and the vendor,
model, field strength and modality as `federated`); the loader reads it,
`nils pack show` prints it, and nothing else reads it yet. Traefik
forward-auth in front of the engine still works with `token` for the app
and `oidc` for a client that carries the issuer's token; the OAuth
resource metadata of C22 belongs to the assistant wave with MCP.

## 12. The gate

On the reference corpus, and on a real cohort re-digested from its archive with
its clinical file imported, because there is no migration:

1. **The registry is built from raw**, `nils digest` plus the importer, with
   nothing copied out of v0, and the counts reconcile to v0's for the same
   cohort or the difference has a named cause.
2. **The release's bookkeeping is flat in files**, measured at 150,000 and
   1,000,000 files, both layouts, and a re-run writes nothing.
3. **Private elements the pack names are readable by a pack** on all three
   surveyed vendors, and **none leaves a release** that the pack did not name.
4. **The CLI suite is green on both backends**, and a raw date projection
   cannot be written without a test failing.
5. **A role match is an equality**, and no reader splits a comma.
6. **The EDSS nearest each scan is the same** from the registry and from the
   tree, under every date policy (Wave 3's deferred bar).
7. **Every one of v0's thirteen import shapes** runs through the one importer,
   and a re-run changes nothing.
8. **The review queue for the reference corpus is readable in an afternoon**,
   and a snapshot-shaped decision is one commit.
9. **A second process drives every verb the CLI has through the API** and gets
   the same answer, in every auth mode.
10. **Every store the engine keeps is in the custody table** with an owner and
    a retention (§13.1).
11. **The budget**, measured on the baseline host and gated in CI.

**As built (slice 16, 2026-09-06).** The gate runs in three places, and the
scripts are `tools/release-check/gate.sh` (the reference corpus, in CI on
every push), `tools/wave4a-gate/cohort.sh` with `check.py` (a real cohort on
the private host that holds its archive and its clinical files; every path is
an argument, and the outputs are counts and timings) and
`tools/wave4a-gate/budget.sh` (the baseline host, from the archive over NFS).
What proves each bar:

1. `cohort.sh` builds the registry from the raw tree with `nils digest` and
   the clinical layer with the one importer, and `check.py` holds the
   subject, study and series counts to the ones read out of v0's database
   for the same cohort, a difference allowed only with a named cause in the
   counts file. The cohort, on its archive host, 2026-09-06: 497,150
   files seen, 493,708 parsed, 3,442 refused by name (3,309 of a SOP class
   the engine does not read, 3,177 of them secondary captures; 124 not
   DICOM; 9 without a study UID); 44 subjects, 82 studies, 2,165 series,
   2,534 stacks, in 737 s at 675 files a second on the busiest host we
   have, at 1,002 MB peak. Against v0's 43 subjects, 203 studies and 3,599
   series the three differences have their causes named in the counts
   file and were measured in Wave 1: the studies and series of 17 subjects
   who are also in other cohorts sit under the other cohorts' roots
   (128,880 instances, excused by name), the 82 studies and 2,165 series
   here are the 82 and 2,165 both sides hold, and the one subject more is
   the folder Wave 1 moved aside on the tank (10,820 instances, a name
   shaped like a code that matches no identifier), still under the archive
   host's own root; the baseline host, reading the tank, counts 43.
2. Slice 1's measurement, §4.4: the release's bookkeeping at 150,000 and
   1,000,000 files in both layouts, flat, and a re-run writing nothing; the
   reference gate's bar 9 and the cohort gate's re-runs keep it so.
3. Slices 2 and 3, §5.5 and §5.6: the pack's private elements read on the
   three surveyed vendors, and the reference gate's bar 8b, which reads the
   released tree with `nils private` and finds nothing the pack did not
   name.
4. Slice 4, §6.1: the CLI suite runs a full round on Postgres, and the
   store's Postgres text decoder is what a raw date projection would have to
   get past.
5. Slice 4, §6.1: one row per axis value, and the role match is `IN`.
6. Slice 9, §7.4: the reference gate's bar 10 compares each session's EDSS
   with the registry's own nearest under keep and shift.
7. Slice 7, §7.2: the cohort gate runs the cohort, membership, demographics,
   disease and event files through the one importer, and the second apply
   changes nothing. The cohort's four files and the cohort row: 1
   cohort, 43 members, demographics for 43 rows (11 added, 3 already equal,
   36 with nothing to import, none refused otherwise), 40 diagnoses, 298
   events, each applied and each second apply changing nothing (every row
   skipped as already held).
8. Slice 13, §10.1: at the pack's thresholds the cohort asks nothing but the
   quarantine, and the mixed corpus asks 77 items in 9 groups; the cohort
   gate counts the open queue after its classification. The cohort at the pack's thresholds: 2,534 stacks
   classified, 0 questions raised, 3 open items, all of them the digest's
   quarantine notes.
9. Slice 14 and 15, §11: the reference gate's bar 9b drives status, custody,
   selection, the review queue and a release through `nils serve` and holds
   the door's answers to the command line's, the release queued through the
   door and run by a worker writing the tree the command line wrote; the
   cohort gate does the same on the cohort; the door's tests do it in every
   auth mode.
10. Every store the custody table lists carries an `owner` and a `kept`
    (§13.1): the job log keeps a year of finished jobs and `nils jobs prune`
    is its deleter, the audit log is kept for ever, and a release is never
    removed, only withdrawn with a reason by `nils release --withdraw VERSION
    --why TEXT` (migration 25), which the audit records.
11. `budget.sh` on the baseline host: the digest's rate held to a floor of
    500 files a second and its peak memory to a cap of 8 GB, the second pass
    over the unchanged tree, and the release's re-run writing nothing; CI
    holds the synthetic benchmark to Wave 1's baseline on every push.
    The baseline host (8 vCPU, 64 GB, the archive over
   NFS), 2026-09-06: 486,326 files digested in 78 s at 6,246 files a second
   and 892 MB peak; the second pass over the unchanged tree 4 s; the
   fingerprint 1 s; the classification under a second; the descriptive
   release of 482,888 files for 43 subjects 482 s; its re-run 1 s and
   nothing written. Every bar.

## 13. Defaults settled in this spec

Written here so that the wave does not stop to ask, and for Nima to strike.

### 13.1 Retention

| store | kept | deleted by |
|---|---|---|
| `release`, `release_stack`, `release_move`, `handover*` | for ever: they are the history of what left | nobody; a release may be *withdrawn* with a reason, never removed |
| `review_item`, `decision` | for ever, withdrawn not deleted | nobody |
| the audit log | for ever | nobody |
| the job log | one year of finished jobs | `nils jobs prune` |
| importer uploads | until applied, then the mapping and the counts stay and the file goes | the apply |
| `private_element` catalogue rows | with the instance they belong to | the same rule as every catalogue row |
| the linkage store | as Wave 1 §7 says | as Wave 1 §7 says |

A store owned by another repository (an app's cache, a pipeline's derivative)
appears in the custody table by that repository registering it through the
API, which is the mechanism C38 asked for and this wave supplies as a door.

### 13.2 Corrections to clinical rows: supersede, never delete (§7.1).

### 13.3 A demographics conflict is a review item (§7.2).

### 13.4 An acknowledgement is not a decision (§9.2).

### 13.5 The epoch

The registry epoch advances on any write that changes a judgement: an ingest
batch, a classification run, a decision, an import, a release. A handle re-read
after a human decision must report a different epoch, or the reproducibility
claim of the question wave is false before it is made.

### 13.6 The store stays blocking; the server pools it.

The compiler of Wave 4b may ask for more, and that is its talk's question.

### 13.7 A cohort is a membership fact in this wave.

Whether a saved selection can also be one is Wave 4b's talk; the fact is built
either way, because the importer needs it.

## 14. Order of work

| # | slice | group |
|---|---|---|
| 1 | the manifest at stack grain, current state plus log, diffed in SQL, measured | debt |
| 2 | private elements ingested, the dictionary as pack data, `nils private` grows | ingestion |
| 3 | the allowlist from the surveys, two lists, vendor coverage stated | ingestion |
| 4 | the five faults | faults |
| 5 | `contracts/` stops being empty | faults |
| 6 | the clinical schema, vocabulary as pack data | registry |
| 7 | one declarative importer, proved on the real files | registry |
| 8 | `Anchor::Event` and the nearest event | registry |
| 9 | clinical export in the release, and Wave 3's bar 10 | registry |
| 10 | selection at the four grains, `nils select` | selection |
| 11 | one job model | jobs |
| 12 | the principal and the audit log | jobs |
| 13 | the review spine, measured first | review |
| 14 | `nils serve` | doors |
| 15 | three auth modes, with the federation riders | doors |
| 16 | the gate | gate |

Slices 1 to 5 start now and are independent of one another. Slices 6 to 9 are
one chain. Slices 10 to 13 depend on 6 only where they touch a cohort. Slices
14 to 16 come last, and the talk that opens Wave 4b can happen while they are
being built.

**A schema change lands with the slice that needs it**, as every wave so far.

## 15. Open questions carried into the wave

1. **Which private dictionary**, and under what licence it may be redistributed
   as pack data (slice 2).
2. **The 484 orphaned private elements** of the mix: a property of a vendor's
   export, of an anonymiser that removed creators and not blocks, or of the
   reader. Answered by `nils private` reporting orphans by group (slice 2).
3. ~~**Whether `release_file` survives as an opt-in** or is simply dropped.~~
   Dropped, no opt-in, slice 1 (§4.3): a second description of the tree is
   one more thing to disagree with the first.
4. **The names of the two apps** (17 §8), which this wave does not need but
   `capabilities` will report.
