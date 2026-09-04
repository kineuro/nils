<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Wave 3: release, anonymize and export

The specification of the third wave of the NILS rewrite. It follows
`wave2-fingerprint-and-classify.md`, which built the classification this wave
selects from, and the design record it cites by id (`docs/decisions/`, D13, D16,
D17, C8, C19, C35, C36).

It was written after reading v0's `anonymize/`, `bids/`, `timeline/`,
`stages/` and `analysis_pipeline/` packages end to end, after reading the BIDS
schema, and after measuring the live archive. Sections 2 and 3 are what that
found, because most of this wave's design is a consequence of two facts that are
not obvious until you look: **v0 anonymizes before it ingests**, and **less than
half of what we hold has a BIDS name at all**.

## 1. What Wave 3 delivers

The same binary, `nils`, gains

1. **`nils session`**: the session scheme, carried whole from v0's `timeline/`,
   which is richer than Wave 1 §4.4 assumed.
2. **the disposition**: a stack's answer to "what kind of thing are you, for
   the purpose of getting you out of here", declared in the pack (§4). This is
   the concept v0 lacks and the reason its export has the bugs it has.
3. **`nils release`**: one job that selects, de-identifies and writes. v0 has an
   anonymize stage and two export callers; v1 has one verb, because the reasons
   they were separate are gone (§3.3).
4. **two layouts from one set of facts**: `descriptive`, which can name
   everything we hold, and `bids`, which is valid and names what BIDS has words
   for and routes the rest honestly (§5, §6).
5. the four **fingerprint fields** Wave 2 ruled were not passes (§8).

## 2. The order inverts, and that explains v0

v0's cohort pipeline, from `stages/ordering.py`, is

    anonymize -> extract -> sort (checkup, fingerprint, classification, completion) -> bids

Anonymization runs **first**, over a directory tree, before a single tag reaches
a database. Almost every strange thing about v0's anonymizer is a consequence of
that, and not a decision anybody made:

| what v0's anonymizer does | why | what v1 does instead |
|---|---|---|
| walks the tree and discovers PatientIDs from the files | there is no registry yet to ask | the registry knows every instance, its subject and its path (Wave 1, D17) |
| carries five ID strategies (sequential, path, deterministic, CSV, CSV with fallback) | five answers to "what should this subject be called", asked before anything knows | one answer: the subject's code under the registry's declared pseudonym scheme (C36) |
| renames patient folders | folders are the only structure it has | there are no folders to rename; the release writes a new tree |
| writes an audit workbook into the folder holding the originals, under a default password | there is nowhere else to put it | registry tables (§7.5) |
| never remaps a UID, and skips every element whose VR is `UI` | the audit and the later extract join on StudyInstanceUID; change it and the pipeline loses itself | the join is the registry's own id, so UIDs are free to remap |
| keeps StudyDate, with a comment saying the exporter needs it as the session key | the session is derived from the date by parsing it out of the tree | the session comes from a scheme over the registry, so the date is free (§9) |

The last two are the ones that matter. **v0 preserves the two identifiers with
the longest reach, dates and UIDs, not as a policy but because anonymizing
before ingesting leaves it nothing else to join on.** In v1 the order is
`digest -> fingerprint -> classify -> release`, identity is resolved and
pseudonymised at ingest with the linkage store behind a key (Wave 1 §7), and the
release is the last step rather than the first. Every entry in the right-hand
column above falls out of that one change.

One consequence worth stating plainly: v1's registry holds true UIDs and true
dates, and a release decides what leaves. That is a stronger position than v0's,
not a weaker one, because the registry is the guarded thing and the release is
the thing you hand over.

## 3. What else the reading found

### 3.1 The anonymizer removes tags and nothing else

`anonymize/scrub.py` deletes the tags its categories name and rewrites
`PatientID`. That is the whole operation. Every UID survives, because the scrub
skips any element whose VR is `UI` and any tag whose name contains `uid`, or
both `referenc` and `sequence`. There is no private-tag policy, no overlay or
curve removal, and no burned-in check: the words do not appear in the package.

`preserve_uids` does not preserve UIDs. It defaults to true and reaches exactly
one line, `ds.save_as(..., enforce_file_format=not opts.preserve_uids)`. Setting
it false remaps nothing. A site that turned it off believing it was
de-identifying UIDs would have been wrong, and nothing in the output would say
so.

