# 12 — Devil's advocate review of the v1 folder (2026-09-01)

I wrote 00 to 11 from a reading of the code. This document is the second pass Nima
asked for: I took the opposite side of every decision and checked each one against
the live system on the production host, the schema, the classifier as it actually runs, the QC flows,
the agent's knowledge bases, the subject and cohort relationships, and the way the
apps couple to the engine. What follows is the evidence, the corrections I applied
in place (facts only), the challenges to decisions (which I did not change; they
need ratification), and the decisions the folder was missing.

> **Status:** every challenge (C1 to C15) and every missing decision (D13 to D19)
> in this doc was ratified on 2026-09-02, D13 as amended by C35 and C36
> ([15](15-ratification.md) §7 to §9). The statuses in section 6 are final.

> **Note:** all database figures below are counts and shapes taken on 2026-09-01
> from the live databases on the production host. No patient-level data was read. Code references are
> against `nils_private` at commit `2d7c374` (v0.5.3).

## 1. What I checked

- The five live Postgres databases on the production host: the engine's application DB and metadata
  DB, the agent DB, the identity DB and the segment DB.
- Three code audits: the classification package end to end (YAML versus Python,
  scoring, branches, persistence, QC flows, ground truth); the schema, stages,
  handovers, cohort ownership, anonymization, BIDS export and ingress; the apps
  (identity, segment, app center, `nilsctl`, pipelines, agent) plus every
  performance claim and the currency of `docs/`.
- External state: Flue, `dicom-rs`, `dicom_reorganizer`, and the group's own Go
  prior art in Bifrost.

## 2. The live system in numbers

| Fact | Value |
|---|---|
| Host | a large shared server (its size is not part of this copy). Metadata DB container capped at 48 GB, `shared_buffers` 4 GB, parallel workers 0; the *prod* defaults ship a 16 GB / 1 GB configuration |
| Registry | 37.5M instance rows, 24 GB metadata DB, 518,365 classified stacks, 5,322 subjects |
| Subjects across cohorts | 1,803 of 5,322 (34%) belong to more than one cohort; 5,319 carry an external identifier |
| Cohorts | 8 (3 with anonymization enabled); 206 jobs; 0 analysis-pipeline runs in the live application DB |
| Human decisions in the whole system | 2 confirmed axis-QC items (0 drafts), 113 main-acquisition acknowledgements, 284 labelled body-part slices, 1 trained body-part model, 5 keyword-override rows, 1 timepoint scheme |
| Review flags | 435,354 of 518,365 stacks (84%) are `manual_review_required`. Top reasons: `body_part:low_confidence` 345,849; `contrast:low_confidence` 302,196; `bodypart:spine_detected` 56,868; `base:low_confidence` 32,730; `contrast:duplicate_prediction` 23,994; `technique:low_confidence` 18,656 |
| Throughput | Extraction 150 to 460 files/s across the recorded jobs (pydicom `stop_before_pixels`, 4 to 12 workers, one DB writer, 100-row commits). Classification of 454k stacks: about four minutes |
| Handover rows | The largest cohort's step-2 handover is 3.1 MB of compressed JSONB holding two lists of 458,121 ids; at 10M stacks that row is about 180 MB of JSON text, materialized as Python lists on every read and rewritten on every boot |
| Origin hole | one cohort was sorted over 194,761 stacks, but only 681 rows carry it as `dicom_origin_cohort`; the rest were claimed first by another cohort, and the origin column is sticky by design. A cohort-scoped export of that cohort returns 681 stacks today |
| Ingest conflicts | 8 rows total: 5 series, 2 study re-links across cohorts, 1 identifier |
| Identity / segment usage | 3 users; segment production: 4 works, 5 units, 3 submissions |
| Code sizes | classifier: 9,957 lines of Python and 4,732 of YAML (3,516 loaded, 2,348 non-comment); CSV importers 5,807 backend + 4,986 frontend lines; `nilsctl` 13,675; nils-agent 82,266 Python (23,237 of it the DeerFlow fork) + 34,270 frontend; 288 raw SQL sites in the engine |

## 3. Corrections applied in place

