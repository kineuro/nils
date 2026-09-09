<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Wave 4c: the assistant, with the desk and Kvasir

The specification of the third of the three waves that the record's Wave 4
became (`docs/decisions/17-wave4-reframed.md`, R1 to R8), written from the study
that 17 §5 required before it could open (`docs/decisions/19-wave4c-the-assistant.md`:
Nima's rulings R9 to R11, the twenty questions Q1 to Q20 answered, decisions D41
to D51 and amendments C43 to C47). It follows `wave4b-the-ask.md` and cites the
record by id.

It is the wave in which NILS becomes a suite. The engine stays what it is, one
binary that owns the registry and answers at its doors, and gains the repairs and
the few doors the rest needs. Three optional parts arrive beside it, each in its
own repository and its own process: **nils-desk**, the one place a person opens,
which holds the login, the question page, every operation the command line has,
the settings, and a view that shows exactly what the deployment has;
**Kvasir**, the one service that holds a model credential and the one address any
part uses to reach a model, local or commercial; and **nils-assistant**, the
service that stands beside the engine and proposes, one agent per decision point,
never applying a judgement of its own. A deployment with none of the three is
still whole (R2). A deployment with all three is the aim of the record's vision:
intelligence available at every decision point, and required at none.

The one measure of the wave is stated in three sentences. For a question built
by hand, the desk shows every version, who made it, what changed and what it
would run. For the same question asked in words, the assistant needs fewer
corrections than the researcher had to give in the real traffic, and every
answer states its grain, its scope, its membership reading, its key, its pick
rule and its denominator in the same object as the number. And nothing the wave
adds can widen a person's reach, leak a row, or apply a decision without a
person.

## 1. What Wave 4c delivers

Four sections, twenty-seven slices, in the order of §13.

**The engine** (§5, §6, slices A0 to A7): the two disclosure defects the study
found on `main` repaired first, with gate fixtures that fail today; the identity
delta (a trust list, JWKS by URL, the role ceiling, the actor); idempotency on the
writing doors; six additions to the ask (the declaration block, the diff, the
draft, the guide, the value sampler, the `contains` operator); the deployment
surface (the policy table, `capabilities.assist`, ingest locations, the backup
job, the batches and packs doors); the knob engine (the four Wave 2 diagnostics,
the signals and counterfactual doors, overlays as registry objects, the ingest
probe); and one contract bump each for `openapi`, `review-item`, `suite` and `mcp`.

**The desk** (§7, slices B1 to B7): one Rust binary with the front end embedded,
the only origin a browser talks to; three identity modes; the question page as a
pure function of the ask document with one write path; the result surface with its
three named states; operations and data, the ingest half behind pre-registered
locations; settings for every part; the assistant pane; the app registry that
mounts later apps.

**Kvasir** (§8, slices C0 to C5): the serving benchmark first, before any code;
the pi-messages door and the catalog over one local backend held warm on an
allocated card; identity, minted keys, per-backend admission control and a
counts-only ledger; purposes, content classes, locality and the typed refusal;
the admission suite; and, last, brought keys and the OAuth slot.

**The assistant** (§9, slices D1 to D7): the bench before the first station; the
Flue host and the one seam; the station framework; `ask-help` with the desk pane
and the command line verb; the concierge, clarification and memory; then the two
knob stations, `keyword-tune` and `identity-check`.

## 2. What it rests on

The facts that constrain the design, each measured or read in the study and
recorded in 19 §2 and §3.

- **Two live defects on `main`.** `POST /api/ask/jobs` asks for the `reader` role
  and queues a run that the worker executes with a scope holding every class and
  `may_project_raw` set, so a reader can queue what the synchronous door refuses.
  `GET /api/ask/handles/{id}` and `/rows` check no owner and no class and write no
  read audit, so any reader pages any handle by integer id. Both were confirmed by
  reading; the live checks are step 0 of §13.
- **The unit of work is the refinement chain.** Three threads held 43 of 101 human
  turns, each one selection edited seven to seventeen times; a third of follow-ups
  were corrections, and every correction reduces to one of six decisions the
  question never stated: grain, scope, membership, key namespace, pick rule,
  denominator. The same question, asked five times, returned three session counts,
  and a definition written in prose in the prompt was read 27 times and still
  ignored. Anything that must be true is a compiled property of an object.
- **The deliverable is a CSV with a stable column list, never a chart.** Zero chart
  requests in nineteen threads of registry traffic.
- **The baseline is 17.9 percent**: 28 frozen gold tasks, one shot, exact result
  set, on the best model available at the time. A one-shot agent is wrong four
  times in five, which is why a station may look, retry and check, and why the bench
  exists before the station.
- **The load is prefill-dominated at roughly 100 to 1** (median input 23,600
  tokens, p90 72,000, median output 201) with prompt cache reads on two thirds of
  assistant messages. Caching is a requirement, not an optimisation.
- **Durability is user-facing.** A sixth of the turns died in the harness; people
  resent verbatim, then re-asked in a fresh thread, which is how one question came
  to have three answers. A dead turn must resume from the document.
- **Local models are a different animal on this workload**: 45 percent SQL error
  rate against 13 for frontier models, and three of seven local threads died in the
  serving layer. Tool-call correctness and chat-template handling decide whether a
  model may be offered, not tokens per second.
- **Nima's rulings** (19 §1): local models may see patient information and one
  whole card may be allocated to them (R9); user management fits Authentik with
  per-user visibility of features, and is optional (R10); the model service is
  Kvasir (R11).

## 3. What Wave 4c does not do

- **Charts.** The result surface is a table, a download and a saved handle.
- **Auto-commit** of any proposal, at any confidence, for any station. The record
  allows a per-kind policy; it stays off for the whole pilot and reopens only with a
  measured rate from the bench.
- **A judge on any gate.** Hard checks only. If a judge is ever added it is
  advisory, on a different model, blind to the hard result, with both rates reported
  side by side.
- **Fine-tuning and trajectory training.** Capture exists as a teaching mode a person
  turns on; nothing trains on it in this wave.
- **A sandbox, a shell or a filesystem for the assistant.** No station has one. If a
  workspace is ever needed it is an explicit-root directory, never the process
  working directory.
- **An MCP client of our own, and the retirement of the MCP door's dated audience
  deviation.** The door stays the third-party surface (D41); the deviation retires
  in the slice that registers the first OAuth-speaking client, which is not here.
- **Cell suppression inside the site** (D51). The egress policy of §8.3 is the
  control; D27's federation defaults stand for a result that crosses to another node.
- **Restore as a button** (D50). Backup is a job; restore is a printed command.
- **Flipping the identity provider's instance-wide issuer mode**, four applications
  in the pilot, or a separate `nils-auth` service. The trust list makes each of them a
  configuration change later.
- **A second Kvasir replica, a Redis, a second Postgres cluster.** One process, one
  server, one database per part.
- **Workload identity federation** (the organisation's model credential as zero
  stored secrets). A spike with a falsifiable test, outside the wave.
- **Migration** of anything from v0 (R4). The bench's corpus is paraphrased and
  rebased; nothing is copied.

## 4. The suite

### 4.1 Parts and repositories

| Part | Repository | Language | Process | Store |
|---|---|---|---|---|
| the engine | `kineuro/nils` | Rust | `nils serve` | the registry (SQLite or Postgres) |
| nils-desk | `kineuro/nils-desk` | Rust, React front end embedded | `nils-desk` | one SQLite file: sessions, display names, preferences, local users |
| Kvasir | `kineuro/kvasir` | TypeScript on the pinned pi-ai | `kvasir` | one database: catalog, keys, credentials, ledger, admission |
| nils-assistant | `kineuro/nils-assistant` | TypeScript on Flue, pinned exact | `nils-assistant` | one database: conversations, verdicts, notes, evals |

All four are AGPL-3.0-only; the contracts stay Apache-2.0 under the DCO. The
minimal deployment is two Rust binaries and needs no Node. The intelligent tier
(Kvasir and the assistant) is Node 22 or later and arrives together, because the
assistant is Kvasir's only consumer in this wave.

### 4.2 Absence

Every part is optional and absence is silence (D1). The engine publishes
`capabilities.assist` only when started with `--assist URL`. The desk composes one
**deployment capabilities document** (§4.3) and every section, control and menu
item is a predicate over it; a missing part removes the control rather than
disabling it. A deployment with no assistant has no chat pane, no helper button,
no assistant section in settings and no `assist` entitlement in use. A deployment
with no Kvasir has no models table. A deployment with no identity provider runs in
`off` or `local` mode (§5.1).

### 4.3 The deployment capabilities document

One JSON object the desk builds at session start and refreshes when the engine's
epoch moves or a part's health changes:

```
{ "engine":    <GET /api/capabilities of the engine, verbatim>,
  "kvasir":    <GET /v1/config of Kvasir, or absent>,
  "assistant": <GET /capabilities of the assistant, or absent>,
  "apps":      [ <each registered app's capabilities document, or its absence> ],
  "person":    { "subject", "display_name", "entitlements", "roles" },
  "desk":      { "version", "mode", "contracts" } }
```

Its shape is fixed in `contracts/suite/v1` (§6.7). Nothing else in the desk may
ask "do we have X".

### 4.4 The app registry

The desk's configuration lists apps:

```
apps:
  - id: pipelines
    title: Analysis pipelines
    url: http://127.0.0.1:7300
    entitlement: operator
    capabilities: /capabilities
```

For each, the desk adds a section, proxies `/apps/{id}/*` to the app's URL on the
one origin with the person's token attached and the CSRF defences of §7.1 applied,
and merges the app's capabilities document into §4.3. An app is any web service
that reads a bearer token the trust list verifies. This is the group's own portal
pattern and it is the answer to R8: a study-management app later is a registry
entry, a set of purposes in Kvasir's configuration, and a directory of station
manifests the assistant loads (§9.4), and nothing else changes.

### 4.5 The suite contract

`contracts/suite/v1` fixes the vocabulary every part shares: the entitlements
(§5.2), the purpose registry entry (§8.3), the content classes and localities
(§8.3), the actor object and the ceiling header (§5.5), the idempotency header
(§6.3), the deployment capabilities document (§4.3), the app registry entry (§4.4)
and the station manifest (§9.4). Every part carries a test that its own documents
validate against it.

### 4.6 Transport (D41)

Our own stations reach the engine over the HTTP doors under `contracts/openapi`,
through one seam module (§9.3). The MCP door stays for third-party clients and
gains four operations in this wave (§6.4). The pack's model-facing content
(`packs/mri/mcp.yml`) is the single source of what a model is told on either
path: the guide door (§6.4) serves it to our stations and the MCP door serves it
to anybody else, so the same grounding and the same worked examples reach a model
whichever socket it dialled. This amends D11, C22 and C23 (C43).

## 5. Identity

### 5.1 Three modes

The desk has three identity modes; the engine and Kvasir have their existing
three (`off`, `token`, `oidc`) and every multi-user deployment uses `oidc`.

| Desk mode | Who it is for | Engine and Kvasir | Login | Users live in |
|---|---|---|---|---|
| `off` | one person on one machine | `--auth off`, loopback | none | nowhere |
| `local` | a group with no identity provider, or one that wants a simple login and admin control (R10) | `oidc`, one trust entry pointing at the desk | username and password at the desk | the desk's store |
| `oidc` | a group with a provider (Authentik in ours) | `oidc`, one trust entry per application | the provider, authorization code with PKCE | the provider |

In `local` mode the desk is a small issuer (C46): an EdDSA signing key generated
at first start, a JWKS at `/.well-known/jwks.json`, a discovery document at
`/.well-known/openid-configuration`, and tokens of fifteen minutes minted only for
the subject that just authenticated, with entitlements no wider than the ones
stored for that user. Passwords are argon2id. An admin (the first user, created by
`nils-desk user add --admin`) grants and revokes entitlements on the settings page.
The signing key file is readable by the desk's account only. The engine and Kvasir
cannot tell this mode from a provider, which is the point: adding a provider later
is one URL and a re-login.

`nils-auth` as a separate service is not built; the name stays in the record.

### 5.2 The entitlement vocabulary

Five names, fixed in `contracts/suite/v1`, carried in the token's `roles` claim as
an array of plain strings:

| Entitlement | What it opens |
|---|---|
| `reader` | the question page, results, handles, selections, status, custody, audit reads |
| `reviewer` | the review queue, saving selections, accepting proposals |
| `operator` | jobs that ingest, releases and handovers, session rebuild, overlays' adoption, the ingest probe, backups |
| `admin` | settings that change other parts: the models table, Kvasir's policy, users in `local` mode |
| `assist` | the assistant in any form: the chat pane, the helper beside the question page, the command line verb |

The first four are the engine's ladder and imply the ones below. `assist` is
orthogonal: a reader with `assist` may use the helper at the reader's reach; an
operator without `assist` sees no assistant at all. In `oidc` mode they are
application entitlements on the one NILS application, bound to groups, reaching the
token through the managed entitlements scope mapping; the engine runs with
`--oidc-groups-claim roles --role reader=reader --role reviewer=reviewer
--role operator=operator --role admin=admin` and ignores `assist`, which only the
desk and the assistant read. In `local` mode the desk stores them per user. In
`off` mode the one person holds all five.

### 5.3 The trust list

`--oidc-trust issuer=URL,audience=ID,jwks=URL`, repeatable, replaces the three
single-valued flags of Wave 4b (which stay as sugar for one entry). Each entry is
verified on its own: the JWKS is fetched at start and refetched on a key-id miss
with a floor of one minute between fetches; a key that is not RSA, EC or OKP is
skipped; an entry with no usable key refuses to start. The token's `iss` and `aud`
select the entry; the audit row records which entry admitted the caller. Kvasir
takes the same flag with the same semantics, and `contracts/suite/v1` ships the
test vectors (tokens, keys and expected outcomes) that both implementations run.

The first deployment has one entry, the NILS application. A second application
arrives with the command line's device-code client or a third-party MCP client,
whenever either exists, and costs one more flag.

### 5.4 The desk as the origin