### 3.2 The export is not a BIDS dataset

`bids/` writes no `dataset_description.json`, no `participants.tsv`, no `README`
and no `_scans.tsv`, and never runs the validator. A directory without
`dataset_description.json` is not a dataset, whatever the filenames say. It does
pass `-b y` to `dcm2niix`, so per-image sidecars exist.

v0's own bug list has three entries; there are four. The fourth is in a
docstring in `bids/query.py` rather than the list: the echo suffix disappears
when a single echo of a two-echo series is exported, because the collision
counter counts stacks per series over the **already filtered** selection. All
four are one mistake, a name derived from what we concluded and disambiguated by
a counter over whatever happened to be selected, which is exactly what an entity
grammar removes.

### 3.3 There are two exports and there is no longer a reason for two

`bids/runner.py` says it plainly: the cohort `bids` stage and the standalone
`export` job "run the same underlying engine ... the two callers only differ in
scope (`cohort_name` vs `include_stack_ids`), output root, and pipeline
coupling."

In v1 all three differences are gone. Digest replaces the cohort pipeline, so
there is no stage to couple to. A cohort is a membership fact a subject can
carry, not a pipeline instance, so "the cohort's stacks" and "these stack ids"
are both just selections. The output root is an argument. **So there is one
export, and this wave builds one.**

### 3.4 Less than half of what we hold has a BIDS name

Every stack of the archive routed under the BIDS schema (BIDS 1.10 objects, read
from the published schema, not from memory):

| route | stacks | share | what it is |
|---|---|---|---|
| raw BIDS tree | 243,705 | 47.0% | a valid datatype, suffix and entity set exists |
| `sourcedata/` | 150,010 | 28.9% | localizers (116,318) and SyMRI working scans (33,692) |
| `derivatives/` | 41,640 | 8.0% | reformats and projections (35,853), SWI images (5,307), maps with no BIDS word (375), subtractions (105) |
| no BIDS name | 1,714 | 0.3% | functional data with no task (1,167), MTw in anat (442), no intent (102) |
| not exported | 81,296 | 15.7% | the pack ruled them out |

**BIDS has no suffix for a localizer, a reformat, a projection, an SWI image or
a synthetic contrast.** It has no `mpr`, no `mip`, no `scout`. That is not a
defect in BIDS; those things are not acquisitions. But it means a BIDS-only
export silently loses or mislabels more than half of a real clinical archive,
which is the whole argument for §5.

### 3.5 SyMRI is three different things

The clearest case that a stack's provenance does not determine its disposition.
36,692 SyMRI stacks split:

| what | stacks | disposition |
|---|---|---|
| the working scan, magnitude and phase of the MDME acquisition | 33,692 | DICOM only. `dcm2niix` cannot convert it: the series is a TI by TE by complex container, not an image. |
| synthetic contrasts (T1w, T2w, FLAIR, PSIR, DIR, PDw, STIR) | 2,543 | images, vendor pre-generated, which the BIDS qMRI appendix says MAY live in raw `anat/` |
| quantitative maps (T1map, T2map, PDmap, R1map) | 82 | raw `anat/`, and BIDS names them exactly |
| MyelinMap, MultiQmap | 375 | images BIDS has no word for, so `derivatives/` |

So `provenance == SyMRI` answers nothing on its own, and v0's single
`NIFTI_INCOMPATIBLE_PROVENANCES = {"SyMRI"}` is right for 92 percent of them and
wrong for the rest: it refuses to convert 3,000 stacks that are ordinary images.

### 3.6 BIDS wants an entity our data does not have

`func` requires `task-`. Of 1,173 functional stacks, ten carry anything
resting-like in their text and **none says "task"**. They are genuinely
functional (1,111 of the 1,120 RESOLVE ones say "bold", 1,081 say "fmri", so the
intent is right and the technique is a multi-shot EPI readout). We simply do not
know what the subject was doing, and no rule can invent it.

That is not a naming problem, it is a missing fact, and v1 already has the shape
for a missing fact: a question at a scope. A functional stack with no task
raises a review item, and a person answers it once per study or per origin.

## 4. The disposition

The concept this wave adds, and the one v0 lacks.