These are factual errors in the folder. I corrected the sentences; the decisions they
sat under are unchanged unless listed in section 5.

1. **"v0 was tamed on an 8-core/64 GB host"** (00, 01, 02, 11). False. The phrase
   occurs nowhere in v0; the production host is many times that size, the database
   container limit is 48 GB, and the tuning was for write stability during
   extraction (the comment says
   "re-enable after extraction"). There is no small-host measurement of v0 at all.
2. **"Prior art in the group: `dicom_reorganizer` was Rust"** (02). The repository
   holds 2,168 bytes of Rust, a pyo3 stub. The group's real compiled-language prior
   art is Bifrost's Go CLI.
3. **"The per-stack single-threaded Python loop does not survive"** (02) was written
   as a performance argument. Classification is not the bottleneck: 454k stacks
   classify in about four minutes; extraction and the write path are the bottleneck.
   The sentence now says why the loop goes (structure), not speed.
4. **"`mri_series_details` becomes the pattern, not the exception"** (03).
   `ct_series_details` and `pet_series_details` already exist, and the fingerprint
   carries 17 `ct_*`/`pet_*` columns. The MRI assumption lives downstream: the
   classifier reads none of them, gap filling is gated on `modality = 'MR'`, and the
   QC joins go through the MRI table.
5. **"Weighted-evidence model"** (04). There is none. Every axis is a first-match
   ordered scan with a fixed confidence per tier (`CONFIDENCE_THRESHOLDS` in each
   detector); `calculate_confidence()` is exported and never called; `alternatives`
   is never populated; evidence and confidence are discarded before the upsert. The
   only real weighted score in the repo is the main-acquisition QC YAML.
6. **"~4.7k lines of YAML carried over verbatim"** (04, README). 852 of those lines
   are a reference file never loaded, 364 are the acceleration file that
   `AccelerationDetector` never reads, 1,499 are comments and blanks. The loaded
   vocabulary is 2,348 content lines; the grammar is 9,957 lines of Python (parsers,
   138 unified flags, tiers, exclusion groups, branch dispatch, branch taxonomies,
   physics windows, the semantic normalizer, and the cross-stack passes in step 4).
   Four YAML keys are already shadowed by Python and would silently no-op if
   "carried verbatim".
7. **"MP2RAGE routing after provenance"** (04). There is no MP2RAGE branch; it is a
   TI-threshold plus keyword rule inside `core/context.py`.
8. **"The 518k-stack cache is the ready-made ground truth"** (04). It is machine
   output with no verified/confidence/pack-version column, 84% of it flagged for
   review; the two human corrections that exist were written per series, and any
   correction is reverted by the next sort run (the upsert sets every axis to the
   fresh machine value; only `body_part` is shielded).
9. **"Five parallel draft-then-confirm QC implementations"** (05). There are eight
   review surfaces in three shapes: per-item draft/confirm (axes QC and its twin,
   one implementation), cohort snapshot with stage/commit/undo/drift signature
   (body part; cohort main, which applies immediately and adds durable
   acknowledgements), and write-through (main-acquisition role). Timepoint labels
   are derived on read and never written; BIDS collisions are automatic.
10. **Segment "reaches around the engine via a read-only DB role on three tables and
    pasted manifests"** (07). Its main path is a 425-line HTTP client on the
    analysis-pipelines API and `/api/export/resolve-text`, authenticated with a
    service token; the DB role is one JOIN used for subject/session grouping; the
    third channel, unmentioned, is the engine's scratch volume mounted at `/cold`.
11. **"120k-line fork"** (08). The DeerFlow fork is 23,237 lines; the ~116k-line app
    is mostly NILS-owned code (gateway, per-user model proxy, optimization loop,
    Next.js frontend).
12. **"Beta-stage… toward its 1.0"** (08). Flue was created 2026-02-07 and
    `@flue/sdk` is at 2.0.3 (2026-08-05), with no tagged GitHub releases. The churn
    risk is real but it arrives as breaking majors, not as a road to 1.0.
13. **"Content-addressed registration"** (09). v0 content-addresses descriptor
    versions only; output registration is deferred (`INGEST_DB_WRITE_ENABLED =
    False`) and `analysis_pipeline/config.py` states there is intentionally no
    result cache. The live application DB holds zero pipeline runs.

