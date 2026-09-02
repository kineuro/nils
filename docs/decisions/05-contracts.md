# 05 — The engine's public contracts

Everything outside the engine — our apps, other people's tools, agents — meets NILS
here and only here. Contracts are versioned independently of the engine build and
declared by `GET /api/capabilities`. This doc records what each contract is *for*;
each gets a real spec in its build wave.

## 1. The HTTP API

Principles over routes: resource-shaped, versioned, and **job-based for anything
heavy** — a POST answers 202 with a job id; progress and results are read from the
job. There is exactly one door per operation (v0's parallel camelCase UI door and
snake_case CLI door, with their two behaviours, are the anti-pattern this replaces).
Server-sent events exist for progress display, never as the execution context.

One door, two tempos (C22 in [13](13-query-and-agent-study.md)): beside jobs there
is a bounded **synchronous** path with a row cap, a short timeout and cancellation
on disconnect, because a notebook that only submits jobs feels dead. Previews,
affordances and small questions use it; digests, exports and pipelines use jobs.
Both run the same AST through the same executor. A request that leaves the node
([14](14-federation.md)) is always a job: remote is never synchronous.

Added by the federation design ([14](14-federation.md) §3.6, ratified 2026-09-02):
`GET /api/capabilities` also reports the loaded pack versions, the registry epoch
and the federation endpoint when configured (C26); and any result handle can be
read as a **disclosure projection**, `boolean | count | aggregate | record`, with
small-cell suppression (k per deployment, complementary suppression so totals do
not leak a suppressed cell), optional rounding and binning of dates and ages
applied by the engine (C28). Projections are a local feature first (a student, a
teaching session, an external reader) and the only thing a peer ever receives.

## 2. The query AST and semantic catalog (D5)

The Metabase lesson, kept: **a query is data** — a JSON AST of predicates, joins
implied by the catalog, no SQL from clients, parameterized by construction.

- The **engine executes the AST**. One implementation of "what does this question
  mean", governed, optimized, and identical for the notebook, the CLI
  (`nils query`), MCP tools, and any script.
- The **semantic catalog** is served by the engine: entities, fields, low-cardinality
  value sets (for filter chips), and the human descriptions that v0 scattered across
  `db_guru` and the `nils-data` skill. The catalog is *the* schema knowledge; nothing
  else re-documents the schema.
- A **saved selection** is a named, versioned AST (see [03-registry.md](03-registry.md)) —
  the v1 cohort. Selections are addressable everywhere data is chosen: export,
  pipelines, segment works, review scopes.
- The TS `@nils/ast` package (zod types + manipulation helpers) exists for the
  notebook UI's editing experience, not for execution.

What the study of Metabase's source and the live agent traffic added
([13](13-query-and-agent-study.md) §5, amendments C17 to C21, accepted 2026-09-02):

- **Shape**: stages; every clause `[op, {opts}, ...args]` with a mandatory options
  map; refs as `["field", {opts}, path]` with catalog name paths, never ids or
  UUIDs; bucketing in ref options; parameters beside the AST, not in it; integer
  `ast_version` upgraded on read; a JSON Schema generated from the engine's schema
  and published; a structural repair pass with a fixed error taxonomy for agent
  input. One external dialect only.
- **Joins implied by the catalog** hold for the registry's tree (instance → stack →
  series → session → subject) and are single-hop; the many-to-one sides (identifiers,
  clinical events, cohort memberships) are explicit clauses with their own semantics.
- **Grain** is declared per stage (`subject | session | stack | instance | event`)
  and changes only through `summarize` or `pick`; counts name their grain, shares
  name their denominator. Temporal windows (`nearest`, `within`, `pairs`, `age_at`),
  set algebra on selections, `values` sources with an identifier namespace,
  identifier projection and derived fields (resolution, protocol fingerprint) are
  clauses.
- **Roles and picks** are catalog objects: a role is a predicate over the axes,
  image-type tokens and parameter windows; a pick yields one stack per session-role
  by an ordered preference list. The main acquisition of C8 is the default role set.
- **Affordances** come from the engine, keyed by `(ast, stage)`: `options`,
  `describe`, `preview`, `diagnose`. Field records carry semantic type, visibility
  with `sensitive`, fingerprints, remaps and `ai_context`; the vocabularies are
  catalog entities with descriptions and examples; curation survives re-sync.
  The same `describe` and `diagnose` serve every judging step's knobs and its
  diagnostics report (C37), and `GET /api/custody` serves the custody table of
  C38.
- **Visibility gains a federation axis** ([14](14-federation.md), C27):
  `local` is the default for free text, paths, exact dates and identifiers;
  `federated` is an allowlist a node maintains per field; `sensitive` never
  crosses. Validation of a request from a peer fails at the catalog if the AST
  touches a field outside the allowlist, so a local-only field never leaves.
- A **result handle carries provenance for reproduction** (C26, extending C21):
  the node, the pack version, the registry epoch and, when it is a projection,
  the disclosure level and the suppression applied.
- **"Parameterized by construction"** is a compiler invariant (the executor never
  inlines a value), stated as such; the AST itself holds literals.
- A run returns a **result handle** (id, name, grain, columns, row count, hash, AST
  version, provenance), paged by continuation token; exports and send-to consume
  handles. This is the concrete form of D14's staged result versions.

## 3. Review items (D7)

The one primitive behind "agentic at every decision point":

```
review_item: id, kind, scope (stack/series/subject/run), evidence (structured,
             from the emitting stage), proposal, confidence, status,
             decided_by (human | agent | rule), decided_at, audit trail
```