A stack's **disposition** is what kind of thing it is for the purpose of getting
it out of the registry. It is derived from the decided axes and the fingerprint
by rules the **pack** declares, exactly as every axis is (Wave 2 §5), and it
answers three questions at once:

- **kind**: `acquisition`, `scanner_derived`, `reformat`, `working_scan`,
  `scout`, `excluded`. What the thing is.
- **convertible**: whether a NIfTI of it is meaningful. A working scan is not an
  image; a reformat is.
- **target**: for each layout, where it lands and what it is called.

The kinds are few and the vocabulary is the pack's, so a site with a scanner we
have never seen adds rules, not Rust. The engine holds no value of any of them,
which is the seam Wave 2 proved for modality.

Two rules the engine does enforce, because they are structural rather than
vocabulary:

- A stack whose disposition is not `convertible` is never handed to a converter.
  v0 discovered this one provenance at a time; here it is a declared property
  with a reason attached, and the reason is reported.
- A disposition never depends on what else is in the selection. That is bug 4
  of §3.2 and it is the same fault as C14: an answer that changes with the batch
  is not an answer.

## 5. Two layouts, one set of facts

Nima's requirement, and §3.4 is the argument for it: **a descriptive layout that
can name everything we hold, and a BIDS layout that is valid.** Neither is a
fallback for the other. They are two renderings of the same decided axes, chosen
per release, and a release may ask for both.

### 5.1 `descriptive`

v0's grammar, carried over because it is good and because people have years of
files named this way:

    [BodyPart_]{Orient}_{base}_{acq}_{mods}_{technique}_{accel}_{construct}
        [_CE][_b{N}][_{PE}][_{n}dir][_e{k}|_ti{k}]

It names every stack in the archive, including the 56.9 percent BIDS cannot
place, which is why it is a first-class layout and not a legacy one. Three
changes, each from a measured fault:

- The echo and inversion suffix comes from the fingerprint's echo number and
  inversion time, not from `stack_key`, so the vendor that splits echoes across
  series is handled (bug 2).
- Disambiguation is computed over the session **as the registry holds it**, not
  over the selection (bug 4).
- A character a filesystem or a downstream tool cannot take is mapped by a
  declared rule, not left to a converter to mangle (bug 1). `T2*w` becomes
  `T2starw` because that is what the rest of the world calls it.

### 5.2 `bids`

The standard's entity grammar, in the standard's order, from the schema rather
than from memory:

    sub-<label>[_ses-<label>][_task-<label>][_acq-<label>][_ce-<label>]
    [_rec-<label>][_dir-<label>][_run-<index>][_echo-<index>][_flip-<index>]
    [_inv-<index>][_mt-<label>][_part-<label>]_<suffix>.<ext>

The mapping is declared in the pack and every entry below is measured against
the live archive (§3.4, §6):

| ours | BIDS |
|---|---|
| `base` T1w, T2w, PDw, T2\*w, FLAIR | the suffix, `T2starw` for the third |
| `construct` Magnitude, Phase, Real, Imag | `part-mag`, `part-phase`, `part-real`, `part-imag` |
| `construct` ADC, Trace, FA, colFA, expADC | the `dwi` scanner-derivative suffixes, which BIDS names exactly |
| `construct` INV1, INV2 with technique MP2RAGE | suffix `MP2RAGE`, `inv-1`, `inv-2` |
| `construct` Uniform with technique MP2RAGE | suffix `UNIT1` |
| `construct` T1map, T2map, PDmap, R1map, QSM | the `anat` parametric suffixes, `Chimap` for QSM |
| multi-echo GRE, multi-echo SE | suffix `MEGRE` / `MESE`, `echo-` required |
| `post_contrast` | `ce-` |
| `technique`, `modifier`, remaining `construct` | `acq-`, under a declared vocabulary |

The entity rules are enforced from the schema, not approximated: `part` takes
only `mag|phase|real|imag`, `mt` only `on|off`, `echo`/`flip`/`inv`/`run` are
indices, and a suffix that requires an entity does not get written without it.
`MEGRE` requires `echo`, which turns v0's bug 2 from a cosmetic complaint into a
validator error, which is the right place for it to be caught.

### 5.3 Where the rest goes

- **`sourcedata/`**, BIDS's own answer for source files, in a BIDS-shaped tree:
  the working scans that are not images, and the scouts. They are kept as DICOM,
  one directory per stack, which is what a SyMRI reader wants anyway.