Two v0 documentation facts found on the way, recorded for the v0 maintainers rather
than fixed here: `docs/nils/suite-plan.md` is stale in three checkable places (boot
seeding exists, the N4 digest is real, `list_pipelines()` exists), and
`docs/cohort/sorting.md` documents a `parallel_workers` option that does not exist.

## 4. Verdicts, decision by decision

**D1, engine standalone and every app optional: holds.** The live coupling
confirms the direction (segment: HTTP plus one JOIN plus a volume; agent: a
superuser DSN with a "planned" read-only role; app center: no backend of its own).
One amendment: independence has to include "safe alone". The v0 engine ships with
auth `off`, and its own token verifier is more lenient than a verifier should be.
In v1, `off` mode must bind loopback only.

**D2, a compiled Rust core: holds as direction, premise corrected, one open
question.** The argument that survives is walk-and-parse throughput, bounded memory,
and a single static binary. The argument that does not survive is classification
speed (correction 3). The prior-art sentence was wrong (correction 2), and the
Rust-versus-Go comparison was never written: the group has shipped Go (Bifrost),
`dicom-rs` is at 0.10 with 558 stars, and neither library has met the production corpus.
Challenge C1 asks for a two-week Wave 0 spike that parses one million production instances
with both stacks and decides by measurement. Whatever the language, Wave 2 must port
the classifier as a faithful interpreter first (semantics 1:1, pinned by the 138-flag
contract test) and vectorize afterwards.

**D3, one schema on embedded and Postgres backends: holds, easier than framed.**
The v0 DDL already builds on SQLite in the test suite, and the genuinely unportable
Postgres features are all absent (no triggers, PL/pgSQL, materialized views,
extensions, partitioning, advisory locks, LISTEN/NOTIFY, window functions, recursive
CTEs, GIN). The work is the query layer (288 raw SQL sites with `ANY`, `ON
CONFLICT`, `UPDATE … FROM`, `COPY`, `LATERAL`), which v1 rewrites anyway. Amendment:
one query builder with a three-dialect test matrix, no hand-written SQL outside the
core, and the embedded mode defined by one engine's semantics (DuckDB as an optional
accelerator over the same file, never a second store).

**D4, subjects global, cohort = ingest batch + saved selection: holds, and the
evidence is stronger than the doc claimed.** 34% of subjects are already
multi-cohort; the cohort exists twice (two id sequences joined by an immutable
lower-cased name); `jobs` has no cohort column; no data row records which run wrote
it or which pack judged it; `cohort.path` is overwritten on every extract; a study
ingested under a second cohort is silently re-linked to the new subject (twice in
the live data); and the sticky origin column produces the origin hole (681 of
194,761). Three things the doc missed:

- *Overrides per selection would make results non-canonical* (04 says per-cohort
  keyword overrides become per-selection). The same stack would classify
  differently depending on who asks. Challenge C2: scope overrides by provenance
  (ingest batch, site, scanner) as versioned pack overlays applied at classification
  time; selections never change a classification.
- *The cohort is the pseudonymization domain today.* `subject_code` defaults to
  BLAKE2b(PatientID) keyed with the upper-cased cohort name, so the same patient
  under two cohorts is two subjects unless a CSV map is supplied; in practice every
  extract job used a CSV map that NILS does not retain. Identity linkage exists, but
  outside NILS, in files. Challenge C3 (with D13): the registry-wide pseudonym key
  and the imported linkage maps are a Wave 1 deliverable; existing codes are
  imported as-is, never re-derived.
- *Decisions a cohort owns must have a home.* Acknowledgements, staged body-part
  labels, the training samples and the chosen timepoint scheme are decisions about
  the data. They attach to stacks, sessions and subjects in the registry (review
  items and staged results, C5) and are visible to every selection; a selection
  owns only its scheme choice, as 03 already says.

