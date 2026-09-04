<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Wave 3: anonymize and BIDS

The specification of the third wave of the NILS rewrite. It follows
`wave2-fingerprint-and-classify.md`, which built the classification this wave
selects from, and the design record it cites by id (`docs/decisions/`, D13, D16,
D17, C8, C19, C35, C36). Every section written before the work says what will be
built; the blocks headed "Settled while building" are amended in as it lands, the
same way Waves 1 and 2 were written.

This wave was specified after reading v0's `anonymize/`, `bids/`, `timeline/` and
`analysis_pipeline/` packages end to end, and after measuring the live registry.
Section 2 is what that reading found, because two of this wave's decisions only
make sense once you know what v0 actually does.

## 1. What Wave 3 delivers

The same binary, `nils`, gains

1. **`nils session`**: the session scheme made real. A session is not a row and
   not a date; it is the output of a declared scheme over a subject's studies,
   with an anchor that can be a clinical event, a cadence with a tolerance, and
   a stated answer for two sessions that want one label and for a session that
   fits no schedule.
2. **`nils anonymize`**: a resumable job that writes a de-identified copy of a
   selection's DICOM, with keyed UID remapping, a declared date policy, a
   private-tag policy, a burned-in check that raises a question rather than
   guessing, and an audit that is a registry table rather than a spreadsheet
   next to the originals (D13).
3. **roles and picks**, the smallest version that the export needs: one stack
   per session and role, chosen by a declared ordered preference, ties reported
   (C8, C19). 82.5 percent of the live registry's sessions that have a T1w have
   more than one, and one session has 462, so an export without picks hands a
   BIDS App a directory and no answer.
4. **`nils bids`**: a validator-clean BIDS dataset. Filenames are the standard's
   entity grammar, not a rendering of our classification; the dataset carries
   the files BIDS requires; the acquisition times live where BIDS puts them; and
   everything our vocabulary can say but BIDS cannot goes under `derivatives/`
   with its own description rather than being bent into an invalid name.
5. the four **fingerprint fields** Wave 2 ruled were not passes: field strength,
   acquisition type, DWI enrichment and the session rescue (Wave 2 §7.3).

And the four things it deliberately does not do are in §12, with reasons.

## 2. What reading v0 found

v0's export is not a BIDS dataset with bad filenames. It is not a BIDS dataset.
And its anonymizer keeps the two identifiers that matter most, for a reason that
turns out to be a coupling this wave can break.

### 2.1 The anonymizer removes tags and nothing else

`anonymize/scrub.py` deletes the tags its configured categories name and rewrites
`PatientID` through a strategy. That is the whole operation. Specifically:

- **Every UID survives.** `_scrub_dataset` skips any element whose VR is `UI`,
  and any tag whose name contains `uid` or contains both `referenc` and
  `sequence`. StudyInstanceUID, SeriesInstanceUID, SOPInstanceUID and
  FrameOfReferenceUID are therefore never touched, and neither are referenced
  sequences, which can carry identifiers of their own.
- **`preserve_uids` does not preserve UIDs.** The setting exists, defaults to
  true, and reaches exactly one line: `ds.save_as(..., enforce_file_format=not
  opts.preserve_uids)`. Setting it false remaps nothing. A site that turned it
  off believing it was de-identifying UIDs would be wrong, and nothing in the
  output would say so.
- **StudyDate is never rewritten**, deliberately, and the code says why: it is
  the session key the sort stage and the BIDS exporter build sessions from.
- There is **no private-tag policy, no overlay or curve removal and no burned-in
  check**. The words do not appear in the package.

### 2.2 The BIDS export is not BIDS

`bids/` writes no `dataset_description.json`, no `participants.tsv`, no `README`
and no `_scans.tsv`, and never runs the validator. Those are not niceties; a
directory without `dataset_description.json` is not a dataset, whatever the
filenames say. It does pass `-b y` to `dcm2niix`, so per-image sidecar JSON
exists.

The filenames are built by `classification.stack_naming` and handed to `dcm2niix`
as `-f`, which is where v0's own bug list starts. That list has three entries;
there are four:

