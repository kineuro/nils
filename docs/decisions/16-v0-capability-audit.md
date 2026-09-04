# 16 — v0 capability audit

Written 2026-09-04, after Nima observed that the Wave 3 draft was built on a
partial reading: "you check some file and forget the rest ... did we cover all
functionality of the sort step?"

The answer to that question is **no**, and this document exists so the question
does not have to be asked again. It walks every package of v0's engine backend
and records, per capability, where it lives, what it does, and what v1 has.

Method: read the code, not the documentation; verify claims against the live
system where a claim is checkable; record the evidence. Counts come from the
live metadata database and the application database, read-only.

Status vocabulary:

- **done** — built in v1 and gated.
- **partial** — built, with a named thing missing.
- **missing** — a real gap. Nothing in v1 does this.
- **dropped** — deliberately not carried, with the reason.
- **planned** — assigned to a named later wave.

## 0. The shape of v0

| package | lines | files |
|---|---|---|
| qc | 14,327 | 51 |
| classification | 9,957 | 27 |
| api | 8,053 | 49 |
| sort | 7,666 | 19 |
| metadata_imports | 5,807 | 13 |
| extract | 4,990 | 23 |
| analysis_pipeline | 3,822 | 12 |
| anonymize | 3,153 | 13 |
| metadata_db | 2,149 | 11 |
| stages | 1,943 | 7 |
| bids | 1,913 | 9 |
| cli | 1,904 | 5 |
| timeline | 1,420 | 6 |
| cohorts | 1,080 | 7 |
| runtime | 842 | 7 |
| backup | 731 | 2 |
| jobs | 682 | 7 |
| db | 506 | 6 |
| compress | 390 | 3 |

The pipeline, from `stages/ordering.py`:

    anonymize -> extract -> sort (checkup, stack_fingerprint, classification, completion) -> bids

**Anonymization runs first, over a directory tree, before a tag reaches a
database.** The on-disk convention (`cohorts/paths.py`) is
`<cohort>/derivatives/dcm-original` for the identifiable source and
`dcm-raw` for the anonymized copy; extract, the exports and the analysis
pipelines all read `dcm-raw`. So in v0 the sensitive artefact is a folder, and
everything downstream works from a de-identified copy.

v1 inverts this: digest reads the original, identity is resolved and
pseudonymised at ingest behind a key, and de-identification is an export. The
sensitive artefact becomes the registry. That is the single largest structural
difference and most of the entries below trace back to it.

## 1. `anonymize`

| capability | where | what it does | v1 |
|---|---|---|---|
| tag scrub by category | `tags.py`, `scrub.py` | deletes the tags of the named categories; which categories a deployment enables is not part of this copy | planned, Wave 3 |
| PatientID rewrite, 5 strategies | `pid_strategies.py` | sequential, **folder**, deterministic, CSV, CSV with hash fallback | **partial**, see §1.1 |
| patient folder rename | `pid_strategies.py` | substring-replaces an old id out of directory names at every depth | n/a: v1 writes a fresh tree |
| leaf resumability | `partition.py` | the unit is the StudyInstanceUID; `study_audit_exists(leaf)` gates rework | done differently (jobs, Wave 1) |
| audit | `audit.py`, `store.py` | two tables and a spreadsheet export; how that export is protected is not part of this copy | planned, Wave 3, as tables |
| UID handling | `scrub.py` | **never remaps**: skips VR `UI` and any tag whose name holds `uid`, or `referenc` + `sequence` | planned, Wave 3, see §1.2 |
| `preserve_uids` | `config.py`, `scrub.py` | **dead knob**: reaches one line, `save_as(enforce_file_format=not preserve_uids)`, and remaps nothing | Wave 3 makes it real |
| date handling | `scrub.py` | StudyDate retained on purpose, with a comment saying the exporter needs it as the session key | Wave 3, policy per release |
| `study_dates` config block | app DB configs | **dead**: `snap_to_six_months`, `minimum_offset_months`; the `_compute_timepoint` it drove moved to `timeline/` | dropped |
| private tags, overlays, curves, burned-in | nowhere | not handled; the words do not occur in the package | planned, Wave 3 |

### 1.1 The identity chain, which is two packages and not one

