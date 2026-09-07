# 17 — Wave 4 reframed: the engine completes, then the two apps

Written 2026-09-06, after Wave 3 closed and after an eighteen-agent analysis of
v0's server, its clinical importers, its QC surfaces, the Metabase and Flue
references, the record's contracts and the v1 engine as built. That analysis
produced a thirty-slice plan for one wave. Nima's rulings below take it apart
into three, put the engine's debt first, drop the migration, and pull the two
apps out into waves of their own with a conversation in front of each. This
document is where the plan lives so it cannot drift, and 11-order.md points
here.

## 1. The rulings (Nima, 2026-09-06)

| | Ruling | What it changes |
|---|---|---|
| R1 | **The query and the agent are independent apps.** Each is optional, per the vision's third sentence: present, the engine acknowledges and uses it; absent, nothing is missing. Neither is part of the engine. | MCP, the notebook and the AST's *editing* leave the engine wave. The record's "parallel agent track" becomes a wave. |
| R2 | **Query before agent.** The engine and the query must work with no AI at all, and become "intelligent" only when the agent is present. | Wave 4b is the question; Wave 4c is the assistant. |
| R3 | **Versioning and private tags first, with the bugs.** We now know what to do for both (see 16 and the surveys of 2026-09-05/06); they and the faults the analysis found land at the start of the engine wave, before the registry layer. | Slices 1-5 of Wave 4a. |
| R4 | **No migration.** This is fresh development. At the end, v1 builds the new registry and workflow by running over the archives and the clinical CSVs; nothing is copied out of v0's databases. | The two migration slices are gone. The importers and `nils digest` *are* the migration, so they carry that weight. |
| R5 | **The engine's selection stays simple.** A cohort; subjects by code or by any identifier the registry can resolve; subject-sessions; stacks by id or by their classification axes. That is an enumeration plus what a stack *is*. Anything a study means by inclusion and exclusion is the query app's, later. | Extends what Wave 3 built (`--select`, see [[release-selection-is-not-a-query]]); no AST in the engine wave. |
| R6 | **The agent is a self-sufficient service**, installed and updated as easily as the engine (a binary, or the npm/node shape), covering local and enterprise model providers and the specialised agents for each kind of work: the engine's knobs, the query, jobs. It knows where the knobs are and what context makes tuning them nearly deterministic. | Wave 4c has a provider discussion and a knob-context design in front of it. |
| R7 | **The query is data, the Metabase way**, so that a person can compose a set of filters that translates to a highly complex question, and so that an agent can build the same question from natural language. Metabase is cloned and read before its wave; so are v0's own notes and the deerflow threads. | Research checkpoint before Wave 4b. |
| R8 | **Expansion is by app.** A study-management app later uses the agent, or extends it, without touching the engine. | The contracts are the product; the apps are clients. |

And two things Nima asks for that shape the document rather than the code: **an in-depth conversation before each app wave**, and a plan written so that nothing important is forgotten along the way.

## 2. The three waves

The record's Wave 4 becomes three, and the record's Wave 5 (nils-query MVP) is absorbed into the second. Waves 6, 7 and 8 keep their numbers and content.

| wave | name | what it is | opens with | closes at |
|---|---|---|---|---|
| **4a** | **the engine completes** | versioning at the right grain, private tags ingested, the faults fixed; the registry layer v0 keeps in its metadata database; simple selection; jobs, the principal and the audit; the review spine; the doors and the contracts; the gate | the spec, written from this document | `nils` alone can build a registry from an archive and a clinical CSV, select, review, release, hand over, and be driven through one API by a second process |
| **4b** | **the question** | the query AST and its executor in the engine (D5 stands: the engine executes, the app edits), the semantic catalog, result handles, affordances, the notebook app | **a talk and a research phase** (§4) | a real study's cohort defined as a selection and released without a hand-written manifest; the ten families expressible; the 28 gold tasks reproduce their hashes |
| **4c** | **the assistant** | the agent service, its providers, its specialised agents, the knob contract (C37), MCP wherever the talk puts it, the evals | **a talk and a research phase** (§5) | the agent pilot's exit criteria (C23, 08): the ten families through the draft-selection loop on a local and a hosted model, gold hashes reproduced, fewer turns than v0 |