The engine's doors have no CORS handling, no cookie, no CSRF token, and the
contract declares no security scheme until this wave. So a browser never addresses
the engine: the desk holds the session in a cookie (`Secure`, `HttpOnly`,
`SameSite=Lax`), holds the person's access and refresh tokens server side, and
proxies `/api/*` to the engine, `/kvasir/*` to Kvasir, `/assistant/*` to the
assistant and `/apps/{id}/*` to registered apps with the bearer attached. Every
non-GET through the proxy requires the header `X-Nils-Desk: 1`, which a
cross-origin form cannot send, and an `Origin` or `Referer` that matches the
desk's own origin; a gate fixture posts a cross-origin form to a write door and
asserts it is refused. Logging out deletes the server-side session; the access
token then dies at its own expiry, at most fifteen minutes.

The desk's own API paths must not sit behind a forward-auth outpost, which strips
`Authorization`; the desk may, the engine may not.

### 5.5 The assistant as the person: the ceiling and the actor

The assistant holds no credential of its own. The desk hands it the person's
token per turn and pushes a fresh one before expiry for every conversation with an
open submission (19 Q8). The assistant's seam sets two headers on every engine
call:

- `X-Nils-Ceiling: reader|reviewer|operator`. Applied before the role check at
  every door, it can only remove roles; the effective roles are the token's roles
  capped at the ceiling. Recorded on every audit row as `ceiling`. Each station
  manifest names its ceiling; `ask-help` runs at `reviewer`.
- `X-Nils-Actor: {"kind":"agent","name":"ask-help","model":"...","version":"...",
  "conversation":"..."}`. Stored on the audit row, the handle's provenance and the
  decision. An absent header is recorded as `{"kind":"absent"}`, so "no actor" and
  "a person acting alone" are never confused. The review-item contract already
  carries `decision.author_kind` with `person|agent|model`; v3 adds the actor object
  beside it.

A forged ceiling is harmless; a forged actor is a claim the caller is already
inside the boundary to make, and the field is what makes the desk's own claim
auditable. When token exchange is enabled in a later deployment, the `act` claim
becomes the actor's source and the engine reads it; A2 records `act` when present.

### 5.6 Machine principals

In `oidc` mode, jobs and pipelines that call the engine on their own use a
provider service account with an app password over the client credentials grant,
which yields a token the trust list verifies; the principal's shape marks it as a
machine in the audit log. In `local` mode the desk's admin mints machine tokens the
same way it mints personal ones, with a name and a role list. In `token` mode
(machines only, never a person) the installer never emits the bare form: a token
value with no role suffix holds every role, and A1 makes the engine warn at start
for any such token.

### 5.7 Registration in three commands

The smallest new deployment on a fresh provider is three commands: one that makes
the desk's secrets, one idempotent registration script (`nils-desk register
--authentik URL --token ...`) that creates the application, the OAuth2 provider
with a signing key, the policy binding, the five entitlements bound to groups the
operator names, and the entitlements scope mapping, then prints the trust flag and
the role flags to paste, and one that starts the engine. A second run changes
nothing. Without the script the word "seamlessly" in the aim is not true.

The registration sets the provider's access token validity to fifteen minutes and
its refresh token validity to thirty days, and it enables the `offline_access`
scope so the desk can refresh.

### 5.8 Command line login

`nils login` gains two paths. `--desk URL` in `local` mode posts username and
password to the desk and receives a token of one day, kept in the user's config
directory. In `oidc` mode the person creates an app password at the provider and
`nils login --issuer URL --client ID` exchanges it over the client credentials
grant for a token the engine verifies, re-exchanging on expiry. The device
authorization grant, which needs a public client and therefore a second
application, is the growth path and is not in this wave.

### 5.9 The principal and the display name

The engine's principal is the subject at the issuer's host, opaque under the
provider's default subject mode; the desk records the display name beside the
subject at first sight, in its own store, and shows it everywhere a person is
named. The engine keeps `preferred_username` and `email` in its claims cache and
lists them in custody (A2), and neither is ever a key. Rows written under `off`
and rows written under `oidc` do not join; the switch is documented as one way,
with a dated boundary marker written into the registry at the moment of the
change.

## 6. The engine

### 6.1 The repairs (A1)

1. **The job carries the caller.** `POST /api/ask/jobs` stores `principal`,
   `roles`, `ceiling`, `actor` and `may_project_raw` (derived exactly as the
   synchronous door derives them) in the job's args; the worker's `nils ask run`
   under a claim builds its scope, its principal and its projection flag from
   them and never from the worker's own. A job whose args carry no roles keeps
   today's behaviour only when it was started from a terminal. `job::finish` takes
   a result document and `GET /api/jobs/{id}` returns it: for an ask run, the handle
   id, the hash, the row count, the content hash and whether a cap truncated it.
2. **A handle read is authorised.** `GET /api/ask/handles/{id}`, `/rows` and every
   export refuse with 403 when the handle's recorded classes exceed the caller's
   scope under the caller's ceiling, and write a `handle_read_audit` row for every
   page and every export, carrying the actor and an optional `purpose` the caller
   supplies. Today the classes are provenance and the only read audit is on an
   identifier reveal.
3. **The events door checks the reader role**, and concurrent streams are capped at
   `--event-streams N`, default half the worker count and at least one, with a typed
   refusal `event_streams_full` above it, so four open tabs cannot pin every worker.
4. **A bare machine token warns at start.**
5. **The ask reader is required where an assistant is installed**: `nils serve
   --assist URL` refuses to start without `--ask-dsn`, and a gate fixture opens a
   connection with the reader's own credentials and asserts that `INSERT`, `UPDATE`,
   `DELETE`, `CREATE` and `COPY ... TO` all fail against a live server.

### 6.2 Identity, the ceiling and the actor (A2)

The trust list (§5.3); JWKS by URL with the key-id refetch; the `act` claim
recorded when present; the display name and mail kept in the claims cache and
listed in custody; the ceiling header and the actor header applied at every door
and recorded on every audit row, on handle provenance and on decisions (§5.5);
`review-item` v3 prepared. Everything lands inside `Auth` and its two call sites,
because every handler downstream already sees only `Caller { principal, roles }`.

### 6.3 Idempotency (A3)

`Idempotency-Key` on `POST /api/ask/run`, `POST /api/ask/jobs`, `POST /api/ask/apply`
and `POST /api/ask/handles/{id}/promote`, the four doors whose repeat creates a row
with an audit consequence (documents are content-addressed, values uploads and
selection writes are idempotent by construction). Scoped to the principal, kept 24
hours with the response, answered with `deduplicated: true` on a repeat, refused 409
when the same key arrives with a different body. The assistant's seam derives the
key from the tool call id it already has. A retried run must not duplicate a
handle, and a retried reveal must not make the custody record say a person read
identifiers twice.

### 6.4 The ask additions (A4)

1. **The declaration block**, on the answer of `run`, `preview` and an ask job:
   `{grain, session_scheme: {name, digest}, membership, key_namespace, pick_rule,
   denominator, disclosure, truncated}`. The engine already emits the sentence;
   nothing called it. This is the cheapest correctness win in the wave, because the
   six silent decisions are what the corpus corrected 22 times.
2. **`POST /api/ask/diff`** (19 Q14): two documents, two handles, or one of each.
   Documents: the structural diff over the canonical form (per set, per clause:
   added, removed, changed) plus both canonical texts. Handles: the content hashes
   and the row-set comparison the handle stores; refused with a sentence when either
   is truncated, because a capped result has no hash.
3. **`POST /api/ask/draft`** and `nils ask draft`: the affordance that exists in the
   library and nowhere else gets its door: authored text in, add-only repair,
   diagnosis, stored when it validates. The engine still never parses a sentence;
   words stay in the assistant.
4. **`GET /api/ask/guide`**: the pack's grounding rules, the worked examples, the
   ask schema digest, the policy table (§6.5) and the caps in force. One source for
   what a caller is told, on both transports (D41).
5. **The value sampler**, `GET /api/ask/catalog/{level}/{field}/values`: for a
   field with a declared vocabulary or low cardinality, the values with counts; for
   free text and for any field classed quasi-identifying or sensitive, shapes with
   counts; the distinct count either way; bounded by `Caps::options_values`; under
   the caller's scope and ceiling. Sampling was the most-used catalog mode in real
   traffic and its absence is what made the model guess a token.
6. **`contains`** (19 Q10): a clause `["contains", {}, <field>, <text>]` over the
   fields the catalog marks as free text, which are the ones with a case-folded
   companion column since Wave 4b, compiled to a substring match on that column,
   never a regular expression, and refused on any other field. A population named
   by an identifier prefix is promoted to a saved selection automatically and the
   describe sentence says so. The pair grain and disk reconciliation get one fixed
   refusal sentence each in `packs/mri/mcp.yml`, naming where the answer does live.
7. **Node-level describe**: `POST /api/ask/describe` takes an optional `node`
   (a set and a path) and answers `{display_name, long_display_name, flags}`, so
   the chip label, the assistant's tool output and the audit line come from one
   call and are byte-identical.
8. **Four MCP operations**: `guide`, `draft`, `job` and `job_status`, in the closed
   operation list and opted in by the pack, so a tool-only third-party client can
   obtain the worked examples and run a long question.

### 6.5 The deployment surface (A5)

- `capabilities.assist` when `--assist URL` is set: `{url}`.
- `capabilities.policy[]`, one row per operation: the role it needs, whether it
  writes, whether it takes an idempotency key, its cost class (`free`, `bounded`,
  `job`), its result cap, and a human label in present and past tense. The desk's
  controls, the MCP door's gating and the audit line derive from it, so a new door
  cannot add a hole.
- **Ingest locations**: `--ingest-root name=path`, repeatable, published as
  `capabilities.ingest_roots[]` by name only. The job kinds `digest`, `classify`,
  `fingerprint` and `linkage-import` accept `{location, path}` where `path` is
  relative and may not escape the root; they refuse any absolute path. This is how
  the desk offers the ingest half of the command line without browsing the host.
- **`backup`** as a job kind writing to `--backup-dir`, audited, cancellable, and
  **`verify`**, which checks an archive's integrity and lists what it holds. Restore
  is `nils restore ARCHIVE`, documented, with a pre-restore backup in the procedure.
- **`GET /api/batches`**, `GET /api/batches/{id}` (the report's counts and
  diagnostics, samples as shapes only), **`GET /api/packs`**, `GET /api/packs/{name}`,
  **`GET /api/quarantine`**.
- `capabilities.event_streams` and `capabilities.ceiling` (the header's name and
  its values), so a client learns both from the engine.

### 6.6 The knob engine (A6)

Built to Wave 2 §10's names, which the code never implemented (C44):

1. **The four diagnostics**: `axis_conflict` (two rule sets reached different
   values for one axis on one stack and the order decided it, both recorded),
   `axis_unresolved`, `keyword_shadowed` (a keyword that can never match because an
   earlier rule always wins, which needs the evaluator to report a pre-empted match)
   and `overlay_unused` (a site term that matched nothing). Counted per batch and per
   kind with samples as shapes; a new review kind is not a contract change.
2. **`GET /api/classify/signals?scope=`**: per axis over a batch, an origin or a pack
   version: how many stacks resolved at each tier, the confidence spread, the open
   review items by kind, the shadowed keywords, the unused overlay terms, and the
   terms that carried the most decisions that disagreed with the rules. Over the
   evidence rows and the raw fingerprint fields, not only the six axes, because the
   signals that decided a real pick were the image type string, the echo time, the
   slice count and the reconstruction variant.
3. **`POST /api/classify/try`**: a candidate overlay against a bounded sample in a
   named scope, writing nothing, answering with what would change per axis (from
   which value to which, how many), how many review items would close and open, and
   the pass or fail of the overlay's own corpus cases. This is the change manifest's
   `predicted_impact`, computed rather than guessed.
4. **Overlays as registry objects**: `POST /api/overlays` at `reviewer` stores a
   candidate with a name, a version, an author, the actor, the scope, the buckets,
   the cases and the `try` result that justified it, status `proposed`, and emits a
   review item beside it; `POST /api/overlays/{id}/adopt` at `operator` queues a
   reclassify job over the scope and writes an audit row. The existing ranking
   (person over agent over model) refuses an adopt by a lower author than the one
   who decided. Export to the pack directory stays a command.
5. **`POST /api/ingest/probe`** at `operator`, queued as a job: an ingest location
   id, a bounded file sample and one or more candidate identity rules; per candidate,
   the shape histogram of each source, whether the first field source is constant
   (`identity_constant`) with its shape, how many files each source answered, fell
   through or failed to parse, the subject and study counts under that rule, and the
   reader's diagnostics. Shapes, never values; no path in any response; nothing
   written. Two candidates side by side is the whole point.

### 6.7 The contracts (A7)

Each in a titled pull request under the DCO: `openapi` v3 (every door of this
section, the security scheme, the three headers, the declaration block, the
policy table; published in A4, see the as-built note of §13), `review-item` v3
(the actor; published in A2; v4 adds the `overlay` scope in A6), and at the end of the engine section `suite` v1
(§4.5) and `mcp` v1 (the operation
vocabulary of sixteen, the input schema per operation, the result envelope, the
paging contract, the policy fields). The desk and the assistant generate their
clients from `openapi` v3; the desk warns on a minor mismatch and refuses only a
major one, so a partial upgrade does not brick a deployment.

### 6.8 The gate fixtures

Eight fixtures join `gate/`, each of which fails on `main` today, run on both
backends in CI:

1. Write refusal measured against a live server with the ask reader's credentials.
2. A reader queuing a document that projects identifiers is refused; a reviewer
   queuing a document with sensitive columns gets the synchronous door's answer.
3. A reader paging an operator's handle is refused; a page read writes an audit row.
4. Two identical run calls with one key produce one handle and one audit row.
5. No seeded marker escapes the value sampler or the probe, seeded in the registry
   and in a synthetic tree.
6. Five event streams against a four-worker engine leave every other door answering.
7. A ceiling of `reader` on an operator's token cannot promote, and the audit row
   records the ceiling and the actor.
8. The published ask schema still contains `prefixItems`, `minItems`, `oneOf` and the
   recursive `anyOf`, so the grammar-backend risk of §8.6 stays visible in a gate
   that has no serving runtime.

A cross-origin write through the desk's proxy is the desk's own fixture (§7.1).

## 7. nils-desk

### 7.1 Shape

One process, the only origin a person's browser talks to (§5.4). It holds the
session, the tokens and one small SQLite store; serves the front end from bytes
compiled into the binary; proxies every part; and owns nothing that answers "who
changed this", which is the engine's actor (D43). It is not new doors on `nils
serve`: the engine writes a content length and never a chunked body on a
thread-per-worker server with no async runtime, and a browser session, server-sent
events and a long-lived proxy do not belong there.

### 7.2 The shell

A pure function of the deployment capabilities document (§4.3) and the person's
entitlements. Sections: **Ask**, **Results**, **Operations** (jobs, review,
releases and handovers, custody, audit, sessions), **Data** (packs, batches,
quarantine, ingest), **Settings**, **Assistant** (present only when the assistant
answered and the person holds `assist`), and one section per registered app. Each
control is a predicate: a missing part, a wrong document shape and a missing
entitlement each remove it. Three states the shell must render by name: the
**unbound person** (an account that exists and holds no entitlement sees a page
naming what an operator must bind, never a 403 body), **warming** (Kvasir's backend
has not produced its first token since start), and **contract mismatch** (the desk
names the version it found and the versions it speaks).

### 7.3 The question page

The editor is derived from the ask document on every render; the only stored UI
state is which empty step is open. A step per named set in the order the engine
reads them, with sub-steps for the clause groups (source, near, attach, has, where,
pick, out, window), each carrying `valid` (answered by the catalog and the grain),
`active`, `visible`, `revert` (the inverse move) and `preview` (a capped run of the
prefix). Every control is populated by `POST /api/ask/options` and every edit goes
through `POST /api/ask/apply`, whether a person clicked a chip, clicked a result
cell (which offers only the moves whose template has a hole the clicked value can
fill) or accepted the assistant's proposal. There is no code path in the desk that
composes ask JSON, and a stale `409 stale_options` is met by refetching, never by
retrying blind.

Printed by default under the title: the declaration block. A **compiled SQL
panel** from `POST /api/ask/explain`, read only, both dialects. **Preview per
step**, ten rows, which goes stale on purpose and shows a Refresh button. A
**diagnose drawer** that appears when a result is empty or smaller than the
previous version. The **version chain** with an author on every line, from the
engine's document parents and actors. A **diff view** from `POST /api/ask/diff`.
The column list the corpus asked for by name (subject key, cohort, session date,
stack id, the six axes, and on request slice count, resolution, sex, birth date,
age at session) is a projection preset in the `out` step. **Compare** is a
command over two handles or two documents.

### 7.4 The result surface

A run leaves a handle; the desk pages it from `GET /api/ask/handles/{id}/rows` and
never re-executes. Three named states with three treatments: **running** (progress
by 30 seconds, designed for a two-minute tail, anything unbounded through the job
door), **stale** (the document moved since the result was produced; the result is
covered by a named overlay), **truncated** (the row count is the control that
edits the cap, "your limit" is told apart from "our truncation", and release and
promote are disabled with the reason on the control). Export is CSV off a handle,
authorised per request against the caller and the handle, with the read audit of
§6.1 and a purpose the person may type. No charts.

### 7.5 Operations and data

Thin tables over doors that exist, each gated by the entitlement the door wants:
jobs (live from `GET /api/events` under the cap, with polling as the fallback),
review (with the keyword tab of §9.13 beside it in D6), releases and handovers
(the desk refetches `describe` and the version chain, shows exactly what will be
released and who authored each edit, and requires the person to type the
selection name), custody, audit, sessions. Data: packs, batches with their reports
as shapes, quarantine, and **ingest forms** for digest, classify, fingerprint and
linkage import over the pre-registered locations of §6.5, each a job. **Backup** is
a job with its archives listed; **verify** is a job; **restore** is a page that
prints the exact command and the pre-restore procedure (D50). Key management stays
on the command line.

### 7.6 Settings

One page, one section per part, each setting owned and enforced by the part that
has it. **Engine**: read only, everything `capabilities` says, and the flags to
paste when a change needs a restart. **Desk**: identity mode, engine URL, session
lifetime, export permissions, retention of its own store; in `local` mode the
users and their entitlements. **Kvasir**: backends and their health, the **models
table** (one row per registered purpose: its app, its content class, its current
backend, the localities its class allows, and the acknowledgement an admin gave
where a `rows` purpose was opened to a remote backend), minted keys, the
organisation's commercial key, and in C5 brought keys and the OAuth slot with the
provider-specific explanation. **Assistant**: the station list with their briefs'
hashes, budgets, whether a teaching session is open, telemetry (content off, and
a content-bearing exporter shown as a separately approved decision). **Apps**: the
registry. Every settings write sits behind the same identity as the data and none
is reachable from any network where model-authored code runs.

### 7.7 The assistant pane

Two panes, question left, chat right, stacked below about 700 px, sharing exactly
two values: the current document id and the epoch. The assistant emits a closed
union of typed parts (§9.8) and one reducer of under 300 lines decides what each
may touch; anything unknown is dropped. A `move_proposal` is applied to a
**scratch** document handle, rendered as a unified diff of the two canonical texts
with the assistant's sentence above it; accept moves the pointer and the engine
records the actor; reject discards the handle and sends the rejected moves back as
feedback so the next turn knows. A `choice` renders typed options with the count
each would produce. The pane shows **warming** when Kvasir says so, never
"thinking". Conversations carry parent pointers from the start; no branch picker
in this wave.

### 7.8 Technology and the bundle

Rust (axum, tower-sessions, an OIDC client with PKCE, jsonwebtoken, rusqlite), the
React and TypeScript bundle built at release and embedded, one artefact, one
process, no web server to configure. The front end keeps no state library, because
the editor derives from the document. Copied from Metabase: the notebook shape,
the affordance naming (`<x>able`, `available_<x>`, `suggested_<x>`), the click
model and the typed data-part frame. Not copied: plugin indirection, an embedding
bundle, content translation, module-boundary linting. The build needs a Node
toolchain at release time only; the shipped binary does not.

### 7.9 Acceptance

For a question built by hand, the desk can show every version, who made it, what
changed, and what it would run, in each of the three identity modes. A caller with
no entitlement sees a named page. Presenting a result twice executes the query
once. The audit line for a cell click and for a chip click are the same shape.
Applying an assistant edit and reverting it produces a byte-identical canonical
document, and the diff of a no-op edit is empty. A deployment with no assistant
renders no assistant section anywhere. A cross-origin form post to a proxied write
door is refused.

## 8. Kvasir

### 8.1 What it is, and what it refuses to be

One service in its own container on the production host beside the allocated card,
one database, one process. It does four things: authenticate a caller and resolve
it to a principal with roles; answer "what model can serve this need" or refuse
with reasons; stream a model response holding the credential, counting tokens and
enforcing caps; record who spent what, in counts. Every part of NILS that talks to
a model talks to Kvasir.

It is **not** an agent runtime (no prompts, tools, memory or turn retries), **not**
a model host (it supervises runtimes for health, warmth and admission only),
**not** a second user directory (no signup, no invitation, no SSO of its own),
**not** a prompt log (no message body, tool argument, tool result or provider error
body is ever written to disk or to a database, and there is no code path that
could), **not** a clever router (no silent fallback, no injected prompt, no cache;
a downgrade is a refusal unless the caller asked for it by name), and **not** a
general egress proxy (provider adapters are a fixed list in code; a caller cannot
name a base URL).

### 8.2 The door

Primary: **pi-messages**, `POST /v1/messages` taking `{model, context, options}`
and answering the SSE event union pi defines, plus `GET /v1/config` returning the
catalog. Choosing pi's own protocol removes every compatibility flag that
URL-guessing would otherwise set on a local runtime, and a thinking block round
trips with its signature intact. Secondary: an OpenAI-shaped `POST /v1/chat/completions`
and `GET /v1/models` for a notebook or a script; its purpose and content class
resolve from the minted key it was called with (§8.4), never from the call, so it
cannot be the hole in the egress policy. No Anthropic-shaped inward door: "both
shapes" is about which providers Kvasir reaches, and pi-ai reaches both.

Kvasir's own doors: `POST /v1/grants`, `GET /v1/purposes`, `PUT /v1/purposes/{id}/policy`
(admin), `GET /v1/backends`, `POST /v1/keys` and `DELETE /v1/keys/{id}`,
`GET /v1/ledger`, `GET /v1/admission`, `POST /v1/credentials` (C5), `GET /healthz`,
`GET /metrics`.

### 8.3 Purposes, content classes, locality, grants and refusals (D44)

A **purpose** is registered by an app in Kvasir's configuration, never invented by
a caller:

```
purposes:
  - id: assistant.ask-help
    app: nils-assistant
    content: rows
    kind: foreground
    description: turn words into an ask document, or tune one step of it
  - id: assistant.compaction
    app: nils-assistant
    content: rows
    kind: background
  - id: assistant.title
    app: nils-assistant
    content: catalog
    kind: background
