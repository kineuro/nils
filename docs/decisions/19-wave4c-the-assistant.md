# 19 — Wave 4c, the assistant with the desk and Kvasir: the twenty questions answered, Nima's rulings, the design and the wave

Written 2026-09-08 from the study that 17 §5 required before Wave 4c could open
(`studies/2026-09-08-wave4c-study/`: ten readers, five designs, three judges, the
report and its twenty open questions, 9,188 lines in `results/`). Status:
**ratified 2026-09-08** (Nima: "19 §3 and §4 confirmed", with every permission for the wave: Asgard, its GPUs and memory, the mixed corpus in scratch, containers as needed; the commercial test key arrives in a file outside every repository). The specification written from it
is `docs/specs/wave4c-the-assistant.md` in the public repository; this document is
the record it cites.

Ids continue 18 §7: decisions D41 onward, amendments C43 onward, Nima's rulings R9
to R11 (continuing 17 §1), and the twenty answers Q1 to Q20 of this record, cited
as `19 Q3`. The study's own numbering of the questions is kept, so `19 Q3` is
question 3 of `results/91-open-questions.md`.

Two facts before anything else, because they shape the wave. The study found two
live disclosure defects in the engine on `main` (slices 7 and 9 of Wave 4b, merged
2026-09-08), and I confirmed both by reading the code: the ask job path composes its
own scope and projects identifiers for any reader, and a stored handle is readable
page by page by any reader with no owner check and no read audit. Neither has been
executed against a live server yet. The wave opens with their repair. And the
hardware line of the aim is wrong in one number: the cards are 96 GB, not 98.

## 1. Nima's rulings, 2026-09-08 (R9 to R11)

| | Ruling | What it changes |
|---|---|---|
| R9 | **Local models may see patient information.** The goal is the best performance a local model can give, the reference being a 27B model of the current generation at over 150 tokens per second with eight concurrent users, and one whole GPU may be allocated to it. A settings section chooses which model serves which flow of the assistant, so a commercial model is usable where the operator says it is fine and the sensitive flows stay local. A commercial key (one vendor, both API shapes) is available for testing. | The disclosure ceiling for a station is where the model runs, not what the model is (Q3, Q4). No cell suppression inside the site (Q5). One card is allocated, which reopens the server design's shared-GPU rule on purpose (Q12, C47). |
| R10 | **User management fits Authentik on Asgard.** NILS and its applications are registered there and visibility is per user: chat for some, the engine for admins, the question for all staff. This is optional: some deployments need no user management at all, and some need only a simple login with admin control over access. | Three identity modes in the desk, one code path in the engine (Q1, Q11, D42). An entitlement vocabulary for the suite. A local login mode with its own users, which amends D8 (C46). |
| R11 | **The model service is named Kvasir.** | D45; the reason is recorded so the first runbook line means something. |

The three criteria Nima set for the design: it follows best practice, and it makes
NILS the most capable it can be in **performance**, **maintenance** and
**expandability** (adding a smart NILS app later, an analysis pipeline or study
management, without touching what exists). §2 says what each meant when a choice
had to be made.

## 2. What the design optimises for

**Performance.** The local model is the bottleneck of everything intelligent, and
the real workload is prefill-dominated at roughly 100 to 1 (median input 23,600
tokens, p90 72,000, against a median output of 201; `04` finding 18). So: one whole
card allocated and warm, SGLang with speculative decoding and the radix prefix cache,
a memoized system prompt with a marked boundary and delta attachments so the cache
prefix stays stable (`07` finding 21), handles that are paged and never re-executed,
the engine kept free of streaming and of an async runtime (`10` finding 17), and a
benchmark with its thresholds written down before it runs (Q12).