- **`derivatives/nils/`**, a dataset in its own right with its own
  `dataset_description.json`: the reformats, projections and SWI images. Real
  data, honestly labelled as derived, and the tree stays valid.
- **Nowhere, with a reason.** A stack with no BIDS name and no derivative home
  is reported per subject and session, never silently dropped. The 1,714 of
  §3.4 are mostly a question waiting for an answer (§3.6), not a loss.

## 6. Selection, roles and picks

One selection, one release (§3.3). A selection is a predicate over the registry:
subjects, cohorts, sessions, stack ids, axes, or any combination.

Within it, a **role** is a named predicate over the decided axes and the
fingerprint, and a **pick** chooses one stack per session and role by an ordered
preference, reporting ties rather than breaking them by row order. Both are
pack-shaped data.

The number that makes this mandatory: **82.5 percent of the archive's sessions
that hold a T1w hold more than one, and the worst holds 462.** An export without
picks hands a BIDS App a directory and no answer, which is D9's complaint.

Wave 3 ships the mechanism and the default role set that C8 calls the main
acquisition. The catalog objects and the query language's `pick` clause are
Wave 4 and 5 (C19); what is built here is the computation, so both read one
thing.

## 7. De-identification

### 7.1 Identifiers

The registry already holds the pseudonym, so the release does not choose one.
Direct identifiers are replaced by the subject's code under the registry's
declared scheme (C36), and v0's five strategies do not survive the inversion of
§2.

### 7.2 UIDs

Remapped, keyed, deterministically: the same UID gives the same new UID for
ever, so two releases of overlapping selections agree and an increment lands in
the same tree. Nothing downstream needs the original, because the join is the
registry's id (§2). `preserve_uids` is a real policy, **defaults to off**, and a
release that turns it on says so in its report and in the dataset description,
because a tree carrying the source PACS's UIDs is linkable to the source PACS.

### 7.3 Private tags, overlays, burned-in pixels

Private tags are dropped by default with an allowlist by
`(creator, group, element)`, declared as pack-shaped data because which vendor
private tag carries a diffusion direction is knowledge that changes without the
engine changing. Overlay and curve groups are dropped. A stack whose
`BurnedInAnnotation` says yes, or whose image type carries a token we know means
a screenshot, raises a review item and is not written until it is answered;
where the tag is absent the release says how many stacks it could not judge,
because absence is not evidence. The engine does not look at pixels (§10).

### 7.4 Dates

**The registry is never rewritten.** This is settled and §9 is why.

A release declares one policy: `keep`, `shift` (one offset per subject, drawn
once, uniformly within +/- 180 days, held in `date_shift`, so every interval
survives and the whole clinical layer joins as before), or `year`. Age at study
is computed before anything is applied.

Under `shift` or `year`, a release refuses to write a session label that is a
date, because a date policy a directory name defeats is not a policy.

### 7.5 The audit

Rows, not a workbook, and not beside the originals: `release` (one per run, with
every policy), `release_file` (one per file written, with its instance and
digest) and `release_change` (`release, tag, action, count`).

There is deliberately no old-value column. An audit that records what was
removed is a copy of the identifiers, in the registry, in clear. What a release
removed is recoverable from the originals by someone entitled to read them.

## 8. The four fingerprint fields

Wave 2 §7.3 ruled these are fingerprint work rather than passes, and this is the
wave that does it: field strength normalised, acquisition type inferred, DWI
enrichment, and the session rescue as a fact about a study. Each is computed
from what was measured and stored **beside** the measured column, never over it,
which is the fault Wave 2 measured in v0.

## 9. The session, and why the date stays

v0's `timeline/` is richer than Wave 1 §4.4 described, and Wave 3 carries all of
it: four anchor kinds (`first_session`, `onset_event`, `diagnosis_event`,
`explicit_per_subject`), a cadence with a float tolerance, four collision
policies with **merge** as the argued default, and a policy for a session that
fits no schedule. A session before its anchor is `PRE06`, never `M-06`, because
a hyphen is BIDS's key-value separator.

The label is therefore a lossy, policy-dependent rendering. `M12` does not
identify a session: two sessions share it under the default policy, an
off-schedule visit keeps its real month, and under a diagnosis anchor 9.9
percent of live sessions precede their anchor with label order running backwards
against date order for a quarter of those subjects.