```

An unknown purpose is a 400. `content` is the class of what the purpose can carry:
`catalog` (names, axis values, describe sentences, the person's words), `rows`
(handle rows, counts, sampled values, funnels, review evidence), `identifiers`
(anything that projects `out.identifiers` or reaches the linkage store). Every
backend has a `locality`, `local` or `remote`. The **policy table** maps each
purpose to the backend it uses; defaults are local. An admin opens a `catalog`
purpose to a remote backend by choosing one in the desk's models table; opens a
`rows` purpose only with a recorded acknowledgement that rows of the archive will
leave the site; and can never open an `identifiers` purpose. A request whose user
text matches an identifier shape is bumped to `identifiers` for that request and
runs local whatever the table says, and the person is told why. `kind` decides the
retry policy: foreground calls retry capacity errors, background calls bail at once.

A **grant** is a requirement in and an answer out:

```
POST /v1/grants
{ "purpose": "assistant.ask-help",
  "need": { "tools": "required", "structured": "strict",
            "context_tokens": 72000, "max_output_tokens": 2048,
            "reasoning": "medium" },
  "pin": null }

200 { "grant": "g_...", "model": "qwen-27b@card", "backend": "card",
      "locality": "local",
      "limits": { "context_tokens": 65536, "max_output_tokens": 8192 },
      "capabilities": { "tools": true, "structured": "strict",
                        "reasoning": ["off","low","medium","high"] },
      "budget": { "remaining_tokens": ..., "resets": "..." },
      "chose_because": ["the policy table maps this purpose to card",
                        "admitted 2026-09-.. against sglang ..."] }
