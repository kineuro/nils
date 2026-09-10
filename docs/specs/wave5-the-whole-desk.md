<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Wave 5: the whole desk

The specification of the wave in which the desk becomes the face of NILS rather
than a shell over its doors. It follows `wave4c-the-assistant.md`, rests on the
suite that wave built, and cites the record by id (`docs/decisions/20-wave5-the-whole-desk.md`:
Nima's rulings R12 to R14, decisions D53 onward, amendments C49 onward, and the
study of 2026-09-10 that read Metabase from its source and the previous
prototype from its own).

Wave 4c closed with the engine answering at fifty-seven doors, four stations that
propose and never decide, a gateway that holds every model credential, and a desk
that shows each of those as a tab. The engine can take a tree of DICOM through
digest, classification, review, identity rules, selection, release and a BIDS
tree, with a job and an audit row at every step; almost none of that is visible
as work a person does. The previous prototype had the opposite fault and got one
thing right: its hub told a person what NILS could do for them and let them walk
in, its cohort pipeline read as stages a person watched move, and its quality
control page put a viewer beside the judgement. It did that on a foundation
nobody could keep. This wave builds that face on the foundation v1 has.

The measure of the wave is stated in three sentences. A person who installs the
one binary on a laptop opens the same desk the group opens through its portal,
with the parts they have and none of the ones they lack, and it looks like one
product. A question is built by clicking and by talking, two hands on one
document, with the counts moving as it is built, and the conversation stays
with the document. And the assistant can be told to do a night's work and does
it as jobs a person can see, stop and audit, proposing anything it may not
undo, and nothing it does widens a person's reach or leaks a row.

## 1. What Wave 5 delivers

- **Two rules that every slice obeys** (§4). The laptop rule: the same desk on any
  machine, from the binary, with nothing beside it required. The site rule: the
  group's own deployment is reachable through its portal from the first slice,
  updated on every merge, so the wave is seen as it is built.
- **The look** (§5): the group's design language as the desk's default theme,
  tokens and type and the mark, held in files a deployment may replace, so the
  product is not the group's site and the group's site is not the product.
- **The shell** (§6): Home, which says what the registry holds, what needs the
  person, what is running and what changed; the sections regrouped by the life of
  the data; a page for every object with one timeline; and the rail, one
  conversation on every page that knows the page it is on.
- **The question workbench** (§7): a question starts from anything or nothing,
  every step carries its live funnel, a condition is added from one picker across
  every set or in words in the rail, a proposal lands on the document as a diff,
  and the conversation belongs to the document's lineage.
- **Data, Review and Release as places** (§8): a batch as a stage strip, judgement
  with the evidence beside it, and the boring, complete page for what leaves.
- **The assistant's ladder** (§9): read; run what can be undone under a standing
  permission the person grants per verb; propose what cannot. An operator station
  that plans a chain of jobs, a scheduler that fires it on an engine event, a jobs
  inbox, and the teaching loop made visible with its gates kept.
- **Settings as the operator's console** (§10): every part with its version and
  health, generated from what the parts publish; places, the named locations with
  a role and the guarantees the engine checks; the database; audit read where it
  is written; and installing and updating from the desk through a supervisor.
- **The viewer, found by measurement** (§11): a study on the largest scans the
  group has, before Review is built around a library.
- **The engine's additions** (§12), each a door or a policy, none a new kind of
  thing.

## 2. What it rests on

Everything of Wave 4c: the trust list and the five entitlements (4c §5), the
desk's shape and its proxy (4c §7), the app registry (4c §4.4), the capabilities
document (4c §4.3), Kvasir's purposes, admission and credentials (4c §8), the
station framework, the verdict and the seam (4c §9), the ask language of Wave 4b
with its options, apply, diagnose and draft doors, the handle with its
declaration block (4b §8.4), and custody (4a §12).