| | what | where |
|---|---|---|
| 1 | `T2*w` reaches the CLI verbatim and `dcm2niix` writes `T2_w` | `docs/known-bugs-bids-export.md` |
| 2 | echo identity is lost, because this vendor splits echoes across series so `stack_key` is null and the echo suffix is suppressed | same |
| 3 | a stale cohort source path degrades the export silently, 96 of 173 stacks dropped, empty subject directories left behind | same |
| 4 | the echo suffix disappears when a single echo of a two-echo series is exported, because the collision counter counts stacks per series over the **already filtered** list | a docstring in `bids/query.py`, not in the bug list |

All four are the same mistake in different clothes: a name derived from what we
concluded about a stack, disambiguated by a counter over whatever happened to be
selected. BIDS solves this by having an entity grammar with `echo-`, `part-`,
`acq-` and `run-`, where the identity is in the entity and the counter is only
ever the last resort.

### 2.3 The session label is not a join key, and the date is

This is the finding that decides §4 and §5, and it came from Nima rather than
from the code.

v0's `timeline/` package resolves a session label from a stored scheme, and the
scheme is richer than Wave 1 §4.4 assumed. The anchor is one of `first_session`,
`onset_event`, `diagnosis_event` or `explicit_per_subject`; there is a cadence
with a float tolerance; two sessions that want one label **merge** by default,
with three opt-in policies for cohorts that need one session per label; a session
that fits no schedule keeps its real month; a session before its anchor is
`PRE06`, never `M-06`, because a hyphen is BIDS's key-value separator. Under a
diagnosis anchor, 9.9 percent of live sessions are pre-anchor, and for a quarter
of those subjects label order runs backwards against date order.

So a label is a lossy, policy-dependent rendering. `M12` does not identify a
session, two sessions can share it, and whether a scan is `M12` or `M13` depends
on a tolerance someone typed. Meanwhile the live registry holds **139,033
clinical events over fifteen observation types**, and they are dated, not
labelled:

| observation | events | subjects | span |
|---|---|---|---|
| MRI scan | 30,664 | 5,319 | 2001 to 2026 |
| EDSS | 17,792 | 3,844 | 1993 to 2024 |
| SDMT | 15,101 | 2,773 | 2004 to 2024 |
| treatment | 10,048 | 3,813 | 1962 to 2024 |
| disease onset | 2,981 | 2,971 | 1953 to 2020 |
| diagnosis | 2,695 | 2,695 | 1971 to 2020 |

The clinical import matches on `(subject_id, event_date)`. The anchors a session
scheme can use are event dates. Age at scan, disease duration at scan and
"the EDSS nearest this scan" are all date arithmetic. **The date is the join key
of the whole clinical layer**, and the session label is a presentation of it.

Two consequences, and they are this wave's spine:

- The registry holds true dates, as classed clinical fields (C35). Nothing in
  this wave removes or rewrites a date **in the registry**.
- A session label never substitutes for a date anywhere a join happens. Where
  BIDS needs a date, BIDS has a slot for one, and §5.4 uses it.

### 2.4 Pipelines are where pixels change

v0 already has the seam: `analysis_pipeline/descriptor.py` reads a
Boutiques-subset `nils.job.yml` with an `x-nils` block, declaring a container
image, BIDS-Apps analysis levels (`run`, `session`, `subject`, `dataset`,
`meta`), a work unit the runner slices over (`stack`, `session`, `subject`,
`group`), runtime needs and an ingest entrypoint for derivatives.

That is where defacing belongs, and normalisation, and every other transform of
the pixels. NILS holds the registry and produces the tree; a pipeline consumes
the tree and produces derivatives that are registered back. Wave 3 therefore
owns no pixel transform at all (§12).

## 3. Words

- **Study**: DICOM's unit, one StudyInstanceUID, a row because it is a fact in
  a file. Unchanged from Wave 1.
- **Session**: the occasion a subject came in, the output of a **session
  scheme** over their studies. Never a row (Wave 1 §4.4).
