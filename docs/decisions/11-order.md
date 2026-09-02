# 11 — Build order, gates, and the do-not-forget list

## Standing rules

- **v0 keeps running on the production host untouched** until every gate below is green. It is the
  oracle (principle 10); decommissioning it early forfeits the regression asset.
- Every wave opens by writing the real spec for its slice (this folder only records
  decisions) and closes at a **gate**: a measurable comparison against v0 or a
  working consumer, not a demo.
- Waves are sequential where they share the registry; the agent track is parallel
  by design.

## Waves

**Wave 0 — ground.** v0 feature work frozen (F1, since 2026-09-02). The public repo
restarted as `kineuro/nils` on 2026-09-02 (15 §11; R1 to R8; AGPL-3.0-only, 10). CI skeleton:
multi-platform binary builds, the two-backend test matrix, the scaled performance benchmark harness (D6). Accepted 2026-09-02 from
[12](12-review-devils-advocate.md): the language spike (C1, judged on speed and
maintainability together), the baseline host with v0 measured on it (C6), and the
pack-format prototype (C11) before Wave 2 opens.

**Wave 1 — parse and digest.** The Rust core: walk, parse, extract, stack
signatures, registry schema (both backends), ingest batches, `nils digest` with
resume. *Gate:* parse-and-compare on the production corpus — field-level agreement with
v0's extraction on 37.5M instances, with every divergence classified (v0 bug, v1
bug, or accepted change); the performance budget met on the 8-core/64 GB baseline
host (C6). Identity linkage, the pseudonym scheme with the v0 key, and the identity
rule land here (C3, C36, C37, D13).

**Wave 2 — fingerprint and classify.** The columnar fingerprint pass, the pack
loader, the MRI pack carried over, evidence storage, `nils classify`. *Gate:*
diff against the 518k-stack classification cache (the v0-parity corpus, machine
output); disagreements individually adjudicated and either fixed or recorded as
intentional pack corrections (each one becomes a case in the verified corpus, C12).
Because v0 stores no pack version, the diff must separate rule changes from step-4
gap-filling drift, which depends on ingestion order (C14).

**Wave 3 — anonymize and BIDS.** Strategies, audit, `dcm2niix` orchestration,
naming/collision rules, `nils anonymize` and `nils bids`. *Gate:* BIDS validator
clean on hand-verified reference selections, with the main acquisition per session
and contrast taken from the registry (C8, D16). v0 exports are compared for
information only: they are not valid BIDS (classification-derived filenames, three
open naming bugs), so "byte-identical against v0" is not the bar.

**Wave 4 — server and contracts.** The thin server: jobs, API, semantic catalog,
AST execution, selections, review items, auth modes, MCP, events
([05-contracts.md](05-contracts.md)). The web UI rebuilt on the job/queue model.
*Gate:* the CLI and the UI drive every stage through the same doors (contract
test), and an off-the-shelf MCP client can query, select, and work a review queue.
Additions from [13](13-query-and-agent-study.md): the AST fixtures of C4
grow to the 28 gold tasks and the ten question families (C16), each with a declared
grain; the affordance endpoints and result handles are part of the contract test
(C20, C21); the MCP shape is exercised by Flue's own client and by a third-party
client through the OAuth resource metadata (C22). From
[14](14-federation.md) (C33): the federation **primitives** land here because
they are cheap now and misery later: the registry epoch and pack versions in
capabilities, `local`/`federated` visibility in the catalog, disclosure
projections on result handles, the `federation.*` review kinds, `user@node`
principals and peer-key verification (C26 to C30). No daemon yet; the contract
test proves a projection suppresses and a `local` field never validates for a
federated principal.

**Wave 5 — nils-query MVP.** Notebook, saved selections, send-to. *Gate:* a real
study's cohort defined as a selection and exported end to end without a hand-written
manifest. Addition (C16): every gold task expressed in the notebook
reproduces its v0 result hash on the migrated registry, and the ten families are
expressible without an escape hatch, roles and picks included (C19).

**Wave 6 — segment rebased.** Port nils-segment onto contracts only (07). *Gate:*
a full annotation work — subset by selection, prep via seeded pipelines, rating,
adjudication, export — with zero database-level integration. This wave is the proof
of D1 and the contracts; treat its friction as contract bugs.

**Parallel track — agent.** From Wave 4's MCP: the Ask-query Flue pilot, then agent
v1 growing alongside (08). Never on the critical path; review-item policies for
agents open only after the human workflows are trusted. The pilot is time-boxed
with its exit criteria written first (C23, 08): the ten families through the
draft-selection loop on a local and a hosted model, gold hashes reproduced, fewer
turns per resolved question than v0, no harness upgrade costing more than a day.
`nils-evals` (C25) is built before the pilot so that it can be judged.

**Wave 7 — pipelines and packs widen.** BIDS-Apps runner hardened, starter catalog
seeded at boot, GPU leasing, descriptor tunables + QC hooks (09). CT pack designed
against real photon-counting data (04). v0 decommission plan executed. Added
(C31): the SLURM and Apptainer executor, with the Amsterdam cluster as its first
target if that is what their cluster is; *gate:* the starter pipeline runs on a
selection through their scheduler with full parameter provenance and rootless.

**Wave 8 — federation** ([14](14-federation.md), D25 to D29, ratified 2026-09-02). The
node daemon, manifests and pinned peers, the federated catalog and node profiles,
fan-out and merge, the daemon's MCP server, the scope chip (06), the agreement
template. *Gate:* the Stockholm–Vienna pilot: two nodes over a mesh, the
composition and protocol questions of 13 §2 answered at both at `count` with
k=5, one request needing a human at the other end, audit read at both ends,
nothing individual-level moved. It opens only after Wave 5, because a federation
of registries nobody can query locally is a federation of nothing.

## The do-not-forget list

Standing items that must not silently drop off between waves — check at every gate:

- The **license** (decided 2026-09-02, 10): AGPL-3.0-only with an SPDX header in
  every source file, Apache-2.0 in `contracts/` and the SDKs, `CONTRIBUTING` with
  the CLA and the DCO, all from commit one (15, R6).
- **Identity linkage** lands with the registry schema (Wave 1), not "later" —
  retrofitting merge semantics is misery.
- **CT/PET readiness** is a schema and pack-router concern from Wave 1 even though
  the packs come in Wave 7 — nothing may assume MRI.
- **Small-machine CI** from Wave 1 — the budget is a gate, not a note.
- **The custody page** (C38) ships with the first store that persists anything a
  user would ask about, which is Wave 1's registry; every later store is added to
  it in the wave that creates it.
- **The one generic CSV importer** replaces the six copies when clinical imports
  port (Wave 4) — do not port the copies.
- **Absence stories** written per app as each ships (D1) and tested: a
  contracts-only deployment must render no dead links and raise no errors.
- **Corpus hygiene**: every adjudicated disagreement in Waves 1-3 becomes a fixture;
  the corpus is the moat.
- Migration of the live registry itself (v0 Postgres → v1 schema) is Wave 4's
  hidden deliverable — spec it there, with the production data as the rehearsal.
- **Federation stays optional at every gate** (14, D25): a deployment without
  `nils node` must show no scope chip, no federation tool, no endpoint, no error.
  The absence test of D1 applies to the daemon like to any app.
- **Ask Vienna and Amsterdam early** what they run, who the controller is, and
  which kind of cluster they mean (14 §7); the answers decide whether Wave 7's
  executor and Wave 8's pilot have real targets or stay declared seams.
- Amend this folder when reality wins an argument. An outdated decision record is
  worse than none.