```

`limits` are measured at start from the runtime's own report, never copied from a
model card, because a declared window larger than the served one silently
strangles output to one token. A person may pin a model (recorded), never above
the policy. When nothing qualifies, the refusal names the layer that removed each
candidate (`deployment`, `requirement`, `policy`, `quota`, `health`) and the one
relaxation that would admit it, so the desk can say one actionable sentence. There
is never a silent downgrade.

### 8.4 Credentials

Three kinds, three tables, three lifecycles (19 Q3, Q19):

- **Minted keys** (`kvs_` prefix): thirty-two random bytes shown once, stored as a
  keyed BLAKE2b hash, compared in constant time, revoked by deleting a row. A key
  carries a principal, a purpose allowlist, a maximum content class and an expiry. A
  key with no purpose reaches only local backends. This is the machine path, the
  script path and the standalone path.
- **The organisation's commercial key** (C3): one per provider, encrypted at rest
  with XChaCha20-Poly1305 under a key held in a file outside the database, the
  provider and the row bound in as associated data, decrypted only in memory at use,
  never returned, never logged. Rotation re-encrypts; it never replaces the
  encryption key. The test provider Nima named is the first one registered, with
  both API shapes admitted.
- **Brought keys and OAuth grants** (C5): a person's own provider key, encrypted the
  same way with the person's subject bound in; an OAuth grant with access and
  refresh, refresh rotated on every use, both deleted on revoke, refresh serialised
  per person and provider through a database advisory lock, PKCE, a state bound to
  the session and one fixed redirect on Kvasir's own origin. The personal
  subscription source is offered for the provider whose terms permit it and shown as
  **absent by policy** for the one whose terms as of 20 February 2026 forbid it, with
  the sentence and the date on screen. The credential path is keyed on the immutable
  subject, never a username.

### 8.5 The runtime supervisor and the profiles

Kvasir holds the only key each runtime knows; the runtimes bind to loopback and
are started by the deployment, long-lived and warmed, never on demand. Kvasir
reports a backend unhealthy until it has produced a first token since start, and
the desk shows "warming".

- **`card`** (this deployment, R9): SGLang on one whole 96 GB card, the 27B model
  in BF16 with an FP8 KV cache, `--grammar-backend llguidance`, a tool-call parser
  and a reasoning parser for the model family, speculative decoding, and
  `--max-running-requests 8` set explicitly, because speculative decoding resets it
  to 48 when unset. No MIG on the host.
- **`slice`** (the smallest supported deployment elsewhere): llama.cpp on a 24 GB
  instance with a 4-bit GGUF, eight slots, a q8 KV cache (never q4, which degrades
  tool calling), `--jinja`, an API key file. Validated once during the benchmark
  window on a temporary instance if the driver allows; otherwise recorded as
  untested. NVFP4 W4A4 is ruled out on this GPU generation until a local run shows
  no NaN and no fallback.

### 8.6 Admission

A local model is not in the catalog until it has passed a suite, recorded with a
date, the runtime version and the build, and re-run on every runtime upgrade:

1. tool calls: twenty fixtures produce well-formed calls against our own schemas;
2. chat template: a system message in the position our clients send it is accepted;
3. enforced schemas: a real schema with `minItems` is either enforced or refused,
   never accepted and ignored; the negative control asks for a one-element clause,
   which a backend honouring `minItems: 2` cannot produce;
4. overflow: a deliberately oversized prompt yields a matching error, a silent
   truncation with a length stop and zero output, or a silent accept, and the answer
   is published as a catalog flag;
5. stream integrity: a thinking block's signature survives the round trip
   byte-identical.

Admission is mechanical. No model judges another model's admission.

### 8.7 The ledger and quotas

One row per stream: subject, purpose, model, backend, grant id, input, output,
cache read, cache write and reasoning tokens, GPU seconds, money where a provider
charges it, time to first token, total latency, outcome (`completed`, `refused`,
`error`, `aborted`, `capped`), and for a refusal the layer and the fact. No content
column exists in the schema, and a test asserts it. A provider error body is kept
in memory for the retry decision only; downstream receives a classified code and a
fixed sentence, never the body. Prometheus counters and histograms carry the same
rule: no label with content or a display name.

Concurrency is admitted per backend: eight streams on the local backend, a queue of
sixteen with a heartbeat, a wait cap of sixty seconds after which the request is
refused at the `health` layer. Per-person daily token caps exist and default to
unlimited (19 Q7). Kvasir cannot stop a runaway agent loop; the station's budget
does, and Kvasir makes the loop visible.

### 8.8 Identity

The engine's three modes with the same flags, the same ladder and the same trust
list, verified against the shared test vectors of `contracts/suite/v1`. A caller
with no mapped role is refused everywhere. In `token` and `off` modes the operator
mints keys with named principals; there is no self-serve. The ledger keys on the
subject; a display name is never a key. Revocation: a minted key dies with its row;
a token dies at its expiry, at most fifteen minutes; a stored credential of a
person disabled at the provider is removed by a nightly reconciliation.

### 8.9 Registration with pi

Twelve lines in the assistant: `createProvider` with `api: piMessagesApi()`, a
catalog fetched from `GET /v1/config` and cached through pi's model store, and an
auth resolver that puts the **person's token** on every request, so a stream that
arrives with no principal is refused rather than billed to the service. The
catalog is deployment-wide and principal-invariant; every per-person fact lives in
the grant. The client checks two things the catalog says: that every base URL is
inside the deployment's own origin, and that `contextWindow` and `maxTokens` are
not larger than the runtime reported. Any tool that produces an ask document uses
strict schemas with `require`, never `prefer`, which degrades silently.

### 8.10 The benchmark and the thresholds (C0)

Run on the production host before any Kvasir code, against the runtime directly;
Kvasir's own overhead is measured again in C4. Discard the first request of every
run. Record the runtime and its version, the build, the GPU shape and every flag.

Shapes: 8 concurrent streams; the comparable load (4,096 tokens in, 512 out) and
the real load (24,000 in, 256 out); temperature 0; `ignore_eos` on. Runs: plain,
plain with speculative decoding, schema-constrained (the generated ask schema
attached), schema with speculative decoding; a single-stream speculative control;
eight interleaved conversations with different long prefixes against eight
repetitions of one prefix, to measure the prompt cache. Every completion of the
schema runs is piped through `nils ask validate --strict`, which is the only
oracle, and the negative control of §8.6 is run.

Thresholds, written here before the run: aggregate output at least 150 tokens per
second at eight streams; at least 15 tokens per second per stream at eight streams;
p95 time to first token under 2 seconds at eight streams on the comparable load and
under 8 seconds on the real load with a cold prefix; schema pass fraction above
0.98 with the negative control failing to produce a malformed clause; tool-call
validity above 0.95 on the admission fixtures; Kvasir overhead under 50 ms at p50
and 200 ms at p95 of time to first token. What a result would change: if the card
cannot hold eight streams at the real load, the deployment guide says so and the
budgets of §12 shrink; if the prompt cache does not survive interleaving, Kvasir
gains conversation affinity; if no local candidate passes admission on tool calls,
the local backend serves background purposes only and the `rows` purposes need a
commercial model, which R9 permits an admin to choose, with the acknowledgement.

## 9. nils-assistant

### 9.1 The runtime

Flue, pinned exact, for one reason: a submission the runtime owes exactly one
terminal outcome through crashes, with a recovery order that never discards
finished work, never resurrects an abort and never re-executes an unresolved
ordinary tool call. Taken from Flue with it: the append-only typed record log with
atomic batches, `durable: true` tools with step memos, skills as progressive
disclosure through one activation tool, and the start and finish hooks as durable
check seams. Not taken: its default control flow, its compaction template, its
telemetry defaults, its channel packages (each an outbound path for study content),
and its local sandbox. Underneath, pi-ai and pi-agent-core at the version Flue
pins, the same pin as Kvasir (19 Q19), whose loop has no turn cap, cost cap or wall
clock anywhere, so every bound is a station's budget.

One process owns one conversation; the deployment runs one assistant process and
the desk is the only thing that addresses it. The persisted store format is
reset-only with no migration, so the record export and import exist before the
first conversation, with a round-trip test.

### 9.2 The host

One Node process running one Flue application, holding no credential of its own
(§5.5), exposing an HTTP surface only to the desk (`GET /capabilities`,
conversations with Flue's history and updates views, `POST /stations/{id}/runs` and
`GET /runs/{id}` for a headless station run, `POST /conversations/{id}/feedback`
for accepted and rejected proposals, teaching sessions), and owning one database
(Postgres in a group deployment, a SQLite file standalone) through Flue's SQL
store contract, validated with its contract test kit. It never opens the registry;
a live check from its container asserts the registry database refuses it (19 Q9).
Telemetry content is off by the exporter's own configuration.

### 9.3 The seam

One module is the only place in the service that speaks to the engine. It holds
the person's token for the turn, builds a typed client from `openapi` v3, applies
the calling station's grant (an operation allowlist plus argument caps: the
requested level, the row budget, the disclosure level, the selection scope) before
it dials, sets the ceiling and the actor headers, derives the idempotency key from
the tool call id, marks GPU-bound tools sequential, and writes every call and its
outcome to its ledger: station, phase, operation, argument digest, grant decision,
outcome, handle. Nothing else in the service may import an HTTP client. A blocked
call returns a reason the model reads and adapts to, not a crash. Adding a tool
without touching the seam leaves it granted, capped and audited, which is the
falsifiable form of the rule.

### 9.4 The station

One decision point, one manifest, fixed in `contracts/suite/v1`:

| Field | What it is |
|---|---|
| `id`, `app` | `ask-help`, `nils-assistant` |
| `purpose`, `content` | the Kvasir purpose it runs under and the class it declares |
| `ceiling` | the role ceiling the seam sets |
| `grant` | the door operations it may call and the caps per operation |
| `brief` | the path and content hash of its skill file |
| `result` | the JSON schema of the verdict it must produce |
| `checks` | named assertions the verdict must pass before a run may settle |
| `budget` | turns, tool calls, wall clock, input tokens |
| `writes` | the closed set of proposal kinds it may emit |
| `phases` | the named phases and their transitions |
| `evals` | the fixture directory that gates any change to any of the above |

A station is a guarded phase machine: a durable phase value, tools mounted per
phase and gated with a check that returns refusal text, one terminal reason per
run (`settled`, `budget_turns`, `budget_tools`, `budget_wall_clock`,
`budget_tokens`, `token_unavailable`, `provider_error`, `provider_refused`,
`loop_stopped`, `aborted`), and a finish hook that refuses to settle until the
checks pass and sends the model back with a specific complaint. Every mutating
tool schema carries a completeness field (an item count) a truncated argument
stream cannot satisfy. The loop-detection rules of v0 (order-independent hash,
per-tool salient keys, warn at three, stop at five) are ported with the counter in
run state and the interventions as typed events. Manifests are loaded from
directories the configuration lists, so an app contributes stations without a
code change (§4.4).

### 9.5 The brief

Model-facing content for one station, a Flue skill file in the assistant
repository: what the decision point is, what the words mean, two or three worked
examples, the refusal sentences. It costs one catalog line in the prompt and its
body arrives only when the model asks. The grounding it builds on comes from the
engine's guide door (§6.4), fetched live, so a rule rewritten in the pack is a pack
release and a station procedure rewritten is an assistant release. A brief may not
contain an exception clause naming another station, which a grep proves. The
system prompt is split into a memoized static half and a dynamic half with a marked
boundary; the catalog digest, the engine's capabilities and the station roster
arrive as delta attachments.

### 9.6 The verdict and the proposal

Every run ends in one typed object produced by a single `finish` tool whose input
schema is the station's result schema, with strict sampling required; a validation
failure is handed back as a tool error and the model repairs it. A verdict carries
the terminal reason, the brief hash, the model, the budget consumed, the evidence
it read as handles and ids (never pasted rows), and `proposals[]`. A proposal is one
of a closed set and maps onto a write the engine already governs:

| Kind | Lands as | Applied by |
|---|---|---|
| `document_version` | a scratch document handle and a diff in the desk | a person accepting |
| `review_decision` | a review item decision with the agent as author | a person, or a per-kind policy that stays off |
| `overlay` | a proposed overlay object with its `try` result (§6.6) | an operator adopting |
| `identity_rule` | a candidate rule with the probe result that argued for it | an operator editing the batch and digesting |
| `note` | a memory note awaiting acceptance | a person |

A **document apply** (a new immutable version through `POST /api/ask/apply`) is
permitted. A **decision apply** (`POST /api/review/{id}/apply`, adopt, promote,
release, reveal, rebuild) is forbidden to every station, and a grep of the grants
proves it.

### 9.7 The ledger and the redactor

Flue's record log is the transcript and the audit trail; compaction changes the
projection and never the log, so what a reviewer reads is byte-identical to what
the tools returned. The seam's call log sits beside it. The redactor is a
**context rule and a store rule**: the seam refuses any document that projects
identifiers, any handle whose provenance records classes above the station's
declared content class, and any linkage path; every store write passes one
redactor; a provider error reaches the store as a classified code only; and the
test seeds markers in tool results and in user text and asserts they reach neither
a store nor a remote provider call. Under R9, a local backend may see what the
person may see; the redactor's job is the class boundary and the egress boundary,
not a blanket.

### 9.8 The desk seam

A closed union of typed parts, decided in one module: `move_proposal` (a set, an
options token, move ids, one sentence), `choice` (typed options with the count each
would produce), `note`, `todo`, `lookup` (what it read from the catalog), `handle_ref`,
`funnel`, `status`. Anything else is dropped. The assistant never calls a desk
function and never navigates the desk; it emits parts.

### 9.9 Memory

Within a thread, the record log is the memory and the working object is a handle;
the station state carries the document handle, the proposed patch, the open choices
and the phase, so a compaction cannot lose them. Compaction uses our template, not
Flue's: the question or knob under discussion, the catalog slice or rule set in
play, the declaration block in force, decisions the person made or corrected,
errors and their fixes, every user message, open handles and pending jobs, one next
step quoted verbatim.

Across threads, per person: typed notes (`person`, `correction`, `study`,
`reference`), one row each, a one-line index, a small-model selector over the index
capped at five, a staleness caveat on anything older than a day, written by a
background fork at turn end with a two-turn budget and tools restricted to the note
store, keyed on the subject. Across people: only corrections, only structural
(station, decision axis, the check that would now catch it, with every domain name
held as a reference the engine resolves per reader), only after a named person
accepts one in the desk, filtered through the reading person's own catalog before
it enters a prompt.

### 9.10 The bench (D1)

Built before the first station, in `bench/` of the assistant repository, and only
one of its five parts needs a model.

1. **The corpus, scrubbed and rebased.** On the production host, by a script kept
   with the private record: the 26 distinct question shapes and the three refinement
   chains paraphrased turn by turn, the 28 gold answers re-derived against the pinned
   synthetic registry the gate already uses, and a record of which tasks could not be
   rebased. The transformation is written down under the corpus review rule of C10
   before the first public commit, and no subject code, cohort name, identifier, UID,
   path or credential from the source survives. The published baseline (17.9 percent)
   is re-run on the rebased corpus and the new number is published beside it.
2. **The taxonomy, closed.** An enum with a counted `other`: the ten failure words
   v0's evaluator used plus infrastructure failure, cohort name misresolution, default
   exclusion omission and identifier fan-out; a second axis of six values for the
   silent decisions.
3. **The gate, deterministic.** Fixtures replayed through a recorded provider, so
   the station test suite passes with no network at all, and a fixture asserts a
   recovery path fired by reading the transition, never the message text.
4. **The attribution order, once.** Domain knowledge, then tool description, then
   prompt, then control flow, then tool behaviour, then configuration, then
   delegation, stored as one table that generates both the code path and the prompt.
5. **The discipline.** Every edit to a prompt, a brief, a tool description or a check
   is a change manifest with a prediction, judged by the nine-state transition matrix
   (keep, partial, revert, inconclusive); a held-out split exists before the first
   loop and both numbers are reported on one screen; the gold set records the registry
   epoch and the pack version beside every hash and goes stale rather than red; no
   judge on the gate; a person merges every harness edit.

### 9.11 `ask-help` (D4)

The decision point: words to a document, or one step of a document tuned. Purpose
`assistant.ask-help`, content `rows`, ceiling `reviewer`. Phases `resolve` (every
proper noun resolved against the catalog before any word is read as a description;
an ambiguous resolution becomes a `choice` with counts), `shape` (the smallest
document that could be right, through `validate` in repair mode, then `describe`;
the six silent decisions named or the phase does not advance), `refine` (moves over
rewrites: `options`, `apply` by id, `options` again; `draft` only when no move
exists), `check` (`diagnose` before anything changes, then `preview`), `finish`.
Grant: `catalog`, `catalog/{level}`, the value sampler, `validate`, `describe`,
`options`, `apply`, `diagnose`, `preview`, `draft`, `diff`, `handles/{id}` and
`/rows`, `guide`. Writes: `document_version` only. Budget: 12 turns, 40 tool calls,
180 seconds, 250k input tokens. Checks before settle: `validate` clean on the exact
hash in the verdict; the declaration block fully populated; no SQL string anywhere;
no truncated handle cited; no identifier value anywhere.

Three surfaces call it: the desk's helper beside the question page (with the
current document as the base), the chat pane through the concierge, and the command
line, `nils assist ask "words" [--base ID] --assistant URL`, which prints the
describe sentence, the diff against the base and the handle. Its evals are the 26
shapes and the three chains, scored on whether the chain reached the same final
selection and how many correction turns it needed against the 22 the researcher
gave.

### 9.12 The concierge (D5)

A fourth agent, deliberately weak. Its grant: `describe`, `handles/{id}`,
`/rows`, `capabilities`, jobs (read), and `delegate(station, brief)`. No `apply`,
no `validate`, no `run`, no catalog paging, no shell, no filesystem. It holds the
conversation, so `steer` and `followUp` land on it, one at a time. It owns the
clarification, a typed `choice` with counts, fired only at a pick, a scope or a
grain, and never about something the person already said. Delegation follows
Flue's shape: a subagent per station, a static task tool, text plus a task id back;
the station writes its verdict to the store and the concierge renders it from
there. Its prompt carries the delegation prohibitions: do not read a running job's
output mid-flight; never fabricate or predict a delegate's result; give status, not
a guess.

### 9.13 `keyword-tune` (D6)

One axis of one pack tuned against the classifier's own signals. Purpose
`assistant.keyword-tune`, content `rows`, ceiling `reviewer`. Phases `survey`
(`GET /api/classify/signals`, the open review items by group, a sample of the text
the axis matched against), `hypothesise` (one change to one rule set or one
threshold, plus a falsifiable prediction: which groups flip, which must not
regress), `rehearse` (`POST /api/classify/try`, writing nothing), `check` (the
nine-state diff between rehearsal and baseline: keep, partial, revert), `finish`.
Writes: `overlay`, and `review_decision` only for groups the rehearsal confirms,
never for a single stack. Checks: the patch touches one file; the prediction was
written before the rehearsal; the rehearsal wrote nothing, asserted from the job
row; every named group exists. The desk's keyword tab sits beside the review queue
and shows the signals, the proposal, its `try` result and the adopt button, which
only an operator sees.

### 9.14 `identity-check` (D7)

Is the identity rule right for this batch, and should identity come from the path
instead of the tag. Purpose `assistant.identity-check`, content `rows`, ceiling
`operator` for the probe door only, which the seam allows this station alone.
Phases `read`, `diagnose` (`POST /api/ingest/probe` with the current rule and the
candidate side by side, over a pre-registered location), `propose`, `check`,
`finish`. Writes: `identity_rule` and `review_decision`. Checks: the proposed rule
passes the engine's own validator, called; the verdict names the rule it applied
and what it saw; no identifier value anywhere, shapes only; if the rule uses a path
source, `path_is_direct_identifier` is answered explicitly, because a folder that
is a personal number makes the path directly identifying. A re-digest under a
changed rule is a person's act. The desk's anonymisation page shows the probe's two
histograms, the counts under each rule and the proposal.

### 9.15 The command line

`nils assist ask` (§9.11), `nils login` (§5.8), `nils ask draft` (§6.4) and
`nils restore` (§6.5) are the engine repository's part of this wave; each lands in
the slice that gives it a door.

### 9.16 What must be true

Six properties, each a test. Typed effects only: no station's grant contains a
decision-apply verb, and no engine write exists outside the seam. Handles, never
pasted rows: presenting a result twice never executes the query twice. A truncated
result is not a result: a station that cites one in a proposal fails its own check.
Failure is a typed outcome, never a sentence: from a run record alone a script
answers "did this run hit a provider error". The record is inviolate: any
conversation rebuilds from the record table alone, and every fold checkpoint can be
deleted without loss. The class boundary holds: seeded markers in tool results and
in user text reach neither a store nor a remote call.

## 10. Custody and retention (C45)

| Store | Part | Holds | Kept | Command |
|---|---|---|---|---|
| desk store | nils-desk | sessions, tokens, display names, preferences, local users | sessions until logout or expiry; the rest until deleted | `nils-desk store ...` |
| conversations and verdicts | nils-assistant | transcripts, tool results, verdicts, proposals | transcripts 90 days by default (C24); verdicts scrubbed of free text, indefinitely as the evals dataset (C25) | `nils-assistant retention ...`, `export`, `delete --subject` |
| notes | nils-assistant | per-person and institutional notes | until deleted | `nils-assistant notes ...` |
| teaching captures | nils-assistant | redacted trajectories of named sessions | until deleted | `nils-assistant teaching ...` |
| ledger | Kvasir | counts per stream, no content | thirteen months at row grain, then aggregates | `kvasir ledger ...` |
| credentials | Kvasir | minted key hashes, encrypted organisation and brought keys, OAuth grants | until revoked; reconciled nightly against the provider | `kvasir keys ...`, `credentials ...` |
| admission records | Kvasir | suite results per model, runtime and build | until the runtime changes | `kvasir admission ...` |
| idempotency keys | the engine | key, principal, response digest | 24 hours | swept with handles |
| read audit | the engine | who paged or exported which handle, with actor, ceiling and purpose | with the audit log | `nils audit` |

Deletion on request reaches the transcripts, the verdicts and the eval rows.
`nils custody` and the desk's custody page list every row above; nothing is
retained that the page does not list.

## 11. The gate of the wave and its closing bars

Each repository runs its own gate in CI on every pull request: the engine's on
both backends (`gate/`, twenty-one fixtures plus the eight of §6.8); the desk's
(the acceptance tests of §7.9, headless); Kvasir's (the contract tests, the ledger
schema test, the trust-list vectors, the admission suite against a recorded
runtime); the assistant's (the bench's deterministic gate on a recorded provider,
the redaction test, the grant grep, the record round trip).

The wave closes when:

1. The eight new engine fixtures are green on both backends and the two defects of
   §2 are gone from `main`.
2. In each of the three identity modes a person builds a question by hand and the
   desk shows every version, its author, what changed and what it would run (§7.9).
3. Kvasir's local model passed admission; the benchmark met its thresholds, or the
   deployment guide restates the floor in writing; the ledger has no content column.
4. The bench's baseline is re-run on the rebased corpus; `ask-help` beats it, and
   needs fewer than 22 correction turns on the three chains.
5. A fresh provider deployment is three commands (§5.7), and a `local` deployment
   with two users works on a laptop.
6. The custody page lists every store of §10.
7. No station holds a decision-apply verb, proved by a grep in CI.
8. `keyword-tune` proposes an overlay whose `try` result matches what adopt then
   moves; `identity-check` reports two candidate rules on a synthetic tree whose
   placeholder tag it names by shape.

## 12. Defaults settled in this spec

| Default | Value |
|---|---|
| access token lifetime | 15 minutes; refresh 30 days |
| desk session | 12 hours idle, deleted at logout |
| command line token in `local` mode | 1 day |
| entitlements | `reader`, `reviewer`, `operator`, `admin`, `assist` |
| ceiling of `ask-help`, `keyword-tune` | `reviewer`; `identity-check` `operator` for the probe only |
| idempotency window | 24 hours |
| event streams | half the workers, at least one |
| station budget | 12 turns, 40 tool calls, 180 s, 250k input tokens |
| Kvasir concurrency | 8 streams per local backend, queue 16, wait cap 60 s |
| per-person daily tokens | unlimited |
| ledger retention | 13 months at row grain |
| transcripts | 90 days (C24) |
| model library pin | the version Flue pins, one owner, one bump review |
| local profile | SGLang, BF16 weights, FP8 KV cache, llguidance, speculative decoding, 8 running requests |
| benchmark thresholds | §8.10 |
| minimum cell size | none (D51) |
| auto-commit | off for every station and kind |

## 13. Order of work

**Closing (2026-09-09).** Against the eight bars of §11: (1) the eight engine
fixtures are green on both backends in CI and the two defects of §2 left `main`
with A1. (2) The three identity modes carry a hand-built question through its
versions, authors, diffs and compiled SQL, headless in the desk's tests and
live on the rig (B2, B3). (3) The local model passed admission on the
production runtime, the serving benchmark met its thresholds with one measured
tunnel caveat, and the ledger has no content column, asserted by a test (C0,
C2, C4). (4) The baseline re-ran on the rebased corpus at one shape in eighteen
on both models; `ask-help` on the local model reaches fourteen of eighteen the
same answer and ten by the strict hash after the rework for the local model,
and needed two correction turns on the three chains where the researcher gave fourteen, with every follow-up turn leaving a document; the composition question returned one number five times with its scheme digest. (5) A provider deployment is the three commands of §5.7 and a
`local` deployment with two users ran on a laptop (B2). (6) The custody page
lists every store of §10, the assistant's included (B5, D2). (7) No station
holds a decision-apply verb: the seam refuses the forbidden operations in code
and the assistant's gate greps every manifest in CI (D2, D3). (8)
`keyword-tune` proposed an overlay whose `try` named seventeen stacks and
adoption moved seventeen; `identity-check` named the placeholder tag by its
shape against the path segment (D6, D7). Left open, named where they arose: the
signals door carries no text sample (kineuro/nils#94), the nightly credential
reconciliation of §10 and a live OAuth grant (C5), the small-model selector and
the compaction template of §9.9 (D5), and the three held-out shapes the local
model still misses, one of them unanswerable as asked.

**As built (C5, brought keys and the OAuth slot, 2026-09-09).** In
`kineuro/kvasir`: a person's own provider key is sealed with the same
XChaCha20-Poly1305 seal key as the organisation's, with the person's immutable
subject bound in as associated data, so a row moved to another subject's name
does not open; a machine or a minted key holds none. The OAuth slot is a public
client with PKCE (S256), a state bound to the subject and the desk's tab,
expiring in ten minutes and used once, and one fixed redirect on Kvasir's own
origin, `/v1/personal/oauth/callback`, the only door served before
authentication, since the browser arrives from the provider with no bearer and
the state is its identity; the code is exchanged with the verifier, access and
refresh are stored sealed under the subject, the refresh rotates on every use
and is serialised per person and provider through a lock, and revoke deletes
both. A stream carries the person's own credential for a provider before the
organisation's. The wave's two providers are the defaults of `oauth` in the
configuration: the personal source is offered for the one whose terms permit
it and absent by policy for the one whose terms as of 20 February 2026 forbid
it, with the sentence and the date carried by `GET /v1/personal` and shown by
the desk's settings section, where a person brings a key, connects or
disconnects a subscription. The bars are tests: two people connect in the same
minute with their own states and sessions and neither sees the other's grant;
a dump holds no readable brought key and the minted keys stay hashes; the
forbidden provider shows the sentence and the date; one refresh serves two
concurrent uses. Not done: the nightly reconciliation against the provider of
§10, and a live grant against a real provider, since no account of the
permitted provider was at hand on the rig.

**As built (D6 and D7, `keyword-tune` and `identity-check`, 2026-09-09).** Two
stations on the framework of D3, checked live on a synthetic knob rig: a tree
of 209 files made by the engine's own DICOM synthesis helpers (five subjects,
a placeholder patient id, subject codes as the first directory, a site's own
contrast word the pack does not know), digested and classified into its own
registry, served with an ingest root and a worker. `keyword-tune` surveys the
signals and the open review items, writes one hypothesis for one bucket of one
axis with a falsifiable prediction (the groups that flip, the values that must
not regress) before anything is rehearsed, rehearses it as an overlay through
`try`, reads the diff as keep, partial or revert, and proposes the overlay for
an operator to adopt; its checks read the run state and the seam's ledger, not
the model's word: one bucket, the prediction before the first rehearsal, no
write before the last rehearsal, every named group one the signals showed.
Closing bar 8, first half: the rehearsal named seventeen stacks moving on
`post_contrast`, adoption queued the reclassify, and the axis's keyword tier
went from ten to twenty-seven. Two things the live run taught: a scope's colon
must stay a colon on the wire, and the signals door carries no sample of the
text an axis matched against (kineuro/nils#94), so the term comes from the
person's words, the shadowed keywords or the overridden terms; without the word
in the brief the station hypothesised a term the bucket already held and spent
its budget. `identity-check` reads the registered locations, probes the
current rule and a candidate side by side through the engine's own door, which
validates every rule and answers shapes only, proposes one of the rules the
probe took with the path question answered explicitly, and settles naming the
source the rule reads and the shape it saw; its check tells a code from its
shape, so a value never passes. The rule grammar sits in the tool's own words
and is checked before the grant is spent, after the first run burnt its three
probes on rules the engine refused. Closing bar 8, second half: it named the
placeholder tag by its shape (`AAAA`, constant across the sample, one subject)
against the path segment (`AAA999`, three subjects) and proposed the path rule
with `path_is_direct_identifier` false. The desk's keyword tab sits beside the
review queue and the anonymisation page in the data section, each starting a
headless run through the proxy and rendering the verdict from the assistant's
store; the desk composes no overlay and no rule, and the re-digest stays a
person's act.

**As built (D5, the concierge, clarification and memory, 2026-09-09).** The
concierge holds exactly the grant of §9.12 (describe, a handle, one page of its
rows, the capabilities, the jobs) plus `delegate`, and writes nothing.
Delegation follows Flue's shape without Flue's subagent, because a station
keeps durable state and a Flue delegate may not: `delegate(station, brief)`
dispatches the station as its own agent instance, a child conversation under
the parent's with the parent's token, and answers a task id at once; the
concierge settles in the same turn with a status sentence; when the delegate
settles the host wakes the concierge with a signal, a follow-up turn starts and
it renders the verdict from the store, so the document it settles with is one
a delegate found, which its own check enforces. Checked live through the desk:
delegated, settled at once, woken, settled again with what the delegate found.
Two things the live run taught: the wake goes through the scanned agent
function the build registered, never the inner render; and a concierge that
polls its delegate spends its budget waiting, so the brief says settle first
and the prompt forbids the poll. A brief carrying SQL is refused: it is the
person's words. Clarification is one typed choice, at a pick, a scope or a
grain and nowhere else, said by the result schema (an enum) and by a check.
Memory across threads: typed notes (person, correction, study, reference) per
subject, one row each, the newest five in a prompt with a staleness caveat past
a day, a study note written mechanically at settle and a correction note from
every rejected proposal, deletion by subject through the host and the command
line; across people only structural corrections (station, decision axis, the
check that would now catch it) accepted by a named person, whose row has no
field a value fits. Not done from §9.9: the small-model selector over the index
(recency stands in), the background fork that writes notes with a model, the
compaction template, and the reader-side catalog filter, since a structural
row carries nothing to filter.

**As built (D4 and B7, `ask-help` and the assistant pane, 2026-09-09).** The
station walks resolve, shape, refine, check and finish over the engine's ask
doors through the seam, one tool per operation the grant names, opened by
phase; the five checks run before settle and the result's hash and declaration
are filled mechanically from validate and describe. The desk seam is one named
data part per kind of the closed union (status on every advance; move_proposal,
choice, handle_ref and status at settle), so a conversation's history keeps the
last of each and the live stream sees every write. The pane: question left,
chat right, stacked below 700 px, sharing the document id and the epoch; one
reducer under 300 lines admits the union and decides what each part may touch,
dropping the rest; a proposal is a unified diff of the two canonical texts
under the assistant's sentence, accept moves the pointer and records the
lineage, reject sends it back as feedback, which the next turn's prompt names
so a rejected proposal is not repeated; a choice renders its options with
counts; the pane says warming when Kvasir does. A follow-up turn re-enters the
station on the settled document, or on the last document stored when the run
ended by budget, the budget starting over. The desk pushes the person's bearer
per conversation since a browser holds none; in off mode with an engine whose
authentication is off, the seam takes the turn without a token and refuses it
against any other engine. `nils assist ask` in the engine repository prints
the describe sentence, the diff against the base and the handle.

The first numbers sent the station back to the drawing board: the commercial
model reached one shape in eighteen and the local one none, both spending
their hundred and eighty seconds on ceremony (the guide, the catalog level by
level, the sampler, `advance` between five phases, settle refused on a schema
slip), and Nima's ruling was that the local model must run the shapes with no
problem at all, the fix pragmatic, in what the station carries, never in a
smarter model (a commercial model is a test of the two provider shapes, nothing
more). Four things changed. The prelude: the engine's catalog and guide
rendered compactly (grains and how a set draws from another, the fields per
level, the axes and their values, the kinds, the cohorts, the schemes, the
derived fields, the functions, the grounding), fetched through the seam under
the person's token once per person and appended as a signal that joins the
first response, about four thousand tokens read once instead of paged for.
The cookbook: worked examples beside the brief, the eleven golds of the
bench's loop split with their questions, in the YAML the draft door takes,
plus three written for the cookbook and marked so (the most common value, the
per-cohort statistics of a count per subject, one stack per session by rules
given one at a time); the seven held-out golds are not in it, which a test
asserts by content hash. Implicit phase moves: a tool called from the phase
before the one it opens in moves the run there, recorded like an explicit
advance, and ask-help mounts no advance tool at all. A brief for a small model:
five steps, the idioms of the language in words, the parent-key rule, and no
rereading of what the prelude carries. Around it: a default temperature per
Kvasir backend (0.2 on the card), a 64k context on the runtime (the 32k of the
serving benchmark overflowed on the longest runs), a budget of 300 seconds
and sixteen turns in place of §9.11's 180 and twelve, and the grant's caps
widened so a refusal costs a draft, not the run. Three engine gaps surfaced
and closed on the way: the repair pass gave every clause under `group.by`
and a relation's `by` a direction as if they were order terms, so the draft
door refused every grouped document (#95, and the same slip for `same` in
#96); strict validate stopped at `prepare` while run compiled, so a document
a door accepted could be refused at run time (#93, closed by #96: validate,
the documents door and the draft door now compile, answering
`not_compilable` with the path). The bench gained the selection measure
beside the hash, the same set of subject codes however the column is reached,
since the golds' column choices are one author's and a table with one column
more is not a wrong answer. The numbers: on the local 27B with thinking off at temperature 0.2, ten of eighteen
shapes by the strict hash and fourteen of eighteen the same answer (every one
of the eleven loop shapes, three of the seven held-out), a median of
forty-six seconds a shape, against none of eighteen and every run ending by
budget before the rework; the three held-out misses are a question the
language cannot answer as asked (a cohort's owner beside its subject count,
which the gold itself drops), a count the model listed instead of counting,
and a composition the gold rebased to a sequence the words do not name. On
MiniMax through the Anthropic shape the same station reached seven of
eighteen with most misses by budget, the test of the provider shape it is
meant to be. The chains, scored as the bar says, the opening of each chain against the
gold's answer with the researcher's corrections held back and sent one at a
time until it is reached, then the additions as follow-up turns of the same
conversation: chain A reached the gold's answer at the second correction
where the researcher gave five, chains B and C at the opening with none where
the researcher gave five and four, two corrections needed where the
researcher gave fourteen on these chains and twenty-two across the traffic;
every follow-up turn of the three chains left a document (eight, seven and
one). The composition question five times: five of five reach the gold's answer, one number, one scheme digest, about
half a minute each, once the brief said that a total of sessions and subjects
is the count level and not a group, and once the value sampler answered an
axis from the registry instead of refusing it five times over.

**As built (D1 to D3, the bench, the host and the station framework,
2026-09-09).** In `kineuro/nils-assistant`. D1, the bench: the twenty-five
shapes and the three chains of the traffic paraphrased into the corpus, each
shape with its silent decisions and its earned tags from the taxonomy, an
attribution per shape to the wave that owns the fix, a replay of the engine's
gate fixtures through the pi-messages wire shape, the manifest matrix and the
loop and held-out split by a hash of the shape's id (one in three held out),
eighteen golds recorded as unnamed handles at the registry's epoch and pack
version with their content hashes, and a one-shot baseline (the rules, two
worked examples and a catalog slice in one prompt) that reached one shape in
eighteen on both the commercial model and the local one, the number the
station had to beat. D2, the host: one Node process on Flue 2.0.3 and pi-ai
0.83.0, pinned exactly, with a Hono surface for the desk only; no credential
of its own, the person's token handed per turn and pushed before expiry, Kvasir
reached with the app's minted key through one provider per station carrying
its purpose; the seam (the grant keeper counting calls and rows per run, the
headers the engine reads, the redactor over what a station may say, the
ledger of every dial with its outcome and never its content, the client over
the engine's generated types); headless runs, verdicts beside them, and the
record store exported and imported at table level, because Flue's append
refuses a record it did not admit. D3, the framework: a station is a manifest
validated against the vendored station schema with its brief's hash checked at
start, a guarded phase machine with the budget counted in run state and a loop
detector over the salient arguments of each call, a verdict whose proposals
are the only effect, checks named in the manifest and run before settle, the
result schema turned into a Valibot schema, and one agent factory around it.
What Flue taught on the way: `finish` is a reserved tool name, so the station
ends with `settle`; one runtime per process; an agent is a `'use agent'`
module with a capitalised export or an `agentName` static; a `tool-output`
chunk carries the call's id, not its name; a read attached late misses the
chunks before it, so the tests read the stream store; the producer sequence
starts at zero per epoch.

**As built (C4, the admission suite, 2026-09-09).** In `kineuro/kvasir`: the
five checks of §8.6 as one suite against a backend through its own adapter,
mechanical throughout. Tool calls: twenty fixtures over our own schemas,
vendored from the engine's MCP door, each a prompt and the call that answers
it, validity above 0.95 to pass. The chat template: a system message in the
position our clients send it. The enforced schema: a tool whose parameters
carry `minItems: 2`, the call forced with `tool_choice` so a model cannot
dodge the control by answering in prose, the positive request first, then the
negative control asking for a one-element clause; enforced or refused passes,
accepted and ignored fails, and a failed positive call leaves the verdict
inconclusive. Overflow: a prompt a quarter past the context window, whose
answer (`error`, `truncated` or `accepted`) is published as the catalog flag.
Stream integrity: a signed thinking block sent back byte for byte in a second
turn, not applicable to a shape that carries none (pi-ai marks the reasoning
field it read where a signature would sit, and that marker is not one). The
records sit in the one database keyed by backend, model, runtime and build
(`kvasir admission run|list`, `GET /v1/admission`, `POST /v1/admission/run`
for an admin) and the gate of §8.6 is on by default: a local model is listed
by `GET /v1/config` only once its newest record for the runtime the backend
reports now has passed, so an upgrade drops it until the suite runs again.
Kvasir's own overhead is measured in the same process: the same prompt and
client straight at the backend and through the door, eight streams with
distinct four-thousand-token prefixes, every prefix warmed once and the order
of the two paths alternating by round so the runtime's prompt cache favours
neither, four rounds, the difference of the times to first token at p50 and
p95 against the thresholds of §8.10. Run live against the C0 runtime on the
production host (the 27B model, xgrammar, plain decoding) three times, the
first two teaching the suite: pi-ai's OpenAI adapter sends the system prompt
as the `developer` role, which that runtime refuses, so a backend now carries
pi-ai's compatibility flags with local defaults set by the operator and never
by a client; a fixture's arguments are matched as a subset; the chat-template
check gives the model's thinking room; and the overhead measurement had let
the cache decide. The third run admits the model: nineteen of twenty calls
well formed (the miss called the guide instead of the catalog), the template
accepted, `minItems: 2` enforced under xgrammar with the forced call, the
oversized prompt refused, and Kvasir's overhead under the noise, thirty
milliseconds below the direct path at p50 and fourteen above at p95 over
thirty-two streams each way. The admission record names runtime and build.
Two things left open: thinking stayed on during the tool calls, and a run
with it off would be another admission; and both paths crossed one tunnel,
so the absolute times are not the host's own.

**As built (B3 to B6, the desk's pages, 2026-09-09).** In `kineuro/nils-desk`,
four slices, each a page of §7 as a function of the engine's doors and the
person's entitlements, checked live against a local engine. B3, the question
page: the editor is derived from the stored ask document on every render, a
step per set in the order the engine reads them (a source before what reads
it, the children of `has` before the parent), sub-steps for the clause groups
with `active`, `visible`, `valid` and `revert`; every control is a move from
`POST /api/ask/options` rendered from its template and holes (fillers as a
select, a value with the sampler's datalist), every edit goes through `POST
/api/ask/apply` against the options token, and `409 stale_options` refetches
the options and says so. The desk composes no ask JSON: a question starts
from a stored id, from authored text through `POST /api/ask/draft`, or from a
worked example of `GET /api/ask/guide`. The declaration block from
`describe`, the compiled SQL panel from `explain` in both dialects, the
preview that goes stale on purpose with a Refresh button, the per-step
funnel from `diagnose` (the preview door runs the document's `out`, so the
"capped run of the prefix" is the funnel's rows and subjects per stage
rather than ten rows per step), the diagnose drawer opened when the answer is
empty or smaller than the version before, the version chain from the
document parents with an author on every line and what changed from `diff`,
the diff view over any two versions, and compare as a command over two
documents or two handles. The projection preset of the `out` step is a
display projection over the answer's columns, because no move sets
`out.columns` (an engine move for it is the one thing this page still
wants). A cell click offers only the moves whose template has a hole the
value can fill; a chip click drops its clause through `remove_where`; both
make the same audit shape. Presenting a result twice executes once, keyed by
document and epoch. Checked through the proxy: apply then revert answers the
root document again, byte-identical canonical forms and an empty diff; a
reused token is `409`; a no-op diff is empty; a cross-origin write is `403`.
B4, the result surface: the engine gained `GET /api/ask/handles` (this
caller's handles newest first within its scope, with `kept`, `truncated`,
the person's own `limit` and the columns); the page pages a handle from the
rows door and never re-executes; running is the ask run jobs not over,
polled by thirty seconds while any runs, the two-minute tail named; stale is
a named overlay when the desk's own lineage record (which document followed
which, ids only, written after every apply) says the document moved on, or
when the registry's epoch moved; truncated tells your limit apart from our
truncation, the row count is the control that points at the `out` step, and
release and promote are disabled with the reason on the control; export is
CSV off a handle through `GET /desk/export/{handle}?purpose=`, which pages
the engine's rows under the caller's own identity so every page carries the
read audit and the purpose the person typed, and `export` in the desk's
configuration names the entitlement it needs or `off`; promote is a job. No
charts. B5, operations and data: jobs live from `GET /api/events` under the
cap with polling as the fallback; review with the item, its members and
evidence, apply, accept, commit and withdraw; releases with the history and a
form that shows exactly what will be released (the handle's first page and
every stack id read off its rows, the question's sentences and who authored
each version, or the cohorts, subjects and axes named by hand) and requires
the selection's name typed before `POST /api/releases`; handovers; custody,
the registry's stores beside `GET /desk/custody` (the desk's own store and
the parts that keep theirs, C45); audit with its filters; sessions rebuild as
a job. Data: packs with a pack's axes, rule sets, passes and buckets; batches
with their reports as shapes; quarantine by batch and class; the ingest
forms for digest, classify, fingerprint and linkage import, each queued over
a pre-registered location as `@root/relative` with an absolute path or a
parent step refused in the desk before the engine refuses it again (the job
door gained `linkage import` as the one linkage verb it queues); backup as a
job with its archives listed; verify as a job; restore as a page that prints
the exact command and the pre-restore procedure and runs nothing (D50). B6,
settings: one section per part; the engine read only with the policy table
and the flags to paste for the desk's identity mode; the desk's own settings
(`desk.settings` in the capabilities document: mode, origin, engine URL,
session and token lifetimes, export, store, retention) read only, users in
`local` mode; Kvasir's backends with their health in words, the models table
where an admin sets a backend, a `rows` purpose only with the acknowledgement
sentence and an `identifiers` purpose never (the option is disabled), the
minted keys with the secret shown once, the organisation's key stored sealed
and shown never; the assistant's own document when it answered; the app
registry. The shell reads the hash for its section (`#ask/12`,
`#results/77`, `#operations/releases/77`, `#data/ingest`).

