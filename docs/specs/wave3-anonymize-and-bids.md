<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Wave 3: repairs, release and export

The specification of the third wave of the NILS rewrite. It follows
`wave2-fingerprint-and-classify.md` and the design record it cites by id
(`docs/decisions/`, D13, D16, D17, C8, C19, C35, C36).

It opens with four repairs rather than with new work, because a capability audit
of v0 written before this spec found four things v0 does that no wave of v1
owns, and two of them block digesting data the group already holds. A release
that names a subject wrongly or cannot date a session is not worth building on
top of, so the ground comes first.

## 1. What Wave 3 delivers

Four repairs, then the release.

**The repairs.** Each is a gap in shipped v1 code, found by reading v0 rather
than by a failing test:

1. **Identity from the path.** Several MS cohorts carry no usable `PatientID`:
   every value is the placeholder `XXXX`. Their subject code exists only as the
   first directory under the batch root. v1 cannot read it, so it would digest a
   five-visit person as five subjects (§3).
2. **The study date.** v0 repairs a missing `StudyDate` from three other date
   fields and then from the date embedded in DICOM UIDs, and refuses to sort a
   study it cannot date. v1 stores what it read and has no repair (§4).
3. **The session scheme**, which Wave 1 §4.4 specified more weakly than v0
   already implements: four anchor kinds including clinical events, a cadence
   with a real tolerance, four collision policies, and an answer for a session
   that fits no schedule (§5).
4. **Handover.** How a dataset physically leaves is a capability v0 has and no
   v1 wave owned (§11).

**The release.** One verb, `nils release`, that selects, de-identifies and
writes, because v0's two export callers differ only in things v1 no longer has
(§2.3). It gains the concept v0 lacks, the **disposition** (§7), and writes two
layouts from one set of facts: `descriptive`, which can name everything we hold,
and `bids`, which is valid and routes the rest honestly (§9).

And the four **fingerprint fields** Wave 2 ruled were not passes (§6).

## 2. What reading v0 found

The audit is the record's document 16. Three findings shape this wave.

### 2.1 The order inverts, and that explains v0

v0's pipeline is

    anonymize -> extract -> sort (checkup, fingerprint, classification, completion) -> bids

Anonymization runs **first**, over a directory tree, before a tag reaches a
database. The on-disk convention is `<cohort>/derivatives/dcm-original` for the
identifiable source and `dcm-raw` for the anonymized copy, and extract, the
exports and the pipelines all read `dcm-raw`. **In v0 the sensitive artefact is
a folder and everything downstream works from a de-identified copy.**

Most of what looks odd in v0's anonymizer follows from that: it walks the tree
to discover identifiers because there is no registry to ask, it renames folders
because folders are the only structure it has, and it writes its audit beside
the originals because there is nowhere else.

Two consequences are not cosmetic. Its audit and the later extract join on
`StudyInstanceUID`, so **UIDs can never be remapped**; and the session is
recovered by parsing `StudyDate` out of the tree, so **StudyDate can never be
rewritten**. The code says so in a comment.

v1 inverts the order: digest reads the original, identity is resolved and
pseudonymised at ingest behind a key, and de-identification is the last step
rather than the first. The sensitive artefact becomes the registry, which is the
thing we guard, and the release becomes the thing we hand over. Every entry
above falls out of that one change.

### 2.2 The identity chain is two packages

The audit's §1.1, and the repair of §3. v0 resolves identity twice:

1. `anonymize` sets `PatientID` from a folder segment by regex at a configured
   depth.
2. `extract` derives `subject_code = blake2b(PatientID or StudyInstanceUID,
   seed)`, with an optional CSV override.

So **folder to PatientID to subject code**. Every cohort that ran anonymization
is configured this way, with `depth_after_root: 1` and `regex: "(.+)"`: the
subject is the first folder, taken whole. This is an import mechanism rather
than a de-identification one. The sender pseudonymised already and put the code
in the path instead of the tag.

### 2.3 There are two exports and no longer a reason for two

v0's own runner says the cohort stage and the standalone job "run the same
underlying engine ... the two callers only differ in scope (`cohort_name` vs
`include_stack_ids`), output root, and pipeline coupling."

All three differences are gone in v1. Digest replaced the cohort pipeline, so
there is no stage to couple to; a cohort is a membership fact a subject carries
rather than a pipeline instance, so both scopes are selections; and the root is
an argument. **One export.**

## 3. Repair one: identity from the path

### 3.1 What is missing

