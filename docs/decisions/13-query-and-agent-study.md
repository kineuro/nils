# 13 — nils-query and nils-agent, checked against Metabase, Flue and what actually ran

The second verification pass, 2026-09-02, focused on the two apps whose design the
folder had borrowed from elsewhere: nils-query (D5, doc 06) is "Metabase-style"
and nils-agent (D11, doc 08) is "a Flue app". I cloned both references and read
them as source, not as marketing, and I read the live nils-agent database on the production host for
what researchers actually asked. The question for each finding was the same as in
[12](12-review-devils-advocate.md): does the folder still hold, and what is missing.
Additions are registered as amendments C16 onward and decisions D20 onward in
section 6; all were ratified on 2026-09-02 ([15](15-ratification.md) §7 and §9).

The rule this doc answers to is unchanged: nils-engine is the core, every app stays
independent, and the query door is one door for humans, scripts and agents alike.

## 1. What I read

| Source | What it is |
|---|---|
| `~/Projects/ref/metabase` | Metabase OSS at `9bf3b136` (2026-09-01). Clojure: `src/metabase/lib/` (the query AST, 35.6k lines, 130 files), `query_processor/` (15.5k), `driver/sql/` (16.8k), `mcp/`, `agent_api/`, `agent_lib/`, `metabot/`, `llm/`; frontend `metabase-lib/` (12.2k) and `querying/` (42.5k). The whole backend is 298.8k lines in 147 modules, plus 50.4k enterprise. |
| `~/Projects/ref/flue` | Flue at `832ad2e` (2026-08-27): 28 workspace packages, all 2.0.3, Apache-2.0, 69k lines of TypeScript, the runtime 41k of them. |
| nils-agent database on the production host | 38 conversation threads (LangGraph checkpoints; I read human turns only, truncated), 28 ratified gold tasks, 27 scored teacher trajectories, the benchmark, evolution and fine-tune run tables. Counts and question texts only, never result rows. |
| Live registry on the production host | Counts behind the proposals in section 5 (candidates per session and role, identifier namespaces, clinical observation types, protocol spread at 7T). |

Nothing in either clone was modified, built or run. No subject or study identifier
from the live data appears in this doc; the use cases are generalized.

## 2. What actually ran

### The shape of the traffic

| Measure | Value |
|---|---|
| Threads | 38: 33 registry questions, 2 general document tasks ("a markdown document with charts explaining a distribution"), 3 empty or one-shot |
| Human turns | 104, median 1 per thread; the 8 threads with 4 or more turns carry 69 turns and 277 of the 490 tool calls |
| Tool calls | 490: 213 SQL executions, 131 schema lookups (`db_guru`), 50 file presentations, 46 shell, 27 todo lists, 23 file reads/writes/edits/globs, 4 clarifications |
| Per human turn | 2.0 SQL executions and 1.3 schema lookups |
| Longest threads | 18 turns, 50 tool calls, 705 checkpoints; 17 turns, 65 tool calls, 823 checkpoints; 9 turns, 32 tool calls, 409 checkpoints |
| Models | teacher sessions on Claude Opus 4.6, 4.7, 4.8, Sonnet 4.6 and Opus 5; 7 threads on five different local GGUF models, one of which spent 53 tool calls on a single question |
| Trajectories | 27 scored by a teacher: 10 correct, 16 suboptimal, 1 unscored; quality 0.72 to 1.0, mean 0.88; error types `inefficient_query` ×6, `wrong_column` ×2, `no_schema_discovery` ×2, and one each of `wrong_count_grain`, `incomplete_filtering`, `ambiguous_age_window`, `ambiguous_denominator`, `ambiguous_requirement_handling`, `incomplete_answer`, `wrong_question` |
| Gold tasks | 28 ratified and frozen, 25 distinct question texts, all `result_set` checks with a canonical result hash |
| Optimization loop | one benchmark run completed (28 tasks, k=1); one evolution run failed; one fine-tune (12 samples on a 35B MoE base) failed |

### The ten question families

Written as the thing a researcher asked, generalized.

1. **Cohort membership and overlap.** Cohorts with owner and patient counts; the
   biggest cohort with subjects and sessions; subjects in more than two cohorts;
   subjects shared between two named cohorts, with everything known about them. A
   third of subjects are multi-cohort, so "are they all from cohort X" was corrected
   by the researcher mid-thread ("they can be in more than one cohort").
2. **Identity and identifiers.** Every other identifier a subject has; "use the
   cohort's id, not the subject code"; a pasted list of study identifiers resolved to
   subjects (22 tool calls); an uploaded CSV of subject-sessions checked against the
   registry. Of the 5,319 subjects with other identifiers, 2,858 carry two namespaces,
   1,337 three and 10 more, so the "why several ids under one column" confusion in one
   thread is structural, not a data error.