**D5, the AST executes in the engine: holds, with a gate.** The catalog replaces
`db_guru`'s table lore, but the agent's real field knowledge is ten query patterns in
the `nils-data` skill (latest session per subject via `DISTINCT ON`, temporal
anchors, multi-window designs, intervals) plus workflow rules hard-wired in the
693-line lead-agent prompt. Challenge C4: the ten patterns become AST fixtures in
Wave 4's contract test; if the AST cannot express one, the AST is incomplete, not the
pattern.

**D6, the 30M-instance / 8-core budget: the principle holds; the numbers are
unanchored.** No small host has ever run v0, so the budget has no baseline, and "a
working day" on eight cores means roughly 1,000 files/s sustained, three to seven
times v0's measured rate on a much larger machine. Plausible for a compiled
header parser, but the wall will be storage: on the new platform the sources sit on
the storage server over NFS, and 30M header reads are 30M metadata operations.
Challenge C6: Wave 0 builds the baseline host (an 8 vCPU / 64 GB VM on Asgard
reading from the storage server over NFS, doubling as the CI benchmark runner),
measures v0 on it first, and
restates the budget in files/s and peak RSS.

**D7, review items as the one primitive: holds as the spine, under-specified in
four ways.**

- *Shapes.* The per-item shape covers two of v0's three shapes. The cohort-snapshot
  shape (apply a model to a whole cohort, stage, inspect, commit or destage, undo)
  is a batch decision on a result set, not N items. Challenge C5: jobs may produce
  *staged* result versions that a commit promotes to canonical; review items
  reference result versions.
- *Scale.* 435k flagged stacks today, 346k of them because the body-part detector
  found no keyword and 302k because the contrast detector returns confidence 0.0 when
  no keyword matches. That is "no evidence", not uncertainty. Items need emission
  thresholds per kind (no-evidence is a column, not an item), grouping by evidence
  signature (one item, N members), and bulk decisions; otherwise the queue is
  unusable and agents rubber-stamp it.
- *Precedence and persistence.* v0 deletes the draft on confirm, writes the
  correction per series, and reverts it on the next sort. In v1 a decision is a
  first-class row at the scope it was made, outranks machine output (human > agent >
  rule), and survives re-classification: a new pack version that disagrees with a
  standing human decision emits a new item, it never overwrites.
- *Labels are training data.* The body-part loop (284 labelled slices → a linear
  probe per cohort) is the one real model in v0. Challenge C7: decisions of a kind
  are exportable as a labelled dataset with provenance, and model artifacts register
  with their training set and version.

**D8, auth delegated to OIDC: holds; three amendments.** The evidence for
retiring nils-identity is stronger than stated: three users, an `app_grants`
table that "nothing checks yet", four verifier copies in three flavours. Amendments: app-level roles stay app-owned (segment's
`seg_admin/annotator/reviewer` and rater modes were right to live in its own DB and
never in the JWT); the CLI gets a device-code flow or engine-minted tokens after
OIDC login; `off` binds loopback (D1).

**D9, BIDS-Apps compatible pipelines: holds; the prerequisite was missing.**
BIDS-Apps need a *valid* BIDS tree. v0's tree is BIDS-shaped with
classification-derived filenames (no `_T1w` suffixes, `T2*w` written as `T2_w`),
which is why every shipped wrapper discovers its input at runtime, and why the
seeded N4 "only works on narrow selections" (about 90% of sessions carry several
T1w stacks). Amendments: Wave 3's export must be validator-clean before Wave 7's
runner means anything (C8); the *main acquisition per session and contrast* becomes a
registry fact (a decided review item, 113 acknowledgements already exist) that
export and pipelines consume, so a BIDS App gets one T1w, not five; derivatives are
actually registered (content hash, producing run) since v0 deferred it; the stack
work unit for DICOM-input tools stays a declared seam; and the runner states its
privilege model (C9).

**D10, open development, one repo per product: holds; one rule added.** Wave 2
plans to generate the corpus from the live cache, and 10 says only synthetic or
fully anonymized fixtures go public. Both are right only if stated together:
fingerprint text (series descriptions, protocol names, station and institution
names) is quasi-identifying and occasionally carries typed-in names. Challenge C10:
the live-derived corpus stays in the private harness on the production host; the public corpus is
synthetic or transformed, reviewed before the first public commit. Licensing stays
open, deadline unchanged.

