# 03 — The registry: subjects are global, cohorts are two things (D4)

## The problem v0 half-solved

v0's metadata DB already keeps subjects global (`subject.subject_code` unique,
`subject_cohorts` many-to-many) — but the engine's *operational* unit is the cohort
as an ingest container with its own stage ledger, and the word "cohort" covers both.
The consequences are familiar: session 1 of a subject digested under one cohort and
session 2 under another look like an integration problem instead of two rows under
one subject, and "how many sessions does subject X have" needs system-level care it
should never need.

## The v1 model

One **registry** is the system of record. Inside it:

- **Subjects** are global. Sessions (studies), series, instances, stacks hang off
  subjects, full stop. Arrival path is metadata, not structure.
- **Ingest batches** replace cohort-as-container: every digest run is a recorded
  batch (source path, config, pack versions, who, when), and every row it touched
  carries the batch id. Provenance becomes queryable data on the data.
- **Cohorts become saved selections**: a versioned query AST
  ([05-contracts.md](05-contracts.md)) whose result is a set of subjects or stacks.
  A CSV membership import is one kind of selection (materialized membership rows a
  selection can reference); a metadata predicate is another; both are "the cohort".
  Anything that consumed a cohort in v0 — export, pipelines, segment works —
  consumes a selection in v1, which is strictly more expressive.

The agent question resolves by construction: sessions-per-subject is a registry
query; per-cohort views are the same query intersected with a selection.

## Identity linkage, promoted to first-class

The real world delivers the same person under different pseudonyms from different
sources. v0's `subject_other_identifiers`/id-types is the seed; v1 adds explicit
**linkage records**: two subject records asserted same-person, with evidence, actor
(human or agent — it is a review item, D7), and reversibility. Merges are logical
(a canonical subject with linked aliases), never destructive row surgery.

## Subject codes: the pseudonym scheme and the identity rule

A registry declares its **pseudonym scheme** once, at creation: an algorithm and
a key reference (C36, [15](15-ratification.md) §8). `blake2b-8` is v0's function
byte for byte (keyed BLAKE2b over the source identifier, eight bytes, hex), and the
KI registry is created with it and the v0 key so that every subject keeps its code
and a returning person lands on the known subject. New registries default to the
stronger scheme of D13. The key lives in the key store, is referred to by name and
appears nowhere else; the per-digest seed of v0 (empty meant the cohort name, and a
different code per digest) goes away.

What the scheme hashes is decided by the digest's **identity rule** (C37): which
source fields identify a subject, how they are parsed when a field carries more
than one thing (an identifier and a date in PatientName), and under which ID type
the identifier is filed in the linkage store. v0's `subjectIdTypeId` and the CSV
mapping are the seed. The rule is a knob: the engine reports what it could not
parse, and an agent, when present, proposes the rule as a review item.

Identity is **node-local** ([14](14-federation.md), C32): the pseudonym
domain of D13 belongs to one registry, linkage records never cross a node
boundary, and a federated count reports overlap between sites as unknown rather
than guessing. Cross-site linkage, if a study ever needs it, is an explicit
privacy-preserving record-linkage step (Bloom-filter PPRL, Mainzelliste-style)
under its own agreement, never a side effect of federating.

Every ingest batch and classification run advances the registry's **epoch**, a
monotonic counter reported by `GET /api/capabilities` and stamped on result handles
(C26), so "as of" is a number and a peer's answer is reproducible.

## Clinical timeline

The event/disease/observation model (events keyed by subject+type+date, diseases,
EAV measures) carries over conceptually — it is sound and registry-shaped already.
It is core, not an extra (D30, stated by Nima 2026-09-02): demographics, identifiers
by type, diseases and subtypes, typed and dated events and measures are what
makes everything after sorting possible, and queries, cohorts, pipeline inputs and
federated aggregates may depend on them; a deployment that only sorts and bidsifies
leaves the layer empty. Birth dates and exact event dates stay in the registry as
quasi-identifying fields, protected by class and role rather than by absence
(C35); direct identifiers alone live in the linkage store. The seeded vocabularies
(diseases, subtypes, observation types) are pack vocabulary (D26).
The six-times-copied CSV importer machinery does not: v1 has **one** declarative
importer (field mapping, preview, validate, apply) driven by a per-entity schema,
replacing ~10k lines of near-identical backend and frontend code.

## Modality readiness

v0's per-modality detail tables (`mri_`, `ct_` and `pet_series_details`, plus the
`ct_*`/`pet_*` fingerprint columns) are the pattern to keep, keyed to series, with
the classifier's modality packs ([04](04-classification-packs.md)) as the consumer.
The MRI assumption in v0 is downstream of the schema: the classifier reads none of
the CT/PET columns, gap filling is gated on `modality = 'MR'`, and the QC joins go
through the MRI table. Nothing in the core schema or the passes may assume MRI.

## Sessions and timepoints

v0's session-timepoint schemes (labeling sessions for BIDS) carry over as selection-
scoped configuration: a scheme belongs to a selection (a study's view of the data),
not to the registry — two studies can label the same subject's sessions differently
without conflict, which is exactly the multi-cohort reality that motivated this doc.
