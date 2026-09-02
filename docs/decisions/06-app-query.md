# 06 — nils-query: the selection notebook

## What it is

The human face of the query AST: a Metabase-style notebook where a researcher
composes a question step by step (filter → join-implied expansion → summarize),
sees the result live, and saves it as a named **selection** — which is, per D4,
what a cohort now is. Its second half is the **send-to bar**: a result becomes an
export run, a pipeline run on the selection, a segment work subset, or an agent
conversation seed.

The v0 suite-plan's design (`docs/nils/suite-plan.md` §2) carries over nearly
intact — AST as data, catalog-driven chips, saved questions — with one amendment:
**execution moved into the engine** (D5). nils-query renders and edits ASTs; it
never compiles SQL. That keeps it in the product plane (TypeScript, `@nils/ast`,
the shared UI kit) with no schema knowledge of its own beyond what the catalog
serves.

## Contracts consumed

- Semantic catalog (entities, fields, value chips) — read.
- AST execution + selection CRUD — the core loop.
- Jobs (send-to export/pipeline) and, later, review items (a selection of
  "everything flagged X" is a natural review workflow).
- Auth: whatever mode the engine runs; the app adds nothing.
- Affordances (`options`, `describe`, `preview`, `diagnose` keyed by `(ast, stage)`)
  and result handles (C20, C21 in [13](13-query-and-agent-study.md)). Metabase can
  compute these in the browser only because its query library is one codebase
  compiled to both sides; `@nils/ast` cannot be that, so the notebook round-trips
  to the engine for what a column can do next, the sentence a query means, and a
  10-row preview of a stage, cached per AST hash.

## What the traffic says the notebook must do

The 38 live nils-agent threads ([13](13-query-and-agent-study.md) §2) are the
notebook's acceptance test: ten question families, each expressible without a
native escape hatch, which the notebook does not have. Three of them are not in the
Metabase repertoire and shape this app:

- **Roles and picks as a step.** Once a session qualifies, the researcher wants
  exactly one stack per role ("3D FLAIR, never derived, sagittal else axial, most
  slices"). The pick step shows the candidates per session-role and the rule that
  chose one, with ties reported. Roles are catalog objects (C19), so a study's own
  roles are saved with its selection.
- **Grain on every step.** The 242/244/271 dispute over "sessions with both a 3D
  FLAIR and an MP2RAGE" was a grain dispute. Each step shows the grain of its
  output; a count says what it counts; a share names its denominator.
- **Temporal windows as widgets.** "EDSS within a year of the session", "session
  pairs four to five years apart", "age at diagnosis" are `nearest`, `pairs` and
  `age_at` clauses with a window widget, not joins the user assembles.

Beyond these, the notebook follows the reference where it is right: steps derived
per stage from the AST with `valid/active/revert` and a preview; filter widgets by
column type with one serializer each; drills computed by the engine (quick filter,
distribution, underlying records, open subject); overwrite-or-create on save with
capped snapshot revisions; column metadata persisted with the selection. A result
is a handle with a name suggested by `describe` and editable; "surface the result"
is a solved problem when every run has one.

## Scope, when there is a federation to scope to

Added by [14](14-federation.md) (C34, accepted 2026-09-02). When the engine's capabilities
name a federation endpoint, the notebook shows a **scope chip**: this node, a
federation, or chosen nodes. Nothing else changes: the same AST, the same steps,
the same draft-selection loop. What the scope adds is honest presentation:
catalog chips show per-node availability from the node profiles ("7T MP2RAGE:
here 412 sessions, Vienna 380, Amsterdam suppressed"), which is the "as if the
data had grown" experience without a single request leaving; a run at federation
scope is a job whose status shows each peer's approval state ("waiting for
Vienna"); results carry a node column, suppression marks (`<5`) instead of
silent gaps, and "overlap unknown" on any sum. Fields that are `local` at a peer
are greyed out at that scope with the reason, and a temporal window or a join
that would need individual-level alignment across nodes is refused at the step
with the same sentence [14](14-federation.md) §3.4 uses. Without a federation
there is no chip.

## Independence (D1)

- **Absent**: the engine still executes ASTs (CLI `nils query`, MCP, API); saved
  selections still exist and remain addressable — users simply author them without
  a notebook. Segment falls back to accepting a selection id or explicit list.
- **Present**: no other app changes behaviour; they gain a "pick a selection"
  affordance that resolves through the engine either way.

## The Ask-query seam

Natural-language-to-AST ("show me all 3T FLAIR sessions of subjects with two or
more timepoints") is deliberately **not** an MVP feature of this app. It arrives as
the agent pilot ([08-app-agent.md](08-app-agent.md)) producing ASTs through MCP —
the notebook then renders the agent's AST for the human to inspect and refine,
which is the honest division of labor: the agent drafts, the human sees exactly
what will run, the engine governs execution.

The seam is a **draft selection** (C23): the agent conversation's state is an AST
the notebook renders live, and every turn ("also 3D", "add slice count", "add sex
and age") is an edit to it through `query_options` and `query_validate`, never a
new derivation. That is the single change that removes the dominant failure class
in the v0 traffic, where each refinement restarted the query from scratch.