This is the entry the Wave 3 draft got wrong. The five strategies are not an
artefact of ordering; they are **five sources of subject identity for data whose
DICOM does not carry one**.

Evidence from the application database: every cohort that ran anonymization
is configured with

    patient_id: {strategy: folder, folder: {strategy: depth, depth_after_root: 1,
                 regex: "(.+)", fallback_template: "XXXX"}}

so **the subject is the first folder under the root**, taken whole. Nima
confirms the reason: several MS cohorts have no PatientID at all, every value is
`XXXX`, and some have no date either.

The chain has two stages in two packages:

1. `anonymize` sets `PatientID` from the folder segment (`PathStrategy`).
2. `extract/subject_mapping.py` derives
   `subject_code = blake2b(PatientID or StudyInstanceUID, seed)`, with an
   optional CSV override on PatientID.

So **folder -> PatientID -> subject_code**. Remove stage 1 and stage 2 falls
through to hashing StudyInstanceUID, which makes every study its own subject.

**v1 status: missing.** Verified in code, not inferred: `nils-digest/src/rule.rs`
line 139 rejects a `identity.from[].field` that is not a DICOM keyword, and line
165 permits exactly one fallback, StudyInstanceUID. There is no path source. For
the MS data described above, v1 digests a five-visit person as five subjects and
the longitudinal cohort is destroyed. This blocks digesting that data at all.

Note this is an *import* mechanism, not a de-identification one: the folder
strategy assumes the sender already pseudonymised and put the code in the path.

### 1.2 UIDs and dates are one question, not two

`sort/date_recovery.py` extracts `YYYYMMDD` from DICOM UIDs by regex when every
date field is null, trying study UID, series UID, frame-of-reference UID,
media-storage SOP UID and SOP UID in that order.

So **the UID is a date source**. Two consequences the Wave 3 draft missed by
treating UID remapping and date policy as independent knobs:

- Remapping UIDs destroys the last-resort date for exactly the studies that have
  no date field.
- Keeping UIDs under a date-shift policy leaks the true date through the
  embedded `YYYYMMDD`.

## 2. `extract`

| capability | where | v1 |
|---|---|---|
| tree scan, resume index, process pool, writer pool | `scanner.py`, `resume_index.py`, `process_pool.py`, `writer_pool.py` | done, Wave 1 |
| per-level writers (subject, study, series, stack, instance, event) | `writer_*.py` | done, Wave 1 |
| **an `event` row per study** | `writer_events.py` | keyed by subject, modality and date: v0 derives the occasion here, and again in QC, and again in the anonymizer | done differently: the session scheme (Wave 1 §4.4) |
| subject resolution: CSV then hash | `subject_mapping.py` | **partial**: v1 has the identity rule but no path source (§1.1) |
| stack signature | `stack_utils.py` | done, Wave 1 and 2 |
| duplicate policy | `config.py` | done, Wave 1 |
| CSV import of stack fields into existing instances | `cli/app.py metadata-instance-stack-import` | **missing**, a repair path for incomplete headers |

## 3. `sort`

The direct answer to "did we cover all functionality of the sort step".

| step | capability | Wave 2 |
|---|---|---|
| 1.1, 1.2, 1.4 | cohort subjects, study discovery, series collection | covered by selection |
| **1.3** | **study date validation and repair**: impute from `series_date`, then `acquisition_date`, then `content_date`; **a study with no recoverable date is excluded from sorting entirely**, and if all are, the step fails | **missing** |
| 1.5 | skip already-classified series | done (the stale supersede pass) |
| 2 | stack fingerprint, stack key, instance counts | done, slice 1 |
| 3 | classification | done, and **per-cohort keyword overrides were found only after the wave closed** |
| 4 | the nine completion phases | done, spec §7.3 |
| n/a | `date_recovery.py`, dates from UIDs | **missing** |
| n/a | `gap_filling.py`, the physics vote | done, and its reference settled by measurement (spec §7.4) |
| n/a | `semantic_normalizer.py` | done |
| n/a | `stack_key.py` | done as `split_reason` |
| n/a | `dwi_enrichment.py` | planned, Wave 3 |

