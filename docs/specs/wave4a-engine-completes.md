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

**The diff is computed in SQL.** This version's per-stack state is written to
the table and the five outcomes (unchanged, moved, rewritten, added, removed)
are a join against the previous state, not two maps in memory. Memory becomes
independent of the number of files.

### 4.3 What is lost, and what replaces it

A per-file digest becomes a per-stack roll-up: the digest of the file digests,
with the count and the bytes. A handover verifies at stack granularity and
names the stack rather than the file; a per-file check is a recomputation. The
handover's own record already carries a checksum per archive, which is the
right place to spend bytes on verification.

`release_file` is dropped, or kept as an opt-in a deployment enables when it
wants a per-file manifest and can afford it. The default is dropped.

### 4.4 Measured, not asserted

The slice lands with a benchmark of the release and its re-run at 150,000 and
1,000,000 files, today's design against the new one, and the numbers go in the
pull request and in §12's budget bar. Nima asked for exactly this: test which
is more efficient rather than assume.

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

### 6.2 `contracts/`

The directory holds a licence and a README. Three contracts are overdue:
`pack/` (the pack format is data and has been since C11; a pack's `contract:`
field already names a version nothing checks), `review-item/` (the schema of
D7), and `openapi/` (empty until §11 fills it). Each gets a skeleton, a version,
and the rule that a version changes only by a pull request that says so. A
contract test exists and is green, even if it tests little, because a test that
exists is amended and one that does not is never written.

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

### 7.3 `Anchor::Event`, and the nearest event

Wave 3 §5 carried the session scheme whole and declared the clinical anchor
without being able to serve it. With events in the registry it is served: a
scheme may anchor on a diagnosis, an onset or a treatment start, and the
labels follow. The one temporal function the engine needs beyond that is
**the nearest event of a type to a date**, which the release of §7.4 and the
gate's bar both use. It is a function, not a clause: the general windows are
Wave 4b's.

### 7.4 Clinical export in the release

`participants.tsv` carries the sex and the age at first session; `sessions.tsv`
carries the age at session and the nearest observation the release names,
under the date policy (an age is computed before a birth date goes, Wave 3
§8.3; under `shift` the observation's date moves with the subject's offset;
under `year` no date is written at all). Sensitivity is enforced here: a field
the pack marks sensitive never reaches the tree.

This closes **Wave 3's deferred bar 10**: the EDSS nearest each scan is the
same computed from the registry and from the tree, under every date policy.

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

## 9. Jobs, the principal and the audit

### 9.1 One job model

Two verbs claim work today through two ~60-line paths of their own. They become
one: per-kind claims, a queue, a worker, a pool. Every verb that runs longer
than a second is a job row with progress; `nils jobs` lists, cancels and
resumes; a job is resumable because nothing is in flight (principle 4). The
doors of §11 answer 202 with this job's id, and nothing else, for anything
heavy.

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

### 11.2 Three modes

`off`, `token`, `oidc` (D8). Groups map to roles; the engine stores no
password, mints no session and owns no user table beyond a cache of claims. In
`oidc` mode the audit principal is the subject. The federation primitives that
cost nothing now ride here (C26, C27, C30, C33): the principal's `user@node`
shape, the epoch in capabilities, and `local`, `federated` and `sensitive` as an
attribute the pack may put on a field, read by nothing yet.

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
3. **Whether `release_file` survives as an opt-in** or is simply dropped
   (slice 1; the default is dropped).
4. **The names of the two apps** (17 §8), which this wave does not need but
   `capabilities` will report.
