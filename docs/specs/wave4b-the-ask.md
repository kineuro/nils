<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Wave 4b: the ask

The specification of the second of the three waves that the record's Wave 4
became (`docs/decisions/17-wave4-reframed.md`, R1 to R8), written from the
study that 17 §4 required before it could open (`docs/decisions/18-wave4b-the-ask.md`:
nine rulings D32 to D40, four amendments C39 to C42, and Nima's eighteen answers
Q1 to Q18 of 2026-09-07). It follows `wave4a-engine-completes.md` and cites the
record by id.

It is the wave in which a person, or a program, can ask the registry a question
without writing SQL and without an assistant: a question is a document, the
engine executes it on either backend and hands back a result that is a handle, a
cohort in waiting, or a table. The document is called an **ask**. Nothing in
this wave is a model, a prompt or a notebook page; those are Wave 4c and the app
that consumes this wave's doors. The one measure of the wave is the yardstick
question of 18 §1, restated in Appendix A: subjects whose course changed from
one type to another, with at least three follow-up sessions after the change,
each near two clinical scores, inside an age window, carrying two named
acquisitions at a stated resolution, and comparable to each other. A language
that cannot say that, or says it and cannot execute it on both backends with
the same rows, has not finished.

## 1. What Wave 4b delivers

Thirteen slices in five groups, in the order of §15.

**The record and the store** (§4, §7, §15 slices 0 to 2): the amendments that
the study forced, written into the record before any compiler exists; the
indexes, columns and stores every later slice needs; and the session cache,
which turns the one thing the engine could never compile into a join target.

**The language** (§4 to §6, slices 3 to 6): the ask, a graph of named sets at
one grain each; its clauses, its time rules, its comparability levels, its
document shape, its desugar and its two validators; the catalog that is the
engine's only schema knowledge; the compiler that emits one SQL text per
backend; and the relations.

**Handles, custody and affordances** (§8 to §10, slices 7 and 8): result
handles, saved asks, promotion to a cohort, the custody rows of every new store;
and the affordances by which an editor or a model authors an ask one typed move
at a time without ever rebuilding the document from memory.

**The doors and the CLI** (§12, slices 9 to 11): `/api/ask/*` on `nils serve`
with its caps published under `capabilities.ask`; `nils ask` in the same binary;
the MCP door, curated and bounded.

**The gate** (§13, slice 12): a synthetic registry made up by design, the
fixtures with a row oracle and three outcomes, and the conformance suite that
is the real portability contract.

## 2. What it rests on

Five rulings of the record shape every section:

- **The engine executes, the app edits** (D5, confirmed under R1 by the talk).
  There is one implementation of what a question means, and it is in the
  engine. The notebook app renders and edits documents; it never computes a
  row. `nils ask` is a runner in the same binary, never a second compiler.
- **The query is data, the Metabase way** (R7 of 17). An ask is JSON that a
  person can compose one filter at a time and an agent can build from a
  sentence; the shape is Metabase's clause form (C17), the tree is ours. What
  the reading of Metabase said not to copy is not copied: no generic join
  resolver (the tree has a handful of declared edges), no twelve attribute
  field record, no cached dropdown where the pack already declares the values.
- **Grain and denominators are explicit** (D20, sharpened by C39 to C42): every
  set declares one grain and nothing changes it; every count names its grain;
  every share names its denominator. The dispute that produced three answers to
  one question five times (18 §1) cannot recur, because a count returns rows and
  subjects and the scheme they were counted under.
- **Forgiving by default** (Q1 to Q3): where a clause admits a wider and a
  narrower reading, the engine takes the wider one and the author opts into the
  narrower with `strict: true`. Six months is 186 days; a date known to the
  year is the whole year; "after" admits what could be after. Describe prints
  which reading applied.
- **Identity is external and optional** (D39, Q8, Q9): NILS owns no user
  registry in any wave. Anyone installs the engine or any app and uses it with
  no identity at all; an installation that wants identity adds one component
  and takes its people from a provider, admitted per app. Nothing in this wave
  depends on it, and the catalog's policy reads whatever the principal carries.

And four facts about the engine as built, measured before the talk (18 §1) and
now binding on the design:

1. **Sessions are not compilable.** The scheme resolver is a fixed point pass
   with cross row dependencies (`session.rs`), so a session is either a cache
   built by that resolver or a grouping done in Rust after the SQL, which
   kills push down on the grain that carries a third of the traffic. It is a
   cache (§7). And four call sites derive sessions today with three hard
   coding a first session anchor, so the release and the picker label under
   the wrong anchor whenever a scheme says otherwise; the cache fixes a live
   wrong answer as a side effect.
2. **A subject-day is not a session key.** One subject-day in seven carries
   two or more studies in the reference archive, so `first` cannot be unique
   under any per study grain and a stored pick keyed on a day that no longer
   opens a session disappears silently. The session key is a surrogate (§7).
3. **The store is a compilation target with four gaps.** `Row` is positional
   and its column names are discarded; `query` materialises every row on both
   backends; the Postgres decoder errors on NUMERIC; reads run in autocommit so
   a statement timeout has nothing to attach to. §11 names what closes each.
4. **The membership fact cannot record history.** `cohort_member` is unique on
   (cohort, subject) while its only writer appends a fresh row per join, so a
   re-join is a constraint violation and nothing writes `left_at`. §8 makes it
   an interval log, and the act vocabulary gains cohort verbs with the writer,
   not after it.

## 3. What Wave 4b does not do

- **No assistant, no model, no prompt.** Wave 4c. This wave ships the
  affordances an assistant will use and the MCP door it will call, and it
  measures nothing about a model. The pilot clock of C23 starts in 4c.
- **No notebook UI beyond the seam.** The app is its own repository; this wave
  publishes the schema digest handshake (`capabilities.ask.schema_digest`) and
  the doors, and refuses to serve a directory of loose files (§12.3).
- **No `pair` grain and no `match` relation.** Reserved and staged (D32). The
  family 6 gold reproduces with `near best` plus `pick per: subject`
  (Appendix B).
- **No membership as of a past date** (Q5). A subject removed from a cohort
  is no longer in it, for every question. The interval log is kept for the
  audit; the language reads open intervals only.
- **No origin at any grain** (Q6). Where a file came from is not a question
  the ask answers; custody and the audit answer it. No field names a batch, a
  digest or a folder.
- **No federated merge.** A local ask runs at home; `federated_scope` refuses
  a `local` field under a federated run, and the merge is Wave 5's.
- **No identity delta.** The engine's OIDC items of D39 (JWKS by URL, the
  audience list, the roles claim) land whenever an installation needs them and
  are not on this wave's clock. One rule of D39 does land here because the ask
  door is the first door that serves clinical values broadly: a token with no
  role is refused, never defaulted to reader (§12.4).
- **No browser facing door** (Q10). `nils serve` is for the apps, the agent
  and the CLI; the web application that puts a browser on NILS is a later
  product and carries the browser's rules itself.
- **No Python client**, unless CSV export off a handle slips from slice 10, in
  which case the client lands rather than let the group reach for a database
  driver around the disclosure projection.

## 4. The ask

### 4.1 The shape in one paragraph

An ask is a document naming a set of sets. Every set declares exactly one grain
and nothing ever changes grain. Sets relate through a fixed handful of typed
relations over the catalog's tree: `of` up to an ancestor, `has`, `attach` and
the bind aggregates down to a descendant, `near` sideways inside one subject
between two dated sets, `from` and `algebra` at one grain, `group` as a derived
grain, `same` as sugar over a group. Aggregation is a set other sets read and
filter, so there is no HAVING problem to repair. Standing predicates belong to
the catalog, not to the author. Every named set is a handle in waiting. The
authoring loop is options, apply, diagnose: the engine offers typed moves,
applies the chosen ones and returns the new state by handle, so a model never
rebuilds a document from memory. JSON is canonical, YAML is the human rendering,
the file is `.ask.json`, the verb is `nils ask`, the capability is
`capabilities.ask`, the door is `/api/ask/*` (Q17).

### 4.2 Grains and keys