**Maintenance.** One identity model with three modes and one engine code path for
every multi-user deployment; one pin of the model library across the two Node
services; contracts as the product, bumped once per wave in a titled pull request;
one Postgres server with one database per part (or SQLite files for standalone), no
Redis, no second cluster; stations declared as data; no judge on any gate; nothing
this wave adds writes model content to disk except the assistant's transcripts, which
sit under custody with a retention job; a minimal deployment is two Rust binaries and
needs no Node at all.

**Expandability.** A new app is a client of the contracts (R8), and this wave writes
the contract that makes that true beyond the engine: `contracts/suite/v1`, the shared
vocabulary of entitlements, purposes, data classes, the actor and the ceiling, and
the shape of the deployment capabilities document the desk merges. A new app then
(a) registers its purposes with Kvasir in configuration, (b) publishes its own
capabilities document that the desk merges, (c) mounts as a desk section through the
desk's app registry, proxied on the one origin under the person's session, and (d)
contributes station manifests, briefs, result schemas and fixtures that the
assistant host loads. None of those four steps touches the engine, the desk's code,
Kvasir's code or the assistant's code.

## 3. The twenty questions, answered

Each answer names the evidence it rests on and whether it differs from the default
the study recommended (`results/91-open-questions.md`). Where a ruling of §1 decided
it, the ruling is named.

### Q1. One Authentik application for the suite, or four?

**One application in the pilot, named NILS, with the roles and the feature
entitlements as application entitlements on it; and the engine grows a trust list
now, so that more applications later cost configuration and not code.** No flip of
the instance-wide issuer mode during this wave.

Why. The engine verifies one issuer string and one audience string, compared
exactly, from a JWKS file read once at boot (`09` findings 1 to 3;
`serve.rs:327-328`). Authentik's default per-provider issuer mode gives every
application a different `iss` and `aud`, so two applications cannot meet the engine
as built (`09` finding 2). Four applications already live on that instance,
including the group's own portal; flipping `issuer_mode` changes `iss` for all of
them at once, which is an operational change to a running service and not a design
choice (`30` must-fix 1). What R10 asks for, chat for some, the engine for admins,
the question for all staff, is per-user visibility of *features*, and every one of
those features lives inside the desk on one origin; there is no separate URL for
"chat" that a library tile could show or hide. Application entitlements are the
mechanism Authentik offers for exactly this, they reach the token through the managed
`entitlements` scope mapping as a `roles` array, and the engine already reads any
named claim as an array of strings with zero code change (`09` finding 8). So one
application with entitlements `reader`, `reviewer`, `operator`, `admin` and `assist`
delivers R10 in full.