**As built (C1 to C3, Kvasir: the door, identity, keys, admission, the
ledger, grants and policy, 2026-09-09).** `kineuro/kvasir`, TypeScript on
Node 22 with `@earendil-works/pi-ai` at the exact version Flue pins (0.83.0).
C1: `POST /v1/messages` takes `{model, context, options}` and answers the
pi-messages event stream, streamed through pi-ai's own adapters
(`openai-completions`, `anthropic-messages`, a fixed list in code) with the
runtime key attached by Kvasir; `POST /v1/chat/completions` and `GET
/v1/models` are the OpenAI shape for a script; `GET /v1/config` is the
catalog as pi's model store reads it (`baseUrl` and `models[]` with the
measured `contextWindow` and `maxTokens`) plus each backend's locality and
health; a local backend is warming until its first token since start and is
warmed at start; a pi client streams through with zero compatibility flags
and a thinking signature round trips byte for byte (tested against fakes).
C2: the engine's three modes, ladder and trust list (jose; JWKS by URL with
a cooldown or by file), verified against the suite's vectors vendored from
`contracts/suite/v1`; a caller with no mapped role and a stream with no
principal are refused; minted keys `kvs_` of thirty-two random bytes shown
once, stored as keyed BLAKE2b hashes under a pepper file made at first start,
compared in constant time, revoked by a deleted row, local only without a
purpose; per-backend admission with a queue of eight behind the admitted
streams (the spec's sixteen read as the total in flight: the ninth queues
with a heartbeat on the socket, the seventeenth is refused at the health
layer, as does a sixty-second wait); the ledger, one row per stream in
counts with no content column, asserted; `GET /metrics` with backend, model
and outcome as its only labels. C3: purposes registered in the configuration
and never by a caller; the policy table (`GET /v1/purposes`, `PUT
/v1/purposes/{id}/policy`) opening `catalog` to a remote backend by choice,
`rows` only with an acknowledgement recorded under the admin's name,
`identifiers` never; `POST /v1/grants` answering the model, backend, measured
limits and `chose_because`, or refusing with the layer and the one relaxation,
a pin recorded and never above the policy, a warming backend refused at the
health layer; the identifier-shape rule (a personal number, a DICOM UID, a
long digit run in the person's text) bumping the request to `identifiers`
and running it local whatever the table says, a pinned remote model set
aside and said so; a stream naming `x-kvasir-purpose` or `x-kvasir-grant`,
a minted key's first purpose applying unnamed within its allowlist and
class, and without any purpose a call reaching only a local backend; the
organisation's key at `PUT /v1/credentials/{provider}`, sealed with
XChaCha20-Poly1305 under a key held in a file outside the database with the
provider as associated data, decrypted in memory at use, never returned,
rotated in place. Checked live against the test provider R9 named through
both of its API shapes, the key posted over loopback and absent from every
log; under the OpenAI shape that provider returns its thinking inline as
text, under the Anthropic shape as a thinking block, which the admission
suite of C4 records per shape. Two things learned: a remote provider counts
as warm, the warm-up rule being a local runtime's; and a catalog entry may
name the provider's own model name apart from its id, so both shapes of one
provider list.

**As built (B2, the three identity modes, 2026-09-09).** In the desk
(`kineuro/nils-desk`): `local` mode is the small issuer of C46, an EdDSA
key made at first start and readable by the desk's account only, a JWKS and
a discovery document under `/.well-known/`, argon2id users added at the
command line (`nils-desk user add`, the first with `--admin`) and granted on
the settings page, and tokens of fifteen minutes minted only for the subject
that just authenticated with the entitlements stored for that user and no
wider, so the engine runs with one trust entry pointing at the desk and
cannot tell; `oidc` mode is the authorization code grant with PKCE as a
confidential client, the id token verified against the provider's JWKS with
one refetch on a key id miss, the person's access and refresh tokens held in
the session row and refreshed before expiry, the entitlements read from the
`roles` claim; `nils-desk register --authentik URL --token FILE --origin URL
--allow GROUP --bind ENTITLEMENT=GROUP` creates or finds the signing key,
the provider (confidential, redirect `{origin}/desk/callback`, fifteen-minute
access and thirty-day refresh tokens, the standard and the entitlements scope
mappings), the application, the policy bindings and the five entitlements,
and prints the engine's flags and the desk's `[oidc]` table; on the group's
provider the first run created what was missing and the second created
nothing, after one correction found only there: the provider's listings do
not filter on every field they accept, so every lookup now checks the
answer's fields itself. The display name is recorded beside the subject at
first sight, and a person renamed at the provider moves no row (tested
against a fake provider; the group's provider is registered but the desk is
not yet deployed at the registered origin, so the live sign-in waits for the
deployment). In the engine: `nils login --desk URL --username U
--password-stdin` keeps the desk's one-day token at `NILS_CONFIG_DIR`
(else `XDG_CONFIG_HOME/nils`, else `~/.config/nils`) as `token.json`, mode
600; `nils login --issuer URL --client ID` exchanges an app password over the
client credentials grant and keeps the app password beside the token to
exchange again on expiry; every `--server` verb reads the kept token when
`--token` and `NILS_TOKEN` are absent; `nils logout` forgets it. The device
authorization grant stays the growth path.

**As built (B1, the desk, the process and the shell, 2026-09-09).**
`kineuro/nils-desk`, one Rust binary (axum) with the React shell embedded by
`rust-embed`, in `off` mode: a session cookie (`HttpOnly`, `SameSite=Lax`,
`Secure` behind https) naming a row in one SQLite file; `/api/*`, `/kvasir/*`,
`/assistant/*` and `/apps/{id}/*` proxied on the one origin with the bearer
the desk holds, responses streamed through, every non-GET refused without
`X-Nils-Desk: 1` and an `Origin` or `Referer` that is the desk (a form post
from elsewhere gets 403, tested); `GET /desk/capabilities` composes the
deployment document of `contracts/suite/v1` from the engine's document
verbatim, Kvasir, the assistant and each app or its absence, the person, and
the desk with `engine_reachable` and `contract_mismatch`; `nils-desk check`
and `serve` compare the engine's `openapi` and `suite` against the desk's:
an engine behind refuses to start by name, an engine ahead is a warning the
shell shows, an engine that does not answer is a state the shell renders;
the shell is a pure function of the document, its sections and controls
predicates over a door and an entitlement with the ladder implying the ones
below, the assistant section present only when the assistant answered and
the person holds `assist`, one section per registered app that answered,
and the named states unbound person, warming and contract mismatch.

**As built (C0, the serving benchmark, 2026-09-09).** Run on the
production host against SGLang directly (the cookbook image for the model,
`flashinfer` attention on SM120, FP8 KV cache, eight running requests), one
whole 96 GB card per configuration, the model in BF16, temperature 0,
`ignore_eos` on, thinking off, the first request of every run discarded; the
client, the raw rows and the full report are in the private record. Against
§8.10's thresholds at eight streams: the aggregate of 150 tokens per second
is reached only with speculative decoding (the in-checkpoint MTP head gives
175, plain gives 101, a block-diffusion draft gives 144); the p95 first token
under two seconds on the comparable load is reached only without it (plain
1.3 s, MTP 5.0 s, the draft 20 s), and a larger prefill chunk does not move
the speculative figure, so the two thresholds name two operating points and
the `card` profile carries both, foreground and background purposes choosing
between them; the real load (24,000 tokens in) is prefill bound in every
configuration, p95 first token 25 to 37 s against the 8 s asked, so the
budgets of §12 are what the prompt cache makes cheap, and the cache does:
a resident prefix answers in 0.2 to 0.8 s and eight distinct 4k prefixes
stay resident across interleaved rounds, so conversation affinity is not
needed at this pool size. Single stream, the MTP head doubles decode (63
against 27 tokens per second). The chosen grammar backend, llguidance, does
not load in the runtime as shipped (a version mismatch inside it), so the
schema pass fraction is not measured; with the default backend every
constrained document parsed and none passed `nils ask validate` zero shot,
because the model invented measures without the guide's grounding, which is
the number behind §6.4; the negative control did not discriminate. The first
constrained run found an engine bug instead: the generated ask schema named
`#/$defs/Arg` and never registered it, fixed in A7's branch with a test that
every reference resolves. C1's admission suite is where the runtime pin and
the backend are settled.

**As built (A7, the contracts: suite v1 and mcp v1, 2026-09-09).**
`contracts/suite/v1` is seven documents and a directory of vectors:
`entitlements` (the five names, the ladder of four, `assist` orthogonal, the
`roles` claim as plain strings), `headers` (the ceiling as a role, the actor
object with `absent` as its own kind, the idempotency key at two hundred and
fifty six), `purpose` (the entry, the three content classes, the two
localities, the five refusal layers, and what an admin may open per class),
`capabilities` (the deployment document with the engine's own document as its
`engine` part and the policy row as a definition), `app` (the registry entry)
and `station` (the manifest, the five proposal kinds, the ten terminal
reasons), plus `vectors/trust-list.json` with the two issuers, the two keys,
the two JWKS documents and eight cases with their outcomes, which the engine
runs against a live server in its door tests and Kvasir will run as the same
file. `contracts/mcp/v1` is one document: the sixteen operations, the input
schema of each verbatim from the engine, the door each calls, the tool object,
the result envelope, the paging by offset and the policy fields, which are the
policy row of the door. There is no schema validator in the engine, so its
tests hold the documents to the engine's own vocabulary structurally (the
enums are the roles the engine knows, the operations are the pack loader's
list, every listed tool's input schema equals the contract's, the engine's
capabilities carry every key the suite requires of them) and the vectors by
running them. `GET /api/capabilities` now names `suite` and `mcp` beside
`openapi`, `review_item` and `pack`. The engine section closes here; §6.8's
eight fixtures stand as: 1 and 5 in the gate, 2, 3, 4, 6 and 7 as the
engine's own door tests named after them (A1 to A3), and 8 in the ask
schema's contract test of Wave 4b.

