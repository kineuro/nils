# 04 — Classification as modality packs (D12)

## What is preserved

The axis system — base, technique, modifier, construct, provenance, acceleration,
plus the persisted contrast-agent and body-part axes — is the scientific core of
NILS and the part of v0 that must survive byte-for-byte in meaning. What v0 actually
runs is a *tiered* model, not a weighted one: each axis is a first-match ordered
scan (exclusive flag, then keywords, then combination, then fallback) with a fixed
confidence per tier; the weighted-evidence helpers in the code are never called and
neither evidence nor confidence is persisted. The knowledge is split: about 2.3k
content lines of loaded YAML vocabulary (of 4.7k lines on disk, two files are never
read) and about 10k lines of Python grammar (the five parsers, 138 unified flags,
exclusion groups, branch dispatch and branch taxonomies, physics windows, the
semantic normalizer, and the cross-stack passes before and after classification).
The branch pipelines (SWI, SyMRI, EPIMix/NeuroMix) and the MP2RAGE TI-threshold
rule are part of that meaning. Details and consequences in
[12](12-review-devils-advocate.md), D12.

Two v0 loose ends get folded in rather than carried: the acceleration detector's
hardcoded class-level lists become pack data like every other axis, and per-cohort
keyword overrides get a new scope. This doc said "per selection"; C2
([12](12-review-devils-advocate.md), accepted 2026-09-02) withdrew that:
per-selection overrides make the same stack classify differently depending on
who asks. The scope is
provenance (ingest batch, site, scanner), applied at classification time as a
versioned pack overlay. The overlay is also the knob an agent tunes (C37): a digest
from Italy carries `senza mdc` as a no-contrast keyword in its overlay, proposed
from the first pass's diagnostics and accepted as a review item, and the pack is
untouched.

## The pack

A **pack** is the unit of classification knowledge:

```
packs/mri/
  pack.yml          # name, version, modality, axes it provides, engine contract version
  axes/*.yml        # vocabulary carried from v0; the grammar must be expressed too (C11)
  branches/*.yml    # provenance-routed branch declarations, with their output taxonomies
  passes/*.yml      # cross-stack passes: session rescue, gap filling, duplicate detection
  corpus/           # expectation fixtures: fingerprint → expected axis results
```

The pack format is not settled by this sketch. v0's grammar lives in Python, four
YAML keys are already shadowed by code, and "carried verbatim" would silently no-op
them. Before Wave 2 the format either expresses the whole grammar (flags, parsers,
tiers, exclusion groups, branches, physics windows, passes) or packs get a sandboxed
code escape hatch: C11 in [12](12-review-devils-advocate.md), accepted 2026-09-02
with a second criterion: adding a modality has to be easy for us and need no code
from a user, who tunes detection through knobs (C37).

- Packs are **versioned and diffable**; every classified stack records the pack
  version that judged it, so re-classification is an honest diff ("pack 2.1 changes
  3,412 stacks, here they are") instead of a blind overwrite.
- Packs are **plugins**: the engine loads packs by manifest; third parties can ship
  one. This is the extension surface that makes the classifier a standard rather
  than a feature — other groups contribute packs and corpus cases upstream instead
  of forking.
- The pack is the **common data model** across sites ([14](14-federation.md),
  D26). Two registries classified by the same pack version answer "3D FLAIR
  at 7T" identically, which is what lets a count from Vienna sit beside a count
  from Stockholm without anyone seeing anyone's rows. The rule that follows: packs
  use semantic versions, any change to a vocabulary (axis values, image-type
  tokens, observation types, identifier namespaces, roles) is a **major**, a
  federated request declares the pack version it was written for, and a peer on
  an incompatible major answers `incompatible` with the vocabulary diff, never
  an approximation. The verified corpus is what makes a pack version mean the
  same thing at every site.
- The **corpus is the contract**: a pack ships expectation fixtures, and the engine
  refuses to load a pack whose own corpus fails. There are two corpora, named
  honestly (C12): the *v0-parity corpus* generated from the live 518k-stack
  classification cache on the production host, which is machine output (84% of it flagged for
  review, no verified or pack-version column, human corrections reverted by the
  next sort), and the *verified corpus* built during Wave 2 by stratified human
  adjudication, seeded by the 113 acknowledgements, 284 body-part labels and 5
  override rows that exist today. Parity is scaffolding; the verified corpus is the
  moat. The live-derived corpus stays in the private harness; only synthetic or
  transformed cases go public (C10).

## Modality routing

The engine routes each stack to packs by modality. MRI ships at v1.0 as the carried
v0 pack. **CT** (including photon-counting) and **PET** are new packs with their own
axes (for CT: kernel, kVp/spectral, contrast phase, gating; for PET: tracer,
reconstruction) — designed with the same evidence model, on the roadmap after the
MRI pack passes its regression gate. Nothing in the engine changes to add a
modality; that is the test of the design, and it only passes once the C11
prototype has decided the format. Where the axes differ, CT having no base
contrast, the vocabulary finds common ground or the modality gets its own axes on
the same flow (15 §7).
Today CT and PET stacks are run through the MRI classifier and come out as `misc`
plus review flags; the v1 router returns an explicit "no pack for this modality"
outcome instead, never a review item.

## Execution

Rules compile to vectorized columnar expressions where their structure allows
(keyword/regex/threshold predicates over fingerprint fields — most of the corpus),
with the remainder evaluated in parallel batches in the compiled core. The port is
interpreter-first: Wave 2 reproduces v0's semantics one to one (pinned by the
138-flag contract test) before anything is vectorized. Evidence is stored, not just
the verdict (v0 discards both evidence and confidence before the upsert): every axis
result keeps its contributing evidence and weight, because evidence is what review
items (D7) show humans and agents alike, and what makes an agent's confirmation
of a low-confidence classification an informed act instead of a rubber stamp.
Cross-stack passes run deterministically: v0's gap filling votes against the whole
registry with no cohort filter, so sorting one cohort can change another; in v1 a
pass runs against a versioned reference and writes its vote as evidence, or it is
replaced by review items (C14).

## ML in classification

Model-based detectors (body-part today; more later) join through the sidecar
pattern: an optional service the engine calls, whose *outputs* enter the same
evidence stream with a model+version stamp. A pack may declare "this axis prefers
the sidecar when present" — absence degrades to rules-only, per D1.