`identity.from[].field` must name a DICOM keyword, and `StudyInstanceUID` is the
only permitted fallback. There is no way to say "the subject code is the first
directory". For data whose `PatientID` is a constant placeholder, every file
takes the fallback, so every study becomes its own subject.

### 3.2 The rule gains a path source

```yaml
identity:
  id_type: study-code
  code: verbatim
  from:
    - field: PatientID
      pattern: '^(?<id>[A-Za-z]{2,}[0-9]{2,})$'
    - path:
        segment: 1
        pattern: '^(?<id>.+)$'
  fallback: StudyInstanceUID
```

`segment` counts directories from the batch root, one-based, which is v0's
`depth_after_root`. `pattern` reads its `id` group as every other source does. A
path entry is tried in order with the field entries, so the example above says
"the tag when it is shaped like a code, otherwise the folder", which is the rule
the MS data needs and is safe on data where the tag is good.

Three properties the engine enforces, each because of something measured:

- **A pattern is what refuses a placeholder.** `XXXX` does not match the pattern
  above, so the tag entry declines and the path entry answers. The existing
  semantics already say a non-matching value moves on; this only extends what a
  source may be.
- **The path is the path as digested**, relative to the batch root, so two
  digests of one tree agree, and a tree moved on disk does not change a subject
  code.
- **A path source requires `code: verbatim` or an explicit pattern.** A folder
  name is a code somebody chose, not an identifier to derive one from, and
  filing it as a derived identifier would put a chosen code and a hashed code
  under one type.

### 3.3 The placeholder diagnostic

A tag that is constant across a batch is a placeholder, and nothing in v1 would
have said so. The digest counts `identity_constant` when one distinct value of
the chosen field covers more than a configured share of a batch's instances,
with the value's **shape** as its sample, never the value. A run that produces
one subject per study while this diagnostic fires is a misconfiguration, and the
report says it in those words rather than leaving someone to notice the subject
count later.

### 3.4 A path can be a direct identifier

Wave 1 §4.3 classes `source_file.path` as quasi-identifying "since a path can
hold a name". If a folder is a personal identity number, the path is
**directly** identifying, and it is stored in the registry in clear. Three rules
follow, and the release depends on them:

- No output path is ever derived from a source path. The release writes a fresh
  tree from registry facts (§9), so a source folder name cannot leak into it.
- The class is enforced wherever Wave 1 already enforces one: a diagnostic
  sample of a path is a shape, never the value.
- The audit of §8.5 records no source path.

## 4. Repair two: the study date

### 4.1 What v0 does, and why it matters

Sort step 1.3 fills a missing `study_date` from `series_date`, then
`acquisition_date`, then `content_date`. `sort/date_recovery.py` then extracts
`YYYYMMDD` from DICOM UIDs by regex, trying the study UID, the series UID, the
frame-of-reference UID, the media-storage SOP UID and the SOP UID in that order,
accepting only a real calendar date inside a year range.

**A study with no recoverable date is excluded from sorting entirely**, and if
every study is, the step fails. That is the answer to why it matters: without a
date there is no session, no order, no clinical join and no `ses-` directory.
The date is a precondition, not a convenience.

### 4.2 What v1 does

The repair runs at digest, where the facts are, and it never writes over what
was measured, which is the fault Wave 2 measured in v0's acquisition-type fill:

- `study.study_date` keeps what `StudyDate` said, and stays null when it said
  nothing.
- `study.date_filled` and `study.date_source` are added. `date_source` is one of
  `study_date`, `series_date`, `acquisition_date`, `content_date`, `uid`, or
  null when nothing yielded a date.
- Everything downstream reads the study's date through one accessor that
  prefers the measured value, so no query has to know the repair happened, and
  every report can say how many dates were derived and from where.

A study still without a date is **not** excluded. It is kept, it counts a
`study_undated` diagnostic, and it raises a review item, because in v1 an
unanswerable question is a question rather than a deletion. What it cannot do is
join a session (§5), and the release says so per subject rather than dropping
the files silently.

The year range that makes a UID date credible is a knob with a default, exposed
like every other knob (C37), because "eight digits that parse as a date" is a
guess and the range is what makes it a reasonable one.

### 4.3 The UID carries the date, so the two policies are one

This is the finding that most changes the release, and the first draft of this
spec had it wrong by treating UID remapping and date policy as independent.

Because a UID can embed `YYYYMMDD`, and because it is the last-resort date
source:

- **A release that shifts or truncates dates must remap UIDs.** Otherwise the
  true date leaves in the UID and the policy is decorative. The engine refuses
  the combination rather than warning about it.