Wave 2's §7.3 analysed "the phases around per-stack classification", which is
step 4 only. Step 1 was never read, and the Wave 2 spec contains no occurrence
of checkup, date repair, impute or recover.

## 4. `classification`

Covered by Wave 2 and gated: 518,365 stacks, every axis, against v0's own code.
The one thing found later is the per-cohort keyword override table
(`cohort_classification_overrides`, in the **application** database), which five
cohorts use and which means a stack's classification in v0 is not a function of
v0's code and the stack. Recorded in 11 and in the Wave 2 spec §11.1.

## 5. `timeline`

| capability | where | v1 |
|---|---|---|
| stored session scheme | `scheme.py` | **partial**: Wave 1 §4.4 specified a weaker scheme |
| four anchor kinds: first session, **onset event**, **diagnosis event**, explicit per subject | `scheme.py` | missing: Wave 1 has `first_session` and a Wave 4 note |
| cadence with a **float** tolerance | `scheme.py`, `resolver.py` | missing |
| four collision policies, **merge** the argued default | `scheme.py` | missing |
| unmatched policy | `scheme.py` | missing |
| `PRE06` for a pre-anchor session, never `M-06`, because a hyphen is BIDS's separator | `resolver.py` | missing |

Under a diagnosis anchor, 9.9 percent of live sessions precede their anchor and
for a quarter of those subjects label order runs backwards against date order.

## 6. `qc` — the largest package, and the least covered

Five products, 14,327 lines.

| product | lines | what it is | v1 |
|---|---|---|---|
| `body_part` | 6,672 | an **image classifier** trained per cohort, whose predictions a person commits into `series_classification_cache.body_part` | **missing**; and its output is why the parity gate saw 4,692 differences |
| `classification_qc` | 2,248 | category-based review with QC sessions, items, a **rules engine**, and **draft changes** | partial: v1 has review items and decisions |
| `cohort_main` | 2,032 | the cohort-wide **auto-pick**, scored by `main_qc_weights.yaml`, with a heatmap and border thresholds | **planned, Wave 3, and richer than the draft assumed** (§6.1) |
| `main_acquisition` | 1,030 | MASQC: a person walks a cohort session by session and writes the `main_acquisition` token | planned, Wave 3, as a decision |
| `axes` | 927 | per-axis review of flagged predictions, draft then confirm | partial |

Cross-cutting: the **draft pattern**. Changes are written to the application
database first and pushed to the metadata database only on explicit confirm.
v1's `decision` is a single-step commit. Whether v1 needs a draft state is an
open question (D14 says jobs can produce non-canonical result versions, which is
the same idea in another place).

### 6.1 The pick already exists, as tuned data

`qc/cohort_main/main_qc_weights.yaml` is C19's "picks" in all but name, and its
header says "Edit this file to tune the auto-pick algorithm. No code change
required." It carries component weights (dimension, technique, modifier, slices,
FOV, cohort share, orientation, completeness), a provenance penalty, per-contrast
technique tiers, Dixon and MP2RAGE canonical-construct preferences, border
thresholds that raise a needs-check, a partial-volume auto-demote, and

    non_canonical_constructs: [MIP, MPR, Reformat, Synthetic]

which is a **disposition** in all but name: v0 already knows that a reformat, a
projection and a synthetic image are not candidate acquisitions.

Wave 3 should carry this file into the pack rather than invent a preference
language.

## 7. `bids`

| capability | where | v1 |
|---|---|---|
| two callers, one engine | `runner.py` | dropped: one export (the callers differ only in scope, root and pipeline coupling, and all three differences are gone) |
| output modes DCM / NII / NII.GZ, layouts BIDS / flat, four roots | `config.py` | planned, Wave 3, as two layouts |
| descriptive stack name | `classification/stack_naming.py` | planned, Wave 3, as the `descriptive` layout |
| destination subfolder by intent, with `anat/SyMRI` grouping | `naming.py` | planned |
| session label from the scheme, else the raw date | `query.py` | planned |
| collision numbering `_1`, `_2` over the filtered list | `naming.py` | planned, fixed (bug 4) |
| SyMRI excluded from conversion | `convert.py` | planned, refined: SyMRI is three things (§7.1) |
| dcm2niix with `-s y -b y --terse` and an explicit file list | `convert.py` | planned |
| source path resolution with cohort-root fallbacks | `convert.py` | dropped: the registry holds every instance's path |
| manifest resolver for standalone exports | `resolver.py` | dropped: one selection |
| **no `dataset_description.json`, `participants.tsv`, `README`, `_scans.tsv`; the validator is never run** | | planned, Wave 3 |

