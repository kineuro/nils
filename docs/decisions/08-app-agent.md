# 08 — nils-agent v1: a Flue app on the engine's contracts (D11)

## The lesson of v0

nils-agent v0 was the exploration that proved the ideas: per-user model access, a
schema-aware DB guru, skills, sandboxes, even a self-improvement loop. It also
proved the cost of owning a ~23k-line fork of somebody else's harness inside a
~116k-line app, most of it our own gateway, per-user model proxy, optimization loop
and Next.js frontend. v1 keeps the ideas and returns the harness to people whose
job it is: **Flue** (Astro's Apache-2.0 TypeScript agent framework — sessions,
tools, skills on the open Agent Skills spec, sandboxes, deploy-anywhere). Our
existing `SKILL.md` files drop in; local vLLM serving is first-class; the chat UI
starts from their demo. Flue is young, and reading its source
([13](13-query-and-agent-study.md) §4) puts numbers on that: 34 releases between
April and August 2026; a 2.0.0 on 2026-07-31 that rewrote authoring, routing,
build, SDK and storage at once with no data migration; one lead, pull requests
auto-closed by policy; "No tests exist in the repo" in its own `AGENTS.md`; a 0.x
model layer. So the app pins an **exact** version, not a major (three patch
releases landed in four days), keeps Flue behind seams we own (the agent modules,
the provider registry, `app.ts`), and treats everything else as framework-neutral:
the engine's MCP server, `SKILL.md` files, JSON-schema tool definitions, the vLLM
OpenAI-compatible endpoint, a React chat UI over SSE. The Ask-query pilot is the
churn probe, time-boxed with exit criteria written before it starts (C23): the
ten question families of 13 §2 pass through the draft-selection loop with a local
model on vLLM and with a hosted teacher; the 28 gold tasks reproduce their result
hashes; median turns per resolved question drop below v0's; no harness upgrade in
the pilot window costs more than a day. The pilot is six weeks from the day Wave
4's AST gate passes (C23, accepted 2026-09-02). Failing any of these switches the harness
(Vercel AI SDK, Mastra, or pydantic-ai / OpenAI Agents SDK if the sandbox pulls
toward Python), not the design; the switch is days because nothing of NILS lives
in the harness.

## The architecture rule

The agent is **a client**. It reaches NILS exclusively through the MCP surface
([05-contracts.md](05-contracts.md) §4) — catalog, AST queries, selections, jobs,
review items under policy, pipelines. No database access of any kind, no private
endpoints, no engine knowledge baked into tools. Consequences:

- Any MCP client is an equal citizen: Claude pointed at the engine gets the same
  governed powers — the agent app adds the *product* (chat UI, memory, skills,
  channels, per-user model auth), not the access.
- Field-awareness lives in **skills + the catalog**, not in code: the `nils-data`
  knowledge and `db_guru`'s table lore become catalog descriptions (served by the
  engine, shared by every client) plus skills for reasoning patterns. Deeper
  literature RAG is a later skill/tool, added without touching the engine.
- **Intelligence at decision points is a policy question, not an agent feature**:
  the agent participates in classification QC, identity linkage, or pipeline QC by
  consuming review items and applying/proposing under the per-kind policies of
  D7. Tightening or widening agent authority is configuration on the engine
  side, auditable, never a new integration.
- **The conversation's state is a draft selection.** The agent holds an AST, the
  notebook renders it live, and every turn edits it through the engine's
  `query_options` and `query_validate` tools; "save" names it. v0's traffic shows
  why: 213 SQL executions and 131 schema lookups over 104 human turns, most of them
  re-deriving a query that a refinement should have edited (13 §2).
- **What the client can consume fixes what the engine serves.** Flue's MCP client
  speaks streamable HTTP, lists tools only (never resources or prompts), sends one
  bearer token per request, flattens results to text without truncation and turns
  `isError` into a tool error the model sees; sub-agents cannot open MCP. The
  engine's MCP shape in [05](05-contracts.md) §4 is written to that client, and to
  any other.
- **Federation is a second server, not a second agent** ([14](14-federation.md),
  C34). Where a node daemon runs, the agent app registers its MCP server
  beside the engine's (Flue holds several connections per session). The draft
  selection loop stays local; "across the federation" is a scope on execute; a
  peer's approval shows up as a job state, so the conversation never blocks on a
  human in another country. No node, no tools, nothing to explain.

## What carries over from v0, deliberately

- **Per-user model authentication** (users OAuth to their own providers; the system
  holds no shared API keys) — reimplemented against Flue's provider registry, which
  has no notion of a user: one provider id per user, registered with a resolver
  closure over that user's credential, is the pattern.
- **Skills machinery discipline** (validation, a security pass on third-party
  skills). Flue packages `SKILL.md` at build time with real progressive disclosure;
  `name` must match the directory.
- **The sandbox seam** — Flue's `SandboxDriver` contract is nine verbs with no
  teardown, limits or network policy; the Docker adapter we own supplies those.
- The fine-tune/optimization experiments stay exploratory and out of the product
  until they earn their way in. Their gate is `nils-evals` (C25): the 28 gold
  tasks with result hashes and the 27 scored trajectories become the first
  labelled dataset of D15, versioned with the corpus and run against the engine
  and the agent, with v0's failure taxonomy as the scoring vocabulary.

## What we build because Flue does not have it

Read from the source (13 §4): forward-auth middleware and conversation ids
derived from the OIDC subject, since Flue has no authentication of its own; the
JWT minting for MCP; the Docker sandbox driver; the Teams outbound client if Teams
is wanted (the channel package is ingress-only); result data parts for tables;
per-user usage attribution from the record log; deletion and retention on the
`flue_*` tables, because the harness retains settled submissions indefinitely and
that log holds prompts, tool results and ASTs with pseudonymous identifiers and
clinical values as a matter of course, which puts it inside D13's custody
boundary (C24, accepted 2026-09-02: full transcripts kept 90 days by default, the
store listed on the custody page of C38); and content capture turned off in
`@flue/opentelemetry`, where it is on by default. Uploaded lists and pasted identifiers become `values` sources of
the draft selection in the engine, so the governed copy is never only in a chat
log.

## The de-risking path (kept from the suite plan)

First build is **Ask-query**: one Flue agent, read-only MCP tools, producing query
ASTs the notebook renders for human inspection. Small blast radius, real Flue
mileage, immediately useful. Full agent v1 grows from there in parallel with the
waves — it consumes everything and blocks nothing.

## Independence (D1)

- **Absent**: NILS is fully usable manually; review queues are worked by humans;
  no endpoint anywhere references the agent.
- **Present**: adds conversation, drafting, and policy-bounded automation.
- **Private for now** (its own repo), public when it stabilizes — per D10.
