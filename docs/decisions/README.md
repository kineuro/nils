# NILS v1: direction and plan

This is the decision record and build plan for the NILS rewrite, which ships as
NILS 1.0. It exists so the bigger picture survives the months of detailed
work: every doc records *what we decided, why, and what it rules out*. When a later
change contradicts one of these, the doc gets amended the same day, and the code
commit that made the change cites the decision. The record is the truth, not a
snapshot.

Status: **ratified 2026-09-02**, every decision and every amendment
([15](15-ratification.md) §7 to §9). Open on evidence only: the language
spike, the baseline measurement and the pack-format prototype (approved work), and
on other people: Amsterdam's cluster and Vienna's answers. The name is NILS (D31,
2026-09-02).

This directory is the public copy of the record, which is kept in the private
repository `kineuro/nils-design` and copied here after a scrub pass
([15](15-ratification.md), R7). The private record describes the live system in
more detail than a stranger should read; what this copy leaves out, and why, is
listed in [SCRUB.md](SCRUB.md). Nothing that was left out changes a decision.

## Version names

- **v0** is the 0.x line: what runs in production today (0.5.3, in `nils_private`). It never
  reached 1.0; it was the ground that taught us what the foundation has to be.
- **v1** is this rewrite. Pre-releases are tagged `v1.0.0-alpha.N`; the first release
  is 1.0.0.
- There is no v2. Earlier drafts of this record called the rewrite "v2" and the 0.x
  line "v1"; both were renamed on 2026-09-02, ids included (`V2-D5` became `D5`).

## Reading order

| Doc | What it settles |
|---|---|
| [00-vision.md](00-vision.md) | What NILS v1 is, and the ten principles every decision answers to |
| [01-architecture.md](01-architecture.md) | The component map: one core, optional apps, how absence behaves |
| [02-engine.md](02-engine.md) | The engine core: language, storage, pipeline model, CLI, performance budget |
| [03-registry.md](03-registry.md) | The data model: global subject graph, ingest batches, cohorts as selections |
| [04-classification-packs.md](04-classification-packs.md) | Six-axis classification as modality packs; the CT/PET path |
| [05-contracts.md](05-contracts.md) | The engine's public contracts: API, query AST, MCP, review items, auth |
| [06-app-query.md](06-app-query.md) | nils-query: the selection notebook |
| [07-app-segment.md](07-app-segment.md) | nils-segment: rebasing on the contracts |
| [08-app-agent.md](08-app-agent.md) | nils-agent v1 on Flue, as an MCP client |
| [09-pipelines.md](09-pipelines.md) | Analysis pipelines as plugins: BIDS-Apps compatibility, tunables, QC hooks |
| [10-repos-licensing.md](10-repos-licensing.md) | Repository strategy, open development, the licensing decision |
| [11-order.md](11-order.md) | Build order, validation gates, and the do-not-forget list |
| [12-review-devils-advocate.md](12-review-devils-advocate.md) | The 2026-09-01 verification against the live system: corrections applied, challenges C1-C15, missing decisions D13-D19 |
| [13-query-and-agent-study.md](13-query-and-agent-study.md) | The 2026-09-02 study of nils-query and nils-agent against Metabase's and Flue's source and the live agent traffic: ten use-case families, amendments C16-C25, decisions D20-D24 |
| [14-federation.md](14-federation.md) | The 2026-09-02 federation design: engines as nodes, optional and node-addable, disclosure levels at the door, compute travels and data stays, the cluster question; amendments C26-C34, decisions D25-D29 |
| [15-ratification.md](15-ratification.md) | The 2026-09-02 ratification sheet: every open item of 12, 13 and 14, the license and the repository restart, each with a recommended verdict; every verdict recorded the same day (sections 7 to 10), the four items Nima raised in section 8, the repository as built (section 11) |

## Decision register

Decisions carry stable ids (D1 onward) so code and later docs can cite them; cite
them as `nils-design D5`, repository and id, so a reference survives any rename.
Amending one means editing its doc and noting the change here. The last column
lists the amendments (the C ids of [12](12-review-devils-advocate.md),
[13](13-query-and-agent-study.md), [14](14-federation.md) and
[15](15-ratification.md) §8) that sharpened each decision; all were
ratified on 2026-09-02, so an amendment is part of the decision it names and the
docs carry it in place.