**D11, the agent as a Flue app, MCP client only: holds; facts corrected.** The
"MCP only" rule is more urgent than the doc knew: today the agent runs raw SQL under
a superuser DSN with the read-only role still "planned". Amendment: pin a Flue major
and keep it behind a thin adapter; the Ask-query pilot is the churn probe.
Knowledge migration is three moves, not one: table lore to the catalog, the ten
query patterns to skills or AST templates, and the prompt-resident workflow rules
(first action, clarification exemptions, zero-row protocol) to skills.

**D12, classification as versioned packs: holds as direction; three premises
were false and the format is under-specified.** Corrections 5 to 8 above. What the
decision must now say:

- *The pack needs a grammar, not just vocabulary* (C11). Flags, parsers, tiers,
  exclusion groups, branch dispatch with output taxonomies, physics windows, the
  normalizer map, and the cross-stack passes (session SECONDARY rescue before
  classification; gap filling, SWI re-routing, contrast duplicate detection,
  incomplete-4D detection after it) are all Python today. Either the pack language
  expresses them, or packs get a sandboxed code escape hatch. "Nothing in the engine
  changes to add a modality" is only true once that choice is made.
- *Gap filling must become deterministic.* Its reference pool is the whole registry
  with no cohort filter, so results depend on ingestion order and sorting cohort B
  can change cohort A. In v1 it runs against a versioned reference (the pack corpus
  plus a recorded registry snapshot) and writes its vote as evidence, or it is
  replaced by review items.
- *Two corpora, named honestly* (C12). The v0-parity corpus is the 518k cache and
  gates behaviour reproduction. The verified corpus is built during Wave 2 by
  stratified adjudication (axis value × provenance × manufacturer), seeded by the 113
  acknowledgements, 284 body-part labels and 5 override rows. The verified corpus is
  the moat; the parity corpus is scaffolding.
- *Seven persisted axes, not six.* Contrast agent (`post_contrast`) and body part are
  written columns, DWI enrichment adds four more, and `directory_type` is
  synthesized intent. Pack outputs must include all of them.
- *Route by modality from day one.* CT and PET stacks are run through the MRI
  classifier today and come out as `misc` plus review flags. The v1 router must
  produce an explicit "no pack for this modality" outcome, never a review item.

## 5. Decisions the folder was missing

**D13, PHI custody and the pseudonymization domain.** v0 was built for one
trusted host: it keeps identifying and clinical fields in the same registry as the
imaging index, without a notion of field sensitivity, and its anonymizer was written
for a single site's needs rather than as a custody boundary (the private record
lists what that means in v0's schema and code; this copy states the requirement).
v1 needs, at minimum: field sensitivity classes in the
catalog (identifying, quasi-identifying, clinical, technical); a registry default of
no direct identifiers (birth date becomes age at study; PatientID lives only in a
separate linkage store with its own key and read audit); a registry-wide keyed
pseudonym with the key held outside the database; review-item evidence and MCP
responses filtered by class; and an anonymizer with UID remapping, per-subject date
shifting, a private-tag policy, collision-free pseudonyms and an audit that never
sits next to the originals. Amended 2026-09-02 by C35 and C36 in
[15](15-ratification.md) §8: absence applies to direct identifiers only, birth
dates and the clinical timeline stay in the registry as classed fields, and the KI
registry keeps v0's subject-code scheme and key. Ratified 2026-09-02 as amended.

**D14, staged results and bulk decisions** (from C5): jobs can produce result
versions that are not canonical until committed; items group by evidence signature;
decisions apply to groups.

**D15, labels and models** (from C7): a decision kind can be exported as a labelled
dataset with provenance; model artifacts register with training set and version;
sidecar outputs carry the model stamp into evidence.

**D16, the BIDS oracle** (from C8): the Wave 3 gate is a validator-clean tree plus
hand-verified reference selections; v0 exports are informative, not the reference,
because they are not valid BIDS and carry three known bugs.