- **A release that preserves UIDs may only keep dates**, and says both in its
  report and in the dataset description.
- **A release always writes the date it used into the tree** (§9.4), because for
  a study whose date came from a UID, remapping the UID means the tree can never
  re-derive it.

## 5. Repair three: the session scheme

Wave 1 §4.4 described a scheme with a window, three label styles and one anchor.
v0's `timeline/` is richer and already right, so v1 carries it whole:

- **Anchor**: `first_session`, `onset_event`, `diagnosis_event`,
  `explicit_per_subject`. The clinical anchors are what make `M00` mean
  something clinical rather than "the first scan we happen to hold". Resolving a
  kind to a date needs event rows, so the caller resolves it and hands the
  resolver a date, which keeps the resolver pure.
- **Cadence** with a **float** tolerance. A session lands on the nearest nominal
  visit inside the tolerance, and otherwise keeps its own real month, so an
  off-schedule visit reads as `M09` rather than vanishing.
- **Collision policy**, four of them, with **merge** the default and the
  argument for it carried over: two sessions on one label is normal clinical
  reality, a continuation scan or a brain study and a spine study a day apart,
  and demoting the second invents a timepoint nobody scanned.
- **Unmatched policy** for a session with no anchor.
- **Pre-anchor** sessions are `PRE06`, never `M-06`, because a hyphen is BIDS's
  key-value separator. Under a diagnosis anchor, 9.9 percent of live sessions
  precede their anchor, and for a quarter of those subjects label order runs
  backwards against date order.

The scheme is data, stored per registry with a selection able to carry its own,
and labels are derived on read so that re-labelling is an edit to one scheme
rather than a migration. Nothing about a session is stored as a fact.

**A label is not a key.** `M12` does not identify a session: two can share it,
one can be `PRE06`, one can be its own real month. Anything that joins reads the
date, and §9.4 puts the date where BIDS puts it.

## 6. The four fingerprint fields

Wave 2 §7.3 ruled these are fingerprint work and not passes, and this is the
wave that does them: field strength normalised, acquisition type inferred, DWI
enrichment, and the session rescue as a fact about a study. Each is computed
from what was measured and stored **beside** the measured column, never over it.

## 7. The disposition

The concept v0 lacks and every one of its export bugs needs.

A stack's **disposition** is what kind of thing it is for the purpose of getting
it out. It is derived from the decided axes and the fingerprint by rules the
**pack** declares, and it answers three questions at once:

- **kind**: `acquisition`, `scanner_derived`, `reformat`, `working_scan`,
  `scout`, `excluded`.
- **convertible**: whether a NIfTI of it is meaningful.
- **target**: where it lands in each layout, and under what name.

v0 has this concept in pieces and under other names. Its export carries
`NIFTI_INCOMPATIBLE_PROVENANCES = {"SyMRI"}`, and its pick config carries
`non_canonical_constructs: [MIP, MPR, Reformat, Synthetic]`. Both are
dispositions written in the one place that needed them.

Two rules the engine enforces because they are structural rather than
vocabulary. A stack that is not `convertible` is never handed to a converter,
and the reason is reported. And **a disposition never depends on what else is in
the selection**, which is v0's fourth export bug and the same fault as C14.

### 7.1 SyMRI, the case that proves it is needed

`provenance` alone answers nothing. The archive's 36,692 SyMRI stacks are three
different things:

| what | stacks | disposition |
|---|---|---|
| the MDME working scan, magnitude and phase | 33,692 | `working_scan`, not convertible: the series is a TI by TE by complex container, not an image |
| synthetic contrasts: T1w, T2w, FLAIR, PSIR, DIR, PDw, STIR | 2,543 | `scanner_derived`, convertible, ordinary images |
| quantitative maps: T1map, T2map, PDmap, R1map | 82 | `scanner_derived`, convertible, and BIDS names them exactly |
| MyelinMap, MultiQmap | 375 | `scanner_derived`, convertible, no BIDS word |

v0's single rule refuses to convert all 36,692, which is right for 92 percent
and wrong for about 3,000 ordinary images.

## 8. The release

One verb over one selection. A selection is a predicate over the registry:
subjects, cohorts, sessions, stack ids, axes, or any combination.

### 8.1 Identifiers

The registry holds the pseudonym, so the release does not choose one. Direct
identifiers become the subject's code under the registry's declared scheme
(C36). v0's five strategies are §3's work, at ingest, where they belong.