- **Emitters**: classification uncertainty, body-part disagreement, parse failures,
  identity-linkage proposals, pipeline QC metrics, BIDS collisions — any stage with
  a judgement it is not certain of. Under D28 and C29 (ratified 2026-09-02):
  a request from another node is an emitter too, as kinds `federation.request`
  (an AST at a disclosure level) and `federation.run` (a pipeline on a selection),
  whose evidence is the signed request, the requesting `user@node`, the purpose
  and the agreement it cites. The policy for these kinds is per federation, level
  and entity: counts above the floor from a trusted node may auto-approve;
  `record` always waits for a human. No second approval machinery exists.
- **Consumers**: the web UI review queues, `nils review` in the CLI, and agents via
  MCP — the same queue, the same evidence, the same apply call.
- **Policies** make it safe: per kind, a policy says what may be auto-resolved, by
  whom, above what confidence, with what audit. "Agent may confirm classifications
  above 0.95 and must only *propose* identity links" is configuration, not code.
  Policies default to human-only; loosening them is an explicit, logged act.

This generalizes v0's eight review surfaces, which come in three shapes (per-item
draft-then-confirm; cohort snapshot with stage, commit, undo and drift signature;
write-through), into one spine, and it is the reason adding intelligence to a new
stage costs one emitter and one policy line. The snapshot shape is a batch decision
on a result set, not N items, so the spine needs *staged result versions* with a
commit, grouped items, bulk decisions and emission thresholds per kind, and decision
precedence (human > agent > rule) with decisions that survive re-classification:
C5 and C15 in [12](12-review-devils-advocate.md). Today 435k of 518k stacks are
flagged, most of them for "no keyword evidence" rather than uncertainty; a queue
built without those rules is unusable.

## 4. MCP

The engine ships an MCP server exposing, at minimum: catalog browse, AST query
execution, selection CRUD, job submit/status/cancel, review-item list/get/apply
(policy-checked), pipeline list/run. Every tool is a thin veneer over the API — MCP
adds *no* capability, only reach: nils-agent, Claude, or any future client gets the
same governed surface. Agent-specific knowledge (schema guidance, query recipes)
ships as skills alongside, not as divergent tools.

The shape, fixed by what the Flue client can consume and by what Metabase's server
does (C22 in [13](13-query-and-agent-study.md) §4 and §5.6): tools are generated
from the API's endpoint metadata and dispatched in-process, which makes the veneer
rule mechanical; streamable HTTP; **tools only** (the catalog, review items and
pipelines are tools, never resources or prompts); short stable snake_case names
with descriptions, because the description is the only routing signal; a
`description` on every node of the AST schema, because validation text is what the
model reads; compact JSON results bounded server-side (row caps, cursors, byte caps)
since the client truncates nothing; `isError` with actionable text; one bearer token
per request carrying the acting user; job submit with a client idempotency key,
because tools re-execute on re-attempt; a tool list that is stable within a
conversation; token scopes that only narrow it. A validated AST gets a **query
handle** that execute, save and send-to consume, so a model never re-emits what it
already got right. Minimum set: `catalog_search`, `catalog_describe`,
`catalog_values`, `query_options`, `query_describe`, `query_validate`,
`query_execute`, `result_page`, `selection_get/save/list`,
`job_submit/status/cancel`, `review_list/get/apply`, `pipeline_list/run`.

The node daemon, when present, serves a second MCP server of exactly this shape
([14](14-federation.md), C34): `federation_list`, `federation_catalog`,
`federation_profile`, `federation_query` (an AST plus level, returning a job) and
`federation_result`. The engine's own tool list does not change, so an agent on a
deployment without a node sees nothing.

## 5. Authentication (D8)

Three modes, engine-enforced:

- `off` — the laptop mode. No middleware, like v0.
- `token` — static API tokens for service callers and simple deployments.
- `oidc` — the server mode: the engine validates OIDC (our deployment: Authentik
  behind Traefik forward-auth); groups map to roles; review-item policies can name
  roles. NILS stores no passwords, mints no sessions, and owns no user table beyond
  a cache of subject claims. nils-identity is retired with honors — its API-token
  and thin-claims designs inform the `token` mode.

In `oidc` mode the MCP endpoint also publishes OAuth 2.0 protected-resource
metadata (`/.well-known/oauth-protected-resource`) pointing at Authentik, so
third-party MCP clients such as Claude Desktop connect through the same identity
provider without a NILS-specific client (C22). The agent app instead mints a
short-lived JWT from its own OIDC session and sends it per request; the engine
verifies it, applies the per-kind review policies to that subject and records it
in the audit log. `token` mode stays for CLIs and services.

Federation adds no fourth mode ([14](14-federation.md) §3.3, C30). A node
is a keypair with a signed manifest; peers are pinned explicitly, the `known_hosts`
way, and revoked by deleting a line. A request is signed by the sending node at the
application layer, so it is valid over a mesh, a relay or plain HTTPS alike. The
user behind it crosses as **claims** signed by the home node (`node`, `subject`,
`groups`); the receiving engine maps `(federation, node)` to a local role such as
`federated-reader` and never honours a remote role. The audit principal is
`user@node` at both ends. The node daemon itself is a `token`-mode service caller
of its own engine, bounded by that same reader role, which is the whole of what a
peer can ever reach.

## 6. Events

A lightweight event stream (job progress, review-item creation, stage completion)
for UIs and automations to subscribe to. Delivery is best-effort display plumbing;
anything that matters is in the registry, so a missed event never means lost truth.