- **Session label**: what a scheme calls a session, for people and for
  directories. Lossy on purpose; never a key.
- **Role**: a predicate over the decided axes and the fingerprint that names a
  kind of acquisition a study cares about ("the 3D T1w", "the FLAIR").
- **Pick**: the one stack a role gets in a session, chosen by an ordered
  preference, with ties reported rather than broken silently.
- **Release**: one run of `nils anonymize` or `nils bids` over a selection, with
  its policy recorded, so the tree on disk can be traced back to what produced
  it.
- **Policy**: what a release does to identifiers and to dates. Declared per
  release, recorded with it, and printed in the report. Never a default that
  goes unstated.

## 4. Anonymization

### 4.1 It is a copy, and a job

`nils anonymize` reads a selection, writes a de-identified copy to a destination
root and writes what it did to the registry. It never edits in place, never
writes into the directory that holds the originals (D13), and is resumable the
way every job in v1 is: its input is a predicate over the registry, its progress
is rows, and a killed run resumes without re-reading what it finished
(Wave 1 §9).

### 4.2 Identifiers

- **PatientID and every other direct identifier** go through the registry's
  pseudonym, which already exists: the subject's code under the registry's
  declared scheme (C36, `blake2b-8` with v0's key for the KI registry, so an
  existing subject keeps the code collaborators already hold). The anonymizer
  does not have an ID-strategy family of its own; v0's five strategies were
  five answers to a question the registry now answers once.
- **UIDs are remapped**, keyed, deterministically: a UID maps to a new UID under
  the registry's key, the same input giving the same output for ever, so two
  releases of overlapping selections agree and a later increment lands in the
  same tree. The map is derivable from the key rather than stored per UID. The
  root is NILS's own registered prefix (§13, open). `preserve_uids` exists as a
  named policy and **defaults to off**; a release that turns it on says so in
  its report and in the tree's description, because a tree with the source
  PACS's UIDs is linkable to the source PACS.
- **Private tags**: dropped by default, with an allowlist by
  `(creator, group, element)`. The allowlist is a pack-shaped file, not code,
  because which vendor private tags carry diffusion directions is knowledge that
  changes without the engine changing. Everything dropped is counted by creator
  in the report.
- **Overlays and curves** (groups `0x60xx` and `0x50xx`) are dropped. They are a
  documented burned-in-annotation surface and nothing in our pipeline reads
  them.

### 4.3 Dates

The policy is declared per release and the registry is never rewritten (§2.3).
Three policies, and a release names one:

- `keep`: dates are copied as they are. The honest default for an internal
  release that stays inside the group, and the only policy under which a tree
  can be joined to an external clinical source keyed by true date.
- `shift`: every date and datetime in the copy moves by one offset per subject,
  drawn once, uniformly within +/- 180 days, stored in `date_shift`
  (`subject_id`, `offset_days`; the table Wave 1 created for this and left
  empty). One offset per subject means **every interval survives**: the EDSS
  three days before the scan is still three days before the scan, the disease
  duration is unchanged, and the whole clinical layer joins as before, because
  the query applies the same offset. What does not survive is a join to a source
  outside NILS that holds true dates, which is exactly the risk this policy
  trades for.
- `year`: dates are truncated to the year. For a release where even an interval
  is more than a recipient needs.

Under every policy, **age at study is computed before anything is applied** and
is available as a field, because age is the thing most analyses actually wanted
from the birth date.

A release under `shift` or `year` refuses to write a session label that is a
date, because a date policy that a directory name defeats is not a policy. This
is the one place the two halves of the wave constrain each other, and it is
checked rather than documented.

### 4.4 Burned-in pixels

A stack whose `BurnedInAnnotation` is `YES`, or whose image type carries a token
we know means a screenshot or a report, raises a **review item** and is not
written until someone answers it. Where the tag is absent, which is most of the
time, absence is not evidence and the release says how many stacks it could not
judge. The engine does not look at pixels: that is a pipeline's job (§2.4), and
a pipeline that classifies burned-in text can write its verdict back as a
decision that this step then reads.