The date is a different thing. The archive holds **139,033 clinical events over
fifteen observation types spanning 1953 to 2026** (30,664 scans, 17,792 EDSS,
15,101 SDMT, 10,048 treatments, 2,981 onsets, 2,695 diagnoses); the clinical
import matches on `(subject_id, event_date)`; the session anchors above **are**
event dates; and age at scan, disease duration and "the EDSS nearest this scan"
are all date arithmetic. **The date is the join key of the clinical layer and
the label is a presentation of it.** So the registry keeps it, a policy governs
only what leaves, and BIDS carries the time where BIDS puts it, in
`_sessions.tsv` and `_scans.tsv`, which is what frees the directory name.

## 10. What Wave 3 does not do

- **Defacing, and every other change to pixels.** A pipeline, not a property of
  the registry. v0 already has the seam: `analysis_pipeline/descriptor.py` reads
  a Boutiques-subset `nils.job.yml` with an `x-nils` block declaring a container
  image, BIDS-Apps analysis levels, a work unit and a derivatives ingest
  entrypoint. A defacer is a BIDS App over the tree this wave produces, and its
  output is a registered derivative. NILS holds the registry and produces the
  dataset.
- **Derivative registration.** The seam is declared here; the machinery is
  Wave 7's with the runner. v0's `ingest_derivatives` is a planning surface with
  its DB write gated off, so there is nothing to carry, only to build.
- **The full catalog of roles and picks** (Wave 4 and 5, C19).
- **The migration of the live registry** (Wave 4).

## 11. The gate

The oracle is not v0 (D16): its export is not valid BIDS, so byte-identity
against it would be a bar against being correct.

1. **The validator passes** on the reference selections, no warnings suppressed.
2. **The reference selections are right**: hand-verified, in the pack's corpus
   the way Wave 2's cases are, each naming the session, the role, the pick, the
   disposition and the resulting filename in both layouts.
3. **Every stack is placed.** Every stack in a selection is in the raw tree, in
   `sourcedata`, in `derivatives`, or in the report with a reason. The counts
   reconcile to the selection's size.
4. **The descriptive layout names everything.** No stack in a selection is
   unnameable under §5.1.
5. **One stack per session and role**, ties reported.
6. **Every file is traceable** through `release_file` to an instance.
7. **The de-identification does what it says**: no tag from the removed set, no
   private tag outside the allowlist, no overlay group, no UID that appears in
   the source, and under `shift` no date that appears in the source.
8. **Round trip and increment**: two runs over one selection agree; a run over a
   superset leaves the first run's files untouched.
9. **The clinical join survives**: for the reference selections, "the EDSS
   nearest each scan" is the same computed from the registry and from the tree,
   under every date policy. §9 as a test.
10. **The budget**, measured on the baseline host and gated in CI.

## 12. Order of work

1. The session scheme and `nils session` (§9).
2. Migration 5: the release tables, the fingerprint columns.
3. The four fingerprint fields (§8).
4. The disposition, in the pack, with its corpus cases (§4).
5. Roles and picks (§6).
6. `nils release`: selection, identifiers, UIDs (§7.1, §7.2).
7. Dates, private tags, overlays, burned-in, the audit (§7.3 to §7.5).
8. The descriptive layout (§5.1).
9. The BIDS layout, the dataset files, `sourcedata` and `derivatives` (§5.2,
   §5.3).
10. The gate (§11).

## 13. Open questions carried into the wave

1. **The UID root** for keyed remapping: a registered OID arc, which is the
   right answer for a tool meant to be adopted, or a UUID-derived root, which is
   legal and ugly.
2. **Where localizers go.** 116,318 stacks, 22 percent of the archive, and BIDS
   has no word for them. `sourcedata/` is this spec's answer; dropping them from
   a release is also defensible and cheaper. It is a per-release choice and the
   default is the question.
3. **Whether synthetic contrasts belong in raw `anat/`.** The qMRI appendix
   permits vendor pre-generated maps there; a purist would put every synthetic
   image in `derivatives/`. 2,543 stacks turn on it.
4. **Who answers the task question** for functional data (§3.6): a decision per
   study, per origin, or a release argument.
5. **The private-tag allowlist seed**, answerable from the corpus.
6. **The default date policy per registry.** `keep` is right for KI today (§9).