The trust list. Rather than an audience list alone (the study's default), the engine
takes a repeatable `--oidc-trust issuer=URL,audience=ID,jwks=URL`, because the two
cases that will want a second application first, a command line client on the device
authorization grant (which needs a public client and therefore a second provider)
and a third-party MCP client, both arrive with their own issuer under the default
mode. A list of tuples costs the same few lines as a list of audiences, and Kvasir
takes the same list with the same test vectors (Q19). The first deployment has one
entry.

One check gates this: mint one token against the live instance and read whether the
`roles` claim is an array of plain strings (`23` risks, `30` should-fix 9). Five
minutes, before any code.

Differs from the study: the trust list holds issuer and audience pairs, not
audiences alone.

### Q2. Do our own stations reach the engine over HTTP doors or over MCP?

**HTTP doors, through one seam module, under the OpenAPI contract. MCP stays the
third-party surface. The pack's model-facing content is the single source for what
a model is told on either path.** Recorded as C43, amending D11, C22 and C23.

Why. The MCP door flattens results to text, pages at 50 rows against the doors' 200,
and deliberately does not carry the evidence the knob stations read (review items,
classify reports, digest diagnostics), nor should it (`20` "How it reaches the
engine"). Flue's MCP client reads only tools, never prompts or resources, so the
pack's worked examples never reach it as things stand (`24` part A). The typed half
of a tool result, which the desk's data-part reducer needs, exists only on the HTTP
path (`05` finding 16). And identity: a third-party MCP client needs a pre-registered
OAuth application on an instance that offers no dynamic registration (`09` finding
20), which is the Q1 trust list's second entry, later. What D11 was protecting, that
any client is an equal citizen, is a property of what a model is told and of which
doors are governed, and both hold on both paths: the doors are the same doors, the
caps are the same caps, and the grounding is one file.

Consequences. The `guide`, `draft`, `job` and `job_status` operations still join the
MCP tool list in this wave, because they are cheap once the doors exist and a
third-party client benefits; `contracts/mcp/v1` is published as the description of
the door that exists. The MCP door's dated audience deviation stays open and is
retired by the slice that registers the first OAuth-speaking MCP client, which is
not in this wave; the spec says so.

Same as the study's default.

### Q3. May any question text reach a commercial model provider, and for which purposes?

**Per purpose, by an admin, with the class of content each purpose carries declared
in its manifest and enforced by Kvasir at the grant, never by the caller.** (R9.)

The rule in full, and it is the same rule as Q4. Every purpose a station or a
helper call declares carries a **content class**: `catalog` (names, axis values,
describe sentences, the person's question text), `rows` (handle rows, counts,
sampled values, funnels, review evidence), or `identifiers` (anything that projects
`out.identifiers` or reaches the linkage store). Every backend has a **locality**:
`local` (inside the site) or `remote`. The deployment's policy table maps each
purpose to the backends it may use. Defaults: every purpose is local-only. An admin
may open a `catalog` purpose to a remote backend by choosing a remote model for it in
the desk's models table, which is recorded and displayed; an admin may open a `rows`
purpose only after a recorded acknowledgement that rows of the archive will leave the
site; an `identifiers` purpose can never be opened. A turn whose user text matches an
identifier shape is bumped to `identifiers` for that turn, so it runs local whatever
the purpose's setting (`31` must-fix 4). The test provider Nima named is registered
as a remote backend and used to prove the policy refuses what it must.

Why per purpose and not per person: R9 says the operator decides which flow may use
a commercial model; a per-person switch would let one person move rows off site under
a flow the operator had not opened. Why Kvasir enforces it: the assistant declares,
the desk displays, and only the service that holds the credential can refuse
(`21` "the capability contract", `31` must-fix 5).

Same as the study's default (b), made concrete by R9.

### Q4. Is the pseudonymous subject key personal data for this deployment?

**For egress, yes: rows keyed by subject never leave the site unless an admin opens
that purpose with a recorded acknowledgement, and identifiers never do. For a local
backend, no ceiling beyond the person's own: a station running on a local model may
read what the person it acts for may read, narrowed by the role ceiling of Q6.**
(R9.)

Why. R9 states that local models may see patient information, which makes the
inside of the site the trust boundary, the same boundary the person's own screen
already has. Enforcing a stricter ceiling on a local station than on the person would
buy nothing and would make the station worse at the six silent decisions the corpus
shows it must get right, which need rows, counts and samples to resolve (`04`
findings 5, 10, 11). The conservative half of the study's default survives where it
matters: the egress rule of Q3, the `identifiers` class that never leaves, and the
per-turn identifier-shape bump.

Differs from the study's default (a): the ceiling is set by locality, not by the
data class alone.

### Q5. What is the minimum cell size below which a count is withheld?

**None inside the site, in this wave; the egress policy of Q3 is the control.**
(R9.)

Why. Suppression is a rule about what leaves a trust zone. Inside the site every
caller is a person of this group under the archive's own governance, or a station
acting for one under the person's roles and a ceiling, and a cohort of three is a
real cohort a researcher needs to see. Applying k = 5 in the engine for every door
would change the answers of the ask for everyone, operators included, to protect
against a reader the site does not have. Where counts would reach a remote backend
they are `rows`-class content, which is local-only unless an admin opens it with an
acknowledgement, and a release already passes through review. The federation
defaults of D27 (k = 5 and 10, complementary suppression) stay what they are, for the
day a result crosses to another node. The decision is recorded as D51 so that nobody
reads the absence as an oversight.

Differs from the study's default (b).

### Q6. Does the assistant ship before the role ceiling exists?

**No. The ceiling is a precondition, and it is small.** A request header
`X-Nils-Ceiling: reader|reviewer|operator`, applied before the role check at every
door, that can only remove roles, recorded on the audit row beside the actor.
Forging it is harmless because it can only reduce. Each station manifest names the
ceiling it runs under; `ask-help` runs at `reviewer`, so no station can promote,
release, reveal identifiers or rebuild sessions, whatever the person holds. Until it
exists an operator must not use the assistant, and the desk says why.

Why. The engine derives roles solely from the token it is shown and has no per-actor
cap (`serve.rs:344-363`), so an assistant holding a person's token holds the person's
roles entire (`31` must-fix 3). Same as the study's default (a).

### Q7. Whose budget does the assistant spend, and which cap refuses first?

**The person's. The station's own budget refuses first and names itself in the
terminal reason; Kvasir's per-backend concurrency admission is the only other limit
in the first version, and it queues with a heartbeat before it refuses.** Per-person
daily token caps exist in Kvasir as a setting that defaults to unlimited; with eight
users on one card, concurrency is the limit that binds (`30` must-fix 8).

Same as the study's default (a).

### Q8. Who refreshes the token that outlives a durable run or a polled job?

**The desk, and it pushes.** The desk holds the refresh token in its server-side
session; for every conversation with an open submission it refreshes before expiry
and pushes the fresh token to the assistant over the desk-to-assistant internal
channel. The assistant holds no credential that reaches the desk. A submission that
cannot obtain a fresh token before its deadline terminates with the typed reason
`token_unavailable`, never silently and never with an apologetic sentence.

Why push rather than pull: pull would give the process that runs model output a
credential for the desk (`31` should-fix 3 makes the same point about a
per-conversation key). Same as the study's default (a), with the direction chosen.

### Q9. Does the assistant's own database share a cluster with the registry?

**One Postgres server, one database per part (registry, Kvasir, assistant), one
role per part, `CONNECT` revoked from `PUBLIC` on every database, no grants across,
and a live check from each container that the other databases refuse it.** In a
standalone deployment each part uses a SQLite file. The desk's own store is a SQLite
file in every deployment, because it holds only sessions, display names, preferences
and, in local mode, the users.

Why one server: the hospital already backs it up, and maintenance was one of the
three criteria. Same as the study's default (b), with (a) rejected on maintenance.

### Q10. What answer do the three expressiveness seams get?

**Token matching becomes a bounded operator of the ask, `contains`, over the fields
the pack declares as free text, case-folded substring, never regex; a population
named by an identifier prefix is promoted to a saved selection automatically and the
answer says so; the pair grain and disk reconciliation get a fixed refusal sentence
in the pack's model-facing content that names where the answer does live.**

Why. Token matching was the most-used pick rule in the corpus and its absence is what
made the researcher correct a literal by hand (`04` findings 8 and 19). The pair grain
is a reserved concept whose cost nobody has estimated. Same as the study's default (b).

### Q11. Standalone: no login, or the desk as a real OIDC issuer?

**Both, as modes, and R10 decides it.** The desk has three identity modes:

- `off`: one person on one machine; engine `--auth off`, desk on loopback, no login.
- `local`: the desk keeps its own users (username, argon2id hash, display name,
  entitlements, disabled flag), an admin who grants entitlements, and it is a small
  issuer: an EdDSA signing key, a JWKS and a discovery document on its own origin,
  short tokens minted only for the logged-in subject with entitlements no wider than
  the stored ones. The engine and Kvasir run in `oidc` mode with one trust entry
  pointing at the desk. `nils login --desk URL` obtains a token for the command line.
- `oidc`: Authentik or any provider; the desk is a relying party with PKCE and holds
  the tokens server side.

`nils-auth` as a separate service is not built; the name stays in the record for the
day a second consumer of the local issuer appears. What is not repeated is v0's shared
symmetric secret copied into every app (`09` finding 11): one component holds a
private key, every other holds a public one.

Why. R10 names the deployment that needs only a simple login with admin control over
access, and `off` cannot serve it. Building the issuer into the desk keeps `oidc` the
only engine path for any deployment with more than one person, so adding Authentik
later is a change of one URL and a re-login. The judges' concern that minting is
concentrated in the browser-facing process is met by construction: the signing
module mints only for the authenticated subject, at or below that subject's stored
entitlements, and the key file is unreadable to anything but the desk (`31`
should-fix 6).

Differs from the study's default (a): the local mode is in the wave, as its own slice,
after the Authentik mode.

### Q12. Does the model service get a guaranteed slice of a GPU?

**One whole card of the production host is allocated to Kvasir; the other card stays
shared.** (R9.) MIG is not enabled. The `card` profile is SGLang, the 27B model in
BF16 with an FP8 KV cache, the llguidance grammar backend, a tool-call and a
reasoning parser, speculative decoding with `--max-running-requests` set explicitly,
one runtime key that only Kvasir holds, bound to loopback. The 24 GB floor in the aim
is a claim about the smallest supported deployment elsewhere, not about this site: it
is validated once during the benchmark window on a temporary MIG instance if the
driver allows, with a 4-bit build under llama.cpp, and otherwise recorded as untested.

Thresholds, written before the run (Kvasir's own spec repeats them): aggregate output
of at least 150 tokens per second at eight streams, which is how R9's number is read;
at least 15 tokens per second per stream at eight streams; p95 time to first token
under 2 seconds at eight streams on a 4k prompt, and under 8 seconds on a 24k prompt
with a cold prefix; schema pass fraction above 0.98 on the admission fixtures with the
negative control failing to produce a malformed clause; tool-call validity above 0.95;
Kvasir's own overhead under 50 ms at p50 and 200 ms at p95 of time to first token. No
published measurement on this card supports 150 tokens per second per stream at eight
concurrent streams (`10` finding 4), so if that was the reading, the benchmark is where
we learn it.

This reopens the server design's rule that the two cards are shared and not
allocated, deliberately; C47 records it, and the server design record gets the same
line.

Differs from the study's default (a): a whole card, allocated.

### Q13. Which station ships first, and may any station ever auto-commit?

**`ask-help` first; no station auto-commits anything for the whole pilot. The two
knob stations, `keyword-tune` and `identity-check`, are in this wave as its last
slices, with the engine work they need, and they open only after `ask-help` has
passed its bench.**

Why the knob stations are in the wave: Nima asked for one wave because everything is
related, and the anonymisation check and the keyword tab are the two decision points
the aim named first. Why last: the station shape is proved once on the cheapest
decision point before the two whose wrong answers are most expensive, and the engine
work they need (the four Wave 2 diagnostics, the signals and counterfactual doors,
overlays as registry objects, the ingest probe) is the largest unbuilt part of the
engine section (`30` must-fix 7). Auto-commit stays allowed by the record as a
per-kind policy and stays off; it reopens only with a measured rate from the bench.

Differs from the study's default in one respect: the knob stations are in this wave,
at its tail.

### Q14. One diff mechanism: a door, or canonical bytes on the document response?

**The door.** `POST /api/ask/diff` takes two documents, two handles, or one of each;
for documents it answers with the structural diff over the canonical form and both
canonical texts, so a client renders a unified view without a second call; for
handles, the content hashes and the row-set comparison the handle already stores,
refusing when either is truncated. The desk, the command line and the assistant all
read this one door. Same as the study's default (a).

### Q15. Backup and restore: what does the desk actually offer?

**Backup as an audited job kind. Restore stays a command the desk prints, with a
pre-restore backup in the procedure and a `verify` job that checks an archive before
anyone types the command.**

Why. A restore replaces the database of the process that would run it; at best it
restores into a new database and swaps, which is a deployment procedure and not a job
(`30` must-fix 11). The aim's "everything the command line does" is honoured
everywhere else (Q18) and the spec says in one sentence what the one exception is
and why. Same as the study's default (a).

### Q16. Retention, and what a kept verdict contains

**A verdict is scrubbed of free text before it joins the evals dataset; the retained
corpus is paraphrased shapes; deletion on request reaches the eval rows.** Transcripts
90 days (C24), verdicts and scores indefinitely as shapes (C25), Kvasir's ledger
thirteen months at row grain then aggregates, idempotency keys 24 hours, desk sessions
until logout or expiry. Every store on the custody page (C45). Same as the study's
default (a).

### Q17. Is trajectory capture a mode a person turns on?

**Yes: a named teaching session, with the redaction contract written and tested
before the first capture.** The test seeds markers in tool results and in user text
and asserts none survive into any store. Same as the study's default (a), which is the
one thing v0 got right (`03` finding 1).

### Q18. What does "everything the command line can do" mean in the desk's first version?

**Everything, in two halves, with one rule for the second.** The read-and-select half
(ask, results, jobs, review, releases and handovers, custody, audit, sessions,
status, packs, handles, selections) is slices B3 to B5. The ingest half (digest,
classify, fingerprint, linkage import) is offered as job forms in the same slice B5,
and every one of them runs against a **pre-registered ingest location** the engine
declares at start (`--ingest-root name=path`) and publishes in `capabilities`,
never a path a caller composes. Two verbs stay on the command line: restore (Q15) and
key management, because a secret is typed where it is used.

Why. R10's "engine only for admin" and the aim's hub both expect the operator to run
the engine from the desk; the security edge the study named is real and the location
mechanism closes it (`31` must-fix 7 is the same rule for the probe). Differs from
the study's default (a): the ingest half is in the wave, behind locations.

### Q19. Who owns the model-library pin, and is it one version everywhere?

**One version, pinned exactly in Kvasir and in the assistant (the version Flue pins),
one bump review with a named checklist (the compatibility table, the overflow
patterns, the retry classifier), one owner.** The trust list's test vectors and the
suite vocabulary are shared fixtures both services run, so the engine's ladder and
Kvasir's cannot drift either (`32` should-fix 9). Same as the study's default (a).

### Q20. What is the model service called?

**Kvasir** (R11). Kvasir was made by mixing, was the wisest of beings, answered any
question put to him, and what he became was afterwards doled out in measured
draughts: a mixture of local and commercial sources, the place answers are drawn
from, and metered. Nothing in this deployment is called Bifrost, because that name is
the group's data bridge (`10` finding 14). Same as the study's default (a).

## 4. Decisions D41 to D51, amendments C43 to C47

### The decisions

| Id | Decision | Where it lives |
|---|---|---|
| D41 | **Our own stations reach the engine over the HTTP doors through one seam; MCP is the third-party surface; the pack's model-facing content is the one source for what a model is told on either path.** | 08, 05; spec §4, §9 |
| D42 | **Identity is three desk modes (`off`, `local`, `oidc`) over one engine path for every multi-user deployment (`oidc` with a trust list of issuer, audience and JWKS tuples); one Authentik application in the pilot; entitlements `reader`, `reviewer`, `operator`, `admin`, `assist` are the suite's vocabulary; the desk is the only origin a browser talks to and holds the tokens; the principal is the subject and the display name is recorded beside it, never as a key.** | 05; spec §5 |
| D43 | **Every writing call a station makes carries an actor (`kind`, `name`, `model`, `version`, `conversation`), and the seam applies a downgrade-only role ceiling before it dials; the engine records both on the audit row, the handle's provenance and the decision, with "absent" as its own value.** | 05, 03; spec §6.2 |
| D44 | **Disclosure into a model follows locality.** A local backend may see what the person may see under the ceiling; a remote backend serves only purposes an admin opened, `rows`-class content only with a recorded acknowledgement, and `identifiers` never; a turn carrying an identifier shape runs local. Kvasir enforces it at the grant. | 08, 05; spec §8.3 |
| D45 | **Kvasir is the only holder of a model credential and the only address any NILS part uses to reach a model.** Requirement in, grant out, with a mandatory purpose that apps register in configuration; a model is listed only after admission; the ledger holds counts and never content; it speaks pi-messages outward; it is TypeScript on the pinned pi-ai; it runs on the production host beside one allocated card. | 08; spec §8 |
| D46 | **The desk is one Rust binary with the built front end embedded, running as its own process, holding the session and the engine token, proxying every part under one origin, with a shell that is a pure function of a merged capabilities document, a question page that is a pure function of the ask document, one write path (`options` then `apply`), and an app registry that mounts later apps as proxied sections.** | 06, 01; spec §7 |
| D47 | **A station is a manifest, a brief, a result schema and a fixture set: a guarded phase machine with a budget, typed terminal reasons and a verdict whose proposals are the only effect; a document apply is permitted, a decision apply is forbidden.** | 08; spec §9 |
| D48 | **The bench exists before the first station**: the corpus scrubbed and rebased on the synthetic registry, a closed failure taxonomy, a held-out split, a recorded provider, a change manifest with the nine-state matrix, no judge on the gate, and a person merging every harness edit. | 08, 15 C25; spec §9.9 |
| D49 | **The engine's two disclosure defects are repaired before any Wave 4c feature**, with gate fixtures that fail today; every writing door whose repeat creates a row with an audit consequence takes an idempotency key. | 05; spec §6.1, §6.3 |
| D50 | **Restore is a printed command; backup is a job; every ingest verb the desk offers runs against a pre-registered location.** | 06; spec §6.5, §7.5 |
| D51 | **No cell suppression inside the site; the egress policy of D44 is the control; D27's federation defaults stand for a result that crosses to another node.** | 05, 14 |

### The amendments

| Id | Amends | Amendment |
|---|---|---|
| C43 | D11, C22, C23 (08, 05, 15 §1) | The agent reaches the engine over the HTTP doors, not exclusively through MCP (D41). C22's MCP shape stands for third parties, and gains `guide`, `draft`, `job` and `job_status`. C23's six-week pilot clock starts the day `ask-help` answers a real question through the desk, not the day the MCP server answers (which it has since 2026-09-08). |
| C44 | C37 (15 §8) | A knob proposal is an overlay stored as a registry object (`proposed`, then `adopt` at operator), with a review item emitted beside it so the queue stays the one place a person is asked; the classifier's four diagnostics are built to Wave 2 §10's names. |
| C45 | C38 (15 §8) | The custody page lists the desk's store, the assistant's database and notes, the evals dataset, Kvasir's ledger and Kvasir's credential store, each with its class, its retention and its command. |
| C46 | D8 (05) | "NILS never mints identities again" is refined: the **engine** never mints and holds no secret; in `local` mode the **desk** mints short asymmetric tokens for its own users and publishes a JWKS, so that the engine's `oidc` path is the only path a multi-user deployment ever takes. `off` and `token` stay as they are; the `oidc` mode accepts a trust list. |
| C47 | the server design's shared-GPU rule (outside this record) | One card of the production host is allocated to Kvasir and held warm; the other stays shared. The server design record gets the same sentence. |

## 5. The design in one page

**Five things, four repositories.** The engine (`kineuro/nils`) changes least: two
repairs, the identity delta, idempotency, six ask additions, the deployment surface,
the knob engine, and one contract bump each for `openapi` (v3), `review-item` (v3),
`suite` (v1) and `mcp` (v1). The desk (`kineuro/nils-desk`, Rust, one binary) is the
one origin: sessions, three identity modes, the question page, results, operations,
data, settings, the assistant pane, the app registry. Kvasir (`kineuro/kvasir`,
TypeScript on the pinned pi-ai) holds every model credential, resolves purposes to
models, admits models, meters counts. The assistant (`kineuro/nils-assistant`,
TypeScript on Flue pinned exact) hosts stations, the concierge, the bench. All three
apps are AGPL-3.0-only like the engine; the contracts stay Apache-2.0.

**The seams.** Browser to desk over a cookie with CSRF defences. Desk to engine, to
Kvasir and to the assistant over bearer tokens the desk holds. Assistant to engine
through one seam module that applies the station's grant and ceiling, sets the actor,
derives the idempotency key from the tool call id, and writes every call to its
ledger. Assistant to Kvasir as one pi provider with the person's token per request.
Kvasir to a runtime over loopback with one key. Engine to nothing.

**What a person sees.** One login, one shell that shows exactly what the deployment
has and the person may use, a question built by hand or with the helper beside it,
every version with an author, a compiled SQL panel, a result that is a handle with
three named states, a CSV, and, where the assistant is installed and the person holds
`assist`, a chat pane whose only powers are to talk, to read handles and to delegate
to a station whose proposal lands as a reviewable diff.

## 6. The wave

Four sections, twenty-seven slices, numbered in one sequence so the build order is
explicit; the full text of each is spec §13. Three checkpoints where the wave could
pause with something whole: after the engine section (the defects repaired, the
contracts published), after the desk and Kvasir's core (a usable hub and a
benchmarked model service), and after `ask-help` (the assistant at its first decision
point).

| Section | Repository | Slices |
|---|---|---|
| A, the engine | `kineuro/nils` | A0 the record and the spec; A1 the repairs and the live checks; A2 identity, the ceiling and the actor; A3 idempotency; A4 the ask additions; A5 the deployment surface; A6 the knob engine; A7 the contracts |
| B, the desk | `kineuro/nils-desk` | B1 the process and the shell; B2 the three identity modes; B3 the question page; B4 the result surface; B5 operations and data; B6 settings; B7 the assistant pane |
| C, Kvasir | `kineuro/kvasir` | C0 the serving benchmark; C1 the door and the catalog; C2 identity, keys, admission control, the ledger; C3 grants, purposes and policy; C4 the admission suite; C5 brought keys and the OAuth slot |
| D, the assistant | `kineuro/nils-assistant` | D1 the bench; D2 the host and the seam; D3 the station framework; D4 `ask-help` with the desk pane and the command line verb; D5 the concierge, clarification and memory; D6 `keyword-tune`; D7 `identity-check` |

Order: A0, then C0 in parallel with A1 to A7; then B1, B2; then C1, C2, C3; then B3
to B6; then C4; then D1 to D5 with B7; then D6, D7 and C5. Step 0 before any code:
the four live checks (page an operator's handle as a reader; queue a job as a reader
with identifiers projected; open five event streams against a four-worker engine and
call another door; mint one token and read the shape of the roles claim).

The wave closes when: the eight new gate fixtures are green on both backends; a
person in each identity mode builds a question by hand and the desk shows every
version, its author, what changed and what it would run; Kvasir's local model passed
admission and the benchmark met its thresholds or the floor was restated in writing;
the bench's baseline is re-run on the rebased corpus and `ask-help` beats it, needing
fewer than the 22 correction turns the researcher gave on the three chains; a fresh
Authentik deployment is three commands; the custody page lists every store the wave
added; and no station holds a decision-apply verb, which a grep proves.

## 7. What stays open

The roles-claim shape against the live instance (five minutes, step 0). Whether the
production host's driver allows a temporary MIG instance for the floor check. The
web application's name is settled by this record as `nils-desk` (18 §7 Q10); the
repository names are `nils-desk`, `kvasir`, `nils-assistant`. The federation spike
(workload identity federation with Authentik as the issuer) is not in the wave. Next
ids: C48 and D52.