### 7.1 The four naming bugs

v0's `docs/known-bugs-bids-export.md` lists three. There are four; the fourth is
in a `bids/query.py` docstring: the echo suffix disappears when a single echo of
a two-echo series is exported, because the collision counter counts stacks per
series over the already filtered list.

## 8. `compress` — an unrecorded capability

`compress/engine.py` packs a de-identified tree into **password-protected 7z
archives in ~100 GB chunks** (`-mhe=on`, so the filenames inside are encrypted
too), with two packing strategies (ordered, first-fit-decreasing), optional PAR2
recovery records, verification and a checksum per archive. Chunks are named
`dataset_chunk0004_pn<first>-<last>.7z` from the folder names they contain.

This is the **handover** mechanism: how a dataset physically leaves. v1 has
nothing for it and no wave owns it. It belongs with Wave 3's release or with
Bifrost.

**Status: missing, and unassigned.**

## 9. `metadata_imports` — the clinical layer

Thirteen importers, 5,807 lines: subjects, cohorts, subject-cohorts,
identifiers, id types, diseases, disease types, subject-diseases,
subject-disease-types, observation types, events. Each is a preview-then-apply
CSV importer with per-field parsers and validation.

They key on `(subject_id, event_date)`, which is why the date is the join key of
the clinical layer (11, and Wave 3 §9). The live archive holds 139,033 events
over fifteen observation types spanning 1953 to 2026.

**Status: planned, Wave 4**, and D13/C35 already say birth dates and the
clinical timeline stay.

## 10. The rest

| package | what it is | v1 |
|---|---|---|
| `stages` | the cohort stage ledger: which stage of which cohort is pending, running, done, with its config, handover and metrics | dropped: digest replaces the cohort pipeline, jobs carry state |
| `jobs` | job records and an inline runner | done, Wave 1 |
| `metadata_db` | schema, seeds, lifecycle, maintenance (indexes, derived counts, bloat), birth-date normalisation, the app-to-metadata cohort bridge | mostly done; **maintenance has no v1 equivalent** |
| `cohorts` | cohort CRUD, stats, the `derivatives/` path convention, keyword overrides | cohort becomes a membership fact; **keyword overrides are a v0 concept v1 replaces with pack overlays** |
| `backup` | pg_dump helpers for both databases | missing, and probably belongs to deployment rather than the engine |
| `api` | 49 files of HTTP routes | Wave 4 |
| `cli` | typer app: metadata init/backup/restore/list/maintenance/ingest, anonymize run, compress run/plan, jobs list, files list | v1's CLI is its own design |
| `runtime`, `db` | logging, executor, shims, sessions | plumbing |
| `analysis_pipeline` | Boutiques-subset descriptors, runner, needs injection, results, **`ingest_derivatives` with its DB write gated off** | planned, Wave 7 |

## 11. What this audit changes

Four things that are not in any wave's plan today:

1. **Identity from the path** (§1.1). Blocks digesting the legacy MS data.
   Wave 1 code, not Wave 3.
2. **Study date repair** (§3), including from UIDs, and the rule that a study
   with no date cannot be sorted. Wave 1 or 2 code, not Wave 3.
3. **The session scheme is under-specified** (§5). Wave 1 §4.4 is weaker than
   v0's `timeline/`.
4. **`compress` is an unowned capability** (§8).

And two that change Wave 3's content rather than its scope: the pick already
exists as tuned data (§6.1), and UID policy and date policy are one question
(§1.2).

## 12. What this audit does not cover

Stated so the next person knows where the edge is. `api/` (49 files) was
surveyed but not read; `qc/body_part` was read only far enough to explain the
parity difference; `qc/classification_qc`'s rules engine was not read; the
frontend is not covered at all; and nothing outside `nils-engine` (the agent,
the app centre, segment, identity) is in scope here.
