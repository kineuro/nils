# 18 — Wave 4b, the ask: the study's rulings, the amendments, and Nima's answers

Written 2026-09-07 from the study that 17 §4 required before Wave 4b could open
(`studies/2026-09-06-wave4b-ask-study/`: eleven readers, three designs, three
judges, a synthesis, thirty-four verifications, nine rulings, the report and its
page, and Nima's eighteen answers, verbatim in `answers-2026-09-07.md` there).
Status: **ratified 2026-09-07** (Nima: "I agree with all. confirmed."). The
specification written from it is `docs/specs/wave4b-the-ask.md` in the public
repository; this document is the record it cites.

Ids continue [15](15-ratification.md) §11: decisions D32 to D40, amendments C39
to C42, and Nima's answers Q1 to Q18, cited as `18 Q7`.

## 1. The yardstick

The wave is measured against one question Nima stated as the test of whether
the query is dynamic: subjects with at least three follow-ups after
transitioning from PPMS to SPMS, sessions close to an EDSS and an SDMT within
six months, all after 40 and before 50, scans with a 3D MPRAGE and a 3D FLAIR at
0.5 mm isotropic, relatively the same acquisition across sessions. His rule for
it: "no amount of pre-thinking on how we should prepare the query can be robust
unless we have the means to do this flexibly." The question stands as stated
(Q1, Q4); it runs on a synthetic registry whose data is made up by design (Q13),
because the live registry's course history is undated, which is the importer's
gap and not the language's.

## 2. The rulings

Each ruling was a recommendation refuted twice (once against the sources, once
through the user's eyes) and then ruled on; the surviving objection of each is
in the report. What is recorded here is the decision.

- **D32, the ask is a graph of named sets at one grain each.** Relations `of`,
  `has`, `attach`, `near`, `algebra`, `group`, and the sugar `same`, `every`,
  `pairs`, `change`, `share`; grains cohort, subject, session, stack, instance,
  event and the derived grain group, with `pair` reserved and staged; C17's
  clause form, refs and versioning kept verbatim; a stage pipeline is a chain
  shaped graph. Four amendments are part of the decision (§3, C39 to C42).
  Nothing changes grain; there is no HAVING; quantifiers are counts.
- **D33, time is a window with a unit, a policy and a tie rule, and dates carry
  precision.** `near` is the only sideways relation; windows are inclusive,
  `month` is 31 days and `year` 366 (Q2); precision is stored per row and coarse
  dates compare forgivingly unless `strict` (Q3); `part` extracts; `change` is
  sugar over `prev(value)` with adjacency as its default reading; `every` is
  three clauses so it is never vacuously true. The forgiving default is a
  composition rule of its own (spec §4.4 rule 16).
- **D34, sessions are a rebuildable cache.** One resolver in `nils-session` over
  each subject's whole timeline, keyed by scheme definition and epoch and never
  by the selection; the key is a surrogate; identity is a function of the
  window alone and labels sit beside it keyed by the full scheme digest; the
  rebuild unit is the subject through a timeline digest; a moved span raises a
  review item and never drops silently; stored picks re-key with
  `session_moved`; `study` leaves the scheme list; the read door never writes.
- **D35, one canonical membership, one way promotion, bounded handles.** A
  cohort is a membership fact recorded as an interval log
  (`UNIQUE(cohort_id, subject_id, joined_at)`) with structured provenance; the
  language reads open intervals only and there is no `as_of` (Q5); origin is not
  a grain and not a field (Q6); a saved ask is a selection with immutable
  versions, referenced by name and pinned to a version at validate (Q11);
  promotion consumes a complete subject grain handle and records its id, epoch,
  scheme digest and bound parameters on the intervals it opens; a handle keeps
  rows for 90 days and its metadata for ever (Q7, Q14); the cohort acts and
  `selection.save` join `Action` and advance the epoch.
- **D36, the engine serves the affordances and `apply` returns a handle.**
  `options`, `apply`, `diagnose`, `preview`, `describe`, `draft`; a move costs
  zero queries by default; `apply` takes an atomic list of moves and refuses a
  stale options list; the error rule never enumerates what the principal may not
  see and always lists the author's own fillers; a 30 kind move catalog cap;
  both authoring modes against one validator; diagnose carries the funnel (Q1);
  no document carrying identifiers enters a model context.
- **D37, one AST, one SQL text per backend, compiled in the engine.** The hook
  table is an enumeration and the two backend fixture is the contract; four
  divergences are closed rather than hooked (null order, `->>` type, case
  folding, integer key ordering); a parameter that changes the SQL desugars into
  the hashed core; caps are contract constants with the numbers set by
  measurement; `query_stream` and `query_with_header` are preconditions;
  DuckDB is struck from D3; the ask door runs as a SELECT-only role (Q18).
- **D38, where the code lives.** `nils-session`, `nils-catalog`, `nils-ask` and
  `nils-synth` in the engine; the app edits and never executes and is one bundle
  with two deployments; `nils ask` is a runner in the same binary; the MCP door
  stays in the engine, curated, with model facing content in the pack; the
  reproducibility clause of the first draft is withdrawn; the engine's door is
  not browser facing and the web application is a later product (Q10).
- **D39, identity is external, optional and admitted per app.** NILS owns no
  user registry in any wave. Anyone installs the engine or any app and uses it
  with no identity; an installation that wants identity adds one component
  (`nils-auth`) and takes its people from an OpenID Connect provider, admitted
  per app in the provider's own panel (Q9). The engine accepts tokens from the
  apps the installer registered and nothing else, roles come from each
  application's own entitlements, and a token with no role is refused rather
  than defaulted to reader. The role ladder suffices for now and the class grant
  is deferred (Q8). None of it is on 4b's clock except the refusal of a roleless
  token, which lands with the ask door. This reverses the study's own first
  ruling of one application per installation, on Nima's answer.
- **D40, the gate is N fixtures with a row oracle and three outcomes.** C16's
  "the 28 reproduce their hashes" is replaced: the frozen table's hashes are not
  an oracle (23 distinct hashes over 28 rows, empty canonicals, hashes without a
  canonical), so the gate is roughly twenty inspectable assertions after the
  triage (Q12), each passing, deferred by clause, or out of language; the
  synthetic registry is a deterministic generator in the public repository
  (Q13); the conformance suite of hooks is the portability contract; the pilot
  clock of C23 starts in 4c.

## 3. The amendments, and the record edits

Four amendments to ratified text, found by the refutations and put into the
ratification table before any compiler code exists:

| Id | Amends | Amendment | Status |
|---|---|---|---|
| C39 | D20, C18 | Denominators come back: a share names its denominator as an uncorrelated scalar count over a named set, legal only in a denominator position and only at count or distinct; the merged model of the study had silently dropped D20's denominator half | accepted 2026-09-07 |
| C40 | D20, C18 | The cohort key is groupable: a subject set that names a cohort set in `of` exposes the cohort key alone, so a per cohort table exists without naming every cohort in the document | accepted 2026-09-07 |
| C41 | C18, C24, C35 | `values` goes by reference: an upload id and a digest of the resolved keys, rows dying on resolution into the linkage side, so no pasted identifier ever sits in a hashed, stored, pinned, printed or federated document | accepted 2026-09-07 |
| C42 | C18 | The event key is the event table's own id, not a composite carrying the subject code; set valued signature components are the sorted list | accepted 2026-09-07 |

The edits to earlier documents, each made in place with a dated note:

| Where | Edit |
|---|---|
| 02 D3 | DuckDB struck: the embedded registry is SQLite alone, the server is Postgres 16, and "expressible on both" is a fixture whose hashes agree on both backends |
| 03 epoch | The rule takes the 4a wording (any write that changes a judgement) and gains the cohort acts and `selection.save` |
| 13 §5.2, D20 | The stage is a named set at one grain; C17's stage list becomes a DAG with the pipeline as its chain shaped case; D20 gains the `group` grain and the rule that nothing changes grain, and its denominator half is restored (C39) |
| 13 §6 C16 | Replaced by the row oracle with three outcomes (D40) |
| 13 §6 C18 | Walked clause by clause in §5 below; every rename and drop recorded; `age_at`, set algebra and derived fields unstaged into 4b |
| 15 | Section 12 carries this document's ids |
| 17 §9 | Wave 4b opened 2026-09-07 |
| 11 | Wave 4b's entry points at the spec |

## 4. Nima's answers, 2026-09-07

The eighteen questions the study left for a person, his answers in one line
each (verbatim in the study folder), and what each settles.

| Id | Question | Answer | Settles |
|---|---|---|---|
| Q1 | The yardstick's primary reading | It hardly matters, but the conditions are deal breakers in order: not in the cohort, nothing to ask; fewer than three qualifying sessions, no sameness to test | The layered reading is primary; describe prints in that order; diagnose reports the funnel |
| Q2 | Six months | Always forgiving: 6 × 31 days or similar | `month` = 31 days, `year` = 366, inclusive; the forgiving rule |
| Q3 | Coarse event dates | Important: a transition is known to the year, an onset or a diagnosis sometimes to the month, and many kinds are like that | Precision stored per row; coarse dates are intervals; comparisons forgiving unless `strict`; the importer invents no day |
| Q4 | "Relatively the same acquisition" | Vague by itself, so rely on the classifier: tiers from absolute to loose, defined in physics, because the asker means to compare images; MPRAGE against a plain GRE differs however close the numbers | Comparability levels in the pack over the axes and physics; categorical axes exact at every level; `signature {level}` and the `same` sugar; the word is level, not tier |
| Q5 | Membership as of a past date | No: removed means no longer in it | `as_of` struck; open intervals only |
| Q6 | Origin at stack grain | Not at all: digest and cohort were separated in v0 to avoid exactly this | No origin field at any grain; in the limits list |
| Q7 | Named results in six months | Explain it | 90 days of rows, metadata for ever, promotion for permanence, a drift note on an expired handle |
| Q8 | The read class | Yes, no limit for now | The role ladder suffices; the class grant deferred |
| Q9 | Application scope | Not about people: auth is optional; add `nils-auth` and NILS takes its people from the provider; admit per app in the panel | D39 as written above; the single application reversed |
| Q10 | Browser reachability | Through the later web application only | The engine's door is not browser facing; the name `nils-server` collides with `nils serve` and is open |
| Q11 | Selection references in a stored ask | Probably yes, explain | References pinned to a version at validate; versions immutable; `selection_outdated` |
| Q12 | The gate's size | You decide | N as measured, with the three outcomes |
| Q13 | The synthetic registry's owner | You decide | A generator in the public repository, owned by the spec, landing with slice 1 |
| Q14 | Who signs the custody table | Why does it matter | The defaults of spec §14.1, the installation's data controller as owner |
| Q15 | Birth date's class | Why does it matter | Quasi-identifying and local |
| Q16 | Stack set defaults | Your recommendation | `disposition != excluded` standing, a warning, a preset move |
| Q17 | Names | Good | An ask, `.ask.json`, `nils ask`, `capabilities.ask`, `/api/ask/*` |
| Q18 | The ask door's database role | Yes, best practice | A SELECT-only role on Postgres, read only on SQLite, caches built by jobs |

Then, on 2026-09-07, to the four explanations and their defaults: "I agree with
all. confirmed. lets write the spec and start."

The eleven items 17 §4 said the talk must settle, and where each landed:
1 the engine executes and the app edits (D38); 2 the session cache, keyed by
scheme and epoch and never by the selection (D34); 3 the pick clause is both the
stored pick and an inline preference list, one row per session-role or n rows by
`n` (D32, D34); 4 the cohort is a membership fact and a saved ask is a selection,
promotion one way (D35); 5 handles as current state plus a change log, their
contents and retention (D35, Q7, Q14); 6 options, apply, diagnose and describe
first, preview off by default (D36); 7 "expressible" means rows on a synthetic
registry, and deferred by clause is an outcome (D40, Q12); 8 caps as contract
constants with override, gold never from a capped run (D37); 9 the epoch
advances on decisions, cohort acts and selection writes (D35, 03 amended);
10 the writer is deterministic and the reproducibility clause is withdrawn
(D38); 11 the document is an ask (Q17) and the app's name stays 17 §8's open
item.

## 5. The C18 and C17 clause walk

Every clause of the ratified text against the op table of D32, with every
rename or drop recorded, as the D32 ruling required.

| Ratified clause | In the ask | Status |
|---|---|---|
| C18: grain declared per stage | `sets.<name>.grain`, one per set | kept; sharpened: nothing changes grain |
| C18: changed only by summarize | a set of `grain: group` with `group {of, by}` | renamed: summarize is a group set |
| C18: changed only by pick | `pick {per, by, n, ties}` inside the finer set, read by the coarser through `attach` | changed: pick never changes grain; the coarser set attaches the picked row |
| C18: counts name their grain | `has` exposes a count; `out.level: count` returns rows and subjects | kept |
| C18: shares name their denominator | `share {of, over}`, a scalar count over a named set | kept, restored (C39) |
| C18: `nearest` | `near {policy: nearest, tie}` | renamed |
| C18: `within` | `has {set, window}` for a count, `near {policy: any}` for a filter | renamed, split in two |
| C18: `pairs` | `pairs` sugar over a hidden clone and `near best`; the `pair` grain reserved | kept as sugar; the grain staged |
| C18: `age_at` | `age_at` in bind, birthday exact | kept; unstaged from Wave 5 into 4b |
| C18: set algebra on selections | `algebra {op, sets, tag}` at one grain | kept; unstaged into 4b |
| C18: cohort membership as a filter macro | `of: <cohort grain set>`; a saved ask as `from: selection:<n>@<v>` | changed: membership is containment in a cohort set |
| C18: ingest batch membership as a provenance filter (D4) | none | dropped (Q6) |
| C18: values sources with a namespace | `values {upload: id}` by reference, the namespace declared on the upload | changed (C41) |
| C18: identifier projection as a field | `out.identifiers` with a `read_audit` row | moved to the answer's shape |
| C18: derived fields (resolution, voxel volume, slice count, field strength, acquisition type) | `derived` refs with declared parameters | kept |
| C18: the protocol fingerprint as a hash | `signature {level}` from the pack's comparability levels | replaced (Q4): a tuple at a level, never a hash |
| C17: stages | named sets forming a DAG; `pipeline` sugar embeds a chain | changed |
| C17: `[op, {opts}, ...args]` | the same, options map mandatory | kept |
| C17: name-path refs | `["field", {}, path]`, `["axis", {}, name]`, `["derived", {params}, name]`, `["param", {}, name]` | kept |
| C17: bucketing in ref options | `bucket`, `part`, derived parameters in ref options | kept |
| C17: parameters outside the query | `params` beside the document; structural parameters desugar into the core | kept, sharpened |
| C17: one external dialect | JSON canonical, YAML rendering | kept |
| C17: generated JSON Schema | generated, plus a hand tightened variant for guided decoding | kept, extended |
| C17: `ast_version` with on-read upgrade | the same | kept |
| C17: repair pass with a fixed taxonomy | nineteen codes enumerated in spec §4.4 rule 14 | kept |

## 6. What stays open

The cap numbers (measured in slice 9); whether guided decoding accepts the
positional clause form; the span movement rate on the first ingest that keeps
dateless studies; the app's repository name (17 §8); the web application's
name (Q10); whether the move catalog stays under 30 kinds. Next ids: C43 and
D41.