| Id | Decision | Doc | Amendments (12, 13, 14, 15) |
|---|---|---|---|
| D1 | The engine is complete alone; every other app is optional, and absence degrades features silently, never errors | 00, 01 | holds; C13 (`off` binds loopback); C37 (every knob is a contract, 15 §8); D25 (the node daemon is one more optional app, in the same binary) |
| D2 | The engine's volume path is a compiled Rust core shipped as a single static binary; ML stays in optional Python sidecars | 02 | holds as direction; C1 accepted 2026-09-02 (Rust vs Go spike, judged on speed and maintainability); prior-art premise corrected |
| D3 | One schema, two storage backends: embedded (SQLite + DuckDB) for standalone, Postgres for the multi-user server | 02 | holds |
| D4 | The registry is subject-centric and global; "cohort" splits into ingest batch (provenance) and saved selection (membership) | 03 | holds, strengthened; C2, C3, C32 (identity node-local; registry epoch); C36 (the KI registry keeps v0's subject-code scheme and key), D30 (the clinical timeline is core) |
| D5 | The query AST executes in the engine — one governed query door for UI, agent, and CLI alike | 05, 06 | holds; C4, C16, C17, C18, C20, C22, C27, C28; D20, D27 (a peer's request is the same door, answered as a projection) |
| D6 | Performance is a budgeted requirement: a 30M-instance digest must fit 8 cores / 64 GB, enforced by a scaled CI benchmark | 02 | principle holds, numbers unanchored; C6 accepted 2026-09-02 |
| D7 | Every automated judgement emits a review item with evidence into one queue; humans and agents consume the same queue under per-type policies | 05 | holds as spine; C5, C7, C15, C21, C25, C29, D14, D15, D22, D28 |
| D8 | Authentication is delegated (OIDC, Authentik in our deployment) with `off` and `token` modes; NILS never mints identities again | 05 | holds; C13, C22 (OAuth resource metadata on the MCP endpoint), C30 (node keys, peer claims, no fourth mode) |
| D9 | The pipeline contract is BIDS-Apps-compatible; `nils.job.yml` is the metadata layer on top, not a private container format | 09 | holds; C8, C9, C19, C31, D16, D18, D29 |
| D10 | Public products develop in the open, one repo per product; the private-superset/public-mirror pattern is retired | 10 | holds; C10 accepted 2026-09-02; license decided 2026-09-02 (AGPL-3.0-only engine and apps, Apache-2.0 contracts and SDKs, CLA and DCO, 10); `nils node` lives in the engine repo (14) |
| D11 | The agent is built on Flue and talks to the engine only through the public contracts (MCP first) | 08 | holds, facts from source; C23, C24 accepted and D23, D24 ratified 2026-09-02; C34 |
| D12 | Classification rules ship as versioned modality packs (manifest + rules + corpus tests); the v0 MRI rules carry over in meaning (vocabulary verbatim, grammar re-expressed) | 04 | holds as direction; three premises corrected; C2, C12, C14, C19; C11 accepted 2026-09-02; C37; D21, D26 (the pack is the common data model across nodes) |
| D13 | PHI custody and the pseudonymization domain: direct identifiers live only in the linkage store; quasi-identifying and clinical fields stay in the registry under sensitivity classes; the pseudonym scheme is declared per registry; the anonymizer remaps UIDs, shifts dates per subject, drops private tags by default and audits away from the originals; the domain is per node | 03, 12 §5, 15 §1 and §8 | C3, C32, C35, C36 |
| D14 | Staged results and bulk decisions | 05, 12 §5 | C5, C21 |
| D15 | Labels and models registered with provenance | 05, 12 §5 | C7, C25 |
| D16 | The BIDS oracle is the validator plus reference selections | 04, 09, 12 §5 | C8 |
| D17 | The walker groups by DICOM tags only, records every path, quarantines refusals as a listed output | 02, 12 §5 | |
| D18 | Rootless container runtime by default; the Docker socket is an opt-in | 09, 12 §5 | C9 |
| D19 | The deployment glue disappears; `nilsctl` is not ported | 10, 12 §5 | R9 |
| D20 | Grain and denominators are explicit in the AST | 05, 13 §5.2 | C18 (staged) |
| D21 | Roles and picks are registry facts; the main acquisition is the default role set | 03, 05, 13 §5.3 | C19 |
| D22 | Results are first-class objects | 05, 13 §5.5 | C21 |
| D23 | The harness is replaceable and the contract is the product | 08, 13 §5.8 | C23; six-week pilot |
| D24 | Transcripts are inside the custody boundary | 08, 13 §5.8 | C24; 90-day default; C38 |
| D25 | Local first, node optional | 01, 14 §3.2 | C33, C34 |
| D26 | The pack is the common data model | 04, 14 §6 | C26; D30's vocabulary seeds |
| D27 | Disclosure levels and safe outputs at the door | 05, 14 §3.4 | C27, C28; k of 5 and 10 |
| D28 | Federated requests are review items | 05, 14 §6 | C29 |
| D29 | Compute travels, data stays | 09, 14 §3.5 | C30, C31 (the executor waits for Amsterdam) |
| D30 | The clinical timeline is core registry | 03, 15 §8 | C35 |
| D31 | The name stays NILS; the federation is not named (Yggdrasil withdrawn) | 10, 14 §7, 15 §10 | |

D13 to D19 were proposed by the review ([12](12-review-devils-advocate.md)
§5), D20 to D24 by the query and agent study
([13](13-query-and-agent-study.md) §6), D25 to D29 by the federation
design ([14](14-federation.md) §6) and D30 by Nima
([15](15-ratification.md) §8); all were ratified on 2026-09-02, D13 as
amended by C35 and C36. Ratified with them: the license (10), the repository restart
R1 to R9 and the freeze F1 (15 §5), the six-week agent pilot and the 90-day
transcript default (15 §1), and D27's defaults of k = 5 and 10. Two questions for
Vienna and Amsterdam are listed in 14 §7 and gate nothing before Wave 7. D31, the
name, was decided by Nima later the same day (15 §10). Next ids: C39 and D32.

## Where this came from

The record was written between 2026-09-01 and 2026-09-02 in the private v0
repository and moved to `kineuro/nils-design` the same day, history intact. This
copy is refreshed from it whenever the record is amended; the commit that refreshes
it names the amendment.

## What this record is not

Not a spec. Interfaces are sketched here only far enough to record a decision; each
wave in [11-order.md](11-order.md) starts by writing the real spec for its
slice, in the code repository.