| grain | key | day | ancestor keys carried | reaches the release |
|---|---|---|---|---|
| cohort | cohort id (the name is a label) | none | | yes |
| subject | subject id (the code is a label) | none | | yes |
| session | `session_cache.id`, a surrogate | `first` | subject | yes |
| stack | stack id | `COALESCE(date_filled, study_date)` | session (under the document's scheme), study, series, subject | yes |
| instance | instance id | study day | stack, session, subject | yes |
| event | `event.id` | `event_date`, with its precision | subject | no |
| group | the by-tuple | none | every ancestor key named in `by` | no |
| pair | reserved, staged | | subject | no |

The session key is a surrogate and never `<subject>:<first>`: `first` moves
when a study date is repaired or a late study arrives, and `(scheme_key,
subject_id, first)` is a non-unique index with `<subject_id>:<first>` a rendered
label printed only where the resolver reports it unique. The event key is the
event table's own id, not a composite carrying the subject code into every key
and handle (C42). Every row of a set carries its grain key, its ancestor keys
and its bindings as flat prefixed columns.

### 4.3 Primitives

| primitive | shape | meaning | compiles to |
|---|---|---|---|
| set | `sets.<name>: {grain, ...}` | a named set of rows at one grain | one CTE `s_<name>`, `AS MATERIALIZED` when referenced more than once |
| from | `from: <name> \| role:<r> \| handle:<id> \| selection:<n>@<v> \| values:<v>` | narrow a same grain set, or start from a library set, a handle, a saved ask or a list | join on the key |
| of | `of: <name>` | containment in an ancestor grain set; its fields and bindings become readable | join on the ancestor key, or EXISTS across the cohort edge |
| algebra | `{op: union \| intersect \| except, sets, tag?}` | set algebra at one grain; `tag` unpivots one row per operand | EXISTS / NOT EXISTS / UNION [ALL] |
| group | `grain: group, group: {of, by}` | the distinct by-tuples of a child set, with aggregates, as a set others read | GROUP BY with `_rows` and `_subjects` |
| near | `[{as, set, window, on?, policy, tie?, order?, optional?, strict?}]` | the one row of a dated set of the same subject inside a signed window | a window CTE with ROW_NUMBER, then rn = 1 |
| attach | `[{as, set, optional?}]` | the one row of a descendant set picked per this grain | LEFT JOIN on the key, INNER when required |
| has | `[{set, window?, on?, min?, max?, as?}]` | the count of related rows, optionally windowed, bounded | EXISTS / NOT EXISTS, else a grouped LEFT JOIN |
| same | `[{as, over, by, min?, all?}]` | sugar: at least `min` rows of a descendant set sharing one value of the `by` tuple | a hidden group set plus two aggregates (§6) |
| bind | `{<name>: <clause>}` | scalars, time functions, per subject sequences, aggregates, derived fields | inline, grouped LEFT JOINs, ROW_NUMBER / LAG / LEAD |
| where | `[<clause>, ...]` (AND) | predicates over everything bound before it | WHERE on the layered subselect |
| pick | `{per, by, n?, ties?}` | the first n rows per ancestor under an explicit preference list | ROW_NUMBER, RANK, COUNT OVER |
| values | `{upload: id}` | an uploaded identifier list resolved through the linkage store | join `values_member` on the key |
| params | `{<name>: {type, value?, description?}}` | typed values beside the document | bound at run, except structural ones |
| scheme | `default \| day \| <registry scheme> \| {…}` | the one session scheme the whole ask is read under | the session cache key |
| keep / out | `keep: [names]`, `out: {set, level, ...}` | which sets become handles, and the answer's shape | projection, paging, the post pass |

Notes on the primitives that carry the weight.

**near** is the only sideways relation and the only place time is compared.
Policies are `nearest` (tie earlier by default, the engine's own rule in
`clinical::nearest`), `first`, `last`, `best` (an explicit order over anchor and
partner bindings) and `any` (a filter only). Windows are signed with a declared
unit and both ends inclusive: `day` is integer arithmetic, `month` is 31 days
per month, `year` is 366 days (Q2); a null bound is unbounded. It exposes
`<as>.date`, `<as>.precision`, `<as>.offset_days` (signed, negative before,
measured to the nearest edge of a coarse date's interval and zero inside it),
`<as>.tied`, `<as>.candidates`, and the partner's fields and bindings, so
`fu.edss.number` is legal. Rows without a partner drop unless `optional: true`,
and the drop is counted by diagnose. `best` is in the first cut because it makes
the family 6 pair task reachable without the `pair` grain.

**has** is the only quantifier syntax. Exists is `min: 1`, none is `max: 0`,
at least n is `min: n`, and "every" is an `except` set with `max: 0`. There is
no other form and no HAVING. The `every` sugar is three clauses, not two:
`except`, then `max: 0`, and `has <universe> min: 1`, so a subject with clinical
rows and no sessions does not pass vacuously; `vacuous: true` opts back in and
describe prints the count that passed on an empty universe. `has` takes the
same `on` anchor override as `near`, so "at least three sessions within five
years of the transition" is one clause at subject grain.

**pick** is one row (or n) per ancestor under an ordered preference list, with
ties reported and never settled by row order; it exposes `pick.tied`,
`pick.candidates` and `pick.rank`. A stored pack pick is the `picked` predicate
instead, which joins on (subject_id, session_day, scheme name) and asserts that
the stored day opens a session under the document's scheme digest, raising
`scheme_mismatch` with a count when it does not.

**bind** carries arithmetic (`+ - * /` with a Double cast so SQLite never
divides integers), `abs`, `round`, `coalesce`, `case`, `concat`; the time
functions `days_between`, `shift`, `age_at` (birthday exact whole years),
`bucket` (truncation, returns text) and `part {unit: year | month | day | dow |
hour}` (extraction, returns an integer); the per subject sequences `ordinal`,
`prev`, `next`; the aggregates `count`, `distinct`, `min`, `max`, `sum`, `avg`,
`list` over a related child or dated set; and the derived fields from the pack
with declared parameters (`acquisition_type`, `field_strength`, `study_day`,
`voxel {third}`, `voxel_min`, `voxel_max`, `resolution`, `signature {level}`,
`course {disease}`). Tuples are always several columns and never a rendered
string, except set valued components (modifier, construct), which are the H5
sorted list. `distinct` of a tuple is not available; group by it instead.

**share** names its denominator. A denominator is an uncorrelated scalar count
over a named set, legal only in a denominator position and only at count or
distinct, so no generic join enters through that door (C39):
`bind: {denom: ["count", {set: <named set>}]}`, or the sugar `share {of: <count
binding>, over: <set name>}` on a group set and on `out`, which desugars to the
same thing and which describe prints by name. A share inside a group over the
group's own `_subjects` is ordinary arithmetic and needs no clause.

**values** goes by reference (C41). The document carries `{upload: id}` and a
digest of the resolved keys; the rows exist only in the request that creates
the upload and die on resolution into `values_member`. A federated request
naming a values source is refused at home. This is C35 honoured: direct
identifiers live only in the linkage store, and no pasted identifier ever sits
in a document that is hashed, stored, pinned, printed by explain or sent to a
peer.

**change** is sugar over an ordered pair of a subject's dated, valued rows:
`change {of: course | <event kind>, disease?, from, to, adjacent?: true}`. It
desugars through the sequence primitive, `prev(value)` over the rows
partitioned by subject and ordered by date then key, so the default reading is
adjacency (a row of the `from` value immediately followed by a row of the `to`
value), `adjacent: false` is the any-earlier reading, and the pair exposes
`<as>.from_date`, `<as>.to_date`, `<as>.precision` and `<as>.gap_days`. A kind
that *is* the transition and carries a date and nothing else needs no sugar: it
is a `min` over that event set.

### 4.4 Composition rules

1. Every set has exactly one declared grain and nothing changes it. To count
   sessions, declare a subject set with `has` over a session set. To see one
   stack per session, declare a stack set with `pick per: session` and `attach`
   it.
2. Grains meet only along the catalog tree and through group: `of` goes up one
   ancestor, `has`, `attach` and the bind aggregates go down or to a group keyed
   by this grain, `near` stays inside a subject between two dated sets, `from`
   and `algebra` stay at one grain. Anything else is `grain_mismatch`; the
   compiler never plans a generic join.
3. An ask is a DAG; file order is irrelevant; a cycle is refused; `out` is the
   answer; `keep` names the other handles.
4. Bindings flow one way: `from` inherits all, `of` exposes the ancestor's
   under its name, `near` and `attach` expose the partner under `as`, `has`
   exposes one number, `same` exposes two, `pick` exposes `pick.*`, `algebra`
   keeps the left operand's (union keeps the common ones plus `tag`), a group
   exposes its `by` fields and its aggregates. The cohort edge exposes the
   cohort key alone when a subject set names a cohort set in `of` (C40), so
   `by: [cohort.id]` is legal with documented fan-out and `_subjects` staying
   DISTINCT; it exposes no cohort bindings.
5. Inside a set the order is source, near, attach, has, same, bind, where,
   pick. `where` reads everything bound before it; `pick` orders over bindings.
6. Quantifiers are counts. There is no other quantifier syntax and no HAVING.
7. Time is always a window with a declared unit, a policy and a tie rule.
   `now` does not exist. A date is data, a parameter or a binding. Age is
   birthday exact whole years.
8. Approximation is explicit and carried by the document: `~=` needs `tol`,
   derived fields carry their parameters in the ref options, and a replay reads
   the same numbers.
9. Roles are library sets shipped by the pack. A stored pick is `picked`, an
   inline pick is `pick`, and any role can be re-derived, overridden or
   extended in a document.
10. Standing predicates belong to the catalog: `superseded_by IS NULL`,
    `withdrawn_at IS NULL` on picks and decisions, `disposition != excluded` on
    stacks (Q16), sensitive kinds excluded for principals without the class.
    Cohort membership reads the open interval. Describe prints them; a document
    cannot switch them off. Scenario exclusions (reformats, scouts, derived) are
    ordinary predicates offered by options as one preset move, and a set that
    reads excluded stacks through an explicit clause draws a diagnose warning.
11. Nothing in a document names a table, a column of a table, an id or a UUID.
    Only catalog paths, axis names, event kinds, cohort names, scheme names,
    role names, level names, handle ids, upload ids and selection names.
    Identifiers enter only through `values` and leave only through
    `out.identifiers`.
12. One scheme per document. Changing it is a different question with a
    different hash.
13. Parameters are typed and live beside the canonical form, and the content
    hash covers the desugared core with parameters unbound and options sorted.
    A parameter that changes the emitted SQL is not a parameter: a window
    unit, a level, a rounding map and a derived field's ref options desugar into
    the core before hashing, exactly as `scheme` does. Only scalars bind.
14. Two validation layers: strict for storage and execution, structural repair
    for authored input, with the fixed taxonomy `unknown_set`, `unknown_field`,
    `ambiguous_path`, `grain_mismatch`, `cycle`, `not_dated`, `not_functional`,
    `ambiguous_parent`, `unknown_value`, `unknown_level`, `forbidden_field`,
    `federated_scope`, `scheme_mismatch`, `selection_outdated`,
    `binding_dropped`, `missing_order`, `not_releasable`, `truncated`,
    `stale_options`. Repair inserts a missing `{}`, wraps a lone clause and
    maps operator and unit aliases; it never rewrites a value and never guesses
    among candidates.
15. Sensitivity is one policy evaluated in the catalog per (principal, role,
    purpose, scope) with no caller opt in. Identifying fields have no field
    record. Quasi-identifying fields (birth date, exact dates, subject code,
    station and institution, free text) are usable in predicates and derived
    fields by anyone who may ask, and projected raw only through
    `out.identifiers` with its audit row. Sensitive fields and kinds are absent
    from options and refused at validate. `local` fields fail
    `federated_scope` under a federated run.
16. Forgiving by default. Where a window, a comparison or a count admits a
    wider and a narrower reading, the wider one applies; `strict: true` on the
    clause asks for the narrower; describe prints which applied (§5.3).

### 4.5 Document shape

```
Ask := {
  ast_version: 1,                       # integer, upgraded on read; unknown keys refused
  name?: string,
  scheme?: "default" | "day" | <registry scheme name> | Scheme,
  params?: { name: {type, value?, description?} },
  values?: { name: {upload: id, digest} },
  sets: { name: Set },                  # a DAG
  keep?: [name],
  out: Out
}
Set   := { grain, from?, of?, algebra?, group?, near?, attach?, has?, same?, bind?, where?, pick? }
Grain := cohort | subject | session | stack | instance | event | group | pair(staged)
Src   := name | "role:<r>" | "handle:<id>" | "selection:<n>@<v>" | "values:<v>" | {handle, pin}
Near  := {as, set, window: Window | Param, on?, policy, tie?, order?, optional?, strict?}
Attach:= {as, set, optional?}
Has   := {set, window?, on?, min?, max?, as?}
Same  := {as, over, by: [Ref], min?, all?}
Pick  := {per, by: [[Clause, dir]], n?, ties?}
Alg   := {op, sets, tag?}
Group := {of: name, by: [Ref]}
Clause:= [op, {opts}, ...args]          # the options map is mandatory
Ref   := ["field", {}, path] | ["axis", {}, name] | ["derived", {params}, name] | ["param", {}, name]
Out   := {set, level: boolean|count|aggregate|record, columns?, measures?, identifiers?, order?, limit?}
```

The JSON Schema is generated from the Rust types with a `description` on every
node and per slot op enums, published at `GET /api/ask/schema`; a hand
tightened variant (every property listed, `additionalProperties: false`,
required lists) is published beside it for guided decoding. `scheme: study` is
not offered: no ratified family needs a per study session, and `by: [study.id]`
on a group set covers it. C17's ratified stage pipeline embeds as a chain shaped
graph through the `pipeline` sugar, so nothing already ratified becomes
inexpressible. Parameter types: `text`, `integer`, `number`, `date`, `list`,
`cohort`, `window`, `rounding`, `level`; the last three are structural and
desugar into the core (rule 13).

## 5. Time

### 5.1 Windows

A window is `{from, to, unit}`, signed, both ends inclusive, in one of three
units. `day` is integer arithmetic on days. `month` is 31 days per month and
`year` is 366 days (Q2): the forgiving reading, chosen because the people who
ask "within six months" mean "not later than six months" and never mean "not
in the seventh calendar month". Describe prints the day count beside the unit
("within 6 months, 186 days each way"). The catalog's window presets are named
by convention (`6 months` = 186 days, `1 year` = 366, `5 years` = 1830) and a
document may always write days. The session labeller keeps its own month rule
for naming sessions, and whenever a month named label and a month window meet
on one row describe prints both rules, because a label is a snap under a
tolerance and a window is a containment test and they can disagree.

### 5.2 Precision

Every stored date carries its precision, row by row: `day`, `month` or `year`
(Q3). The `event` table gains `event_date_precision`; the onset and diagnosis
dates of `subject_disease` and the course assignment date gain the same column
beside them. A kind's declared precision in the catalog is the default the
importer applies when the source has nothing finer; a source that gives a year
is stored as that year at `year` precision, and no day is invented. The
reference registry's transition events, all stored as 1 January today, migrate
to `year` mechanically, since they are one placeholder.

A coarse date denotes the interval it names: 2012 is 2012-01-01 to 2012-12-31,
2012-03 is the whole of March. `part` extracts from it as before; `bucket`
truncates to no finer than its precision; `<as>.precision` is exposed wherever
`<as>.date` is, and diagnose counts the rows whose precision is coarser than
the clause's unit.

### 5.3 Comparing across precision

A comparison against a coarse date takes the reading that could be true unless
the clause says `strict: true`, in which case it takes the reading that must be
true (rule 16):

| clause | forgiving (default) | strict |
|---|---|---|
| `a > d` | a is after d's first day | a is after d's last day |
| `a < d` | a is before d's last day | a is before d's first day |
| `a = d` | a falls inside d's interval | d is at day precision and equal |
| `near ... window` | some day of d's interval lies inside the window around a | every day of d's interval does |
| `offset_days` | the signed distance from a to the nearest edge of d's interval, zero inside it | the same |
| `days_between(a, d)` | the same distance | the same |
| `ordinal`, `prev`, `next` | ordered by the interval's first day, then precision (finer first), then key | the same |

Two coarse dates compare as intervals under the same rule. `age_at` on a coarse
birth date is the age at the interval's last day under forgiving (the youngest
the person could be) and its first day under strict; describe says so. Nothing
here is a parameter: `strict` is part of the clause and of the hash.

### 5.4 Sequences and `change`

`ordinal`, `prev` and `next` run over a subject's rows of one dated set,
partitioned by subject and ordered by date, precision and key. Undated rows
are excluded from sequences with a diagnose count, and every ORDER BY the
compiler emits carries an explicit NULLS LAST (§11.3). `change` (§4.3) is the
one piece of sugar over them and the clause the yardstick stands on: with
adjacency, "a PPMS row immediately followed by an SPMS row"; without, "an SPMS
row with any earlier PPMS row". The tie rule for same-date rows is the key,
and it is attributed as an engine rule plus an ask rule because the event table
has no unique key on (subject, kind, date).

## 6. Comparability

"Relatively the same acquisition" is never one tag and never a field list the
author writes (Q4). It is a **comparability level** the pack declares over the
classifier's axes and the fingerprint's physics, and the rule that orders the
levels is physical: the reason anyone asks is that they mean to compare
images, so a magnetisation prepared gradient echo against a plain gradient echo
with no inversion is a different acquisition at every level however close the
numbers sit, while an inversion time of 1500 against 1490 is the same
acquisition at every level but the exact one.

So the categorical axes (base, technique, modifier, construct, provenance,
acceleration, contrast agent, body part) and the acquisition type (2D or 3D)
compare exactly at every level, and only the numeric physics widen level by
level under rounding thresholds the pack declares. Three levels ship with the
MR pack; a pack may declare levels between them:

| level | categorical axes and 2D/3D | resolution | timing (TR, TE, TI, flip angle) | field strength |
|---|---|---|---|---|
| `exact` | equal | equal | equal | equal |
| `strict` | equal | rounded to 0.1 mm | rounded to 1 ms, 1 degree | equal |
| `loose` | equal | rounded to 0.5 mm | ignored | equal |

The levels live in the pack as data beside the axes (`levels/*.yml`: name,
the fields in the signature, the rounding per numeric field), because they are
knowledge and differ by modality. The word is level and not tier, because the
pack already uses tier for the ordered scan inside an axis (Wave 2 §2).

In the language, `signature {level}` is a derived field at stack grain whose
value is the level's tuple, several columns, with set valued axes as the H5
sorted list; describe prints the level's definition from the pack, its members
and its thresholds. Sessions share an acquisition at a level when the
signatures of their stacks at that level are equal, which is a `group` by the
signature. The sugar `same` on a set writes the common case in one clause:

```yaml
same:
  - {as: comparable, over: good, by: [["field", {}, "t1.sig"], ["field", {}, "flair.sig"]],
     min: ["param", {}, "min_sessions"]}
```

desugars to a hidden group set over `good` keyed by this set's key and the `by`
tuple, with `n = count`, and two bindings on this set, `<as>.largest` (the
largest group's count) and `<as>.groups` (the number of groups), plus the
predicate `<as>.largest >= min`. `all: true` writes `<as>.groups = 1` instead:
every row shares one signature. Both bindings stay readable, so "how many
protocols did this subject's sessions use" is a column.

## 7. Sessions

Sessions are a rebuildable cache built by the one resolver in `nils-session`
over each subject's whole timeline, never by the selection you look through,
read by the query, the release, the picker and the CLI alike (D34). Nothing in
this section is compiled; the cache is a join target.

**Identity and labels are two things.** Session identity is a function of the
scheme's `window_days` alone: grouping runs first and everything else in a
scheme (anchor, naming, collision, unmatched, the explicit CSV) only names what
grouping made. So `session_cache` stores identity keyed by (window, per subject
timeline digest) and `session_cache_study` the membership of studies, and the
labels sit beside identity keyed by the full scheme digest. A label tweak in a
notebook never rebuilds identity; `picked` compatibility compares the window,
not the labels.

**The key is a surrogate**, `session_cache.id`; `(scheme_key, subject_id,
first)` is a non-unique index and `<subject_id>:<first>` a label (§4.2).

**The rebuild unit is the subject.** The cache is keyed by (scheme definition
digest, anchors digest, registry epoch) with a per subject timeline digest, so
an epoch bump rebuilds only the subjects whose timeline changed. A rebuild
diffs old spans against new; where a session's span moved it remaps the stored
picks and handle members that named it or raises a review item, and it never
drops silently. `cache_inline_subjects` and every other threshold are deleted:
the whole archive study read is tens of milliseconds at the reference scale.

**Stored picks re-key.** The picker and the release group today over a subset
of a subject's studies (the role's candidates; the selection's), so their
`first` differs from the resolver's under any window wider than a day. Moving
them to the cache re-keys every stored pick: re-resolve under the recorded
scheme, or withdraw with reason `session_moved`, counted in the migration
report; and a release re-run reports relabelled sessions instead of moving
files silently. `pick` gains `scheme_digest` beside the scheme name, and
`picked` asserts the digest.

**`study` leaves the scheme list.** `by: [study.id]` on a group set is the per
study grain where a question needs one.

**The read door never writes.** `POST /api/ask/run` under a reader principal
creates no table; a cold cache is built by a job under the worker's principal,
or the ask spills into a per connection TEMP table, and on Postgres the door
runs as a SELECT-only role (§12.4).

## 8. Cohorts, selections and handles

### 8.1 The membership fact

One canonical meaning: a cohort is a membership fact, `member_of` and the
cohort grain read facts only (D35). The fact is an **interval log**, not a row
per pair: `cohort_member UNIQUE(cohort_id, subject_id, joined_at)`, at most one
open interval per pair, structured provenance on the row that opened it (who,
when, node, the source: an import, a promotion with its handle id, epoch,
scheme digest and bound parameters, or a hand edit with a reason). The
language reads open intervals only (Q5): a subject removed is no longer in the
cohort for every question, and the closed intervals are the audit's.

The act vocabulary is widened with the writer: `cohort.create`,
`cohort.member.add`, `cohort.member.remove`, `cohort.promote` and
`selection.save` join `Action`, all judgement changing, so each advances the
epoch (4a §13.5).

### 8.2 Saved asks

A saved ask is a **selection**: a question with a version log, addressed as
`selection:<name>@<version>`. A version is immutable; an edit makes the next
version. A stored ask may reference a selection (Q11), and validate pins the
bare name to a version inside the stored and hashed core, so two people saving
the same text a week apart get different hashes only if the referenced question
changed between them, which is what the two documents mean. The handle records
its pinned versions; describe prints "converters as of version 7, 2026-09-07";
diagnose reports `selection_outdated` with a one move "update to version 8".
Validate refuses a selection named identically to a cohort unless it is that
cohort's own source ask, so `member_of X` and `from: selection:X` are never two
numbers under one word.

### 8.3 Promotion

Promotion runs one way, from ask to fact, and consumes a complete, never
truncated, subject grain handle: it opens intervals recording the handle id,
its epoch, scheme digest and parameters as bound, writes the ask hash and the
selection version onto the intervals and the cohort id onto the selection.
Re-promotion appends and never edits; the tool reports when a promoted
cohort's source ask has moved. Promotion is a job.

### 8.4 Result handles

A handle carries id, name, grain, columns with types, row count, content hash,
ask version and pinned selection versions, provenance (who, when, node, pack
version, epoch, scheme digest), disclosure level and suppression applied, and
`values_unresolved: {n, sample}` with the sample gated by the identifier class
and living in the values source. It degrades to exactly the primitive the
release, the pipelines and segment work already consume: a list of keys.

Four bounds. A named handle keeps `handle_member` and `handle_page` for 90
days (Q7, Q14) and then keeps its metadata, hash and ask while dropping the
rows; permanence for a subject set is available exactly one way, promotion;
a handle may not be named unless its desugared ask is stored with it; and
retention for a store is never shorter than for the object that cites it. A
handle is pinned while a release, a selection, a job or a cohort names it. A
session grain handle stores its session ids so `pin` replays from rows and the
cache may evict freely, and its pin path reports `unmatched: n` whenever a
stored key no longer resolves.

`from: handle:<id>` joins stored member keys when the handle's epoch equals
the current one or `pin` is set; otherwise the stored ask is re-evaluated at the
current epoch and a `drift` note (keys added, keys removed) rides on the
result. An expired handle read this way says so.

## 9. The catalog and the policy

The catalog is the engine's only schema knowledge and lives in its own crate,
`nils-catalog`, because release, review and federation need the same policy.
It carries: grains with their keys and days; the tree edges with cardinality
and the standing predicates each carries; fields per grain (type, kind, class,
visibility, `has_values`, fingerprint, remaps, description, caveats,
`ai_context`, provenance) with curation in `catalog_curation` keyed by path so
a re-sync never overwrites it; axes with value tables; event kinds and views
with their declared precision; diseases and course types; identifier
namespaces (names only); cohorts with owner and member count; schemes with
digest and anchor kind; roles and pick models as library sets; comparability
levels; derived fields with parameter schemas and defaults; window presets;
the function table; the caps; the epoch; and the ask-schema digest.

One invariant the compiler enforces: no field that feeds the pack's search
text, a parser predicate or a derived field may carry a non-reproducible mark,
because a column flag cannot make a derivation reproducible when the field is
an input to it. The reproducibility clause the study first proposed is
withdrawn (D38); the writer converges by construction.

Serving is per principal with rule 15 applied inside the provider and no
caller flag. Field listings are grain scoped and paged; the byte budget is a
published capability constant set from a rendering of the real response, and
one grain is on the order of 7 to 10 KB. The policy this wave applies:

- **Roles suffice** (Q8). The ordered ladder of 4a §11.2 (reader, reviewer,
  operator, admin) gates the doors; any role may read clinical values in a
  question. The class labels stay on fields and kinds in the pack, describe
  prints them, and an unordered class grant beside the ladder (D39) lands the
  day an installation needs a person who may act but may not read.
- **Birth date is quasi-identifying and local** (Q15): usable in predicates
  and in `age_at` by anyone who may ask, projected raw only through
  `out.identifiers` with its `read_audit` row, never leaving the node in a
  federated answer.
- **`out.identifiers` writes a `read_audit` row at every role.**
- **Any count an agent principal sees** in options or diagnose passes the
  disclosure projection's k rule, because a bounded count on a narrow
  predicate is a record level fact under another name.

## 10. The affordances

Keyed by (document hash, set, epoch, scheme digest, principal roles, scope),
and served by the engine because the notebook cannot compute them and a model
must not guess them (C20, D36):

- **options**: the set's resolved shape, its describe sentence, diagnostics,
  and typed `moves` with stable small integer ids, templates with holes and
  legal fillers, `presets` and `next`. The bounded count and the preview are
  off by default (`count_on_options: false`, `preview_on_options: false` are
  published constants): every apply changes the hash, so an affordance cache
  misses by construction on every step of an authoring session, and a move
  costs zero queries unless asked.
- **apply**: `apply(document_handle, epoch, moves: [{move_id, args}])` applies
  one move or a list atomically and returns `{document_handle, hash, epoch,
  changed: [{set, describe_sentence}], options}`. It returns a handle, not a
  document; the document JSON is fetched by handle only when something needs
  it (the notebook, a save, an export). Move ids are stable within one options
  response only, and apply refuses a stale list with `stale_options` naming the
  re-call.
- **diagnose**: taxonomy errors with `sets.<name>.<slot>[i]` paths and the
  next call; repairs applied; warnings; a one line zero-row explanation in
  domain words; per clause and per null drop counts; ties per pick; unresolved
  values rows; rows coarser than a clause's unit; the cost class; and the
  **funnel** (Q1): the count of subjects surviving each named set from the
  cohort down, so an author sees where subjects fall out. The leave-one-out
  variant is a job, not a sync call.
- **preview**: 10 rows or the count, by level.
- **describe**: one deterministic sentence per set in deal breaker order
  (membership, then counts, then sameness), the conventions block (windows in
  days, precision and the reading applied, the scheme's month rule beside a
  window's whenever both meet on one row, the level's definition), the
  denominators by name, the mechanism that chose each attached row, and the
  disclosure level.
- **draft**: whole document emission under guided decoding against the hand
  tightened schema, then add only repair, then diagnose.

Both authoring modes ship against the same validator and neither is named
default yet; each is the other's fallback. The move catalog is bounded and
published (opening cap 30 kinds); a question needing a move outside the cap is
composed by `draft`, never by growing the catalog. The error rule is two rules:
never enumerate catalog values, fields or kinds the principal may not see (name
the next call instead), and always list the fillers the author's own document
defines (set names in scope, bindings under a given `as`, a group's `by`
fields, the operand sets of an algebra clause, the available move ids). No
document carrying a values block, a subject code, a UID or a raw path enters a
model context; the few-shot gallery is built by a scrub pass from documents
authored against the synthetic registry, or it is not built.

## 11. Execution on both backends

### 11.1 The pipeline

Parse, upgrade `ast_version`, desugar (a fixed order, add only, so
`desugar(d) == desugar(desugar(d))`), validate strictly, plan, compile,
execute, post pass, handle. One compiler in the engine, one AST, one SQL text
per backend (D37). `nils ask explain FILE --dialect sqlite|postgres` prints
both texts.

Compile: one statement, `WITH s_a AS [MATERIALIZED] (...), ... SELECT ... FROM
s_out`, sets in topological order, each set a layered subselect in the order of
rule 5. `near` becomes a window CTE with ROW_NUMBER; `attach` a LEFT JOIN
promoted to INNER when required; `has` EXISTS / NOT EXISTS or a grouped LEFT
JOIN; `group` a GROUP BY with `_rows` and `_subjects`; `algebra` EXISTS / NOT
EXISTS / UNION; `pick` ROW_NUMBER, RANK and COUNT OVER; sequences ROW_NUMBER /
LAG / LEAD partitioned by subject. Every ORDER BY ends with the grain key, and
every key ordering uses the integer surrogate, so pages and hashes depend on
neither scan order nor text collation. The planner does three things only: it
folds three or more counts over one target into one grouped LEFT JOIN with
SUM(CASE) columns, it turns a required LEFT JOIN into an INNER JOIN, and it
hoists a subject level predicate out of a session or stack set. A set is a CTE
and its count is a real count.

### 11.2 The hooks

Allowed: window functions with frames, WITH and MATERIALIZED, EXISTS, GROUP BY,
CASE, COALESCE, ABS, ROUND, LIKE ESCAPE, UNION, and `->>` at a top level key.
Forbidden outside the named hooks: LATERAL, DISTINCT ON, arrays, regex, math
functions, interval arithmetic, boolean literals, nested JSON paths, HAVING, any
uncast NUMERIC.

| hook | SQLite (bundled 3.53) | Postgres 16 |
|---|---|---|
| H1 `days_between` | `CAST(julianday(a) - julianday(b) AS INTEGER)` | `(a - b)` |
| H2 `shift(d, n, unit)` | `date(d, '+n days')` (months and years are days, §5.1) | `d + n` |
| H3 `age_at` | strftime year plus a `%m%d` comparison | EXTRACT plus `to_char(d, 'MMDD')` |
| H4 rounded key and display | `CAST(round(x * 100.0) AS INTEGER)` with a literal scale, never `10^n`; `round(x, n)` | the same integer scaling; `round(x::numeric, n)::double precision` |
| H5 sorted list | `group_concat(x, ',' ORDER BY x)` | `string_agg(x::text, ',' ORDER BY x)` |
| H6 pixel spacing split | retired by writing `pixel_spacing_row` and `pixel_spacing_col` in the fingerprint | the same |
| H7 placeholders and key lists | `?`, 500-key chunks | `$n`, `$n::text::date`, `= ANY($1::bigint[])` |
| H8 projection casts | `text_of_qualified`; COUNT and SUM to BIGINT; AVG and ratios to REAL | the same, DOUBLE PRECISION |
| H9 timeout and cancel | a watchdog thread on rusqlite's interrupt handle | `SET LOCAL statement_timeout` inside the read transaction |
| H10 `bucket` and `part` | `strftime`, text for bucket, `CAST(... AS INTEGER)` for part | `to_char`, `EXTRACT(...)::int` |
| H11 least and greatest | multi-argument `MIN` / `MAX` | `LEAST` / `GREATEST` |

The eleven hooks are not the portability contract; they are the current
enumeration of one. The contract is: **one fixture exercising every hook and
every disputed construct executes on both backends and its content hashes
agree** (§13.3). Two independent readings found disjoint defects in this
table before a line of code existed, which is evidence the space was never
enumerated, so every fixture runs on both backends and the table grows when a
fixture fails.

### 11.3 The closures

Four divergences are closed rather than hooked. **Null order**: SQLite sorts
NULL first ascending and Postgres last, and `assigned_on`, `echo_time` and
`inversion_time` are nullable, so every ORDER BY the compiler emits for
`ordinal`, `prev`, `next`, `near best`, `pick` and keyset paging carries an
explicit NULLS LAST (rendered as a CASE on SQLite), and undated rows leave
sequences with a diagnose count. **JSON**: `->>` returns a typed value on
SQLite and text on Postgres, so a `->>` read is cast to text on both.
**Case**: `contains` and `starts_with` compile against a `text_*_ci` companion
column written by the fingerprint in Rust (NFKC plus Unicode lowercase),
because the folded columns keep case today, no `case_sensitive_like` pragma is
set, and the same construct over-matches on the laptop and under-matches on
the server. **Ordering**: the integer surrogate everywhere, with the public
text key in projection only.

### 11.4 The store

`Store::query_stream` is a precondition of every cap, not an addition: today
`query` materialises a `Vec<Row>` and the Postgres client buffers, so a cap can
only trim rows already in Rust. `Store::query_with_header` gives the handle
its column names while `Row` stays positional for the crates that read it.
Compiled asks bypass the statement cache on a one-shot path, because
`prepared()` is an unbounded map per handler thread and an ad hoc question app
is a shape generator. The executor opens an explicit read transaction on both
backends, so the SQLite watchdog and the Postgres `SET LOCAL` have something to
attach to (4a §13.6 stays: the store is blocking and the server pools it). Two
indexes come before any fixture, `stack_fingerprint(study_id)` and
`stack_fingerprint(subject_id)`, because a session to stack or subject to
stack set at the reference scale is not measurable without them.

### 11.5 Caps

Caps are contract constants published under `capabilities.ask` with per
deployment override. The mechanism is ratified; the numbers are provisional
until the timing run of slice 9.

| cap | proposed | status |
|---|---|---|
| `sync_timeout_ms` | 20000 | provisional; the constant that bounds work, timed first |
| `sync_max_rows` | 5000 | provisional |
| `sync_max_bytes` | 4 MiB | provisional |
| `page_rows` | 200 (max 1000; MCP doors 50) | provisional |
| `preview_rows` | 10 | provisional |
| `options_values` | 50 | stands |
| `values_inline_rows` | 500 | on the upload call |
| `diagnose_variants` | 24 | job only |

Measured so far (slice 9, the synthetic half of bar 10): on the 48 subject
synthetic registry, SQLite, a debug build, the yardstick runs in 62 ms at
p50 and 64 ms at p95 and its diagnose pass in 473 ms over 31 stages; the
gold documents in under 10 ms. The reference corpus half sets the numbers.

A row cap bounds rows returned, not work: a MATERIALIZED CTE runs to
completion before the outer scan yields its first row, so the timeout is the
only bound on work. A capped run is flagged `truncated: true`; it can be paged
and read, never hashed, released, pinned or used as a gold baseline. Hashes are
BLAKE2b over the projected rows sorted by public key, values rendered by one
Rust renderer, subject codes digested; nothing is ever silently omitted from
the hash of a result that contains it.

## 12. The doors, the CLI, the split

### 12.1 Where the code lives

Three crates, because the session resolver and the policy already have
consumers outside the query layer (D38):

- **`nils-session`** owns the scheme resolver and the session cache. The
  release, the classifier and the CLI already call the resolver.
- **`nils-catalog`** owns the catalog, the curation table, the sensitivity and
  visibility policy and the disclosure projection.
- **`nils-ask`** owns the ask types, desugar and repair, the compiler, the
  executor over Store and Dialect, the affordances, handles and pages, and
  selections. The test for any future move is one line: no crate that
  `nils-release`, `nils-review` or the federation daemon must depend on may
  live under `nils-ask`.
- **`nils-synth`** owns the synthetic registry generator (§13.1) and is a
  normal crate with a normal subcommand, `nils synth`, because "try it with
  made-up data" is how anyone installs NILS without an archive.

### 12.2 The doors

On `nils serve`, one door per operation, authenticated per request, with
`capabilities.ask` carrying the caps, the schema digest and the epoch:

| door | what | path |
|---|---|---|
| `GET /api/ask/schema` | the generated schema and the hand tightened one | sync |
| `GET /api/ask/catalog`, `GET /api/ask/catalog/{grain}` | the catalog, per principal, paged | sync |
| `POST /api/ask/validate` | strict validation, or repair with `mode: repair` | sync |
| `POST /api/ask/run` | the bounded path: rows or a handle, inside the caps | sync |
| `POST /api/ask/jobs` | the unbounded path: 202 with a job id | job |
| `POST /api/ask/explain` | both SQL texts and the plan | sync |
| `POST /api/ask/options`, `/apply`, `/diagnose`, `/preview`, `/describe` | the affordances | sync |
| `POST /api/ask/documents`, `GET /api/ask/documents/{handle}` | a document by handle | sync |
| `PUT /api/ask/selections/{name}`, `GET /api/ask/selections/{name}@{v}` | saved asks, versioned | sync |
| `GET /api/ask/handles/{id}`, `GET /api/ask/handles/{id}/rows` | a handle and its keyset paged rows | sync |
| `POST /api/ask/handles/{id}/promote` | promotion to a cohort | job |
| `POST /api/ask/values` | an identifier upload, resolved on arrival | sync |
| `POST /api/sessions/rebuild` | the cache, for a scheme or a subject list | job |

Exports, send-to, session cache builds, post pass measures over the row cap
and anything that leaves the node are always jobs.

### 12.3 The CLI and the app

`nils ask` is a non-interactive runner in the same binary: `run`, `explain`,
`validate`, `options`, `diagnose`, `describe`, `handles` (list, show, export to
CSV), `selections` (save, list, show), and `gate` (§13). It calls the crate in
process for a standalone registry and the doors for a server, never compiling
on its own, and the same document produces the same content hash either way.
`nils session list|rebuild` reads and rebuilds the cache; `nils synth` writes a
synthetic registry.

The app edits and renders, never executes. One bundle built in its own
repository, with two deployments: embedded in the binary behind a flag (off by
default on a server) on a route prefix whose handler opens no registry
connection and resolves paths inside the embedded set only, for the standalone
laptop; and served from a static front on the server, with the engine serving
only the API. A directory of loose files is refused. `capabilities.ask` carries
the ask-schema digest and the app refuses to run against an engine whose digest
it was not generated from. The app's repository name is 17 §8's open item.

The MCP door stays in the engine, curated rather than generated, with an
explicit per door opt-in (the tool list is not the endpoint list). Model
facing content (tool descriptions, grounding rules, few-shot selection) ships
in the pack, versioned like axes and picks, so a model behaviour change does
not cut an engine release. It serves RFC 9728 metadata, 401 with
`resource_metadata` and 403 with `insufficient_scope`, and states that audience
binding to the MCP resource is a named, dated deviation until a client that
speaks OAuth exists.

### 12.4 Identity and the door's role

Identity is external, optional and admitted per app (D39, Q9). The three modes
of 4a §11.2 stand: `off` for a standalone registry, `token` for machines and
for any installation without a provider, `oidc` for any OpenID Connect
provider. The engine's delta for a provider, none of it shaped for one provider and
none of it on this wave's clock: JWKS by discovery or URL with `kid` caching and
re-fetch, a claim neutral roles claim, an audience list bounded to the apps the
installer registered, `preferred_username` and `email` in a claims cache listed
in custody, `act` recorded beside the principal when present. Roles come from
the application's own entitlements, so they are per app: a person may be a
reviewer through the notebook and nothing through the assistant. Two things do
land in this wave because the ask door is the first to serve clinical values
broadly: **a token with no role is refused**, never defaulted to reader (an
installer binds roles before upgrading), and **the ask door runs as a
SELECT-only role** (Q18): on Postgres a dedicated role with SELECT on the
registry schema and TEMP on the database, `default_transaction_read_only` on
and a `statement_timeout` set on the role; on SQLite the door opens the file
read only with `query_only` set. Every cache is built by a job under the writer
role, never by the door.

## 13. The gate

### 13.1 The synthetic registry

The gate runs on a registry that is made up by design and never a sample of
the archive (Q13). `nils-synth` is a deterministic generator, code plus a seed
plus the declared adversarial cases, never a checked-in dataset: subjects with
birth dates at every precision, a course history per subject with and without
intermediate courses, transition events at year precision beside dated course
rows, an SDMT and an EDSS population, sessions that split and merge under
different windows, one subject-day in seven carrying two studies, two
protocols at 0.5 mm that differ only at the exact level and two that differ at
every level, cohorts with closed intervals, and the yardstick's positive cases
planted so that the expected rows are known before the ask runs. It lands with
slice 1 so every later slice's gate runs on it, and slice 12 completes the
adversarial seed. The private study's counts inform its shapes and nothing
else.

### 13.2 The fixtures and the oracle

C16 asked that the 28 gold tasks reproduce their hashes. The frozen table
carries 28 rows with 23 distinct hashes and 25 distinct texts, two canonicals
that are the empty array and three hashes with no canonical, so a hash is not
an oracle. The gate is therefore stated as N (Q12): roughly twenty inspectable,
non-degenerate assertions after the triage, each a fixture with expected rows
and a declared normalisation, and each fixture has one of three outcomes
(D40): **passes**, **deferred by clause** (the fixture names the staged clause
it needs, `pair` or `match`), or **out of language** (the question is one §14's
limits list names). A fourth outcome does not exist; a fixture that fails for
any other reason fails the gate. The yardstick contributes four fixtures: the
layered primary reading (Q1), the strict reading, the all-share-one reading,
and the same ask over year precision anchors (the precision path). The gold
tasks of Appendix B contribute three.

### 13.3 The conformance suite

Fixture A exercises every hook and every disputed construct on both backends
and compares content hashes: a rounded group key; a projected rounded value; a
tuple distinct refused at validate; a month window crossing a month end; a key
list over 500 keys; a sorted list; AVG and COUNT projected; a `->>` read;
`contains` with a lowercase pattern against uppercase rows; a pick whose tie
falls to the key over UIDs differing at `.` and `#`; a NULL in every nullable
field a sequence orders by; a coarse date under every row of §5.3; a record
longer than `page_rows` paged to the end. The suite is the contract; the hook
table is its enumeration.

### 13.4 The bars

1. **The record carries the amendments** before any compiler code: C39 to
   C42 in the ratification table, C18's clause set walked clause by clause
   against the op table with every rename or drop recorded.
2. **Every fixture passes on both backends** with row diffs against the
   canonicals, each classed identical, differs by declared design change,
   differs by grain choice, or not expressible; the seed's declared adversarial
   cases are present and each is exercised.
3. **The yardstick's four fixtures** return their planted rows, and diagnose's
   funnel names the set where each planted negative falls out.
4. **The conformance suite's hashes agree** on both backends.
5. **The leak test**: publish an ask with an uploaded identifier list, save it
   as a selection, pin a handle, run explain, and grep every registry table,
   the handle store, the explain output and an outbound federated request; any
   hit outside the linkage store fails.
6. **Every new store is in the custody table** with an owner and a retention
   (§14.1); a named handle without a stored ask is refused.
7. **The same document produces the same content hash** standalone and
   against the server, and `apply` returns a handle and never a document.
8. **The read door never writes**: under the SELECT-only role every ask of the
   gate runs, and a cold cache is built by a job.
9. **Sessions agree**: `nils session list`, `nils release run` and the picker
   produce identical labels under an event anchored scheme, a moved span
   produces a review item and no silent pick loss.
10. **The caps are measured**, not asserted: the 26 shapes of the traffic plus
    one diagnose pass on the synthetic registry and on the reference corpus,
    both backends, with and without the two indexes, p50 and p95, and the
    numbers of §11.5 are replaced by the run's.

The gate is a node local job, `nils ask gate`, with canonicals and outcomes in
the repository, and CI runs it on SQLite and on Postgres.

## 14. Defaults settled in this spec

### 14.1 Retention

Every store this wave adds enters the custody table (4a §13.1) from its first
commit, with the installation's data controller as owner and these `kept`
values as the defaults, overridable in configuration and printed by custody as
a proposal until confirmed (Q14):

| store | kept | deleted by |
|---|---|---|
| `handle`, its ask and its hash | for ever | nobody; a handle may be withdrawn with a reason |
| `handle_member`, `handle_page` | 90 days after the last read, longer while a release, a selection, a job or a cohort names the handle | `nils ask handles prune` |
| `values_member` | the lifetime of the handle that cites it; the upload itself is gone on resolution | the same |
| `selection`, `selection_version` | for ever: they hold the question, never subject data | nobody |
| `session_cache`, `session_cache_study` | no retention: rebuildable, dropped on rebuild | the rebuild |
| `catalog_curation` | for ever: no subject data | nobody |
| `read_audit` | for ever, like the rest of the audit | nobody |

Retention for a store is never shorter than for the object that cites it.

### 14.2 Standing defaults

- Stack sets read `disposition != excluded`; `acquisition` is never a silent
  default (Q16).
- Cohort membership reads the open interval; there is no `as_of` (Q5).
- Windows: `month` is 31 days, `year` is 366, both ends inclusive (Q2).
- Precision is stored per row; comparisons are forgiving unless `strict`
  (Q3).
- Comparability is a level from the pack; `loose` is the level describe
  suggests when a question says "relatively" (Q4).
- A handle keeps rows for 90 days; permanence is promotion (Q7).
- A selection reference is pinned to a version at validate (Q11).
- Birth date is quasi-identifying and local (Q15).
- The names: an ask, `.ask.json`, `nils ask`, `capabilities.ask`,
  `/api/ask/*` (Q17).
- The ask door runs as a SELECT-only role and a token with no role is refused
  (Q18, D39).
- The epoch advances on `selection.save` and on every cohort act (4a §13.5
  extended).

## 15. Order of work

**As built (slice 1, schema and store, 2026-09-07).** Migrations 26 to 30
(`SCHEMA_VERSION` 30), each a Rust function over the declaration as every
wave's are: the fingerprint's eight `text_*_ci` companions and
`pixel_spacing_row/col`, filled in Rust for rows a fingerprint job already
wrote because SQLite's `LOWER` folds ASCII only, plus the two indexes; the
precision columns (`observation_type.precision`, `event.event_date_precision`,
`subject_disease_type.assigned_on_precision`), existing rows reading `day`;
`cohort_member` rebuilt as the interval log with its provenance columns;
`session_cache`, `session_cache_study` and `session_label`, `session_scheme.digest`
computed for the schemes kept and `pick.scheme_digest` filled by name; and
`handle`, `handle_member`, `handle_page`, `values_source`, `values_member`,
`selection`, `selection_version`, `catalog_curation` and `handle_read_audit`.
Two things differ from the text above. The identifier read audit is
`handle_read_audit`, because the linkage store already owns a table named
`read_audit` for a reveal. And the year rule for a placeholder date is applied
by the vocabulary load and not by the migration: a kind declares `precision`
in the vocabulary (`SP Transition` is `year`), and each load re-reads a `day`
row of that kind on the placeholder day (1 January for a year, the first for a
month) at the kind's precision, idempotently; the importer writes the kind's
precision on every event it adds. The store gained `query_with_header`,
`query_stream`, `begin_read`/`end_read` (`PRAGMA query_only` on SQLite,
`BEGIN READ ONLY` on Postgres) and `cancel_handle`; a write refused inside the
read transaction aborts it on Postgres, so the bulk path forgets its temporary
tables on rollback. `Action` gained the five acts of §8.1.

**As built (slice 1, the synthetic registry, 2026-09-07).** `nils-synth` is a
crate with one function, `build(registry, plan)`, and a verb, `nils synth
--seed N --subjects N [--manifest FILE]`, into an initialised, empty registry
on either backend, in one transaction, advancing the epoch. The dice are
SplitMix64 written out, so the same seed is the same registry on every
platform and the two backends' manifests compare equal. The first
twenty-four subjects are the yardstick's planted cases: thirteen positives
(two with a second study on one day, one with a session before the
transition that must not count, one whose two MPRAGE timings alternate so it
is comparable at `loose` and `strict` but not at `exact`) and eleven
negatives with one defect each, every case naming the set of the layered
reading it falls out of (`converted`, `followups`, `good`, `comparable`, or
`precision` for the transition known to its year). The background plants the
rest of §13.1: courses with and without an intermediate one, a fifth of the
transitions known to the year, a transition event at `year` precision beside
every SPMS row, EDSS and SDMT at random distances, one subject-day in seven
carrying two studies, five acquisition kits (the 0.5 mm pair; two MPRAGE
timings alternating; an MPRAGE at 3 T and at 1.5 T alternating; 1.0 mm; no
FLAIR), a scanner reformat and a localizer on every study so the standing
disposition predicate has something to exclude, and two cohorts with a
twentieth of one's members left. Instances are not written; nothing in the
gate reads the instance grain yet. The manifest carries the counts and the
cases, and the gate's fixtures are checked against it.

**As built (slice 2, sessions, 2026-09-07).** `nils-session` owns the
cache; the pure resolver stays in `nils-registry::session` and is the only
resolver. `ensure(registry, scheme, anchors, subject, force)` reads every
dated study of every subject (the whole timeline, never a selection), digests
each timeline, and rebuilds only the subjects whose digest changed: identity
rows keyed by (subject, window) with a surrogate id, membership in
`session_cache_study`, and the scheme's labels in `session_label` keyed by the
scheme's digest, so a second scheme with the same window adds labels and
rebuilds nothing. A rebuild matches the resolver's sessions to the stored
ones by the studies they share; a session whose first day moved keeps its
id, its picks made under the scheme (or before picks named a digest) are
re-keyed to the new day, and a `session.moved` review item at subject scope
says so; a session that vanished withdraws its picks and says so too. The
release, the picker and `nils session list` all call `ensure` and then read
`labels_by_study` or `sessions_of`, under the scheme's own anchor (an event
anchor comes from the clinical layer through `Anchors::resolve`), which
closes the three call sites that hard-coded a first-session anchor; the
picker writes `pick.scheme_digest`. `nils session rebuild
[--scheme|--scheme-name] [--anchors] [--subject] [--force] [--json]` is the
verb, queueable through `POST /api/jobs` as `session rebuild`; a door of its
own, `POST /api/sessions/rebuild`, waits for the OpenAPI contract's next
version, which slice 9 cuts for `/api/ask/*`. Not built here: the span
movement rate on a real ingest (§16), and any threshold; the whole-archive
read is the rebuild's unit of work and it is measured by the gate's timing
run.

**As built (slice 3, the ask, 2026-09-07).** `nils-ask` holds the types of
§4.5 as Rust with serde and schemars derives, the clause as a custom
`[op, {opts}, ...args]` array with the options map mandatory, bindings in the
order written, and the source forms of `from`. `desugar` runs in a fixed
order and only adds: an older `ast_version` is upgraded, `pipeline` becomes
sets chained by `from`, a structural parameter (window, rounding, level) is
inlined where it is read and its declaration stays, `every` becomes a hidden
`except` set with `max: 0` plus `min: 1` on the universe, `pairs` a hidden
clone with `near best`, and `same` a hidden group set keyed by the set's key
and the `by` tuple with `<as>.largest` and `<as>.groups` bound on the set;
hidden sets are `<set>__<what>`. `change` and `share` stay as clause ops of
the core with their meaning fixed in §4.3; the compiler emits them. The
content hash is BLAKE2b over the canonical JSON of the core with `name` and
every parameter's value removed, so a value never changes the hash and a
declaration, a window, a level or `strict` does. Validation reaches the
catalog through the `Names` trait, which slice 4 implements over a registry;
it walks the sets in topological order, resolves every path through the
set's own fields, its bindings, a partner's `as`, the `of` ancestor, a
carried level (`subject.birth_date`, `cohort.id`) and a `change` pair's four
fields, and produces seventeen of the nineteen codes with a
`sets.<name>.<slot>[i]` path and a next call; `truncated` and
`stale_options` are run time and come with slices 8 and 9. Three codes are
warnings that leave a document valid: `selection_outdated`,
`binding_dropped` (a union keeps the common bindings) and `not_releasable`
(a kept group or event set is a table to read, never a release, which the
gold C fixture itself does). A bare `selection:<name>` is pinned to its
current version by `pin_selections` before the hash is taken. Repair
inserts a missing options map, wraps a lone clause or relation in its list,
gives an order term without a direction `asc`, and maps operator and unit
aliases; it never touches a value. The generated schema carries a description
on every node; the tightened one closes every struct with
`additionalProperties: false` and lists the op table on the clause's first
slot, and its digest is what `capabilities.ask` will carry. The four
documents of the appendices are the crate's fixtures.

**As built (slice 4, the catalog, 2026-09-07; pack contract v3).**
`nils-catalog` builds `Catalog` from a registry and a pack once per epoch
and pack version: the grains with their keys, days and carried ancestors, the
five edges with their standing predicates, every field of every level (the
catalogue's own columns with their class, plus the fixed fields the
registry, the fingerprint, the session cache and the clinical layer add) with
the pack's visibility applied and the curation of `catalog_curation` laid
over it by path, the axes with their values and labels, the kinds with their
precision, the diseases with their courses, the identifier namespaces, the
cohorts with their open member counts, the schemes with their digests (the
default first), the roles and pick models of the pack, the comparability
levels, the derived fields with their parameters and defaults, the window
presets, the function table by family, the caps of §11.5 as constants, the
epoch and the ask-schema digest. It implements the ask's `Names`, so the
four fixtures validate against the real catalog of the synthetic registry
and the MR pack, which is now a test. The policy of rule 15 lives inside it:
an identifying field has no record, a sensitive field or kind is absent for
a principal without the class (`visible`) and refused at validate, and a
quasi-identifying field is usable in a predicate and in `age_at` by anyone
who may ask but projected raw only with the class (`may_project_raw`). A
level's field listing pages inside `PAGE_BYTES` (8,192) with a cursor on the
last path served, and the stack level needs more than one page, which is
what the budget exists for. The comparability levels are pack data:
`levels/*.yml`, each naming what compares exactly (every axis, always), what
rounds to a step, and what is ignored, checked against the pack's axes and
the fingerprint's physics on load; the MR pack ships `exact`, `strict` and
`loose`, and the manifest's new `levels` key is pack contract version 3. The
validator gained one rule with the catalog: an event kind named by its
literal is checked against the vocabulary, unknown is `unknown_value`,
sensitive without the class is `forbidden_field`. Not here: the release,
the review and the federation still read the registry's own kind flags and
the pack's visibility directly; they move onto this crate when the federated
merge (Wave 5) needs one policy in one place, and `has_values` is not
computed yet.

**As built (slice 5, the compiler, 2026-09-07).** `nils_ask::compile` turns a
desugared, validated ask into one statement per backend: the sets in
topological order as CTEs (`MATERIALIZED` when read more than once), each
set a layered subselect in the order of rule 5, the answer a final SELECT
with keyset paging and a limit. Every grain has one base relation with fixed
aliases (a stack's joins its series, study, subject, fingerprint, the MR
detail row and its session under the document's window and scheme digest),
and every CTE projects the same spine: the key, the subject, the day and its
precision, the carried ancestor keys, the bindings as `b_*`, and the fields
its own clauses and its readers' aggregates name as `f_*`, so a layer above
never touches a base table. `from` a set, a role (the stacks a stored pick
chose), a handle or an uploaded list; `of` through the cohort edge (open
intervals only) or the ancestor key; `algebra` as EXISTS, NOT EXISTS and
UNION over the common bindings; `group` as GROUP BY with `_rows`,
`_subjects` and its aggregates, arithmetic over them in layers above;
`has` as a correlated count, a binding under `as`, a predicate under `min`
and `max`; `pick` as three window layers (`ROW_NUMBER`, `RANK`, the count
over the partition, then `pick.tied` from the ranks, then the cut), ordering
by projected columns and ending on the integer key; the comparisons of
§5.3 against a coarse date, forgiving unless `strict`, with the interval's
last day as a hook. The dialect layer is one struct: H1 to H5, H7, H8, H10,
H11 and the four closures (NULLS LAST as a CASE on SQLite, `->>` as text,
`contains` and `starts_with` on the `_ci` companion, the integer key last in
every ORDER BY); H6 is retired by the written spacing; H9 is the executor's.
Two things the fixture taught before a line was right: a parameter read
twice (`age_at` reads each date twice, a coarse `=` reads each side twice)
binds twice, since SQLite's placeholders are positional, and an integer or
double parameter is cast on Postgres, since the driver sends an int8 that a
placeholder beside an int4 expression would refuse. `nils_ask::exec::run`
opens the read transaction (`PRAGMA query_only` on SQLite, `READ ONLY` on
Postgres), sets `statement_timeout` inside it on Postgres and arms a
watchdog on SQLite's interrupt handle, streams rows through
`query_stream` on a one-shot statement, cuts at the row and byte caps
(flagging `truncated`, which forfeits the hash), and hashes a complete
answer with BLAKE2b over the rows in the answer's order through one
renderer: integers as digits, doubles at nine decimals with trailing zeros
trimmed (the two engines sum an average in different orders and disagree in
the last bit), dates as text, subject codes digested. Fixture A's rows that
this slice owns run on both backends with agreeing hashes: the rounded
group key and the projected rounded value, AVG and COUNT projected, a key
list over 600 keys, a sorted list, `contains` with a lowercase pattern
against uppercase rows, a pick whose tie falls to the key, a NULL in the
field a pick orders by, the coarse date under every comparison of §5.3
with `part` and `days_between`, the cohort edge with `age_at`, a capped
answer marked truncated and paged to the end by key. The rows that need
`near`, the sequences, `->>` on a JSON field and the derived fields come
with slice 6, and the tuple `distinct` refusal with the validator's next
pass. The SELECT-only database role is the door's, in slice 9; every run
here is already inside a read transaction that refuses a write.

**As built (slice 6, the relations, 2026-09-07).** Every set's frame now
records everything the set exposes by name, each name a column of its CTE,
so a reader, a partner or a group reads it by column and never by path: a
`from` source carries its bindings, partners and pick columns along, `of`
carries the ancestor's bindings under `<ancestor>.<binding>`, and the fields
other sets read through an aggregate, a group or a partner are found to a
fixed point before any set compiles, so a partner's partner
(`later.score.number`) is one carried column. `near` is three layers: a
LEFT JOIN of the partner's CTE on the subject and the window, with
`ROW_NUMBER` over the policy's order ending on the partner's key, `RANK` and
`COUNT` over the anchor row; then `<as>.tied` from the ranks; then the cut
at rank one, dropping anchors without a partner unless `optional`. The
window is literal days at compile (the parameter desugared), read as
interval overlap by default and containment under `strict`; the offset is
the signed distance to the nearest edge of the partner's interval, zero
inside it; `nearest` orders by the absolute offset then the day under the
tie rule, `first` and `last` by the day, `best` by the declared order over
the anchor's terms and the partner's under `as`, `any` by the day. `attach`
is a LEFT JOIN of the picked set on this grain's key, inner unless
`optional`. `change` is a window over the subject's course rows (or an event
kind's rows): `LAG` for adjacency or a running `MAX` for any earlier row,
the first qualifying row per subject joined on the subject and exposed as
`<b>`, `<b>.from_date`, `<b>.to_date`, `<b>.precision` and `<b>.gap_days`.
`share` divides a binding by the distinct subjects (or rows) of the named
set; `ordinal`, `prev` and `next` are window functions ordered by the day,
the precision finer first, then the key; `picked` is an EXISTS over the
pick's stacks asserting the role, no withdrawal and the document's scheme
digest. The derived fields read the columns they need, added to the set's
projection: `acquisition_type` coalesces the filled and the read value,
`voxel` is the greatest of the spacings and the named third, `voxel_min`,
`voxel_max`, `resolution`, `study_day`, `course` (the latest course row),
and `signature {level}`, which deviates from §6's "several columns": it is
one text column, the level's exact axes as their sorted values, the
acquisition type and the field strength as read, every rounded physics
number as an integer at its step, joined with a bar, since a group key and
an equality read one column and the pack's level file names the members.
Two things the yardstick taught: SQLite's placeholders are numbered now
(`?N`), because a layer wraps the one below it and an outer expression's
placeholder sits before an inner one in the text while it binds after it
(slice 5's care with binding order held only inside one layer); and the
synthetic registry's loose alternation is now an MPRAGE at 3 T against the
same at 1.5 T, since a plain GRE never enters a set that asks for the MPRAGE
technique and so fell out at `good`, not at `comparable`, the stage it was
planted to exercise. The gate's three fixtures run on both backends with
agreeing hashes: the yardstick returns exactly the planted positives
(fourteen, the exact-only and the year-precision cases among them), every
planted negative falls out where the manifest says, the year-precision case
leaves under `strict` and the exact-only case under `level: exact`; gold B
returns one pair per subject through `near best` and `pick per: subject`
with both scores carried; gold C returns its seven columns over the two
cohorts with no value list in the document. `measures` stays with slice 7;
the `pair` grain stays reserved.

**As built (slice 7, handles and custody, 2026-09-07).** `nils_ask::run`
is the pipeline end to end and the one function the CLI and the doors
call: prepare (desugar, pin, validate, hash), inline the selections,
re-evaluate a drifted handle, compile, execute, the post pass, the
identifiers, the handle, the kept sets. A selection source is inlined
before compile: the stored, desugared ask's sets, parameters and uploads
join the document under a `<name>__v<version>__` prefix with every
reference renamed, the reading set's source becomes the stored answer set,
and the hash was taken before the inlining, over the pinned name and
version, so two people saving the same text a week apart hash alike unless
the referenced question moved. `nils_ask::handle` saves the row of §8.4
(the columns with the type each showed, the count, the content hash, the
desugared ask with the parameters as bound and the selection versions
pinned, the provenance, the disclosure as the scope's, the values
unresolved by upload as counts and positions, never a value), the keys as
`handle_member` when the answer names them, and the rows as `handle_page`
at the page size, every double at nine decimals so a page reads alike on
both backends. A named handle refuses to save without its ask (bar 6); an
unnamed one may. `keep` runs each kept set once more at record level with
no columns and saves it as a handle of keys named `<answer name>/<set>`.
`prune` drops the rows of every handle unread for ninety days unless a
cohort was promoted from it or a stored selection reads it (a release or a
job naming a handle is a later wave's column); `withdraw` drops the rows
with a reason and keeps the record. `from: handle:<id>` joins the stored
keys when the handle's epoch is the current one or `pin` is set; otherwise
the stored ask is re-evaluated into a fresh transient handle, the document
reads that one, and a `drift` note (keys added, keys removed, `expired`
when the rows were already dropped) rides on the outcome; a pinned read of
an expired handle is refused. `nils_ask::values::upload` resolves a list
through the linkage store (the seeded `patient-id` and any type it holds)
and keeps only the shape: the upload id, a digest, the count and one
member per position with the subject or none; the values themselves reach
no table. `nils_ask::selection` saves a version (a judgement changing act,
so the epoch advances), refuses a cohort's name unless the selection is
that cohort's own source ask, and `nils_ask::promote` opens intervals
from a complete subject grain handle recording the handle, its epoch,
scheme digest and parameters, the ask hash and the selection version whose
hash it is, writes the cohort onto that selection, appends on
re-promotion, and reports `source_moved` when the selection has a newer
version than the promoted one; a parameter's value is not part of the
hash, so only a structural edit moves a source. The post pass is Rust over
the rows: `share {of, over}` adds a column, the value over the distinct
subjects of the named set (one count query per set); `stddev`, `median`
and `percentile {of, p}` are scalars over the answer, or, when the answer
is a group and the column is the child's, one column per group computed
from the child's rows read with the group's by tuple (gold C's `stddev
{of: age}` is per cohort); a truncated answer has no measures.
`out.identifiers` is refused unless the door says the role may project
raw, and then each namespace is one column decrypted per subject through
the linkage store, which writes its own read audit, plus one
`handle_read_audit` row (who, which handle, which columns, how many rows,
the epoch); the handle's pages hold the answer before the identifiers.
The custody document spells out each store's retention of §14.1 and names
`nils ask handles prune` as the deleter. The leak test of bar 5 runs on
both backends: five subjects get an identifier in the linkage store, six
are uploaded, the list is run, saved as a selection, read through it,
explained, and projected with the role; then every text cell of every
table of the registry, the handle pages and both explain texts are read,
and the identifier appears in none. Deferred: the disclosure projection
and suppression beyond the scope's classes, and a handle's session ids
for a `pin` that replays from rows, both with the doors of slice 9.

**As built (slice 8, the affordances, 2026-09-07).** `nils_ask::moves` is
the move catalog: thirty kinds, published under the catalog document as
`moves` with `move_kinds_cap`, and a compile time assertion holds the cap.
`options` is pure over the desugared document and the catalog (no query:
its signature has no store): the set's resolved shape (the fields its
grain and its ancestors' levels expose, its bindings, its near partners'
dates and offsets and its attached partners' bindings, its ancestor's
bindings, a group's by fields, `pick.*`), its sentence, the warnings that
touch it, and the moves with ids 1 to n, each a template with holes and
the legal fillers; a hole with no fillers is free text or a number. The
error rule holds by construction: fillers come from `Names` listings
filtered by the scope's classes (a sensitive kind is absent from the kind
move without the class, present with it), and the author's own set names
are always listed (partners for `near`, picked descendants for `attach`,
descendants for `has`, every visible set for `set_out`, `keep_set` and
`rename_set`; hidden `__` sets never). The options token digests the
document hash, the epoch, the scheme digest, the set and the scope.
`nils_ask::document` is the document store of §10: `ask_document` (schema
version 31, in custody as ninety days after the last use) keyed by the
digest of the whole canonical text, so the same text posted twice is one
handle. `nils_ask::affordance::apply` takes a document handle, the epoch,
the token and a list of moves, refuses another epoch or token with
`stale_options` naming the re-call, applies the list atomically on the
authored document (every move is checked against its fillers before any
is applied), validates the result strictly, stores nothing when the
result is invalid, and returns a new document handle with the parent, the
hash, the sets touched with their new sentences, and fresh options for
the same set. `preview` runs the answer with a limit at record and
aggregate level and the one row at count and boolean level. `describe` is
`nils_ask::describe`: one deterministic sentence per set in the order of
rule 5, every clause rendered in words (`the course changes from
{from_type} to {to_type} (adjacent)`), the conventions block (days,
precision and the reading applied, the open interval, the standing
predicates, the scheme's window beside a near window, each level's
members), the denominators by name, the mechanism behind every near and
attach, the disclosure and the answer in words. `draft` parses with add
only repair, diagnoses, and stores the document when it validates.
`nils_ask::diagnose` returns the issues with their paths and next calls
without a query when the document is invalid, and otherwise the funnel:
every named set in topological order, every stage of it (source, each
near, each attach, each has, each where clause, the pick), each stage
one count query over the document cut after that stage with the set's
readers removed, and with `keys` the subject keys surviving, so
`falls_out` names the first stage on the subject's path (a cohort,
subject or session set; a stack or event set is a helper a partner reads)
a subject is missing from; then the answer's where clauses with the rows
before, after and the rows the clause could not judge (its field null),
the ties per picked set, the rows coarser than a day per event set, the
unresolved uploads, the cost class by shape, and a zero row explanation in
domain words naming the first empty stage. On the yardstick the funnel
names the manifest's set for every planted negative: the course cases at
`converted`, the age case at `followups`, the kit cases at `good` (the
attach) or at `answer`'s `has good`, the alternating scanner at
`answer`'s `where comparable.largest`. Deferred: `count_on_options` and
`preview_on_options` stay off with no way to turn them on until a door
carries the flag (slice 9); the leave one out variant of diagnose is a
job; `best` is not a policy a move sets, since it needs an order, which
`draft` composes.

**As built (slice 9, doors and caps, 2026-09-07; OpenAPI contract v2).**
`nils serve` has the doors of §12.2 in `crates/nils/src/ask_doors.rs`,
one per operation, routed before the older doors and checking their own
roles: a reader asks, runs, explains, previews, diagnoses, describes,
posts documents, applies moves, uploads a list and reads handles; a
reviewer saves a selection; an operator queues a promotion or a session
rebuild. The catalog's policy reads the caller's roles from whichever auth
mode supplied them: a reader's scope projects no class, a reviewer's the
quasi identifying fields, an operator's the sensitive kinds, and only an
operator may project identifiers. Every handler thread keeps, beside its
registry connection, the pack, the catalog rebuilt when the epoch moves,
and the reader of §12.4: on SQLite the file opened read only with
`query_only` set (`Store::open_sqlite_read_only`), on Postgres a session
under `--ask-dsn` (the deployment's SELECT only role) or the registry's
own DSN, with `default_transaction_read_only` on either way
(`Registry::open_ask_reader`). The compiled statement runs on that reader
(`Request.reader`, threaded through the runner, diagnose, preview and
draft); the handle, the pages and the audit rows are written through the
registry, and every cache is a job's. `capabilities.ask` carries the caps
(the published defaults of §11.5, a deployment overriding them with
`--ask-caps` as a JSON object), the catalog's schema digest, the ask
schema's digest, the epoch, the move cap, the pack, how the reader is
opened, and the doors. A capped run is flagged truncated and has no hash;
the promotion of a truncated handle is refused. No default reader: an
OIDC caller with no mapped group holds no role, a token carries its roles
as `TOKEN=user@node:reader,operator` (no suffix is every role, a machine
token; an empty suffix is none), and a caller with no role gets 403 at
every door with a message that says an installer binds roles first. The
job path: `POST /api/ask/jobs` stores the document and queues `nils ask
run --document ID`, `POST /api/ask/handles/{id}/promote` queues `nils ask
promote`, `POST /api/sessions/rebuild` queues `nils session rebuild`;
`ask` joins the queueable verbs, and `nils ask run`, `nils ask promote`
and `nils ask time` exist for the worker and the timing (the rest of the
runner is slice 10's). The OpenAPI contract is version 2: version 1 plus
these doors, in a titled pull request. Every reply carries a content
length and is never chunked, because the ask doors answer above tiny
http's 32 KB threshold and a client that reads the bytes it was told
about must get the whole document. The synthetic timing (bar 10, the
synthetic half): `nils ask time` on the 48 subject registry, SQLite, a
debug build, five runs, gives the yardstick a p50 of 62 ms and a p95 of
64 ms with its diagnose pass at 473 ms over 31 stages, gold B 6 ms, gold
C 4 ms, gold A 3 ms; the reference corpus half and the Postgres half run
on the private host and set the numbers of §11.5, which stay provisional
until then. Deferred to slice 10 and 12: the 26 shapes of the traffic as
a timing suite, the CSV export off a handle, and the gate.

**As built (slice 10, the command line, 2026-09-07).** `nils ask` is the
non-interactive runner of §12.3 in the same binary: `run`, `validate`,
`explain`, `options`, `diagnose`, `describe`, `handles` (list, show,
export, prune) and `selections` (save, list, show), beside slice 9's
`promote` and `time`. Every verb builds one JSON document and prints it
through one renderer, so a call reads alike whichever side answered, and
`--json` prints the document itself. `--pack-dir DIR` answers from this
registry, in process; `--server URL` asks a running engine through the
doors, with `--token` or `NILS_TOKEN`. The client is
`crates/nils/src/door_client.rs`: one HTTP/1.1 request per call over the
standard library, no runtime and no TLS stack, reading the body by the
content length the engine now always sends. TLS belongs to whatever sits
in front of the engine, so an `https` URL is refused by name and the
command line speaks to a local port or a tunnel; that is a named,
deliberate limit, not an omission. A listing and a prune stay on the node,
because they read the registry rather than a door: `handles list`,
`handles prune` and `selections list` have no `--server`. `handles export`
writes CSV from the handle's stored pages, so a handle exports with no
compiler, no statement and no database driver; it takes `--out FILE` or
standard output. Bar 7 holds and is a test: the same document produces the
same content hash standalone and against the server, and `explain`,
`describe` and `validate` print the same text either way. A refused
document prints every issue with its path and the call that settles it,
and exits 2. What the tests taught: a server bounded by `--requests N`
hangs the run that waits for it when a verb makes one request fewer than
the count, so the command line's own tests stop the engine instead of
counting its requests. Deferred: `gate` is slice 12's, and the MCP door
slice 11's.

**As built (slice 11, the MCP door, 2026-09-07; pack contract v4).** The
door is `POST /mcp` on `nils serve`, streamable HTTP with no stream of its
own: a JSON-RPC message in, its answer out, a notification answered with
nothing at all, `GET` refused with 405 and `DELETE` accepted with 204. It
speaks `2025-06-18` and `2025-03-26`, and answers `initialize`, `ping`,
`tools/list`, `tools/call`, `prompts/list` and `prompts/get`. Everything a
model is told ships in the pack: contract v4 adds the optional `mcp` key,
one file (`packs/mri/mcp.yml`) holding the content version, the grounding
rules that become the server's instructions, the tools opted in with their
descriptions and their own rules, and the worked examples that become
prompts. The tool list is not the endpoint list, and the loader proves it:
a pack names an operation from a fixed list and the MRI pack opts in nine
of the twelve, leaving `handle` and `selections` served by the doors and
unseen by a model. A tool call runs the ask door itself through
`serve::ask_call`, so the roles, the read only reader, the catalog policy
and the caps are the doors' own, with no second path to the registry. A
domain refusal comes back as `isError` with the message and every issue as
text, because a model reads text and retries; a role refusal comes back as
a protocol error instead, because it is the caller's to fix. Every answer
is bounded: a result is cut at 16 KB of rendered JSON, and rows are cut to
`page_rows_mcp`, a run pointing at the handle for the rest while the rows
tool pages inside a page by `offset` and says `next_offset` while rows
remain. Identity is §12.4's, plus RFC 9728: a public protected resource
metadata document at `/.well-known/oauth-protected-resource` naming the
resource and the authorization servers (`--mcp-authorization-server`), a
401 whose `WWW-Authenticate` carries `resource_metadata`, and a 403 that
says `insufficient_scope`. Audience binding to the MCP resource stays a
named, dated deviation (2026-09-07) in that document until a client that
speaks OAuth exists. The tests drive the transport as a client does, over
a socket: the handshake, the opt-in list, a bounded run paged by offset, a
refusal read as text, an unknown tool and an unknown method, the public
metadata, the two refusals with their headers, and the bar's second half,
a grounding rule edited and an example swapped in a copied pack changing
what the same binary says. What the tests taught, twice: a test server
must never inherit the run's stderr, since a server that outlives a panic
holds the pipe open and hangs the run long after the tests are done.
Deferred: a real MCP client (an inspector, an assistant) connecting to a
deployed engine is the operator's check and not CI's, and the pack's
few-shot gallery stays two worked examples until the assistant wave asks
for more.


Ordering constraints, before the table. Nothing is written before the record
carries the amendments (slice 0). The two `stack_fingerprint` indexes, the
store's stream and header, and the `text_*_ci` columns come before any fixture,
because without them the gate measures the wrong thing. The synthetic registry
comes with the schema, so every slice after it has data. Sessions come before
the compiler, because the compiler needs a join target. The catalog comes
before affordances, because options serve the catalog's policy. Handles come
before the gate, because a gold hash is only taken from a complete handle. The
MCP door comes after the caps are measured, because its page size is one of
them. A schema change lands with the slice that needs it, as every wave so far.

| # | slice | what lands | gate |
|---|---|---|---|
| 0 | Record | C39 to C42 in the ratification table; C17's stage list becomes a DAG; D20 gains `group` and the "nothing changes grain" rule with its denominator half restored; D3 amended (SQLite embedded, Postgres server, DuckDB struck); 03's epoch rule amended to the 4a wording plus the cohort acts; C16 replaced by the row oracle with three outcomes; the interval log recorded; Nima's answers Q1 to Q18 recorded as rulings | The ratification table carries every row, and C18's clause set is walked clause by clause against the op table with every rename or drop recorded. No compiler code exists before this. |
| 1 | Schema, store, synth | Indexes on `stack_fingerprint(study_id)` and `(subject_id)`; `pixel_spacing_row/col`; `text_*_ci`; precision columns on `event`, `subject_disease` and the course assignment, with the 1 January migration; `cohort_member UNIQUE(cohort_id, subject_id, joined_at)` plus structured provenance; `pick.scheme_digest`; `session_cache`, `session_cache_study`; `handle`, `handle_member`, `handle_page`; `values_member`; `selection`, `selection_version`; `catalog_curation`; `read_audit`; five new `Action` variants; `Store::query_stream` and `query_with_header`; the explicit read transaction; `nils-synth` and `nils synth`; SCHEMA_VERSION bump | The migration applies on both backends; insert a membership, close it, insert the pair again succeeds where the old constraint refused; the 1 January rows read back at `year`; `nils synth --seed 1` builds the same registry twice on both backends and its planted counts match its manifest |
| 2 | Sessions | One resolver in `nils-session`; the surrogate id; identity keyed by window, labels by scheme digest; the per subject timeline digest; the span diff raising a review item; release, picker and CLI read the cache; stored picks re-keyed with `session_moved`; `study` dropped from the scheme list; `nils session list|rebuild` and `POST /api/sessions/rebuild` | Bar 9 of §13.4; the cold digest build on the reference corpus is inside the published fraction of `sync_timeout_ms` or the job path is exercised |
| 3 | The ask | Types, desugar (fixed order, add only), strict validation, structural repair, both schemas, selection pinning at validate | `desugar(d) == desugar(desugar(d))` over every fixture; `ast_version` N to N+1 yields identical cores or a migration rewrites stored sugar; every taxonomy code has a fixture producing it with a `sets.<name>.<slot>[i]` path and a next call |
| 4 | Catalog | `nils-catalog`: grains, edges, standing predicates, fields with class and precision, axes, kinds, schemes, roles, levels, derived fields, curation, the policy inside the provider, paging; the synthetic registry's catalog rendered | The stack grain listing renders inside the published byte budget with a working next page; a sensitive kind is absent from options and refused at validate with no caller flag; birth date is usable in a predicate and in `age_at` by a reader and projected raw only through `out.identifiers` with an audit row; release, review and federation call this crate |
| 5 | Compiler | One statement per ask, the fixed per set order, the eleven hooks, the four closures, one-shot statements, timeouts inside the read transaction, the SELECT-only role | Fixture A (§13.3) executes on both backends with agreeing content hashes, under the SELECT-only role |
| 6 | Relations | `near` (five policies, precision aware), `attach`, `has` with `on`, `group`, `same`, `algebra`, `pick`, `picked` with the digest assertion, `share`, `part`, `change` with adjacency, `every` and `pairs` sugar, `signature {level}`, the derived fields | The yardstick's sets compile on both dialects with agreeing hashes and return the planted rows; family 6's pair gold reproduces through `near best` plus `pick per: subject`; family 7's table produces its columns with no value list in the document |
| 7 | Handles and custody | `keep`, `out` levels, `measures`, `identifiers` with `read_audit`, keyset paging, drift notes, uploads, promotion, the custody rows of §14.1 | Bars 5 and 6 of §13.4 |
| 8 | Affordances | `options`, `apply`, `diagnose` with the funnel, `preview`, `describe`, `draft`, the move catalog, the repair taxonomy, `selection_outdated` with its move | `apply` returns a handle and never a document; a stale move id is refused with `stale_options`; options with counts off issues no query; one fixture proves a catalog value the principal may not see is never enumerated and the author's own set names always are; the funnel names the set where each planted negative of the yardstick falls out |
| 9 | Doors and caps | `/api/ask/*` on `nils serve`, `capabilities.ask` with the caps, the schema digest and the epoch; jobs; no default reader; the catalog policy reads the principal's roles from whichever auth mode supplies them | Bar 10 of §13.4 sets the cap numbers from measurement; a capped run is flagged truncated and refused for hashing, release, pinning and gold; a token with no role gets 403 |
| 10 | CLI | `nils ask` with run, explain, validate, options, diagnose, describe, handles and selections in the same binary, in process for a standalone registry and through the doors for a server; CSV export off a handle | Bar 7 of §13.4; `explain --dialect` prints both texts; a handle exports to CSV without a database driver |
| 11 | MCP door | The curated tool list with per door opt-in, model facing content in the pack, bounded pages, `isError` for domain refusals, RFC 9728 metadata, 401 with `resource_metadata`, 403 with `insufficient_scope` | A real MCP client connects over streamable HTTP, lists only the opted-in tools, pages a bounded result, and reads a domain refusal as text; a grounding rule edit and a few-shot swap both ship without cutting an engine release |
| 12 | The gate | The adversarial seed completed, N fixtures with the row oracle and the three outcomes, the H1 to H11 suite, per family in-scope and out-of-scope member lists, the four yardstick fixtures, `nils ask gate` in CI on both backends | Every bar of §13.4 |

Slices 0 to 2 are one chain. Slices 3 and 4 start after slice 1 and are
independent of each other. Slices 5 to 8 are one chain over both. Slices 9
and 10 depend on 8; slice 11 on 9; slice 12 closes.

Importer work the yardstick depends on and this wave does not own: the course
importer carries the source's date at the source's precision.

## 16. Open questions carried into the wave

1. **The cap numbers**, set by the timing run of slice 9 (§11.5).
2. **Whether guided decoding accepts the positional clause form** (`prefixItems`
   in the schema) on the serving stack Wave 4c will use; one afternoon, and
   the hand tightened schema exists for it either way.
3. **The span movement rate** on the first v1 ingest that keeps dateless
   studies, which decides whether the per subject rebuild unit was worth its
   five write paths (D34).
4. **The app's repository name** (17 §8), which this wave does not need but
   `capabilities` will report.
5. **The web application's name**, which is not `nils-server`, because that
   reads as `nils serve` (Q10).
6. **Whether the move catalog stays under 30 kinds** once the fixtures are
   authored through it; if a validating document needs materially more,
   compose-by-choosing is a refinement mechanism only and this spec says so
   rather than widen the cap.

## Appendix A. The yardstick in the language

The question, as Nima stated it: subjects with at least three follow-ups after
transitioning from PPMS to SPMS, sessions close to an EDSS and an SDMT within
six months, all after 40 and before 50, scans with a 3D MPRAGE and a 3D FLAIR at
0.5 mm isotropic, relatively the same acquisition across sessions. The primary
reading is layered in deal breaker order (Q1): in the cohort with the course
change and inside the age bounds; then at least three sessions after the
transition that each meet the per session conditions; then, among those, at
least three sharing one acquisition at the chosen level. Against the synthetic
registry, whose data is made up by design.

```yaml
ast_version: 1
name: PPMS-to-SPMS converters with three comparable follow-ups between 40 and 50
scheme: default
params:
  disease:      {type: text,    value: "Multiple Sclerosis"}
  from_type:    {type: text,    value: PPMS}
  to_type:      {type: text,    value: SPMS}
  score_window: {type: window,  value: {from: -6, to: 6, unit: month}}   # 186 days each way, inclusive
  age_from:     {type: integer, value: 40}
  age_to:       {type: integer, value: 50}
  iso_mm:       {type: number,  value: 0.5}
  tol_mm:       {type: number,  value: 0.05}
  min_sessions: {type: integer, value: 3}
  level:        {type: level,   value: loose}     # a comparability level of the pack

sets:
  # the transition: the first SPMS course row immediately preceded by a PPMS row
  # (adjacent: false would accept PPMS, then anything, then SPMS)
  converted:
    grain: subject
    bind:
      transition: ["change", {of: course,
                              disease: ["param", {}, "disease"],
                              from: ["param", {}, "from_type"],
                              to: ["param", {}, "to_type"],
                              adjacent: true}]
    where:
      - ["not_null", {}, ["field", {}, "transition.to_date"]]
      - ["not_null", {}, ["field", {}, "birth_date"]]     # a missing birth date excludes explicitly

  # follow-up sessions after the transition, inside the age window
  # (the transition may be known to the year: "after" is forgiving, section 5.3)
  followups:
    grain: session
    of: converted
    bind:
      age: ["age_at", {}, ["field", {}, "subject.birth_date"], ["field", {}, "first"]]
    where:
      - [">",  {}, ["field", {}, "first"], ["field", {}, "converted.transition.to_date"]]
      - [">=", {}, ["field", {}, "age"], ["param", {}, "age_from"]]
      - ["<",  {}, ["field", {}, "age"], ["param", {}, "age_to"]]

  # the two required acquisitions, one per session, with their signatures at the level
  t1:
    grain: stack
    of: followups
    where:
      - ["=",  {}, ["axis", {}, "base"], "T1w"]
      - ["=",  {}, ["axis", {}, "technique"], "MPRAGE"]
      - ["=",  {}, ["derived", {}, "acquisition_type"], "3D"]
      - ["~=", {tol: ["param", {}, "tol_mm"]},
               ["derived", {third: slice_thickness}, "voxel"], ["param", {}, "iso_mm"]]
    bind:
      sig: ["derived", {level: ["param", {}, "level"]}, "signature"]
    pick: {per: session, ties: report,
           by: [[["field", {}, "n_instances"], desc], [["field", {}, "id"], asc]]}

  flair:
    grain: stack
    of: followups
    where:
      - ["=",   {}, ["axis", {}, "base"], "T2w"]
      - ["has", {}, ["axis", {}, "modifier"], "FLAIR"]
      - ["=",   {}, ["derived", {}, "acquisition_type"], "3D"]
      - ["~=",  {tol: ["param", {}, "tol_mm"]},
                ["derived", {third: slice_thickness}, "voxel"], ["param", {}, "iso_mm"]]
    bind:
      sig: ["derived", {level: ["param", {}, "level"]}, "signature"]
    pick: {per: session, ties: report,
           by: [[["field", {}, "n_instances"], desc], [["field", {}, "id"], asc]]}

  edss: {grain: event, where: [["=", {}, ["field", {}, "kind"], "EDSS"]]}
  sdmt: {grain: event, where: [["=", {}, ["field", {}, "kind"], "SDMT"]]}

  # follow-ups that satisfy every per-session condition (near and attach drop rows with no partner)
  good:
    grain: session
    from: followups
    near:
      - {as: edss, set: edss, window: ["param", {}, "score_window"], policy: nearest, tie: earlier}
      - {as: sdmt, set: sdmt, window: ["param", {}, "score_window"], policy: nearest, tie: earlier}
    attach:
      - {as: t1,    set: t1}
      - {as: flair, set: flair}

  answer:
    grain: subject
    from: converted
    has:
      - {set: good, min: ["param", {}, "min_sessions"], as: n_good}
    same:
      - {as: comparable, over: good,
         by: [["field", {}, "t1.sig"], ["field", {}, "flair.sig"]],
         min: ["param", {}, "min_sessions"]}

keep: [converted, followups, good, answer]
out:
  set: answer
  level: record
  columns:
    - ["field", {}, "code"]
    - ["field", {}, "transition.to_date"]
    - ["field", {}, "transition.precision"]
    - ["field", {}, "n_good"]
    - ["field", {}, "comparable.largest"]
    - ["field", {}, "comparable.groups"]
  order: [[["field", {}, "code"], asc]]
```

What describe prints for it: sessions under scheme default; the transition is
the first SPMS course row immediately preceded by a PPMS row; follow-ups are
sessions after that date (forgiving where the date is known to the year) on
which the subject is 40 to 49, birthday exact; an EDSS and an SDMT within 6
months, 186 days each way, of the session's first day, earlier wins a tie; one
3D MPRAGE T1w and one 3D FLAIR T2w acquisition each isotropic at 0.50 mm within
0.05, third dimension slice thickness, most instances wins, ties reported;
excluded stacks are not read; at least 3 such sessions, of which at least 3
share one acquisition at level loose (base, technique, modifier, construct,
provenance, acceleration, contrast agent, body part and 2D/3D exact,
resolution rounded to 0.5 mm, timing ignored). What diagnose prints beside it:
the funnel, subjects in `converted`, with a follow-up, with a good session,
with three, with three comparable.

The three other fixtures are one line edits, and each changes the hash: add
`bad: {grain: session, algebra: {op: except, sets: [followups, good]}}` and
`{set: bad, max: 0}` to `answer.has` for the strict reading; `all: true` on the
`same` clause for one acquisition over every good session; and the same
document over a registry whose transitions are year precision events, with
`transition` bound as `["min", {set: sp_transition}, ["field", {}, "date"]]`
for the precision path.

## Appendix B. Three gold questions in the language

**Gold A, family 3, the grain regression.** The question that was asked five
times and answered three ways. `level: count` returns rows and subjects, so
the dispute cannot recur: the answer names both numbers and the scheme they
are counted under.

```yaml
ast_version: 1
name: sessions holding both roles at high field
scheme: default
params:
  cohorts:   {type: list,   value: [cohort_a, cohort_b]}
  tesla:     {type: number, value: 7.0}
  tesla_tol: {type: number, value: 0.25}
sets:
  scope:   {grain: cohort,  where: [["in", {}, ["field", {}, "name"], ["param", {}, "cohorts"]]]}
  people:  {grain: subject, of: scope}
  visits:  {grain: session, of: people}
  flair:
    grain: stack
    of: visits
    where:
      - ["=",   {}, ["axis", {}, "base"], "T2w"]
      - ["has", {}, ["axis", {}, "modifier"], "FLAIR"]
      - ["=",   {}, ["derived", {}, "acquisition_type"], "3D"]
      - ["~=",  {tol: ["param", {}, "tesla_tol"]},
                ["derived", {}, "field_strength"], ["param", {}, "tesla"]]
  mp2rage:
    grain: stack
    of: visits
    where:
      - ["=",  {}, ["axis", {}, "technique"], "MP2RAGE"]
      - ["~=", {tol: ["param", {}, "tesla_tol"]},
               ["derived", {}, "field_strength"], ["param", {}, "tesla"]]
  both:
    grain: session
    from: visits
    has:
      - {set: flair,   min: 1}
      - {set: mp2rage, min: 1}
keep: [both]
out: {set: both, level: count}
```

**Gold B, family 6, session pairs with the nearest score.** Two sessions of
one subject four to five years apart, each within a year of an EDSS, keeping
the pair whose scores sit closest to their sessions. This is the task that
justifies keeping `best` and staging the `pair` grain.

```yaml
ast_version: 1
name: session pairs four to five years apart with scores
scheme: default
params:
  cohort:      {type: cohort, value: cohort_a}
  gap:         {type: window, value: {from: 4, to: 5, unit: year}}
  score_reach: {type: window, value: {from: -366, to: 366, unit: day}}
sets:
  scope:  {grain: cohort,  where: [["=", {}, ["field", {}, "name"], ["param", {}, "cohort"]]]}
  people: {grain: subject, of: scope}
  edss:   {grain: event,   where: [["=", {}, ["field", {}, "kind"], "EDSS"]]}
  scored:
    grain: session
    of: people
    near: [{as: score, set: edss, window: ["param", {}, "score_reach"],
            policy: nearest, tie: earlier}]
  paired:
    grain: session
    from: scored
    near:
      - as: later
        set: scored
        window: ["param", {}, "gap"]
        policy: best
        order: [[["+", {}, ["abs", {}, ["field", {}, "score.offset_days"]],
                          ["abs", {}, ["field", {}, "later.score.offset_days"]]], asc],
                [["field", {}, "later.first"], asc]]
    bind:
      distance: ["+", {}, ["abs", {}, ["field", {}, "score.offset_days"]],
                          ["abs", {}, ["field", {}, "later.score.offset_days"]]]
      gap_days: ["days_between", {}, ["field", {}, "later.first"], ["field", {}, "first"]]
    pick: {per: subject, n: 1, ties: report,
           by: [[["field", {}, "distance"], asc], [["field", {}, "first"], asc]]}
keep: [paired]
out:
  set: paired
  level: record
  columns:
    - ["field", {}, "subject.code"]
    - ["field", {}, "first"]
    - ["field", {}, "later.first"]
    - ["field", {}, "gap_days"]
    - ["field", {}, "score.number"]
    - ["field", {}, "later.score.number"]
    - ["field", {}, "pick.tied"]
  order: [[["field", {}, "subject.code"], asc]]
```

**Gold C, family 7, the per cohort table.** The family the first model had
quietly lost: it needs the cohort key exposed for grouping (C40) and a
denominator that names a set (C39). Both shares are here: one inside the group
over its own subjects, one over a named set.

```yaml
ast_version: 1
name: per-cohort demographics
scheme: default
params:
  cohorts: {type: list, value: [cohort_a, cohort_b, cohort_c]}
  as_of:   {type: date, value: "2026-01-01"}
sets:
  scope:  {grain: cohort,  where: [["in", {}, ["field", {}, "name"], ["param", {}, "cohorts"]]]}
  people:
    grain: subject
    of: scope
    bind:
      age:    ["age_at", {}, ["field", {}, "birth_date"], ["param", {}, "as_of"]]
      female: ["case",   {}, ["=", {}, ["field", {}, "sex"], "F"], 1, 0]
  by_cohort:
    grain: group
    group: {of: people, by: [["field", {}, "cohort.id"]]}
    bind:
      n_female:   ["sum", {set: people}, ["field", {}, "female"]]
      mean_age:   ["avg", {set: people}, ["field", {}, "age"]]
      min_age:    ["min", {set: people}, ["field", {}, "age"]]
      max_age:    ["max", {set: people}, ["field", {}, "age"]]
      pct_female: ["/",   {}, ["field", {}, "n_female"], ["field", {}, "_subjects"]]
keep: [by_cohort]
out:
  set: by_cohort
  level: aggregate
  columns:
    - ["field", {}, "cohort.id"]
    - ["field", {}, "_subjects"]
    - ["field", {}, "n_female"]
    - ["field", {}, "pct_female"]
    - ["field", {}, "mean_age"]
    - ["field", {}, "min_age"]
    - ["field", {}, "max_age"]
  measures:
    - {share:  {of: "_subjects", over: people}}   # this cohort's share of the whole scope
    - {stddev: {of: age}}
  order: [[["field", {}, "cohort.id"], asc]]
```

A subject in two cohorts contributes to two groups (documented fan-out) while
`_subjects` stays DISTINCT, which is the family 1 fact that a third of subjects
are multi-cohort, made visible rather than averaged away. Describe prints
"share of the subjects of `people`", so no percentage is nameless.

## What the language cannot express

Pairs and n-tuples as rows (the `pair` grain is reserved and staged); relations
between different subjects (matched controls, twins), with a `match` relation
scheduled on the reserved grain; recursion and chains of unbounded length;
sessions under any rule but the document's one scheme; membership as of a past
date; where a file came from; arithmetic across unrelated sets except
aggregates on a common ancestor and the denominator clause; regex, JSON path
predicates into private elements, fuzzy search; any relation to "now";
identifier pattern matching (which is why a population defined by an
identifier prefix must be named as a cohort, a saved selection or an upload);
questions about the judgement record (evidence, decision history, jobs,
audits, as-of replays); nearest by anything but time; file grain
reconciliation against disk; individual level alignment across federation
nodes; statistics beyond count, distinct, min, max, sum and avg inside sets,
with median, stddev and percentile available only as `out.measures`.