### 4.5 The audit

v0 writes a workbook into the folder that holds the identifiable originals,
protected by a hard-coded default password (D13). v1 writes rows.

- `release`: one row per run. The selection, the policies of §4.2 to §4.4, the
  destination root, the pack and engine versions, the actor, the job.
- `release_file`: one row per file written. Its instance, its path in the
  release relative to the release root, and the digest of what was written.
- `release_change`: one row per **kind** of change, not per change. `(release,
  tag, action, count)`. v0 writes one audit event per tag per file, which on
  37.5 million instances is a table nobody can read and a cost nobody wants; the
  question people actually ask is "what did this release do to
  PatientName", and that is a count.

The old-value column is the one thing that must not exist: an audit that records
what was removed is a copy of the identifiers, in the registry, in clear. What a
release removed is recoverable from the originals, which are still there, by
someone who has the right to read them.

## 5. BIDS

### 5.1 Roles and picks, the smallest version

A role is a named predicate over the decided axes and the fingerprint. A pick is
an ordered preference list that chooses one stack per session and role, and a tie
is reported, never broken by row order. Both are **pack-shaped data** in
`packs/mri/roles/`, not engine code, for the reason every other piece of
knowledge is (Wave 2 §5).

Wave 3 ships the default role set that C8 calls the main acquisition, and the
mechanism that computes it. The catalog objects, the saved per-study role sets
and the query language's `pick` clause are Wave 4 and 5 (C19); what is built here
is the computation and its storage, so that both this wave's export and Wave 4's
catalog read one thing.

The gate for this section is the number in §1: with picks, a session yields one
T1w; without them it yields, in the worst live case, 462.

### 5.2 Names are BIDS entities

The name of a file is built from the standard's grammar, in the standard's
order, and never from a rendering of our axes:

    sub-<label>[_ses-<label>][_task-<label>][_acq-<label>][_ce-<label>]
    [_rec-<label>][_dir-<label>][_run-<index>][_echo-<index>][_flip-<index>]
    [_inv-<index>][_mt-<label>][_part-<label>]_<suffix>.<ext>

Our axes map onto it. `base` becomes the **suffix** (`T1w`, `T2w`, `FLAIR`,
`T2starw`, `dwi`, `bold`), which is where v0's `T2*w` problem disappears: the
BIDS suffix for that contrast is spelled `T2starw` by the standard, so there is
nothing to sanitise. `technique`, `modifier` and `construct` become an `acq-`
label under a declared vocabulary. `post_contrast` becomes `ce-`. Echo number
becomes `echo-`, and it comes from the fingerprint's echo number, **not** from
`stack_key`, which is what makes bug 2 structural rather than incidental: the
vendor that splits echoes across series still has an echo number in every file.
Magnitude and phase become `part-mag` and `part-phase` from the image-type
tokens. `run-` is the last resort and is assigned over the **whole session as it
exists in the registry**, never over the filtered selection, which is bug 4.

A stack the grammar cannot express is not bent to fit. It goes to §5.5.

### 5.3 The dataset, not just the files

A release writes what BIDS requires and what makes the tree usable:

- `dataset_description.json`, with `Name`, `BIDSVersion`, `DatasetType`,
  `GeneratedBy` naming NILS and its version, and `SourceDatasets` naming the
  release of §4.5 that produced the DICOM it converted.
- `participants.tsv` and its sidecar, carrying the subject code and the fields a
  release's policy allows, which is where sensitivity classes are enforced.
- `README` and `.bidsignore`.
- `sub-<label>_sessions.tsv` per subject, and `sub-<label>_ses-<label>_scans.tsv`
  per session.

### 5.4 Where the date goes

`_sessions.tsv` has an `acq_time` column and `_scans.tsv` has one per file. That
is BIDS's own answer to §2.3, and it is the answer this wave takes: the
directory is named by the session scheme, the time is carried in the standard's
slot, under the release's date policy, and anything that needs to join on a date
reads the column rather than parsing the directory name.