### 8.2 UIDs

Remapped, keyed, deterministically, so the same UID gives the same new UID for
ever and two releases of overlapping selections agree. Nothing downstream needs
the original because the join is the registry's id. `preserve_uids` is a real
policy, defaults to off, and is constrained by §4.3.

### 8.3 Dates

The registry is never rewritten. A release declares `keep`, `shift` (one offset
per subject, drawn once, uniform within +/- 180 days, held in `date_shift`, so
every interval survives and the clinical layer joins as before) or `year`. Age
at study is computed before anything is applied. §4.3 binds this to §8.2, and a
release under `shift` or `year` refuses to write a session label that is a date.

### 8.4 Private tags, overlays, burned-in pixels

Private tags are dropped by default with an allowlist by
`(creator, group, element)`, declared as pack-shaped data because which vendor
tag carries a diffusion direction is knowledge that changes without the engine
changing. Overlay and curve groups are dropped. A stack whose
`BurnedInAnnotation` says yes, or whose image type carries a token we know means
a screenshot, raises a review item and is not written until answered; where the
tag is absent the release says how many stacks it could not judge. The engine
does not look at pixels (§13).

For the record, because it changes what "carried over from v0" means here: every
cohort v0 anonymized set `anonymize_categories: ["Patient_Information"]`, so the
Clinical_Trial and Healthcare_Provider categories in its table are declared and
were never applied.

### 8.5 The audit

Rows, not a workbook beside the originals under a password kept in a database:
`release` (one per run, with every policy), `release_file` (one per file
written, with its instance and the digest of what was written) and
`release_change` (`release, tag, action, count`).

There is deliberately no old-value column and no source path. An audit that
records what was removed is a copy of the identifiers, in the registry, in
clear. What a release removed is recoverable from the originals by someone
entitled to read them.

## 9. Two layouts, one set of facts

Less than half of what we hold has a BIDS name. Routed against the published
BIDS schema, the whole archive:

| route | stacks | share | what it is |
|---|---|---|---|
| raw BIDS tree | 243,705 | 47.0% | a valid datatype, suffix and entity set exists |
| `sourcedata/` | 150,010 | 28.9% | localizers (116,318) and working scans (33,692) |
| `derivatives/` | 41,640 | 8.0% | reformats and projections (35,853), SWI images (5,307), maps with no BIDS word (375), subtractions (105) |
| no BIDS name | 1,714 | 0.3% | functional data with no task (1,167), MTw in anat (442) |
| not exported | 81,296 | 15.7% | the pack ruled them out |

BIDS has no suffix for a localizer, a reformat, a projection, an SWI image or a
synthetic contrast. That is not a defect in BIDS: those are not acquisitions. It
is why there are two layouts and neither is a fallback for the other.

### 9.1 `descriptive`

v0's grammar, carried because it is good and because people have years of files
named this way:

    [BodyPart_]{Orient}_{base}_{acq}_{mods}_{technique}_{accel}_{construct}
        [_CE][_b{N}][_{PE}][_{n}dir][_e{k}|_ti{k}]

It names every stack in the archive, including the 56.9 percent BIDS cannot
place. Three changes, each from a measured fault: the echo and inversion suffix
comes from the fingerprint rather than from `stack_key`, so the vendor that
splits echoes across series is handled; disambiguation is computed over the
session as the registry holds it rather than over the selection; and a character
a filesystem or a downstream tool cannot take is mapped by a declared rule
rather than left to a converter to mangle.

### 9.2 `bids`

The standard's entity grammar, in the standard's order, enforced from the
schema rather than approximated:

    sub-<label>[_ses-<label>][_task-<label>][_acq-<label>][_ce-<label>]
    [_rec-<label>][_dir-<label>][_run-<index>][_echo-<index>][_flip-<index>]
    [_inv-<index>][_mt-<label>][_part-<label>]_<suffix>.<ext>

The mapping is declared in the pack, and every row is measured against the
archive:

| ours | BIDS |
|---|---|
| `base` T1w, T2w, PDw, T2\*w, FLAIR | the suffix, `T2starw` for the third |
| `construct` Magnitude, Phase, Real, Imag | `part-mag`, `part-phase`, `part-real`, `part-imag` |
| `construct` ADC, Trace, FA, colFA, expADC | the `dwi` scanner-derivative suffixes |
| `construct` INV1, INV2 with technique MP2RAGE | suffix `MP2RAGE`, `inv-1`, `inv-2` |
| `construct` Uniform with technique MP2RAGE | suffix `UNIT1` |
| `construct` T1map, T2map, PDmap, R1map, QSM | the `anat` parametric suffixes, `Chimap` for QSM |
| multi-echo GRE, multi-echo SE | `MEGRE`, `MESE`, with `echo-` required |
| `post_contrast` | `ce-` |
| the rest of `technique`, `modifier`, `construct` | `acq-`, under a declared vocabulary |