3. **Session-grain composition.** Sessions that have both a 3D FLAIR and an MP2RAGE at
   7T in two cohorts; sessions with both a main T1w MPRAGE and a main T2w FLAIR, both
   3D; "give me unique subject-session". The same question was asked in five threads
   on five models, and the counts disagreed between runs (242, 244, 271) until the
   human pinned the grain.
4. **The representative stack per session and role.** Once a session qualifies, the
   researcher wants exactly one stack per role, chosen by rules given one at a time:
   never an MPR; original over derived; the field-map-corrected one; the one with more
   slices; the "mean" (or "result") image-type variant of the SWI; the phase image of
   the last echo; sagittal 3D FLAIR, else axial 2D. In the live cache v0's
   `main_acquisition` label still leaves 155 session-roles with two candidates and 13
   with three to five; for MP2RAGE a quarter of sessions have more than one candidate.
   The tie-breakers the researcher used live in the DICOM image-type tokens
   (`DERIVED`, `MPR`, `REFORMATTED`, `AVERAGE`, `PROJECTION IMAGE`: at least 68k
   stacks), not in the six classification axes, which is why the agent fell back to
   `text_search_blob LIKE` and got the token wrong once.
5. **Acquisition parameters and protocol homogeneity.** Slice count and resolution as
   a "1.0x1.0x0.5" string; the most common resolution of T2-weighted images; sessions
   in a cohort with a 1x1x2 mm T2; whether every session's MP2RAGE, 3D FLAIR and SWI
   share one protocol (scanner model, physics parameters, resolution) across sessions
   rather than within one. At 7T the live data has 8 distinct parameter tuples for T2w
   SPACE over 155 stacks and 9 for MP2RAGE over 151.