Two studies of the record: the reading of Metabase from its source
(`20 §2`, eighty findings that survived three verification lenses and the
critic's eighteen gaps), and the reading of the previous prototype's hub, cohort
pipeline, quality control page and agent (`20 §3`).

Three rulings (`20 §1`). R12: the desk is a workbench designed toward one scene,
not a directory of doors. R13: the five answers of 2026-09-10 (the ladder on the
group's identity provider, the supervisor done and shown, the viewer by benchmark,
places and the database in Settings, teaching now). R14: the design language of
the group is the default look, the group's deployment is on the portal from the
first slice, and none of it may make NILS depend on the group's machines.

## 3. What Wave 5 does not do

- It does not add a chart builder or dashboards. The registry's answers are
  counts, tables, handles and trees; the funnel is the honest picture.
- It does not let the assistant change state without a job or a proposal. The
  ladder makes it able to work; the job makes the work legible.
- It does not build a second portal. The group's portal says which systems a
  person may enter; Home says what needs them inside this one.
- It does not move a ratified rule of disclosure. Rows of people appear only
  under the declaration's disclosure through the gated door with its audit row; a
  truncated result is read and never released; the answer is a handle.
- It does not choose a viewer before measuring one (§11).
- It does not promote a fine-tuned model on anyone's word. The bench and the
  admission suite are the gate, as `08` and 4c §9.9 say.
- It does not build federation's surfaces (`14`); they arrive with their wave.

## 4. The two rules

### 4.1 The laptop rule (R14)

`nils` installed on a laptop, run as `nils serve` with the desk beside it in
`off` mode, shows the same Home, the same sections, the same workbench and the
same Settings as the group's deployment, with these differences and no others:
the sections and controls a missing part would serve are absent by the predicate
rule of 4c §7.2; places are directories the person chose, with the guarantees
they carry and no more; the assistant is absent until a Kvasir answers; the
supervisor is absent and Settings shows the command instead of the button; the
theme is the default one. Nothing in the desk, the engine or the assistant may
read a path, a host name, a group name or a pool of the group's own. A test in
the desk's gate starts the suite from the binary in a temporary directory with a
synthetic registry and walks every section (§13).

### 4.2 The site rule (R14)

The group's deployment runs from the first slice of the wave, on the synthetic
registry until the group decides otherwise, reachable through the portal by the
people the identity provider names, updated on every merge to `main` of each
part by the same deploy-on-merge path the group already uses for its other
services. The desk shows, in the shell, which registry it is on when that
registry is synthetic. The deployment's specifics (the host, the mounts, the
identity provider's application, the portal entry) are the record's (`20 §6`) and
never this document's.

## 5. The look

### 5.1 The theme as a file (D53)

The desk ships one theme: `web/src/ui/theme.css`, the tokens, and
`web/public/brand/`, the mark and the favicon. The tokens follow the group's
design language: a warm ground, a plum accent, an ink that is not black, a serif
for headings and a sans for everything else, a mono for documents and paths. The
values are the group's own (`20 §5`); the names are the product's. A deployment
replaces the look by replacing the two files; nothing else references a colour,
a face or an image directly, and a lint rule in the desk's gate refuses a literal
colour outside the theme file.

The tokens, defined twice, light on `:root` and dark under the scheme query with
the same key set, so a key that exists in one exists in the other:

- surfaces: `--n-bg-page`, `--n-bg-raised`, `--n-bg-sunken`, `--n-bg-hover`,
  `--n-bg-selected`;
- text: `--n-text`, `--n-text-dim`, `--n-text-faint`, `--n-text-on-brand`;
- lines: `--n-line`, `--n-line-strong`;
- the brand: `--n-brand`, `--n-brand-hover`, `--n-brand-soft`;
- six intents, each a quad of foreground, background, border and icon: neutral,
  brand, ok, caution, blocked, and **gated**, the product's own, for the
  projection door, which must look like neither an error, which people dismiss,
  nor a warning, which people ignore;
- seven grain tints, one per grain, low in chroma and close in lightness, spent
  on the thing a person loses track of in this language: what a set counts;
- one control height, `--n-control`, and a four-step spacing scale, so the
  sections stop feeling like four applications.

Type: three faces, a serif for headings, a sans for the body, a mono for
documents, paths, hashes and codes, loaded from the deployment's own origin,
never from a third party, so a desk on an air-gapped machine looks the same as
one on the portal.

### 5.2 The mark (D54)

NILS gets a mark of its own in the family of the group's marks: the same square
ground and the same cream stroke as the group's data bridge, carrying the rune
naudiz. The mark is the favicon, the top-left of the shell, and the icon of the
desk on the portal. The glyph is the record's to settle (`20 Q21`); the desk
reads it from `web/public/brand/nils-mark.svg` and asks nothing else of it.

## 6. The shell

### 6.1 Home (D55)

The first page after login, and the answer to "what should I do". Four bands,
each a predicate on the parts present and the person's entitlement:

1. **What the registry holds.** Subjects, sessions, stacks, by cohort and by
   pack version, from the summary door (§12.1); the registry's epoch and, when it
   is synthetic, that word.
2. **What needs you.** Review items assigned to or open for the person, by what
   they cost if wrong; quarantined files by reason; proposals from a station
   waiting for a decision; jobs that failed. Each a link to the object.
3. **What is running.** Jobs with their clocks, from the jobs door and the event
   stream; the person's own first.
4. **What changed since you were here.** Batches that landed, handles that were
   produced, adoptions and releases, since the person's last visit, from the
   timeline door (§12.2).

The previous prototype's hub counted cohorts and stages; this one is the to-do
that is true. It never shows a row of a person.

### 6.2 The sections (D56)

Home, Ask, Data, Review, Release, Pipelines, Assistant, Settings, and one section
per registered app. Operations is gone: jobs live where they are queued and in
the inbox (§9.4); review, decisions, overlays, rehearsals and identity rules are
Review; releases, handovers and custody are Release; audit and sessions are
Settings. The predicate rule of 4c §7.2 stands: a missing part, a wrong document
shape and a missing entitlement each remove a section or a control, and the
three named states render by name.

### 6.3 Objects and their pages (D57)

Every noun of the registry has a page: a cohort, a subject (gated), a session
(gated), a batch, a document, a handle, a release, a handover, a pack, an overlay,
a rule, a job, a review item, a conversation. The page shows the object, what
hangs off it, and one **timeline**: every event on it in order, edits, runs,
decisions, proposals, adoptions, releases, each linking to the object it
produced. The sections are entry points; the pages are the product. A page's
address is stable (`#cohort/3`, `#document/9f3c`, `#handle/71`), so a link in an
email opens the object.

### 6.4 The rail (D58)

The right-hand third of every page is the assistant's conversation, present
when the assistant is and the person holds `assist`, absent otherwise without a
gap. It knows which object is under it: on a batch page it reads the batch, on a
document it proposes a diff, on a review item it explains the rule, on Home it
plans. The Assistant section is the same conversation at full width for work
that is not about one object. The rail's context is typed (§9.2): each page
declares what it contributes, and the type is the allowlist; no row payload can
enter it, and a test in the assistant's gate asserts that.

### 6.5 The wait, the failure, the empty (D59)

One wait component in three sizes, re-timed for a station whose turn takes
twenty seconds: the phase name and an elapsed count from the first second,
"usually about N seconds" at the station's measured median, "longer than usual"
past its ninetieth percentile, the browser tab's title changed for an engine run
so a person who switched to another system sees it finish. One failure component
with five outcomes the engine chooses (too long, unavailable, not permitted,
invalid, internal), one sentence and at most one action each, the raw text
behind a closed disclosure and shown only when the engine tagged it safe (§12.6).
Every non-success ending of a station turn has its own sentence and one typed
next move; stopped-by-you is a quiet line and not an alert. Zero rows is an
answer with a way back, never an error.

### 6.6 What the reading of Metabase settled for the shell

From `20 §2`, applied in this wave and not argued again: a stale answer is veiled
behind the affordance that leads to the document, the funnel withdrawn, and hash,
export, release, pin and promote refused at the server as well as in the browser;
a blocked control keeps its button and carries its one reason, ordered truncated,
stale, incomplete, role; a truncation is one word in the count slot; the row
count links to the document's `out` step; every veil is `inert` and out of the
tab order, because a veil that only a sighted mouse user cannot cross is not a
guarantee; keyboard shortcuts are declared in one registry with a discoverability
page, and no single-letter binding collides with typing a document; a print
stylesheet expands the declaration and the funnel and a "copy as a methods
paragraph" affordance produces prose with the short hash in it; and the desk
decides what a copy from a table is: the gated columns are not selectable, a copy
of the rest writes an audit row, and the declaration says so.

## 7. Ask: the question workbench

### 7.1 Start from anything (D60)

A new question opens on one set, everyone, with its count. The first move is a
click or a sentence. The picker offers what a person may start from: one or
several cohorts, a saved selection, a handle, a document, a pasted list of
identifiers that resolves through the linkage store (4b §4.3 `values`), or
nothing. Choosing a cohort narrows the set and shows the new count and the
session count under it; no question has been asked yet, and none needs to be.
The start-from resolver is a door (§12.1).

### 7.2 Counts that move (D61)

Every set card carries its live funnel: subjects, sessions and stacks kept at
that step and where the previous step lost them, from the diagnosis door keyed by
set and clause group (§12.3), in the language's own order: source, near, attach,
has, where, pick, out. Counts are always safe to show. Rows appear in a preview
only where the declaration's disclosure allows them, ten at most, and the
preview is replaced by a button the moment the document moves; a stale table is
never on screen.

### 7.3 Click or say it (D62)

A click opens one condition picker across every set: a section per set headed
by its name and grain, its fields under it, a search across all sections; the
chosen field decides which set the clause lands on, which takes the hardest part
of the language away from a person who does not want to learn it. The legal next
moves of a set are one row of chips at the end of its card, in a fixed order,
large only on the set the document answers; an illegal move is not on the
screen. A sentence in the rail goes to `ask-help` with the open document as its
base and comes back as a diff on the document, inline, with accept and reject;
typing in the document rejects. Both paths apply a move and produce a version
with a hash. The document's own text is one disclosure away for the person who
wants it, and it is the artifact that hashes, nothing else.

### 7.4 The conversation belongs to the document (D63)

A conversation is keyed by the document's lineage, not by a run. Opening a
question a month later shows in the rail what was said when it was made, what
was rejected and why, and the handles each version produced. The versions panel
of 4c §7.3 interleaves runs, releases and promotions into the same list as edits.
Revert-in-place is not offered; "open this version" and "start a new version
from here" are.

### 7.5 The declaration, always (D64)

The declaration block of 4b §8.4 stays on the page, above the fold, with two
fields added in this wave: the timezone and the week start the engine read the
dates under (§12.6), because a day window is a semantic and not a rendering
choice. The stale veil, the blocked reasons and the refusals of §6.6 all hang on
the same flag: the core hash of the editor against the core hash of the last run.

## 8. Data, Review and Release

### 8.1 Data (D65)

Where things come in. A source is an ingest root bound to a place with the
`source` role (§10.2). A batch is one walk of it, and the batch page is a stage
strip: walked, digested, classified, reviewed, with counts under each, the pack
version, the quarantine list by reason, and the jobs that did each stage. A
digest, a reclassification and a session rebuild are queued from the page, and
appear in the rail and the inbox. Packs and their versions live here as pages;
overlays live in Review, because they are judgements.

### 8.2 Review (D66)

Where judgement happens. Items sorted by what they cost if wrong; an item's page
shows the stack, the header fields the rule read, the candidates the pack
considered with the evidence for each, and the decision as a row with a name and
a rule version. Beside it, the viewer (§11), once the study has chosen one; until
then, the fields and the evidence alone. Overlays, rehearsals, adoptions and the
identity rules of 4c §9.13 and §9.14 are here. Every irreversible act opens a
closure panel fed by the engine's dependency door (§12.4): "adopt overlay 7 and
move 412 stacks?", the moves by axis, the review items that open and close, a
button that says what it does. Bulk decisions exist for items that do not need
looking at, write one audit row per item, show the closure count first, and are
refused for items that do.

### 8.3 Release (D67)

Where things go out, and the page that must be boring and complete. A selection
becomes a release under a policy; the page shows the pseudonym namespace, the
BIDS layout the pack chose, the fields held back, the audit rows the release
will write, the export place it will write to, and the handovers that carried
it. Custody, the engine's own list of every store, its retention and the command
that changes it, is here, and places (§10.2) is the layer above it.

## 9. The assistant

### 9.1 The ladder (D68, C49)

4c's rule, a station proposes and never decides, gains one rung and loses no
guarantee. Three rungs, in the language of the engine's own doors, each of which
already declares whether it writes and whether it is idempotent (4c §6.5):

1. **Read.** Anything the person's ceiling allows. No permission beyond the
   entitlement; every read audited as the person.
2. **Run what can be undone.** A digest, a classification, a session rebuild, an
   ask, a rehearsal, a fine-tune: verbs that are jobs, idempotent, resumable and
   cancellable, whose result is a new object beside the old. The assistant may
   queue these under a **standing permission** the person granted for that verb.
3. **Propose what cannot.** Adopt, release, hand over, erase, change an identity
   rule, promote a model. Always a proposal a person accepts, with the closure
   shown first.

The identity provider decides what a person may do: the five entitlements of 4c
§5.2 are bound to its groups, and the assistant can never exceed the person it
acts as, so a standing grant to run a digest is only possible for an operator. A
standing grant is consent: personal, per verb, revocable, stored in the
assistant, shown in Settings and in the rail before anything runs, listed for an
admin who may revoke any of it. The audit row of a job the assistant queued
names both the person and the grant. If the group later wants a coarse policy, a
sixth entitlement `assist-run` in `contracts/suite/v1` (C49, optional) empties
rung two for anyone without it whatever they tick.

### 9.2 Context, typed (D69)

Each page declares what it contributes to the rail's context: document id and
content hash, set names with grains, funnel counts, pack id and version, handle
id, declaration fields, error codes, job ids. The type is the allowlist. A test
in the assistant's gate assembles the context of every page and asserts no row
payload in it. A station's tool never returns raw values into the transcript;
the previous prototype's failure of shipping result values to the model (`20 §2`)
is refused by construction.

### 9.3 The operator station and the scheduler (D70)

The concierge delegates to a new station, `operator`, whose verdict is a plan:
an ordered list of jobs with their arguments, each marked by rung, and the
proposals for the rung-three steps. The person sees the plan restated before it
runs and confirms it once. A scheduler in the assistant fires a confirmed plan on
an engine event (a batch landed, a job finished) or at a time, and every step is
a job the person finds in the inbox and the audit. "When the second batch lands
tonight, digest it, classify it with the same pack, run my pinned question, and
release nothing" is three rung-two jobs and one rung-three proposal.

### 9.4 The inbox (D71)

A per-person list of what the assistant and the engine did for them: jobs
queued, running, finished, failed, with their clocks and their objects; plans
that ran overnight with one line per step; proposals waiting. It is Home's third
and fourth bands, kept. A completion never leaves the desk in this wave; when
one does, the rule is written first: the notification names the job, never the
content.

### 9.5 Teaching (D72)

The loop of 4c §9.9 and `08`, made visible with its gates kept. The Assistant
section gains Teaching: the corrections the group gave (rejected proposals with
their reasons, decisions that overturned a station, review decisions that
overturned a pack), curated by a reviewer into a set; a fine-tune as a job on the
group's card, with its recipe recorded; Kvasir's admission suite against the
result; the bench's numbers on both corpora beside the suite's; and promotion as
a rung-three proposal that the desk refuses to offer until both gates are green.
Kvasir gains the lifecycle a candidate model moves through: registered, admitted,
promoted, retired (§12.8). No byte of a capture leaves the deployment; the
captures are the redacted trajectories custody already lists.

## 10. Settings

### 10.1 Parts (D73)

The first page of Settings: every part the deployment has, engine, desk,
assistant, Kvasir, the model runtime, each registered app, each pipeline image,
with its version, its contract versions, its health, and whether a newer release
exists on the channel (§10.4). Under each part its own settings, generated from
the capabilities document that part already publishes and the custody it
declares, never hand-built: the engine's registry, packs, ingest roots, backup,
retention and policy; Kvasir's backends, models table, purposes, admission
records and credentials; the assistant's stations, briefs and budgets, memory
and retention, teaching, standing grants; identity in its three modes with the
trust list and the people; apps as registry entries with an entitlement; the
desk's origins, export policy, timezone, week start and display rules.

### 10.2 Places (D74)

A place is a named location with a role and the guarantees behind it, a registry
object (§12.5). Every path the engine takes is bound to a place by role rather
than typed. The roles:

| Role | What it holds | What it must guarantee |
|---|---|---|
| `source` | the original data, read only for the engine; an anonymised copy sits beside it when one is made | protected storage with snapshots |
| `registry` | the registry, the linkage store, the keys | protected storage and a routine backup elsewhere |
| `working` | digests in flight, viewing pyramids, rehearsals | fast; may be lost |
| `export` | releases and BIDS trees | protected storage with snapshots |
| `share` | subsets a question wrote out, results handed to a colleague | reachable by the group |
| `exchange` | what leaves or arrives through a bridge | its own rules |
| `backup` | the engine's archives | protected, elsewhere from the registry |

The rules are checked, not documented: a release writes only to an `export`
place; a question's subset writes only to a `share` place; the `registry` role is
refused on a place without a backup; a `source` place is never written except
beside the original by the anonymiser. The engine probes what it can, the mount,
the free space, the presence of snapshots where the filesystem shows them, and
records what it cannot as a declaration by the operator. On a laptop a place is a
directory and its guarantees are what the person declares; the rules are the
same. The Settings page shows every place, its role, its guarantees and which
paths are bound to it, and lets an operator add, bind and retire places. The
group's own places are the record's (`20 §6`).

### 10.3 Database (D75)

Which place the registry is on and whether it passes the rule; the registry's
size, epoch and schema version; the last backup and when the next runs; a backup
now button that queues the engine's backup as a job; a restore rehearsal that
verifies an archive without applying it; for the server backend, the connection,
the schema and the read-only role the ask reader uses (4b §12.4), with the
password never shown.

### 10.4 The supervisor and the release channel (D76, C50)

`nils supervise` is a subcommand of the one binary, run as a service on a host
with the privilege to replace the parts on that host and nothing else. It
reports what is installed with versions and contract versions; watches a release
channel, one URL per part naming signed artifacts; fetches an artifact, verifies
its signature against a key the deployment holds, applies it, restarts the part,
and answers to the desk over the same contract everything else does, under the
`admin` entitlement. Settings shows the update and applies it with one button
behind a closure panel that names what changes ("engine 1.1 changes the pack
contract to v5; two apps speak v4"), and beside the button, always, the command a
person would run by hand. Where no supervisor is installed, the command alone.
A guide page in the desk's own documentation says what the supervisor does and
how to do the same without one. The channel and the signing key are the
deployment's; the product ships the verifier and the format.

### 10.5 Audit and identity (D77)

Audit is read in Settings: every row, filtered by person, by door, by object and
by month, with the raw projections of identifying fields as their own view,
because the person who asks for them is not an operator. The audit view is a
disclosure surface of its own: it names which colleague opened what, it has a
retention, and reading it writes a row. Identity shows the mode, the trust list,
the people and their entitlements, the sessions, and the standing grants.

## 11. The viewer study (D78)

Review needs a viewer and the wave chooses none until it has measured. A
photon-counting head at a fifth of a millimetre is on the order of a thousand by
a thousand by two to three thousand slices at sixteen bits, three to six
gigabytes for one volume, and a browser does not hold that. Smooth cannot mean
load and scroll; it must mean stream the slab being looked at, at the resolution
being looked at, decoded on the GPU, with the rest arriving behind it. That
fixes the shape before any library is chosen: a viewing pyramid precomputed at
digest as a job, stored in a `working` place, in a format that carries
resolution levels natively, and a gated door (§12.7) that serves tiles of the
current slab. The study measures that shape against the candidates: the
previous prototype's library, a lighter WebGL2 volume viewer, a full volume and
MPR toolkit, and a server-side path that renders the slab and streams it. The
measures: time to first image, scroll latency through the whole stack, memory at
rest and at peak, MPR and window-level interaction at sixty frames, and the cost
of the precompute. The corpus is the group's own largest scans, on the group's
own machines, over the mount the group uses and from local scratch; nothing
leaves the deployment. The study runs after the workbench and before Review is
built, and its report in the record (`20 §7`) designs the Review slice.

## 12. The engine's additions

Each is a door or a policy on the doors that exist, under the roles of 4c §5.2,
audited like every door.

1. **A summary door and a start-from resolver.** `GET /api/summary`: subjects,
   sessions, stacks by cohort and by pack version, the epoch, whether the registry
   is synthetic, and what changed since a date. `POST /api/ask/start` resolves a
   cohort, a selection, a handle, a document or an uploaded list into the opening
   set of a document. `GET /api/ask/documents` lists documents with name, grain,
   last run, versions and author.
2. **A timeline door.** `GET /api/timeline/{kind}/{id}`: every event on an object
   in order, from the rows the engine already keeps (versions, runs, decisions,
   adoptions, releases, audit), typed, each with the object it produced.
3. **Diagnosis by clause group.** `POST /api/ask/diagnose` gains a `by: clause`
   mode that keys the funnel by set and clause group in the language's order, so
   a step's own counts are computable without a document being rewritten.
4. **A dependency door.** `GET /api/depends/{kind}/{id}`: the closure of an
   adoption, a pack bump, a rule change or an erasure: the stacks that move, the
   review items that open and close, the handles and releases that stop being
   reproducible. The confirm panels of §8.2 are fed by it and refuse to render
   without it.
5. **Places as registry objects.** `GET /api/places`, `POST /api/places`,
   `PUT /api/places/{id}` under `operator`; the binding of every path-taking verb
   to a role; the probes and the operator's declarations; the rules of §10.2 as
   validation on every verb that writes a path, so a wrong place is refused at the
   door and not discovered on disk.
6. **Disclosure on errors; timezone in the declaration.** Every error the engine
   returns carries one of `safe`, `gated`, `internal`; the desk renders `gated`
   through the projection door with its audit row and `internal` as a fixed
   sentence. The declaration block gains `timezone` and `week_start`, read from
   the registry's setting and never from the browser, and both are covered by the
   content hash's desugared core.
7. **The gated instance door.** `GET /api/instances/{id}/tiles/{level}/{z}`:
   pixel data of one stack at one resolution level and one slice range, under the
   disclosure policy, with burned-in annotation held where the header says it is
   present, one audit row per stack opened; and the pyramid as a job of digest,
   written to a `working` place. Shaped by the study of §11 before it is built.
8. **The content hash as a cache key.** A run of a document whose core hash has
   run before, at the same epoch and pack, answers the existing handle with its
   date and its declaration and offers a fresh run as a choice; an adoption, a
   pack bump or an erasure invalidates the handles the dependency door names,
   and the invalidation is a row.

Kvasir gains the model lifecycle of §9.5: a candidate registered from a
fine-tune job, admitted by the suite, promoted by a rung-three proposal, retired;
`GET /v1/models/lifecycle` lists it and the desk's Teaching page reads it.

## 13. The gate of the wave and its closing bars

Each repository runs its own gate in CI as in 4c §11, and two tests are added
to the desk's: the laptop test of §4.1, which starts the suite from the binary in
a temporary directory with a synthetic registry and walks every section with no
Kvasir, no supervisor and no place beyond directories; and the theme lint of
§5.1. The assistant's gate gains the context test of §9.2 and the ladder test: a
rung-three verb can never be queued by a station, proved by a grep over the
station manifests and a test that a station asking for one is refused.

The wave closes when:

1. A laptop install shows the same desk in `off` mode with no part beside the
   binary required, and the group's deployment is reachable through the portal
   and has updated on merge since the first slice.
2. The scene of the record (`20 §4`) runs end to end on the deployment: a
   question started from nothing and built by clicking and by talking, with its
   counts moving and its conversation kept; a batch walked through Data; an item
   judged in Review with the evidence beside it; a release described in full
   without being made.
3. The overnight instruction runs as a plan: three rung-two jobs under standing
   grants and one rung-three proposal, every step in the inbox and the audit, the
   proposal unapplied until a person accepts it.
4. A place with the `registry` role and no backup is refused at the door; a
   release to a place without the `export` role is refused at the door.
5. The supervisor updates a part from a signed artifact, refuses an unsigned one
   with a named reason, and the guide describes the same steps by hand.
6. The viewer study's report is in the record with its numbers, and Review is
   built on what it chose.
7. Teaching runs the loop once on the bench: a set curated, a fine-tune as a job,
   the admission suite and the bench beside each other, promotion refused until
   both are green.
8. No literal colour outside the theme file; no row payload in any page's rail
   context; no host, path, group or pool of the group's own in any repository.

## 14. Defaults settled in this spec

- The sections are Home, Ask, Data, Review, Release, Pipelines, Assistant,
  Settings, plus one per app. Operations is retired (D56).
- A standing grant is per person and per verb, stored in the assistant, shown
  before it is used, and revocable by the person and by an admin (D68).
- The ladder's line: a verb is rung two when its door declares it idempotent and
  its result is a new object beside the old; everything else is rung three.
- The theme is two files; the mark is one; a lint refuses literals (D53, D54).
- The timezone and the week start are the registry's, never the browser's, and
  are in the declaration and the core hash (D64).
- A copy from a table: gated columns are not selectable, a copy of the rest
  writes an audit row (§6.6).
- The group's deployment starts on the synthetic registry and says so in the
  shell (§4.2).
- The viewer is chosen by the study of §11 and not before.

## 15. Order of work

A slice's letter names its repository: A the engine, B the desk, C Kvasir, D the
assistant, E the supervisor and channel, S a study, W the group's deployment and
site (whose specifics are the record's). Each slice is one merged pull request
with its own tests and its gate.

| # | Slice | What lands | Gate |
|---|---|---|---|
| A0 | **The record and the spec** | Record 20 scrubbed into `docs/decisions/`; this document. | The record's ids resolve; no forbidden term in the copy. |
| W0 | **The deployment** | The suite on the group's host on the synthetic registry, the desk in `oidc` mode registered against the identity provider, the portal entry, deploy-on-merge for every part, the synthetic banner. | The portal opens the desk for a person in the right group; a merge on any part is live within the hour; the shell names the registry synthetic. |
| B1 | **The theme and the mark** | §5: `theme.css` from the group's design language, the twin tables, the six intents, the grain tints, one control height, the mark and the favicon, the lint. | No literal colour outside the theme file; the dark table has the light table's key set; the mark renders at 16 and at 256. |
| A1 | **The summary and the start-from doors** | §12.1: `GET /api/summary`, `POST /api/ask/start`, `GET /api/ask/documents`. | Fixtures on the synthetic registry for each; the start-from resolver refuses an uploaded list under a reader. |
| A2 | **The timeline door** | §12.2 over the rows that exist. | A document's timeline shows its versions, runs and a decision in order; a handle's shows its release. |
| B2 | **The shell** | §6: Home, the sections regrouped, object pages with timelines, stable addresses, the rail with typed context, the wait, failure and empty components, the stale veil and blocked reasons, `inert` veils, the shortcut registry, print. | The laptop test walks every section; Home's four bands render from the doors; a stale handle's release button carries its reason and the server refuses the release. |
| A3 | **Diagnosis by clause group; disclosure on errors; timezone in the declaration** | §12.3, §12.6. | The funnel keyed by clause group sums to the funnel keyed by set; an error with a subject code in it is tagged `gated`; two registries in two timezones give two hashes for one document. |
| B3 | **The question workbench** | §7: start from anything, live counts, the one picker, the move row, inline diffs, the conversation kept with the lineage, the declaration with its two new fields. | The authored corpus runs through the workbench headless with the same numbers as through the station; a question built by clicking and one built by talking to the same shape hash the same. |
| D1 | **Conversations by lineage; typed context; proposals as diffs** | §7.4, §9.2, the rail's context contributors. | The context test; a proposal on a document that moved reads as stale and cannot be accepted. |
| B4 | **Data, Review without the viewer, Release** | §8 on the doors that exist; the stage strip; the item page with its evidence; bulk decisions; the release page in full. | A batch page shows every stage with its job; a bulk accept writes one audit row per item and is refused for an item that needs reading. |
| A4 | **Places** | §12.5 and §10.2: the objects, the bindings, the probes, the rules at the doors. | A `registry` place without a backup is refused; a release to a non-`export` place is refused; the laptop test binds directories. |
| A5 | **The dependency door; the hash as a cache key** | §12.4, §12.8. | Adopting an overlay names the stacks that move and the handles that stop reproducing; running an identical core twice answers one handle. |
| B5 | **Settings: Parts, Places, Database, audit, identity** | §10.1 to §10.3, §10.5, generated from capabilities and custody; the closure panels on Review fed by A5. | Every part's version and contract on one page; a place added, bound and retired from the page; the audit view writes its own row. |
| E1 | **The supervisor and the channel** | §10.4: `nils supervise`, the artifact format and the verifier, the channel, the update button with its closure panel, the command beside it, the guide. | Bar 5. |
| D2 | **The ladder** | §9.1, §9.3, §9.4: standing grants, the `operator` station, the scheduler on engine events, the inbox; C49 if the group asks for it. | Bar 3; the ladder test in CI. |
| S1 | **The viewer study** | §11 on the group's own scans, the report into the record. | The report names its numbers per candidate and the shape it recommends. |
| A6 | **The gated instance door and the pyramid job** | §12.7, shaped by S1. | One audit row per stack opened; burned-in annotation held; a tile request under a reader without the class is refused. |
| B6 | **Review with the viewer** | §8.2 completed with what S1 chose. | Bar 6; scroll through a study's largest volume at the study's frame rate on the group's workstation. |
| C1 | **The model lifecycle** | §12: candidates, admission, promotion, retirement, the door. | A candidate cannot be promoted without an admission record. |
| D3 | **Teaching** | §9.5 on C1 and the bench. | Bar 7. |
| W1 | **The site** | The mark and the desk's page on the group's site, the guide linked from the portal. | The record's, not this document's. |

## 16. Open questions carried into the wave

- `20 Q21`: the rune's glyph on the mark, settled by Nima against the drawing.
- `20 Q22`: whether the group's deployment moves from the synthetic registry to
  the group's data inside this wave or after it.
- `20 Q23`: whether `assist-run` (C49) is wanted from the start or left optional.
- `20 Q24`: the viewer, by the study's report.
- `20 Q25`: whether a completion may ever leave the desk to the group's chat
  channel, and under which words.