Names are open (§8). Until they are settled the record keeps `nils-query` and `nils-agent`.

## 3. Wave 4a — the engine completes

Ordered by R3 first, then by dependency. Each slice is one merged pull request with its own tests, the granularity Wave 3 proved over thirteen.

### First: the debt, the ingestion, and the faults (R3)

| # | slice | what lands | gate |
|---|---|---|---|
| 1 | **The manifest at stack grain** | The release's bookkeeping moves from a row per file per version to a **current state per stack plus a change log** (`release_stack` updated in place, `release_move` as history), with the five outcomes diffed **in SQL** rather than in memory. A per-file digest becomes a per-stack roll-up; `release_file` is dropped or made opt-in. Measured against today on the same 150k and 1M-file corpora. | Memory flat in the number of files; rows per dataset 518k-ish total, not per version; both layouts; the handover still verifies. |
| 2 | **Private elements ingested** | Digest reads private elements **by creator and offset**, into a catalogue the pack declares (which elements, from which creator, with what meaning), so fingerprint and classification can use them. `nils private` grows: orphans reported by group, and a private dictionary so values come back as something other than `UN`. | The three surveys' per-acquisition candidates are readable by a pack; a Siemens 0051 block is read; a GE `GEMS_PARM_01` block is read; nothing is ever kept whole. |
| 3 | **The allowlist from the surveys** | The 139 candidates (varies per acquisition, printable, short) become pack entries after Nima strikes what he does not want; the pack states the vendor coverage it has. | The MS, nmosd and mix surveys each lose no candidate a person kept. |
| 4 | **The faults** | (a) `nils session --anchors` reads a raw DATE (`main.rs:3414`) and fails on Postgres; (b) the CLI tests have **no Postgres half** at all (1,872 lines, zero references to the DSN); (c) `Dialect::text_of_qualified` is made unavoidable: projections resolve through `&Column` and a debug assertion fires when a Date/Time/Timestamp/Json column is selected raw; (d) **multi-valued axes stop being comma-joined strings** (`classification_axis.value`, v0's `classification_string` mistake ported deliberately, which is why a role match is four `LIKE` patterns); (e) `participants.tsv` carries nothing (`run.rs:909`), a fault that stays until the clinical layer exists but is named here. | Whole CLI suite green on both backends; a role match is an equality; a raw DATE projection cannot be written without a test failing. |
| 5 | **`contracts/` stops being empty** | The three contracts already overdue get their skeletons with a version each: `pack/` (the pack format, which is data and has been since C11), `review-item/` (the schema), `openapi/` (an empty document that the doors fill in). The rule that a contract version changes only by a pull request that says so. | A contract test exists and is green, even if it tests little. |

### Then: the registry layer v0 keeps in its metadata database

| # | slice | what lands | gate |
|---|---|---|---|
| 6 | **The clinical schema** | Cohorts and memberships; diseases and disease types; observation types; **events in v0's one event-attribute-value shape** (139,033 of them over fifteen types in the live archive); subject demographics. Subjects, identifiers, id types and linkage already exist (Wave 1). The **vocabulary is pack data** (observation types, disease types, id types), because it is knowledge about a clinic. | The schema holds v0's live counts as shapes; every table has an owner in the custody table (C38). |
| 7 | **One declarative importer** | v0's thirteen importers (5,807 lines) become **one**: a mapping file names the target, the columns, the parsers and the key; preview then apply; idempotent on the key; a correction path (soft delete, supersede, or review-mediated retraction: open question Q6). This is the migration under R4, so it is proved on the real CSVs. | Every one of v0's thirteen import shapes runs through it; a re-run changes nothing; a corrected row is traceable. |
| 8 | **`Anchor::Event`, and the join the release needs** | The session scheme's clinical anchor (Wave 3 §5 declared it and refused it); the **nearest event to a date**, which is the one temporal function the engine needs for the release and the gate. General windows (`within`, `pairs`, `age_at`) are the query wave's. | `nils session list --scheme` with a diagnosis anchor; the EDSS nearest each scan computed from the registry. |
| 9 | **Clinical export in the release** | `participants.tsv` and `sessions.tsv` carry what the policy allows: age at session, sex, and the nearest observation the release names, under the date policy (an age computed before the birth date goes, §8.3). | **Wave 3's deferred bar 10**: the EDSS nearest each scan is the same computed from the registry and from the tree, under every date policy. |

### Then: selection, jobs, the principal, review

| # | slice | what lands | gate |
|---|---|---|---|
| 10 | **Simple selection (R5)** | `--select` and its file take a **cohort** (by name), **subjects by code or by any identifier the registry resolves** (v0's manifest resolver did this across every id type, kept), **subject-sessions**, **stacks by id**, and **stacks by classification axes** (equality on pack axes, which is "what a stack is"). Nothing else, on purpose. | A release of "cohort X, T1w MPRAGE stacks only" from one file; an identifier of any type resolves; `nils select` prints what a file would select without releasing it. |
| 11 | **One job model** | The two ~60-line claim paths (digest's and classify's) become one: per-kind claims, a queue, a worker, a pool. Every long verb is a job; a job is resumable because nothing is in flight (principle 4). | Every verb that runs longer than a second is a job row with progress; `nils jobs` lists, cancels and resumes. |
| 12 | **The principal and the audit log** | Who did what: a principal on every decision, import, release and handover (today `actor` is a string from `$USER`), and an audit log that is a table. The doors will map an OIDC subject to it; until then it is the local user. | Every write that changes a judgement names a principal; the audit read side answers "what did X do in September". |
| 13 | **The review spine, measured first** | First **turn the pack's emission thresholds up and count what survives**, before building grouping: v0 flags 435k of 518k stacks. Then grouped items, bulk decisions, `review apply`, staged commits with undo, decision precedence (human > agent > rule) surviving re-classification (C5, C15). v0's three shapes served by one spine. | The queue for the reference corpus is readable by a person in an afternoon; a snapshot-shaped decision is one commit, not N items. |

### Then: the doors

| # | slice | what lands | gate |
|---|---|---|---|
| 14 | **`nils serve`** | The HTTP server: **one door per operation**, resource-shaped, versioned, jobs answer 202 with an id, progress read from the job, SSE for display only. The route inventory is the OpenAPI document of slice 5 filled in. Selection, release, review, jobs, custody, `GET /api/capabilities` with the pack versions and the registry epoch (C26). **No AST endpoint yet**: that is 4b's. | A second process drives every verb the CLI has through the API and gets the same answer (the contract test, half of the wave gate). |
| 15 | **Auth: off, token, oidc** | The three modes (D8); groups to roles; no user table beyond a claims cache; the audit principal becomes the OIDC subject. Federation adds no mode (C30) and the primitives that are cheap now ride here: `user@node` as a principal shape, the registry epoch in capabilities, `local`/`federated`/`sensitive` as a field attribute the catalog will read (C26-C30, C33). | Authentik in front of `nils serve` on the build host; a token-mode service caller bounded by a reader role. |
| 16 | **The gate** | Everything above, on the reference corpus and on a real cohort re-digested from the archive (R4): the registry built from raw plus CSV; the release with clinical export; the handover; the CLI and a second process through the same doors; the budget. | Green in CI and on the build host. |

Sixteen slices. Slices 1-5 can start now; 6-9 are one chain; 10-13 depend on 6 only where they touch cohorts; 14-16 come last.

## 4. Before Wave 4b: the talk and the research

Wave 4b does not open until these are done, and the spec is written from them.

**Research (read before we talk, so the talk is about choices and not about facts):**

- **Metabase, cloned and read again** at `~/Projects/ref/metabase`, guided by 13 §3 and by the eighteen-agent reading: the MBQL stage and clause shape, the query processor's layers (parse, normalise, resolve, plan, compile, execute, page), the metadata provider, how "what can I do next" is computed, and how the language is versioned. The reading found what **not** to copy: a generic join resolver (ours is ~10 declared edges over one tree), a twelve-attribute field record (ours needs five plus curation), and a cached dropdown where the pack already declares the axis values.
- **v0's own document on the query design** (`docs/nils/suite-plan.md` §2 in the v0 tree) and 06-app-query.md, which carry the notebook's shape nearly intact.
- **The deerflow threads** in the nils-agent database on the production host: how a real question becomes complicated (13 §2 read 38 of them; the "user turns only, counts and question texts" rule holds). The ten families and the 28 gold tasks are the acceptance test (C16).
- **The v1 store**, read as a compilation target: `Row` is positional and seven crates depend on it (header or side-channel?); the store is blocking (pool, or async rewrite?); D3's "expressible on both backends" against a compiler that wants window functions and JSON predicates; DuckDB is named in D3 and absent from the tree.

**The talk must settle:**

1. The engine executes the AST (D5) and the app edits it; confirm that stands under R1.
2. **Session materialisation**: a rebuildable cache keyed by `(scheme, epoch)`, or a post-SQL grouping stage that never pushes a session filter down. And whether the cache key includes the selection (03 says a scheme belongs to a selection).
3. **The pick clause**: reads stored `pick` rows (Wave 3's weighted score) or C19's ordered preference list; and whether a pick fans out to one row per session-role or n rows for a Dixon family.
4. **Cohort**: a membership fact, a saved selection, or both with one canonical (Q9 of the analysis; slice 6 needs an answer before then, so the *fact* is built in 4a and the talk decides whether a selection can also be one).
5. **Result handles under the versioning lesson**: current state plus change log for saved selections; what a handle carries (node, pack version, epoch, disclosure level); retention.
6. **Affordances**: which of `options`, `describe`, `preview`, `diagnose` the notebook needs first, and how they serve C37's knobs later.
7. **"Expressible"** for C16: validates against the schema, or returns a right answer on a synthetic registry; and whether a family that needs a later clause fails or passes as deferred-by-clause.
8. **The sync path**: row cap, timeout, byte cap, page size, as contract constants or deployment configuration; and whether gold tasks are forbidden on it so a capped result never hashes a truncated set.
9. **The epoch**: advances on decisions and selection writes, or only on ingest and classification.
10. **Order-dependent columns** (`sequence_name` with up to 26 spellings in one series, the stack signature): declared non-reproducible, or the writer made deterministic first.
11. **The name** of the app (§8).

## 5. Before Wave 4c: the talk and the research

**Research:**

- **Flue, re-read** at `~/Projects/ref/flue` (13 §4, 08): what its client requires of a server, transport, tool discovery, schema handling, size limits, error shape, auth. The eighteen-agent reading found the veneer rule to be mechanical and the hard parts to be idempotency on re-attempt, server-side bounding of results, and the OAuth resource metadata.
- **The nils-agent pilot's traffic** (13 §2): where turns were wasted, where the grain was pinned by a human, where the agent fell back to `LIKE` on a blob because the tokens it needed were not structured.
- **The knobs**: every judging step's tunables as the engine exposes them (C37), and what context makes tuning them nearly deterministic: the diagnostics report, the corpus cases, the evidence rows.
- **Providers**: what local (Ollama-class) and enterprise (Azure, Bedrock, a hospital's gateway) providers need from a service that has to run inside a hospital network, and how keys and endpoints are configured without the engine knowing.

**The talk must settle:**

1. **Where MCP lives.** The record (05 §4, C22) puts an MCP server in the engine as a veneer over the API. R1 says the agent is an app. Either the engine keeps a thin veneer so any third-party client can reach it, or the agent app serves MCP and the engine has no AI-shaped surface at all. Both keep D1.
2. **The service's shape**: one binary beside the engine, or a node-installed package; how it is updated; how it discovers the engine (capabilities) and how the engine acknowledges it (absence is silence).
3. **The specialised agents**: one per kind of work (knob tuning, query composition, review triage, job supervision), what each is given as context, and what each may write (review-item policies: propose only, or confirm above a confidence).
4. **Providers and secrets**: the provider abstraction, local first; where credentials live (never the registry); what leaves the host when a hosted model is used, under which policy.
5. **The evals** (C25): built before the pilot so the pilot can be judged; the gold tasks as the benchmark.
6. **The pilot's exit criteria** (C23), restated against the new shape.
7. **The name** (§8).

## 6. What the analysis found that must not be lost

Concrete, verified in the tree on 2026-09-06, and each with a home above:

- `main.rs:3414` projects a raw DATE and reads it as text: fails on Postgres → slice 4a.
- `crates/nils/tests/cli.rs` never sets `NILS_TEST_POSTGRES_DSN` → slice 4b.
- `classification_axis.value` is comma-joined for multi-valued axes; `run.rs:1525` matches a role with four `LIKE` patterns → slice 4d.
- `run.rs:909` builds every `Participant` with `extra: BTreeMap::new()`: the released tree carries no clinical value at all → slice 9.
- `contracts/` holds a LICENSE and a README → slice 5.
- `nils release --select -` already parses enumeration JSON from stdin; the analysis's "handle retrofit" is deleting flags, not designing → Wave 4b, cheap.
- `Anchor::Explicit` already works and only `Anchor::Event` refuses: session materialisation **does not depend on the clinical layer** → the two chains in 4a are independent.
- `Dialect::text_of_qualified` already exists with a doc comment naming the exact failure; the Postgres rule is missing discipline, not a missing mechanism → slice 4c.
- Two ~60-line job-claim paths → slice 11.
- The federation primitives are riders (one enum variant in the field record, one property in the resolver, four columns on a result row, two declared-and-unemitted review kinds), not a slice → slice 15 and Wave 4b.
- Retention has no owner for any of: result handles and pages, staged versions, query handles, selection logs, catalog curation, the session cache, tokens, the job, event and audit logs, importer uploads → one retention table, answered store by store, in the 4a spec (custody, C38).
- Acknowledgement ("the machine was right and I checked") either writes a decision row and inflates the count of human-authored values, or needs its own home → slice 13.
- The pixel and DICOM door: v0's is a sequential-integer address with no cohort scoping, `Access-Control-Allow-Origin: *`, and an unbounded never-invalidated cache of patient images. Wave 6, with a **declared absence** in 4a: the visual review kinds stay in v0's UI until then, which extends the dual-run window.

## 7. What is deferred, and to where

| deferred | to |
|---|---|
| The AST, the executor, the catalog, handles, affordances, the notebook, `@nils/ast`, the sync path | Wave 4b |
| MCP, the agent service, providers, specialised agents, knobs as contract (C37), evals (C25), disclosure projections (C28, first consumer is a reader on a projection) | Wave 4c |
| The web UI rebuilt on the job/queue model | after 4b, where a frontend engineer is engaged and the contracts have stopped moving; 4a gates on a second *process*, not a UI |
| The pixel/DICOM door, the visual review kinds, the body-part model registry, decisions as labelled datasets (D15, C7), v0's five QC products | Wave 6 |
| `pairs`, `age_at`, set algebra, derived fields, protocol fingerprints | Wave 4b, staged (C18) |
| Migration of v0's databases | **nowhere** (R4) |

## 8. Naming

Both apps are placeholders. What each is: the first lets a person **ask** the registry a question and keep the answer as a cohort; the second is an **assistant** that knows the engine, the question and the jobs. Candidates to choose from in the talks: `nils-ask` / `nils-question` for the first; `nils-assist` / `nils-agent` kept for the second. The engine's own name for what it acknowledges should be the capability, not the product: `capabilities.ask`, `capabilities.assist`.

## 9. Where to start

Now: the Wave 4a spec, written from §3, with §6 as its do-not-forget list and the 4a questions (retention, the correction path, acknowledgement, the multi-valued axis storage) settled in it. Slices 1-5 start when the spec merges; the private-tag candidates (slice 3) wait on Nima striking the list.

**Wave 4b opened 2026-09-07.** The talk of §4 was held as a study
([18](18-wave4b-the-ask.md)): the eleven items it had to settle are answered in
18 §4, the spec is `docs/specs/wave4b-the-ask.md` in the public repository with
thirteen slices, and the app's name (§8) stays open.