6. **Clinical temporal windows.** EDSS within one year of a session; a "diagnostic"
   scan as one within six months of diagnosis (then "maybe combine diagnosis and
   onset"); session pairs four to five years apart, each with an EDSS in reach,
   choosing the closest; converters versus non-converters from an SP-transition event
   with the baseline five years before conversion; pregnancy deliveries and age at
   delivery; age at diagnosis; patients under 30 at first session. The registry holds
   17,792 EDSS observations, 2,695 diagnoses, 2,981 onsets, 724 SP transitions, 10,048
   treatments and 283 deliveries, all in one event-attribute-value shape.
7. **Demographics and data quality.** Per-cohort n, sex split, mean/sd/min/max age;
   subjects with no sex recorded; "you only showed me the pixel spacing".
8. **Treatment distributions.** Treatments across a sub-population with counts and
   percentages, where the denominator had to be asked for.
9. **Lookup and reconciliation.** Subject and session from a StudyInstanceUID; the
   path of the subject's raw folder; "count the files in it and check they all exist
   in the database"; the distinct free-text search blobs of a cohort at 7T; "tell me
   about the schema".
10. **The deliverable.** Almost every long thread ends in a CSV with a named column
    list: subject code, the cohort's own identifier, session date, stack id,
    orientation, acquisition type, base, technique, modifier, construct, slice count,
    resolution, sex, date of birth, age at session; sometimes one row per role per
    session. "Surface the query and the result", "you did not surface the result" and
    "choose a proper name" are recurring turns.

### What the failures say

- The dominant failure is not wrong SQL but **re-derivation**: every refinement
  ("also 3D", "add slice count", "add sex and age") restarted the query from scratch.
  That is where `inefficient_query`, the loop-detector trip in one thread and the four
  consecutive "try again" turns in another come from. A refinement is an edit to a
  selection, not a new question.
- **Grain** (`wrong_count_grain`, the 242/244/271 dispute) and **denominator**
  (`ambiguous_denominator`) errors are the AST's job to make impossible: a result
  declares its grain and a percentage names its denominator.
- **Vocabulary** confusion (SWI is a construct, not a base; MP2RAGE is a technique;
  "mean" versus "result" in the image type) is a catalog job: the six axes and the
  image-type tokens must be enumerated with descriptions and examples, and free-text
  search must be the last resort.
- **Schema discovery** was skipped twice in 27 scored runs, but 131 `db_guru` calls in
  38 threads say the opposite problem is real too: schema knowledge is fetched over
  and over because it is not part of the tool contract.
- **Silence**: "are you there" twice, two threads with a question and a single
  tool-less reply, one empty thread. The product needs visible run state and a
  first-class failure message; that is harness work, not modelling.

## 3. Metabase, read from the source

### The headline

Metabase has already built the thing docs 05 and 06 describe, and more of it than the
folder assumed. This snapshot stores questions as **MBQL 5** (`src/metabase/lib/`), a
staged AST; it defines an **external dialect** of the same AST with string name paths
and no UUIDs for LLMs (`lib/schema.cljc` `::external-query`), generates a JSON Schema
from it (`lib_be/json_schema.clj`), and ships in OSS an **MCP server**
(`src/metabase/mcp/`, streamable HTTP at `/api/metabase-mcp`), an **Agent API**
(`agent_api/`), a **repair layer** for agent-written queries
(`agent_lib/representations/repair.clj`, 2,861 lines), and the **Metabot** agent loop
with skills, an LLM provider registry (Anthropic default, OpenAI, Bedrock, vLLM among
others) and a 406-line query-authoring reference for the model. The question "one AST
for humans and agents" has an answer in this repo; the interesting parts are where
they reused their own machinery and where they did not.

### The AST

- A query is `{stages: [...]}`. Stage 0 has exactly one source (table or saved
  card); later stages consume the previous stage's output; native SQL is allowed only
  as a first stage. Index `-1` means the last stage everywhere.
- Every clause is `[op, {options}, ...args]` with the options map mandatory. Column
  refs are `["field", {opts}, id-or-name]`, and the options carry what makes a ref
  self-describing: `join-alias`, `temporal-unit`, `binning`, `base-type`, and
  `source-field` for an implicitly joined column. Bucketing and binning are ref
  options, not nodes.
- Filters, expressions (arithmetic, conditional, string, temporal, window) and
  aggregations (`count avg distinct count-where share median percentile stddev sum
  ...`) are typed: each argument slot has a type, and `diagnose-expression` returns a
  friendly sentence ("Types are incompatible: {0} expects {1} as the {2} parameter")
  instead of a stack trace.
- Joins are explicit (alias, conditions, strategy) **and** FK-implied implicit joins
  exist, but implicit joins are single-hop and an ambiguous FK is an error.
- Parameters and template tags live beside the AST, not in it; the query processor
  splices them into filters at preprocess time.
- The external dialect replaces ids with name paths
  (`["field", {}, ["Sample Database", "PUBLIC", "ORDERS", "TOTAL"]]`) and drops the
  UUIDs. Its own authoring reference says the missing `{}` and wrong identifiers are
  "the two most-violated rules".
- Versioning is by an integer `card_schema` (currently 20 → 24) upgraded on read.
  Metabase also still carries MBQL 4, a legacy converter on the execution path and a
  repair-input dialect: four dialects at once, which is the cost of not having started
  with the external one.

### "What can I do next"

`lib/core.cljc` (1,771 lines) is the affordance API the notebook is built on, every
function keyed by `(query, stage)`: `filterable-columns`, `breakoutable-columns` with
`available-temporal-buckets` and `available-binning-strategies` (from fingerprints),
`aggregable-columns` plus an operator registry, `orderable-columns`,
`suggested-join-conditions`, `available-drill-thrus` (18 kinds), `display-info`,
`describe-query`, `suggested-name`, `preview-query` (a stage truncated at a clause),
`can-run`. The frontend never inspects the AST; it asks lib through 169 exported
functions over an opaque handle. The notebook (`querying/notebook/utils/steps.ts`)
is a pure projection: steps derived per stage, each with `valid/active/revert` and a
10-row preview, an empty stage auto-appended after a summarize.

Two things follow for NILS. First, Metabase can run this in the browser only because
lib is one `.cljc` codebase compiled to JavaScript; doc 06's "`@nils/ast` for editing
only" means the notebook must round-trip to the engine for affordances, previews and
display info, and the engine must serve them from the start. Second, and surprising:
**Metabase's own agent path does not use the affordance API.** The MCP tools and
Metabot call `filterable-columns` a handful of times and never `available-*`; the
model gets a static prose reference, a field-values tool (30 values), and repair
errors. That is a gap NILS can close, and it matters most for small local models.

### The catalog

- A field record carries `base_type`, `effective_type`, `semantic_type` (a derive
  hierarchy: PK/FK, Category, Name, Quantity, Score, Temporal, ...), `visibility`
  (`normal | details-only | hidden | sensitive | retired`; `sensitive` fields error
  in queries), `has_field_values` (`list | search | none`), a fingerprint
  (`distinct-count`, `nil%`, min/max/avg or earliest/latest), `description`,
  `caveats`, `points_of_interest`, FK target and a remap (display value for coded
  fields). Human curation lives in a side table re-applied after every sync, so sync
  never clobbers it.
- Distinct values are cached only for list fields (up to 1,000 values of up to 100
  characters); everything else gets a search endpoint.
- Per-entity `ai_context` (`{instructions, synonyms[], examples[]}`) and a glossary
  are injected into every agent prompt. Metabot's `read_resource` renders tables as
  XML with fields, related tables, pre-defined measures and segments, and a
  copy-pasteable reference for every field; lists cap at 25 with a next-page URI.
- Reusable definitions: segments (filter macros), measures (single aggregations bound
  to a table), metric cards, models, and a newer dimension registry: three overlapping
  "reusable aggregation" concepts coexisting is the anti-pattern.

### The query processor

Preprocess (44 steps, one of them inserted three times with issue numbers explaining
the order), compile (`->honeysql` multimethods per clause, stages as nested
subqueries, 60-byte aliases), execute. Limits: 2,000 rows unaggregated, 10,000
aggregated, 1,048,575 absolute. Ordinary queries are **synchronous** streams (a 202
chunked response, cancellation on client disconnect, a 20-minute timeout); job-style
runs exist only in the newer Explorations module. Caching keys on a hash of
`stages + parameters + database`; column metadata is persisted with the saved
question so consumers get types without re-running. Permissions are checked as the
user at execute time (view data per table, compose ad-hoc per table, native per
database), sandboxing rewrites the source at preprocess; API keys are synthetic users;
the MCP server and Agent API run as the connecting user, and token scopes only hide
tools, never widen access.

### The agent path

MCP tools are **generated from the API's endpoint metadata** and dispatched as
in-process requests: `search`, `read_resource`, `construct_query`, `query`,
`execute_query`, `execute_sql` ("only when MBQL cannot express the question",
switchable off), `create_question`, `visualize_query`. `construct_query` runs repair →
permission check → validate → resolve and returns a **query handle** (a UUID in
`mcp_query_handle`) that `execute_query`, `visualize_query` and `create_question`
consume, so the model never re-emits an AST it already got right; results page at
200 rows with a continuation token. The chat hands the AST to the human as an
unsaved question opened in the notebook; in embedded MCP hosts the editor is hidden
and drills stay. The MCP endpoint authenticates with OAuth 2.0 (protected-resource
metadata, dynamic client registration) because desktop MCP clients expect that flow.
Grounding is a prompt rule: "never reference a table, field, metric or enum value you
haven't seen through the metadata tools in this session".

### Where docs 05 and 06 diverge from the reference

- "Joins implied by the catalog" (05 §2): implication works for the registry's tree
  (instance → stack → series → session → subject) and fails for the many-to-one
  sides (identifiers, clinical events, cohort memberships), which need explicit
  semantics: which namespace, which event, within what window. Metabase's answer is
  single-hop implicit joins plus `source-field` disambiguation and explicit joins for
  the rest.
- "Parameterized by construction" (05 §2): the AST holds literals; parameters appear
  at compile time. It is a compiler invariant ("never inline a value"), not an AST
  property, and should be stated as one.
- "Job-based for anything heavy, a POST answers 202 with a job id" (05 §1): a
  job-only query door makes a notebook feel dead. Metabase's notebook lives on
  bounded synchronous previews. NILS needs both tempos behind one door.
- "`off | token | oidc`" (05 §5): the MCP endpoint additionally needs OAuth resource
  metadata pointing at Authentik so third-party MCP clients can connect in `oidc`
  mode; static tokens suit CLIs and services.
- "The notebook renders the agent's AST" (06): matches the reference exactly.

### Not to copy

The 44-step middleware chain; four query dialects at once with a legacy converter on
the execution path; three reusable-aggregation concepts and the question/model/metric
card triad; a 169-function JS bundle over opaque handles (NILS cannot share one
codebase across engine and browser, so it should not try to mirror lib in
`@nils/ast`); the one-way native-SQL escape hatch; x-ray entity inference from table
names; 20-minute synchronous HTTP as the main execution model.

## 4. Flue, read from the source

### The facts that matter

- **Velocity and governance.** 34 releases between April and August 2026; 2.0.0 on
  2026-07-31 rewrote authoring, routing, build, SDK and storage at once ("the agent is
  the function", workflows removed, `flue build/dev` replaced by Vite) and offers no
  data migration ("pre-1.0 persisted schemas are reset-only"; the store gates on a
  single `FLUE_FORMAT_VERSION = 1`). `CONTRIBUTING.md` describes one lead and no
  co-lead; pull requests are auto-closed by policy and converted into issues.
  `AGENTS.md` states "No tests exist in the repo" and there is no CI test job. The
  model layer is a 0.x dependency (`@earendil-works/pi-ai ^0.83.0`).
- **The durability model is the strong part.** Every input is a durable submission
  (HTTP 202), one runs at a time per conversation, attempts and leases converge to
  exactly one of completed/failed/aborted, history is an append-only record stream,
  compaction is model-driven with a structured checkpoint. `durable: true` tools get
  exactly-once step recording with at-least-once execution.
- **Skills are the open Agent Skills spec**, packaged at build time from `SKILL.md`
  imports, with real progressive disclosure (one catalog line each; `activate_skill`
  returns the body as a tool result so the cached system prompt survives). Our
  existing skills drop in unchanged; `name` must match the directory.
- **The MCP client sees tools only.** `useMcpConnection` speaks streamable HTTP (SSE
  fallback), never stdio; it lists tools with pagination and never resources or
  prompts; names surface as `mcp__<server>__<tool>`; `auth` is a bearer string or a
  per-request function and nothing else can be added to a request; results are
  flattened to text, `isError` becomes a thrown tool error the model sees, and
  nothing truncates. Sub-agents cannot open MCP connections.
- **What is not there.** Per-user model credentials (one provider id per user,
  registered with a resolver closure over that user's key, is the workable pattern);
  any authentication ("anyone who can reach a conversation URL can talk to that
  conversation"); a human-approval primitive (state-gated tool mounting is the
  documented pattern); long-term or cross-conversation memory; a Docker sandbox driver
  (the contract is nine verbs, no teardown, no limits, no network policy); structured
  output on the main loop; per-user usage attribution; deletion or retention
  ("settled submission data is retained indefinitely"); an eval framework (Vitest
  driving the runtime in-process is the pattern); an outbound Teams client (the
  channel package is verified ingress only). No example uses MCP; no example is "chat
  with your data".
- **Telemetry captures content by default.** `@flue/opentelemetry` records prompt and
  tool content in spans unless turned off or scrubbed.

### What it means for D11

The decision was "the agent is a Flue app, MCP client only, Ask-query pilot first". It
stands, with the honesty turned up: Flue is the harness for the pilot because its
design is right and its cost is low, and it is held at arm's length because its bus
factor is one and its history is a rewrite every few months. The portable parts of
the design are framework-neutral by construction: the engine's MCP server, `SKILL.md`
files, JSON-schema tool definitions, the vLLM OpenAI-compatible endpoint, a React
chat UI over SSE. Flue-specific code is confined to the agent modules, the provider
registry and `app.ts`. If the pilot fails its exit criteria, the nearest substitutes
are the Vercel AI SDK (mature, MCP client, structured output, no durability), Mastra
(memory, RAG, evals and UI built in, heavier) or a Python stack (pydantic-ai or the
OpenAI Agents SDK) if the sandbox pulls the app toward Python. The switch is days,
not months, exactly because nothing of NILS lives in the harness.

### The MCP server the engine must be, so that any harness can use it

1. Streamable HTTP; stdio is a CLI convenience only.
2. Everything is a tool: the catalog, review items and pipelines included. Resources
   and prompts are never listed by this client.
3. Short, stable, lowercase snake_case names; the description is the only routing
   signal the model gets.
4. Inputs are JSON Schema objects with a `description` on every node of the AST
   schema, because the engine's validation error text is what the model reads.
5. Compact JSON text results, bounded server-side (row caps, cursors, byte caps):
   nothing else stops an oversized result from landing whole in the context window
   and in the durable log.
6. `isError: true` with actionable text for user-fixable failures.
7. One bearer token per request carrying the acting user: the agent app mints a
   short-lived JWT from the OIDC session, the engine verifies it, per-kind review
   policies apply to that subject, the audit log records it.
8. Jobs are submit plus status with a client-supplied idempotency key, because tools
   re-execute on re-attempt.
9. The tool list is stable within a conversation: it is discovered at render and
   frozen into the system prompt until compaction.

## 5. What changes in the design

Everything below is a proposal (section 6 has the ids). The order is the order of
the traffic: what the AST must say, what the catalog must know, what a result is, how
the door looks to an agent, and what the agent app becomes.

### 5.1 The AST adopts Metabase's shape, external dialect only (C17)

Stages; `[op, {opts}, ...args]` with a mandatory options map; refs as
`["field", {opts}, path]` where the path is a catalog name path
(`["session", "date"]`, `["stack", "technique"]`), never a numeric id or a UUID;
bucketing in ref options; parameters outside the AST so a saved selection can be
re-run with a different window without editing it; integer `ast_version` on saved
selections with on-read upgrade; a JSON Schema generated from the engine's own schema
and published over HTTP and inside the query skill; two validation layers, a strict
one for storage and a structural repair pass for agent input with a fixed error
taxonomy (`unknown_field`, `ambiguous_path`, `missing_source`, `grain_mismatch`,
automatic post-aggregation stage split) and no typo correction. Metabase paid for
starting elsewhere with four dialects; NILS starts where they ended.

### 5.2 Grain, windows, sets and lists are primitives (C18, D20)

The traffic's errors are about what a row *is*. So:

- Every stage declares its **grain**: `subject | session | stack | instance | event`.
  A stage changes grain only through `summarize` or `pick`; a filter never does.
  `count` counts the stage's grain and says so in the result; a `share` names its
  denominator as a ref to another stage or selection.
- **Temporal windows** are clauses, not SQL idioms: `nearest` (the closest observation
  of a type within a window around an anchor date, with a declared tie rule),
  `within` (any observation in the window), `pairs` (session pairs of one subject
  with a gap in a range, first or all), `age_at(subject, date)`. The clinical EAV
  store, diagnoses, onsets, transitions and deliveries are all reached this way, and
  the family 6 questions become one stage each.
- **Set algebra on selections**: `union`, `intersect`, `except` of selections at a
  declared grain; membership in a named cohort is `["selection", {}, id]` as a filter
  macro, membership in an ingest batch is a provenance filter (D4).
- **Values sources**: a stage may start from a literal list (pasted identifiers, an
  uploaded CSV) with a declared identifier namespace; the list is stored with the
  selection, resolved through identity linkage, and the unresolved rows are part of
  the result.
- **Identifier projection**: `["identifier", {"namespace": "..."}, subject]` is a
  field, so "use the cohort's id" is a column choice, not a join the model must get
  right.
- **Derived fields** the traffic asks for by name: resolution as a string and as
  three numbers, voxel volume, slice count, field strength, acquisition type, the
  protocol fingerprint (a stable hash of scanner model and the physics tuple) so that
  "same protocol across sessions" is a `group by`.

### 5.3 Roles and picks become registry facts (C19, D21)

A **role** is a catalog object: a predicate over the six axes, the image-type tokens
and parameter windows ("T2w FLAIR, 3D, not derived", "MP2RAGE uniform denoised", "SWI
mean", "phase of the last echo"). A **pick** chooses exactly one stack per session and
role by an ordered preference list (`reject derived; prefer field-map corrected; max
slices; prefer sagittal`), with ties reported. The image-type tokens become a
structured multi-valued attribute of every stack in the schema, ending the
`LIKE '%mean%'` era. The default role set is v0's "main acquisition", so C8's "main
acquisition per session and contrast as a registry fact" is implemented as picks
computed at classification time and re-computable; a study's own roles are saved with
its selection. Every long thread in section 2 is then a selection with three or four
picks and a column list.

### 5.4 The catalog serves affordances, vocabulary and lore (C20)

- Engine endpoints keyed by `(ast, stage)`: `options` (filterable, aggregable,
  breakoutable and orderable columns with their operators, buckets and value chips),
  `describe` (the AST as a sentence, which is what a human checks before running an
  agent's draft), `preview` (at most 10 rows of a stage), `diagnose` (an expression's
  friendly error). The notebook round-trips to these; caching is per AST hash.
- Field records as in Metabase: semantic type, visibility with `sensitive` (date of
  birth, free-text notes, paths outside the pseudonym domain of D13), `has_values`,
  fingerprints, remaps for coded fields, `description`, `caveats`, and `ai_context`
  (instructions, synonyms, examples) that carries what `db_guru` and the `nils-data`
  skill know today. Curation survives re-sync.
- The vocabularies are catalog entities with descriptions and examples: the six axes
  and their values, the image-type tokens, the observation types, the identifier
  namespaces, the roles. "SWI is a construct" is then a chip, not a correction.
- `sensitive` fields are excluded from affordances unless asked for explicitly, and
  agent tokens can never see them at all.

### 5.5 Results are first-class (C21, D22)

A run returns a **result handle**: id, name (suggested by `describe`, editable),
grain, columns with types, row count, content hash, the AST version that produced it,
who and when. Rows are paged by continuation token; exports, send-to and the
notebook's downloads consume handles, never re-run SQL; a handle is what the agent
"surfaces". This is the concrete form of D14's staged result versions: a review
snapshot and a query result are the same object with different consumers, and the 28
gold tasks' result hashes are handles avant la lettre.

### 5.6 One door, two tempos, and the door as an agent sees it (C22)

- Beside jobs, a bounded **synchronous** path: row cap, short timeout, cancellation
  on disconnect. Previews, affordances and gold-task-sized questions use it; digests,
  exports and pipelines use jobs. Both are the same AST through the same executor.
- MCP tools are **generated from the API's endpoint metadata** and dispatched
  in-process, which is 05 §4's "thin veneer" rule made mechanical; scopes on a token
  only narrow the tool list. Minimum tool set: `catalog_search`, `catalog_describe`,
  `catalog_values`, `query_options`, `query_describe`, `query_validate`,
  `query_execute` (returning a result handle and the first page), `result_page`,
  `selection_get/save/list`, `job_submit/status/cancel`, `review_list/get/apply`,
  `pipeline_list/run`. Shape per section 4: streamable HTTP, tools only, JWT per
  request, bounded compact JSON, `isError`, idempotent jobs, stable list.
- **Query handles** the Metabase way: a validated AST gets an id that execute, save
  and send-to consume, so a model never re-emits what it already got right.
- The MCP endpoint publishes OAuth protected-resource metadata pointing at Authentik
  in `oidc` mode, so Claude Desktop and its kind connect without a NILS-specific
  client; `token` mode stays for CLIs and services.

### 5.7 The notebook, adjusted (06)

Steps derived from the AST per stage with `valid/active/revert` and a 10-row preview;
filter widgets by column type with typed parts objects and one serializer each;
drills computed by the engine (quick filter, distribution, underlying records, "open
subject", sort, summarize by time); overwrite-or-create on save with capped snapshot
revisions and a single "verified" flag; column metadata persisted with the selection.
Two things Metabase does not have and the traffic demands: **roles and picks as a
step** (5.3) and **the grain shown on every step**. The MVP acceptance is the ten
families of section 2: each must be expressible in the notebook without a native
escape hatch, which the notebook does not have.

### 5.8 The agent app, adjusted (C23, C24, D23, D24)

- **Conversation state is a draft selection.** Every turn edits the AST (through
  `query_options` and `query_validate`), never re-derives it; the notebook renders
  the draft live; "save" names it. This alone removes the dominant failure class and
  most of the 213 SQL executions.
- Flue **pinned to an exact version** (not a major: three patch releases landed in
  four days and the store format has no migration path), behind seams we own: agent
  modules, a provider registry, `app.ts`. Everything else is framework-neutral.
- What we build because Flue does not have it: forward-auth middleware and
  conversation ids derived from the OIDC subject; per-user providers; the JWT minting
  for MCP; a Docker `SandboxDriver`; the Teams outbound client if Teams is wanted;
  result data parts for tables; usage attribution per user from the record log;
  deletion and retention on the `flue_*` tables; content capture off in telemetry.
- **Pilot exit criteria**, written before the pilot: the ten families pass through
  the draft-selection loop with a local model on vLLM and with a hosted teacher; the
  gold tasks reproduce their hashes; median turns per resolved question drop below
  v0's; no harness upgrade in the pilot window costs more than a day. Failing any of
  these switches the harness, not the design.
- **Transcript custody.** The harness log keeps every prompt, tool result and AST
  indefinitely; that log contains pseudonymous identifiers and clinical values as a
  matter of course and is part of D13's custody boundary. Retention is a policy we
  write; uploaded lists and pasted identifiers become values sources of the selection
  (5.2) so the governed copy is in the engine, not only in a chat log.

### 5.9 Evaluation is a product asset (C25)

The 28 gold tasks with hashes and the 27 scored trajectories with their failure
taxonomy are the first labelled dataset of D15. They become `nils-evals`: versioned
with the corpus, run by Vitest against the engine (does each gold task have an AST,
does it reproduce the hash) and against the agent (does the loop reach it, in how
many turns, with which model), and the taxonomy (`inefficient_query`, `wrong_grain`,
`ambiguous_denominator`, ...) stays as the scoring vocabulary. The fine-tune and
evolution experiments stay outside the product until this gate exists.

### 5.10 A sketch, to make the primitives concrete

Not a spec. One stage list for "7T sessions in two cohorts with both a 3D FLAIR and
an MP2RAGE, one FLAIR each, EDSS within a year, with the cohort's identifier":

```json
{"ast_version": 1, "stages": [
  {"source": ["session"], "grain": "session",
   "filters": [
     ["=", {}, ["field", {}, ["session", "field_strength"]], 7],
     ["selection", {}, "cohort:a"], ["selection", {}, "cohort:b"],
     ["has_role", {}, "flair_3d"], ["has_role", {}, "mp2rage_uni"]]},
  {"grain": "session",
   "picks": [["pick", {"as": "flair"}, "flair_3d",
              [["reject", {}, ["field", {}, ["stack", "image_type"]], ["DERIVED", "MPR"]],
               ["prefer", {}, ["field", {}, ["stack", "orientation"]], "sagittal"],
               ["max", {}, ["field", {}, ["stack", "slices"]]]]]],
   "joins": [["nearest", {"as": "edss", "within": [1, "year"], "tie": "closest"},
              ["event", {}, "EDSS"], ["field", {}, ["session", "date"]]]],
   "fields": [["field", {}, ["subject", "code"]],
              ["identifier", {"namespace": "cohort_a"}, ["subject"]],
              ["field", {}, ["session", "date"]],
              ["field", {}, ["flair", "stack_id"]], ["field", {}, ["flair", "resolution"]],
              ["field", {}, ["edss", "value"]],
              ["age_at", {}, ["field", {}, ["subject", "birth_date"]], ["field", {}, ["session", "date"]]]]}]}
```

`describe` renders it as one sentence; `options` at stage 1 lists what else can be
picked, joined or projected; the result handle says `grain: session`, 154 rows, and
its hash.

## 6. Amendments and decisions register (continued from 12)

Ids continue [12](12-review-devils-advocate.md) §6. All accepted 2026-09-02; C18
staged ([15](15-ratification.md) §2 and §9).

| Id | Affects | Proposal | Status |
|---|---|---|---|
| C16 | D5, C4 | The 28 gold tasks (with result hashes) and the ten families are the AST gate: Wave 4 proves every one is expressible as an AST fixture; Wave 5 proves each gold task reproduces its hash on the migrated registry | accepted 2026-09-02 (15 §9) |
| C17 | D5, 05 §2 | The AST adopts Metabase's shape (stages, `[op, {opts}, ...args]`, name-path refs, bucketing in ref options, parameters outside), one external dialect only, generated JSON Schema, `ast_version` with on-read upgrade, structural repair pass with a fixed error taxonomy | accepted 2026-09-02 (15 §9) |
| C18 | D5 → D20 | Grain declared per stage and changed only by summarize or pick; counts name their grain and shares their denominator; `nearest`, `within`, `pairs`, `age_at` clauses; set algebra on selections; values sources with a namespace; identifier projection; derived fields including the protocol fingerprint | accepted 2026-09-02, staged: grain, summarize, pick, `within` and `nearest` in Wave 4; `pairs`, `age_at`, set algebra and derived fields with the notebook in Wave 5 (15 §2) |
| C19 | D12, C8, D16 → D21 | Roles as catalog objects, picks as ordered preferences yielding one stack per session-role with ties reported; image-type tokens as a structured stack attribute; "main acquisition" = the default role set, computed as picks | accepted 2026-09-02 (15 §9) |
| C20 | 05 §2, 06 | Engine-served affordances `options`, `describe`, `preview`, `diagnose` keyed by `(ast, stage)`; field records with semantic type, `sensitive` visibility, fingerprints, remaps, `ai_context`; vocabularies as catalog entities; curation survives re-sync | accepted 2026-09-02 (15 §9) |
| C21 | D14, 05 §1 → D22 | Result handles (id, name, grain, columns, row count, hash, AST version, provenance) paged by continuation token; exports and send-to consume handles; the same object as D14's staged result versions | accepted 2026-09-02 (15 §9) |
| C22 | 05 §1, §4, §5, D8 | A bounded synchronous path beside jobs; MCP tools generated from endpoint metadata, tools-only over streamable HTTP, JWT per request carrying the user, bounded results, `isError`, idempotent jobs, query handles, scopes that only narrow; OAuth protected-resource metadata on the MCP endpoint in `oidc` mode | accepted 2026-09-02 (15 §9) |
| C23 | D11 → D23 | Flue pinned to an exact version behind owned seams; conversation state is a draft selection edited per turn; pilot exit criteria written first; substitutes named; per-user providers, forward-auth, Docker driver and Teams outbound are ours | accepted 2026-09-02 (15 §7) |
| C24 | D13, D11 → D24 | Transcript custody: the harness log is inside the custody boundary; retention and deletion are ours; telemetry content capture off; uploaded lists and pasted identifiers stored as values sources in the engine | accepted 2026-09-02 (15 §7) |
| C25 | D15 | Gold tasks and scored trajectories become `nils-evals`, versioned with the corpus, run against the engine and the agent, with the v0 failure taxonomy as scoring vocabulary; fine-tuning stays out until this gate exists | accepted 2026-09-02 (15 §9) |

Decisions proposed for the register:

- **D20, grain and denominators are explicit in the AST** (from C18). Ratified
  2026-09-02.
- **D21, roles and picks are registry facts** (from C19): a role is a catalog
  predicate, a pick is an ordered preference producing one stack per session-role,
  and the default role set is the main acquisition. Ratified 2026-09-02.
- **D22, results are first-class objects** (from C21). Ratified 2026-09-02.
- **D23, the harness is replaceable and the contract is the product** (from C23).
  Ratified 2026-09-02.
- **D24, transcripts are inside the custody boundary** (from C24). Ratified
  2026-09-02; custody is also visible to the user (C38).

C26 onward and D25 onward, from the federation design, continue in
[14](14-federation.md) §6.

## 7. What I did not change

D5 and D11 stand as written; the amendments sharpen them. No doc in the folder
was rewritten, only extended in place where a fact was missing (05 §1, §2, §4, §5; 06;
08; 11 gates; README). Nothing in the reference clones was modified or built. I did
not read assistant or tool output from the threads, only human turns, and no result
row from either database was read or reproduced. The optimization tables (evolution,
fine-tune, local models) were counted, not evaluated; whether local fine-tuning is
worth pursuing is a question for after `nils-evals` exists.