Entity rules come from the schema: `part` takes only `mag|phase|real|imag`, `mt`
only `on|off`, `echo`, `flip`, `inv` and `run` are indices, and a suffix that
requires an entity is not written without it. `MEGRE` requires `echo`, which
turns v0's second export bug from a cosmetic complaint into a validator error,
which is the right place to catch it.

**`func` requires `task`, and no stack has one.** Of 1,173 functional stacks,
ten carry anything resting-like in their text and none says "task". They are
genuinely functional, we simply do not know what the subject was doing, and no
rule can invent it. So a functional stack with no task raises a review item and
a person answers it once per study or per origin. It is a missing fact, not a
naming problem.

### 9.3 Where the rest goes

`sourcedata/` in a BIDS-shaped tree for working scans and scouts, kept as DICOM,
one directory per stack, which is what a reader of them wants anyway.
`derivatives/nils/` for reformats, projections and SWI images, as a dataset in
its own right with its own description, so the tree stays valid and the data
stays present. And nowhere, with a reason, reported per subject and session,
never silently dropped.

### 9.4 Where the date goes

`_sessions.tsv` has an `acq_time` column and `_scans.tsv` has one per file. The
directory is named by the session scheme, the time is carried in the standard's
own slot under the release's date policy, and anything joining on a date reads
the column rather than parsing a directory name. This is the coupling of §2.1
broken, and it is what makes §4.3's third rule cheap.

### 9.5 The dataset, not just the files

`dataset_description.json` with `GeneratedBy` and `SourceDatasets` naming the
release; `participants.tsv` carrying what the policy allows, which is where
sensitivity classes are enforced; `README`; `.bidsignore`; `sessions.tsv` and
`_scans.tsv`. v0 writes none of these, which is why its tree is not a dataset
rather than an invalid one.

### 9.6 Conversion

`dcm2niix` per pick with an explicit file list and the final name already
decided. Three things v0 lacked: the source of every file comes from the
registry, which since Wave 1 records the path of every instance, so there is no
cohort root to go stale; directories are created only after a conversion
succeeds, so a failure leaves no empty tree to be mistaken for a selection bug;
and a release preflights its roots, its converter and its free space, reporting
one refusal rather than N identical failures.

## 10. Roles and picks

A **role** is a named predicate over the decided axes and the fingerprint. A
**pick** chooses one stack per session and role by an ordered preference,
reporting ties rather than breaking them by row order.

**This already exists in v0 as tuned data** and should be carried rather than
reinvented. `qc/cohort_main/main_qc_weights.yaml` says in its own header "edit
this file to tune the auto-pick algorithm, no code change required", and carries
component weights (dimension, technique, modifier, slices, field of view, cohort
share, orientation, completeness), a provenance penalty, per-contrast technique
tiers, canonical-construct preferences for Dixon and MP2RAGE, border thresholds
that raise a needs-check, and a partial-volume auto-demote.

It becomes pack-shaped data beside the rule sets. What Wave 3 adds is that a
pick is a decision with evidence: which role, which preference fired, what the
alternatives were, and whether it was a tie.

The number that makes it mandatory: **82.5 percent of the archive's sessions
that hold a T1w hold more than one, and the worst holds 462.**

v0 has both an automatic pick and a human one (its Main Acquisition QC, where a
person walks a cohort session by session and writes a token). v1 needs both, and
already has the shape: the pick is computed with evidence, and a person's call
is a `decision` at a scope that outranks it.

## 11. Repair four: handover

`compress/` packs a de-identified tree into password-protected 7z archives in
roughly 100 GB chunks, with encrypted headers so the filenames inside are
covered too, two packing strategies, optional PAR2 recovery records,
verification and a checksum per archive. It is how a dataset physically leaves,
and no v1 wave owned it.

It belongs here because it is the last step of a release, and because a release
that cannot be handed over is not finished. What v1 adds is that the archive set
is part of the release record: each archive is a row with its checksum, its
members and the release it belongs to, so "what did we send them, and is it
still intact" is a query rather than a folder someone remembers.

The password is a key, handled as the registry handles keys, and never a column.