**As built (A6, the knob engine; review-item contract v4, 2026-09-09).** The
four diagnostics are the evaluator's to report, not the evidence rows' to
imply (§14's question, answered by reading `eval.rs`): a rule whose axes an
earlier set closed was skipped before it fired, and `fire` stops at the first
clause, so nothing recorded the pre-empted match. The evaluator now fires
those rules for the record and answers `Verdict.diagnostics` with
`axis_conflict` (the value it would have stored beside the one that won,
both cited), `axis_unresolved` (no hit, no default, not told to stay empty)
and `keyword_shadowed` (a keyword found in the text and not cited, whether
hidden by an earlier keyword of its own list or by an earlier rule of its
set), capped at sixty four per stack. `classify` tallies them per batch and
writes one row per kind to the batch's `diagnostic` table beside the digest's
own, with up to ten samples, replacing the last run's rows for the batches
it read; `keyword_shadowed` at batch level counts keywords shadowed somewhere
and cited nowhere, which is "can never match", and `overlay_unused` is a term
the overlay added that no evidence row of the batch cites, since the pack
keeps no bucket provenance after load. They are `diagnostic` rows and not
review items, so no kind joins the review-item contract. `GET
/api/classify/signals?scope=` takes `batch:<id>`, `origin:<name>` (matched
against the manufacturer, the model and the station) or `pack:<version>`
and answers per axis the tier counts, the confidence spread as bounds, mean
and counts under 0.5, 0.7 and 0.9 (a histogram both backends compute the
same way), the open review items by kind through the grouped items'
members, the shadowed keywords and unused terms from the diagnostic rows of
the batches in scope, the terms whose citations a person's stack decision
overrode most, and beside the axes the image type, echo time, slice count
and sequence variant of the overridden stacks as values, because those are
acquisition parameters and what a rule is written from. `POST
/api/classify/try` parses the overlay from the body (JSON is YAML), loads
the pack bare and amended, judges the overlay's cases as a count rather than
a refusal, and classifies a bounded sample of the scope under both (two
thousand stacks unless asked, twenty thousand at most), answering the moves
per axis, the review items that would close and open by the same gate
`classify` uses, and the cases. Overlays are a registry table (migration 35:
name, version, pack, author and kind, actor, scope, status, the document,
the rehearsal, why, who decided) with a review item of kind
`overlay.proposed` and scope `overlay` beside each proposal, which is the one
addition of review-item v4; adopt refuses a status other than proposed and
an author kind ranked below the proposer's, queues `classify --overlay-id`
under the caller's roles, writes `overlay.adopt` to the audit log and closes
the item; `nils overlay list|show|export|refuse` are the command line, and
export writes the document as JSON into a directory a pack loads it from.
`POST /api/ingest/probe` takes a registered location by name, never a path:
the queued command line carries `@name/relative`, and the worker resolves it
from its own `--ingest-root` flags (`nils jobs work --ingest-root`), so the
job document never holds a path either. `nils ingest probe` reads the sample
once for the union of every candidate's fields and traces each rule over it
without touching the file (`Rule::trace`), answering per candidate and per
source the shape histogram and how many files it answered, had nothing,
could not parse or were not read because an earlier source answered,
`identity_constant` over the first field source, the subject and study
counts, the fall backs and the reader's diagnostics, all written to the
job's result. The engine's tests seed a placeholder and codes in a tree and
assert none of them and no path appears in the answer.

**As built (A5, the deployment surface, 2026-09-09).** `capabilities.policy`
is one row per door (the role, whether it writes, whether it takes an
idempotency key, the cost class `free`, `bounded`, `job` or `stream`, the
result cap by name, and a label in the present and the past tense), kept as
one table in the engine. Ingest locations are `--ingest-root NAME=PATH`
(absolute directories, repeatable), published as `capabilities.ingest_roots`
by name; `POST /api/jobs` resolves `@name/relative` against them for `digest`
and `linkage import`, refuses a parent step or an unregistered name, and
refuses an absolute path a caller composes. `backup` and `verify` join the
queueable verbs: `backup` writes to `--backup-dir` (409 without one) and
`verify NAME` checks one archive in it. The verbs: `nils backup [--dir]`
writes one archive directory (a `VACUUM INTO` of the registry and the linkage
store on SQLite, a custom format `pg_dump` per schema on Postgres, the
configuration, and a manifest with the registry id, epoch, schema version and
every file's size and BLAKE2b digest; the key store is never in it), audited
as `backup`; `nils verify ARCHIVE` recomputes the digests; `nils restore
ARCHIVE --yes` refuses without the flag, refuses an archive that does not
verify or belongs to another registry or backend, writes a pre-restore
archive under the home first, then replaces the stores; it is a command an
operator runs with the engine stopped and never a job (D50). New doors:
`GET /api/packs`, `/packs/{name}`, `/batches`, `/batches/{id}` (the report,
samples as shapes) and `/quarantine` (reviewer). Custody lists backups.

**As built (A4, the ask additions; openapi contract v3, 2026-09-09).** The contract is bumped in this slice rather than in A7, because the engine's own test refuses to serve a door the published contract does not describe, and that rule is worth more than the plan's ordering: v3 is written once here with every door of §6.1 to §6.6, the slices after A4 serve what it describes, and A7 keeps `suite` v1 and `mcp` v1. The declaration block
(`grain`, `session_scheme {name, digest}`, `membership`, `key_namespace`,
`pick_rule`, `denominator`, `disclosure`, `truncated`) is computed pure over
the desugared document from the description and rides on the answer of
`run`, `preview` and `describe`; it is derived, not stored, so a job's result
carries the handle and the desk reads the declaration from `describe` of the
handle's document. `POST /api/ask/diff` takes `{a, b}` each a `document`, a
`document_id` or a `handle`: two documents answer the structural diff (per
set and part: added, removed, changed) with both canonical texts; two handles
answer by content hash and refuse by name when either is truncated.
`POST /api/ask/draft` and `nils ask draft --file` give the library's draft
its door and verb. `GET /api/ask/guide` answers the pack's grounding and
examples, the content version, the schema digest, the door list and the caps.
`POST /api/ask/describe` takes an optional `node {set, part, index}` (part
one of `set`, `where`, `has`, `near`, `attach`, `pick`, `columns`) and answers
`display_name`, `long_display_name` and `flags`. The value sampler is
`GET /api/ask/catalog/{level}/{field}/values`, a grouped count under the
caller's scope: values with counts for a technical or clinical field, shapes
(digits 9, lower a, upper A, cut at forty) for a quasi-identifying, sensitive
or identifying one, the distinct count either way, capped by
`options_values` or `?limit=`. `contains` and `starts_with` already existed
in the language since Wave 4b (the gate's case-blind fixture), so §6.4's
sixth item reduces to the two refusal sentences, which now open the pack's
grounding: the pair of sessions as one row, and where a file sits on disk.
The MCP door gains `guide`, `draft`, `job` and `job_status` in the closed
operation list, and the pack opts all four in, `nils_guide` first, so the tool
list is thirteen. Deferred from this slice: the automatic promotion of a
prefix population to a saved selection, because `starts_with` on a name is
already a clause and the subject key itself is not a filterable field; it
waits for a real question that needs it.

**As built (A3, idempotency, 2026-09-09).** `Idempotency-Key` (at most 256
characters) rides on the caller like the two headers of A2 and is honoured on
`POST /api/ask/run`, `/jobs`, `/apply` and `/handles/{id}/promote`, the doors
whose repeat creates a row with an audit consequence; documents are content
addressed, an upload and a selection write are idempotent by construction, so
they take none. The door looks the key up per principal before it answers: the
same body digest is answered again from the stored reply with `deduplicated:
true` and the original status, a different body under the same key is refused
with 409, and a fresh key records a successful answer (status below 300) in the
`idempotency` table (migration 34) for 24 hours, swept on every record.
`capabilities.idempotency` names the header, the doors and the hours. Fixture 4
is the engine's own test: two runs under one key leave one handle, two job
submissions leave one queued job, and a changed body is refused.

**As built (A2, identity, the ceiling and the actor, 2026-09-09).** The trust
list is `--oidc-trust issuer=URL,audience=ID,jwks=URL` (repeatable; `jwks` may
also be a file), with the three Wave 4b flags kept as one entry; each issuer
holds its own keys, a JWKS by URL is fetched at start and refetched once on a
key id the engine does not hold, at most once a minute (`--jwks-refetch-secs`,
hidden, sets the floor for tests), and the audit principal is the subject at
that issuer's host. The claims cache keeps `preferred_username` (or `name`)
and `email` beside the subject; `GET /api/capabilities` answers `display`,
`email`, `ceiling` and `actor` for the caller, and `nils custody` lists the
cache as a store held in memory for the token's lifetime. The two headers:
`X-Nils-Ceiling` (a role name, else 400) can only remove roles and is written
into the actor as `ceiling`; `X-Nils-Actor` (a JSON object with a `kind`, else
400) names who acts for the principal, and an exchanged token's `act.sub` is
read as `{"kind":"agent","name":...}` when no header is given. The actor is
carried by a thread-local the door sets for the request and a worker hands to
its verb as `NILS_ACTOR`, and every writer of provenance reads it there, so no
call site threads it: the audit row (`audit.actor`), the handle (`handle.actor`)
and the decision (`decision.actor_detail`) each record it, with
`{"kind":"absent"}` as its own value (migration 33). A queued job records it in
its args beside the roles. `review-item` is bumped to v3 in this slice rather
than in A7, because a property added to the item is a version by that
contract's own rule: `decision.actor_detail`. The engine gained one outbound
dependency, `ureq` with rustls, for the JWKS fetch. Fixture 7 is the engine's
own test (a ceiling narrows an operator, the actor lands on the handle, the
job, the audit row and the worker's handle; a person alone reads `absent`),
and a second test drives two issuers, a rotation refetched by URL, a crossed
audience refused and the `act` claim.

**As built (A1, the repairs, 2026-09-08).** Step 0 ran first and all four
checks confirmed the study: a reader read an operator's handle (200, no audit
row), a reader's queued job ran under the worker's own scope and left a
subject-grain handle, five event streams hung the capabilities door, and the
provider's managed entitlements mapping emits `roles` as a list of plain
entitlement names, so `--oidc-groups-claim roles` needs no engine change. The
repairs, as landed: the jobs door refuses `out.identifiers` below operator
before anything is queued and records `roles` and `may_project_raw` in the job's
args, the worker hands them to the verb as `NILS_JOB_ROLES` and `NILS_JOB_RAW`,
the verb builds its scope and its projection from them and never from its own
when they are present, the adopted row keeps what the queue recorded, and the
verb writes its result (handle, hash, counts, truncation) on the row, read back
under `result` by `GET /api/jobs/{id}` (migration 32, `job.result`). A handle
read (`GET /api/ask/handles/{id}`, `/rows`, and the command line's export) is
refused with 403 when the classes recorded on the handle exceed the caller's
scope, whoever produced it; every page read and every export writes a
`handle_read_audit` row with the optional `purpose` from the query, and the
handle answers `reads`. `GET /api/events` asks for the reader role and is capped
by `--event-streams` (half the workers, at least one; a refusal is 503 with a
reason starting `event_streams_full`), published as `capabilities.event_streams`.
A token with no role suffix warns at start. `--assist URL` publishes
`capabilities.assist` and on Postgres refuses to start without `--ask-dsn`. The
gate runs `write-refusal` first (`nils ask gate --ask-dsn`, deferred on Postgres
without a DSN; CI creates a SELECT only role and passes it), and the three door
fixtures are the engine's own tests. Contract deltas collected for `openapi` v3
(A7): `result` on the job, `reads` and the 403 on the handle doors, `purpose` on
the rows query, `assist` and `event_streams` in capabilities, the jobs door's 403.

Step 0, before any code: the four live checks. Page an operator's handle as a
reader. Queue a job as a reader with identifiers projected. Open five event streams
against a four-worker engine and call another door. Mint one token against the
provider and read the shape of the roles claim. Each is minutes.

Twenty-seven slices, each one merged pull request with its own tests. The order
interleaves the sections by dependency; a slice's letter names its section and
repository.

| # | Slice | What lands | Gate |
|---|---|---|---|
| A0 | **The record and the spec** | Record 19 scrubbed into `docs/decisions/`; this document. | The record's ids resolve; no forbidden term in the copy. |
| C0 | **The serving benchmark** | The runtime alone on the allocated card, the runs and thresholds of §8.10, the floor check if possible, the report into the record. In parallel with A1 to A7. | The report names every flag and the first request was discarded. |
| A1 | **The repairs** | §6.1: the job carries the caller and its result; handle reads authorised and audited; the events door checks role and caps streams; the bare token warning; `--assist` requires `--ask-dsn`. | Fixtures 1, 2, 3 and 6 of §6.8 green on both backends. |
| A2 | **Identity, the ceiling and the actor** | §5.3, §5.5, §6.2: the trust list, JWKS by URL, `act`, the claims cache fields in custody, the two headers on every door and every audit row, handle provenance and decisions; review-item v3 prepared. | Fixture 7; two entries, two tokens, one engine, both accepted; a rotated key keeps a running engine serving. |
| A3 | **Idempotency** | §6.3 on the four doors. | Fixture 4. |
| A4 | **The ask additions** | §6.4: the declaration block, `diff`, `draft` and `nils ask draft`, `guide`, the value sampler, `contains` with the prefix promotion and the two refusal sentences in the pack, node-level describe, the four MCP operations. | Fixture 5; the yardstick and the gold questions still reproduce; a tool-only MCP harness obtains the worked examples. |
| A5 | **The deployment surface** | §6.5: `capabilities.assist`, the policy table, ingest locations and the four job kinds over them, `backup` and `verify`, the batches, packs and quarantine doors, `nils restore` documented. | A job with an absolute path is refused; a backup job writes an archive `verify` accepts; the policy table lists every door. |
| A6 | **The knob engine** | §6.6: the four diagnostics, `signals`, `try`, overlays as objects with adopt and the review item, the ingest probe. | A contrast keyword added to an overlay: `try` names the stacks that would move and adopt moves exactly those; the probe on a synthetic tree names the placeholder by shape and no response field matches a seeded value or a path segment. |
| A7 | **The contracts** | §6.7: `openapi` v3, `review-item` v3, `suite` v1, `mcp` v1, each a titled pull request under the DCO; fixture 8. | The engine's contract tests pass; the desk's and the assistant's generated clients build. |
| B1 | **The desk: the process and the shell** | §7.1, §7.2 in `off` mode: the binary, the embedded bundle, the proxy with its CSRF defences, the capabilities merge, the sections as predicates, the three named states, the contract check, the app registry. | A cross-origin write is refused; a deployment with no assistant renders no assistant section; an unknown major contract version refuses to start with a named message. |
| B2 | **The three identity modes** | §5.1, §5.4, §5.7 to §5.9: `oidc` with PKCE and the registration script; `local` with users, the issuer, the JWKS and the admin page; `nils login` on both; the unbound-person page; display names. | Three commands on a fresh provider; two users on a laptop in `local` mode; a person renamed at the provider moves no row. |
| C1 | **Kvasir: the door and the catalog** | §8.2, §8.5: pi-messages in and out, the OpenAI-shaped door, `GET /v1/config`, the `card` profile supervisor, the runtime key, warm-up and the health rule. | A pi client streams through Kvasir with zero compatibility flags; a thinking signature round trips; the first token after a restart is reported as warming until it arrives. |
| C2 | **Identity, keys, admission control and the ledger** | §8.4 (minted keys), §8.7, §8.8: the trust list with the shared vectors, minting and revoking, per-backend queueing with the heartbeat, the ledger, the metrics. | The vectors pass in Kvasir and in the engine; a stream with no principal is refused; the ledger schema has no content column; the ninth stream queues and the seventeenth is refused. |
| C3 | **Grants, purposes and policy** | §8.3, the organisation's commercial key of §8.4, the identifier-shape rule, the models table API the desk reads and writes. | A `rows` purpose cannot reach the remote backend until an admin acknowledges; an `identifiers` purpose never can; a request with an identifier shape runs local whatever the table says. |
| B3 | **The question page** | §7.3. | The acceptance sentences of §7.9 about versions, the write path and the diff. |
| B4 | **The result surface** | §7.4. | Presenting twice executes once; the three states render; an export writes a read audit row. |
| B5 | **Operations and data** | §7.5, including the ingest forms over locations, backup, verify and the restore page. | Each role's absent control renders nothing; a release requires the typed name; an ingest form cannot name a path outside a location. |
| B6 | **Settings** | §7.6, the models table against C3. | A remote model cannot be chosen for a `rows` purpose without the acknowledgement being recorded and displayed. |
| C4 | **The admission suite** | §8.6, and Kvasir's overhead measured against C0. | The negative control fails to produce a malformed clause; overhead within the thresholds; the admission record names runtime and build. |
| D1 | **The bench** | §9.10, the corpus scrubbed and rebased on the production host under the corpus review rule, the taxonomy, the split, the recorded provider, the manifest and the matrix, the baseline re-run. | The public repository holds no term from the source; the baseline number is reproducible from the repository alone. |
| D2 | **The host and the seam** | §9.1 to §9.3, §9.7: the Flue application pinned, the store on both backends with export and import, the generated client, the grant and ceiling enforcement, the actor, the idempotency key, the ledger, the redactor, retention, telemetry off, the Kvasir provider, token push from the desk. | The container cannot connect to the registry database; a seeded marker reaches no store and no remote call; a conversation survives a process kill and round-trips through export. |
| D3 | **The station framework** | §9.4, §9.6: the manifest loader, the phase machine, the finish tool, the checks, the budget and the terminal reasons, the proposal path, loop detection, the completeness fields. | A tool stubbed to fail forever terminates within the budget with the named reason; a truncated argument stream is refused; the grant grep passes. |
| D4 | **`ask-help`** | §9.11 with its brief and fixtures; B7 (§7.7) in the desk; `nils assist ask` in the engine repository. | The bench: beats the baseline, fewer than 22 correction turns on the chains, the composition question returns one number five times with its scheme digest; a rejected proposal is not repeated in the next turn. |
| D5 | **The concierge, clarification and memory** | §9.12, §9.9. | The concierge's grant is exactly the list of §9.12; a clarification fires only at a pick, a scope or a grain; an institutional note names no value a reader may not see. |
| D6 | **`keyword-tune`** | §9.13, with the desk's keyword tab. | Closing bar 8, first half. |
| D7 | **`identity-check`** | §9.14, with the desk's anonymisation page. | Closing bar 8, second half. |
| C5 | **Brought keys and the OAuth slot** | §8.4's third kind: encryption with the subject bound in, the advisory lock, PKCE, the fixed redirect, the one-provider offer and the other's explanation, the settings surface. | Two people connect in the same minute; a database dump yields no usable minted key; the forbidden provider shows the sentence and the date. |

Three checkpoints where the wave could pause with something whole: after A7 (the
engine repaired and contracted), after C4 (a usable desk and a benchmarked model
service), after D5 (the assistant at its first decision point).

## 14. Open questions carried into the wave

- The shape of the roles claim on the live provider (step 0).
- Whether the production host's driver allows a temporary MIG instance for the
  floor check; otherwise the floor is recorded as untested.
- Whether `keyword_shadowed` can be derived from the evidence rows rather than from
  the evaluation loop; A6 decides by reading the evaluator.
- The exact budget numbers of §12, which the bench measures and may move.
- Whether the prompt cache survives eight interleaved conversations (C0), which
  decides conversation affinity in Kvasir.
- The federation spike (workload identity federation with the provider as the
  issuer), which is not in the wave and would make the organisation's model
  credential zero stored secrets.