This is the coupling of §2.1 broken. v0 keeps StudyDate because the exporter
needs it in the directory name; v1's exporter reads the session from the scheme
and the date from a column, so the date policy is free.

### 5.5 What BIDS has no word for

Our vocabulary says things BIDS does not: SyMRI-derived quantitative maps, EPIMix
contrasts, projection-derived outputs, ADC and trace and FA maps that are
reconstructions rather than acquisitions. Two rules:

- Anything the raw grammar can express, in the raw tree.
- Everything else in `derivatives/nils/`, which is a dataset in its own right
  with its own `dataset_description.json` naming NILS as `GeneratedBy`, so the
  tree stays valid and the data stays present. A derived map is a derivative;
  putting it in `anat/` is what makes a tree invalid, and dropping it is what
  makes an export useless.

Nothing is silently omitted. A release reports, per subject and session, what it
wrote to the raw tree, what it wrote to derivatives, and what it wrote nowhere
and why.

### 5.6 Conversion

`dcm2niix` is invoked per pick with an explicit file list, as v0 does, and the
name it is told to write is already the final BIDS name. Three things v0 lacked:

- The source of every file comes from the **registry**, which since Wave 1
  records the path of every instance relative to its batch root (D17). There is
  no cohort root to go stale, which is bug 3 removed rather than worked around.
- Directories are created only after a conversion succeeds, so a failure leaves
  no empty `sub-*/ses-*/anat/` to be mistaken for a selection bug.
- A release preflights: the roots it will read, the binary it will call and its
  version, and the free space it needs, all checked before the first file, and
  reported as one refusal rather than as N identical failures.

The sidecar JSON `dcm2niix` writes is merged with what the registry knows, not
replaced by it, and every field we add is one BIDS names.

## 6. The four fingerprint fields

Wave 2 §7.3 ruled that these are fingerprint work rather than passes, and this is
the wave that does it. Each is computed from what was measured, stored beside the
stack, and never written back over a measured column, which is the fault v0 has
and Wave 2 measured (§7.3, the acquisition-type write-back).

- **Field strength**, normalised to the standard values, as a derived column
  beside the raw one rather than in place of it.
- **Acquisition type**, inferred where DICOM did not say, in its own column, so
  a rule that reads it can say whether it read a measurement or an inference.
- **DWI enrichment**: b value, phase-encode direction and direction count, from
  the vendor private tags the reader already handles.
- **Session rescue**: whether a study has any primary stack, a fact about the
  study rather than a phase that depends on which stacks are in a batch.

They are additive columns and a migration. Nothing in Wave 2's classification
changes meaning; the rules that want them are a later pack version.

## 7. The shape of the code

- `nils-release` (new crate): the anonymizer, the audit tables, the policies.
- `nils-bids` (new crate): roles and picks, the entity grammar, the dataset
  files, the conversion.
- `nils-classify`: gains the four fingerprint fields (§6).
- `nils-registry`: migration 5, the release tables and the fingerprint columns.
- `packs/mri/roles/`: the default role set.
- `nils`: `nils session`, `nils anonymize`, `nils bids`.

No BIDS name, no role name and no tag number appears in `nils-registry`. The
seam Wave 2 proved for modality holds here: a pack declares roles, the engine
computes picks.

## 8. Evidence and review

A pick is a decision with evidence: which role, which preference fired, what the
alternatives were, and whether it was a tie. A tie is a review item. So is a
burned-in candidate (§4.4), and so is a session whose label the scheme flagged
(`no_anchor`, `pre_anchor`, `demoted`, `collision`, from v0's own vocabulary,
which is a good one).

The queue discipline of Wave 2 §8.2 applies unchanged: a question is asked when a
person could answer it and the answer would change something. v0 flags every
stack it touches; this wave will be measured the same way Wave 2 was, and the
number goes in the report.

## 9. Knobs and the report