**D17, source layout and the walker.** v0 assumes one directory level (every
subdirectory of the root is a subject, its name is the subject key) and drops every
non-MR/CT/PET file with a debug log and no record. Live layouts are heterogeneous.
v1's walker groups by DICOM tags only, records the path of every file relative to
the batch root, and quarantines what it refuses as a listed output (02 already says
this for parse failures; it must include modality refusals). On the new platform
the batch root is the raw DICOM dataset of each study on the storage server.

**D18, container runtime privileges.** v0 mounts the Docker socket read-write into
the engine in development, which is root on the host. v1's pipeline subsystem runs
rootless (podman or apptainer) by default; the Docker socket is an explicit,
documented opt-in.

**D19, the deployment glue that disappears.** `nilsctl` (13,675 lines, including a
hand-written YAML parser) exists to assemble tiers, generate the gateway and tiles,
provision DB roles and rotate secrets across apps that share databases. With D1, D8
and D10 in force most of it has no job left; what remains (our compose files,
Authentik and Traefik config) is the private glue repo of 10. Record it so nobody
ports it.

## 6. Amendments register

All ratified 2026-09-02; each one names the decision it changes.
C16 onward, from the query and agent study, continue in
[13](13-query-and-agent-study.md) §6; C26 onward, from the federation design, in
[14](14-federation.md) §6.

| Id | Affects | Proposal | Status |
|---|---|---|---|
| C1 | D2 | Wave 0 language spike: parse 1M production instances with `dicom-rs` and a Go DICOM library; decide by files/s, RSS and vendor-file failures. Classifier port is interpreter-first either way | accepted 2026-09-02; judged on speed and maintainability together (15 §7) |
| C2 | D4, D12 | Overrides are provenance-scoped pack overlays, never per selection | accepted 2026-09-02 (15 §9) |
| C3 | D4, D13 | Registry-wide pseudonym key held outside the DB; existing subject codes and CSV linkage maps imported as linkage records in Wave 1 | accepted 2026-09-02 (15 §9) |
| C4 | D5 | The ten `nils-data` query patterns become AST fixtures in the Wave 4 gate | accepted 2026-09-02 (15 §9) |
| C5 | D7 → D14 | Staged result versions with commit; grouped items; bulk decisions; emission thresholds per kind | accepted 2026-09-02 (15 §9) |
| C6 | D6 | Baseline host (8 vCPU / 64 GB VM on Asgard over NFS) built in Wave 0; v0 measured on it; budget restated in files/s and RSS | accepted 2026-09-02 |
| C7 | D7 → D15 | Decisions exportable as labelled datasets; models registered with training provenance | accepted 2026-09-02 (15 §9) |
| C8 | D9 → D16 | Wave 3 gate = validator-clean + reference selections; main acquisition per session/contrast becomes a registry fact | accepted 2026-09-02 (15 §9) |
| C9 | D9 → D18 | Rootless runtime by default; Docker socket opt-in | accepted 2026-09-02 (15 §9) |
| C10 | D10 | Live-derived corpus stays private on the production host; public corpus synthetic or transformed, reviewed before the first public commit | accepted 2026-09-02; a named step of the restart (15 R7) |
| C11 | D12 | Pack format specifies a grammar (flags, tiers, branches, physics windows, cross-stack passes) or a sandboxed code escape hatch; decided before Wave 2 | accepted 2026-09-02, with the modality-extension criterion (15 §7) |
| C12 | D12 | Two named corpora: v0-parity (the cache) and verified (adjudicated in Wave 2); the gate reports against both | accepted 2026-09-02 (15 §9) |
| C13 | D1, D8 | `off` mode binds loopback only | accepted 2026-09-02 (15 §9) |
| C14 | D12 | Gap filling deterministic against a versioned reference, or replaced by review items | accepted 2026-09-02 (15 §9) |
| C15 | D7 | Decision precedence human > agent > rule; decisions keyed at their scope; re-classification emits new items, never overwrites | accepted 2026-09-02 (15 §9) |

## 7. What I did not change

The twelve decisions stand as written; the language choice, the budget numbers and
the wave order are untouched. I corrected only sentences that were false, and I
annotated the places where a decision is challenged so that a reader of 00 to 11
sees the challenge without leaving the page. The decision register in
[README.md](README.md) was updated on 2026-09-02, when the amendments above were
ratified; an outdated register is worse than none (11).