## 12. The gate

The oracle is not v0 (D16): its export is not valid BIDS, so byte-identity
against it would be a bar against being correct.

1. **The repairs are proved on the data that needed them.** The MS trees with
   placeholder identifiers digest into the right number of subjects and
   sessions, hand-verified against the folder structure; and studies whose date
   is only in a UID are dated, with the source recorded.
2. **The validator passes** on the reference selections, no warnings suppressed.
3. **The reference selections are right**: hand-verified, in the pack's corpus
   the way Wave 2's cases are, each naming the session, the role, the pick, the
   disposition and the resulting filename in both layouts.
4. **Every stack is placed**: in the raw tree, in `sourcedata`, in `derivatives`
   or in the report with a reason, and the counts reconcile to the selection.
5. **The descriptive layout names everything.**
6. **One stack per session and role**, ties reported.
7. **Every file is traceable** through `release_file` to an instance.
8. **The de-identification does what it says**: no tag from the removed set, no
   private tag outside the allowlist, no overlay group, no UID that appears in
   the source, and under `shift` no date that appears in the source **including
   inside a UID**, which is §4.3 as a test.
9. **Round trip and increment**: two runs over one selection agree; a run over a
   superset leaves the first run's files untouched.
10. **The clinical join survives**: for the reference selections, the EDSS
    nearest each scan is the same computed from the registry and from the tree,
    under every date policy.
11. **The handover verifies**: the archive set unpacks, every checksum matches,
    and the release record accounts for every file.
12. **The budget**, measured on the baseline host and gated in CI.

## 13. What Wave 3 does not do

- **Defacing, and every other change to pixels.** A pipeline, not a property of
  the registry. v0 already has the seam: a Boutiques-subset descriptor with an
  `x-nils` block declaring a container image, BIDS-Apps analysis levels, a work
  unit and a derivatives ingest entrypoint. A defacer is a BIDS App over the
  tree this wave produces and its output is a registered derivative. NILS holds
  the registry and produces the dataset.
- **Derivative registration.** The seam is declared here; the machinery is
  Wave 7's, and v0's own ingest has its database write gated off, so there is
  nothing to carry.
- **The image-model body-part QC.** 6,672 lines in v0, and the reason the parity
  gate saw 4,692 differences. It is a model plus a review workflow, and it wants
  its own wave alongside the other QC products.
- **The full catalog of roles and picks** (Wave 4 and 5, C19) and **the
  migration of the live registry** (Wave 4).

## 14. Order of work

The repairs first, because everything after them assumes a subject and a date.

1. Identity from the path, and the placeholder diagnostic (§3).
2. The study date repair, at digest (§4).
3. Migration 5: the date columns, the release tables, the fingerprint columns.
4. The session scheme, whole (§5).
5. The four fingerprint fields (§6).
6. The disposition, in the pack, with its corpus cases (§7).
7. Roles and picks, carrying v0's weights (§10).
8. `nils release`: selection, identifiers, UIDs, dates, with §4.3 enforced (§8).
9. Private tags, overlays, burned-in, the audit (§8.4, §8.5).
10. The descriptive layout (§9.1).
11. The BIDS layout, the dataset files, `sourcedata`, `derivatives` (§9.2-§9.6).
12. Handover (§11).
13. The gate (§12).

This is a large wave, and it is worth saying so rather than discovering it.
Slices 1 and 2 are small and unblock real data, so they can land and be used
before the rest is written.

## 15. Open questions carried into the wave

1. **The UID root** for keyed remapping: a registered OID arc, which is right
   for a tool meant to be adopted, or a UUID-derived root, which is legal and
   ugly.
2. **Where localizers go.** 116,318 stacks, 22 percent of the archive, and BIDS
   has no word for them. `sourcedata/` is this spec's answer and dropping them
   from a release is also defensible.
3. **Whether vendor synthetic contrasts belong in raw `anat/`.** The BIDS qMRI
   appendix permits vendor pre-generated maps there; a purist would put every
   synthetic image in `derivatives/`. 2,543 stacks turn on it.
4. **Who answers the task question** for functional data: a decision per study,
   per origin, or a release argument.
5. **Whether a decision needs a draft state.** All five of v0's QC products
   write a draft to one database and push to the other on explicit confirm.
   v1's `decision` is a single-step commit, and D14's staged results are the
   same idea in another place.
6. **The private-tag allowlist seed**, answerable from the corpus.
7. **The default date policy per registry.** `keep` is right for KI today.