Every policy of §4 and §5 is a knob the engine exposes, with `describe` and
`diagnose` over it (C37), so the release a person or an agent asks for can be
inspected before it runs. The report of a release says: what was selected, what
each policy was, what was written where, what was refused and why, what was
picked and what tied, and the counts of §4.5. A release whose report a reader
cannot reconstruct the tree from is a bug.

## 10. The gate

The oracle is not v0 (D16). v0's export is not valid BIDS, so byte-identity
against it is not a bar and would be a bar against being correct.

1. **The validator passes.** `bids-validator` clean, no warnings suppressed, on
   the reference selections.
2. **The reference selections are right.** Hand-verified selections, in the
   pack's corpus the way Wave 2's cases are, each naming the session, the role,
   the pick and the resulting filename. A change that renames a file fails here
   before anyone opens the tree.
3. **One stack per session and role**, with every tie reported.
4. **Every file is traceable**: every file in the tree joins back through
   `release_file` to an instance in the registry, and every instance the
   selection named is either in the tree or has a reason.
5. **The de-identification does what it says.** A scan of the written tree finds
   no tag from the removed set, no private tag outside the allowlist, no overlay
   group, and no UID that appears in the source, and under `shift` no date that
   appears in the source.
6. **Round trip.** Two runs of a release over the same selection produce the
   same tree, byte for byte where the converter is deterministic and
   name-for-name everywhere.
7. **Idempotent increment.** A release over a selection, then over a superset,
   leaves the first release's files untouched and adds the rest.
8. **The clinical join survives.** For the reference selections, "the EDSS
   nearest each scan" gives the same answer computed from the registry and
   computed from the tree, under every date policy. This is §2.3 as a test.
9. **The budget.** The small-machine numbers of D6, measured on the baseline
   host and gated in CI as the other waves are.

## 11. Order of work

1. The session scheme and `nils session` (§2.3), including v0's anchors,
   cadence, tolerance and collision policies, which Wave 1 §4.4 under-specified.
2. Migration 5, the release tables and the fingerprint columns.
3. The four fingerprint fields (§6).
4. Roles and picks, and the default role set (§5.1).
5. `nils anonymize`: identifiers and UID remapping (§4.2).
6. Dates, private tags, overlays, burned-in (§4.3, §4.4), and the audit (§4.5).
7. The entity grammar and the dataset files (§5.2, §5.3, §5.4).
8. Conversion and derivatives (§5.5, §5.6).
9. The gate (§10), and the reference selections as corpus cases.

## 12. What Wave 3 does not do

Written down so each is a decision rather than an omission.

- **Defacing, and every other change to pixels.** A pipeline, not a property of
  the registry (§2.4). It plugs in at the descriptor seam v0 already has and
  Wave 7 hardens: a BIDS App over the tree this wave produces, whose output is a
  registered derivative. NILS holds the database and produces the dataset.
- **Derivative registration.** The seam is declared here, the machinery is
  Wave 7's with the runner.
- **The full catalog of roles and picks.** Wave 3 computes them; Wave 4 and 5
  make them catalog objects with saved per-study sets (C19).
- **The migration of the live registry.** Wave 4's, with this wave's release
  tables as one of its targets.

## 13. Open questions carried into the wave

1. **The UID root.** Keyed remapping needs a prefix. Registering an OID arc for
   NILS is a form and a small fee and it is the right answer for a tool meant to
   be adopted; the alternative is a UUID-derived root, which is legal and ugly.
   The decision is Nima's and it blocks nothing until §4.2 lands.
2. **Which private tags the allowlist starts with.** Answerable from the corpus:
   the vendor tags our own reader already reads for DWI enrichment are the first
   entries, and the rest is measurement.
3. **Whether a release may be deleted.** A release is a copy on disk and a set
   of rows; deleting the copy is a filesystem operation, but the rows are the
   record that it existed. The default is that rows are never deleted and a
   release can be marked withdrawn.
4. **The default date policy per registry.** `keep` is right for the KI registry
   today, because the clinical layer and its imports join on true dates (§2.3).
   Whether an outbound release should default to `shift` is a question about
   what the group promises collaborators, not a technical one.
