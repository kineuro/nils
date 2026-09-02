# 14 — Federation: nodes, and data that stays where it is

Two conversations put this on the table. A PI from Vienna spent time with the group
to see how we work, and the question came up whether NILS could be federated: each
group runs its own NILS, local and secure, and when two groups choose to connect, a
query in Stockholm reaches Vienna as if the data had grown. Colleagues in Amsterdam
described their setup as a cluster, which is a different shape again. This doc
designs for both, 2026-09-02, so that the primitives federation needs are in the
contracts from Wave 4 (where they cost almost nothing) and the federation itself is
built later, behind a flag, without touching anything already decided.

The stance is the folder's: the engine is complete alone; every addition is optional;
one door. Federation is optional twice over: a node is off by default, and adding a
node is adding a line of trust, not a deployment. Decisions D25 to D29 and amendments
C26 to C34 are in section 6, all ratified on 2026-09-02 ([15](15-ratification.md)
§7 and §9).

## 1. What "federated" has to mean here

Three different things hide behind the word:

1. **Federated query**: a question runs at every site and only what each site allows
   comes back, usually counts and aggregates. This is what hospital networks do with
   i2b2/SHRINE and TriNetX, what genomics does with GA4GH Beacon, and what the
   EBRAINS Medical Informatics Platform does for neurology.
2. **Compute-to-data**: the analysis travels to the data as a container, runs there,
   and only results travel back. The Dutch Personal Health Train (Vantage6),
   DataSHIELD, OHDSI network studies, COINSTAC in neuroimaging, and federated learning
   (FeTS, Flower, NVIDIA FLARE) are all this.
3. **Pooling under agreement**: individual-level data moves to one place. That is a
   data transfer, not federation; the group already has Bifrost for it, and nothing
   here replaces it.

NILS v1 does the first two natively and leaves the third to Bifrost. It can do so by
construction, because of four things the folder already decided:

- **A query is data** (D5). An AST can be sent to another engine and executed
  there, unchanged, with the receiving engine governing it exactly as it governs a
  local query.
- **The pack is a versioned, shared vocabulary** (D12). Two registries classified
  by the same pack version answer "3D FLAIR at 7T" the same way. The pack is NILS's
  common data model, in the sense OMOP's CDM is OHDSI's: the thing that makes results
  from different sites comparable without anyone seeing anyone else's rows.
- **Pipelines are containers under the field's contract** (D9). A BIDS App is
  already the "train" of the Personal Health Train; sending it to a peer is sending a
  digest-pinned image and a selection.
- **Judgements are review items under policy** (D7). "May Vienna's request run
  here" is a judgement with evidence, a policy and an audit trail: the machinery
  exists, federation only adds a kind.

The legal frame fixes the default. Under GDPR each site stays the controller of its
data; ethics approvals bind data to the site that collected it; the European Health
Data Space regulation (in force since 2025, its secondary-use obligations phasing in
from 2029) makes secure processing environments and cross-border use through them
the European direction of travel. So the design assumption is: **individual-level
data never leaves a node unless a written agreement and a human at that node say
so.** What crosses routinely is ASTs, containers, counts and aggregates.

## 2. Prior art, and what to take from each

| System | Model | What NILS takes |
|---|---|---|
| i2b2/SHRINE, TriNetX | federated cohort counts across hospitals; small counts masked, noise added | count-first; small-cell suppression as a per-node policy, not a courtesy |
| GA4GH Beacon v2 | `boolean`, `count`, `record` response granularity; filtering terms; networks of beacons | the disclosure-level vocabulary; a Beacon endpoint is a thin adapter on the node daemon later |
| EBRAINS Medical Informatics Platform | hospitals run local nodes; a federation shares a variable catalogue and runs analyses over harmonised variables | local node first; the federation is a named thing with its own catalogue |
| Personal Health Train / Vantage6, DataSHIELD (Opal, Armadillo) | algorithms as containers sent to stations; nodes only need outbound connections; server-side disclosure filters | outbound-only nodes; containers as the unit of travelling compute; disclosure enforced at the door. This is also the vocabulary the Amsterdam side will recognise |
| OHDSI / OMOP | a common data model, study packages, aggregate results with a minimum cell count | the pack as CDM; a "network study" is a federation with a purpose |
| ENIGMA | standardised protocols run locally; summary statistics meta-analysed | meta-analysis as the honest default for cross-site aggregates; harmonisation (ComBat and kin) as a pipeline |
| COINSTAC | decentralised neuroimaging computations with iterative aggregation | iterative compute across nodes as a later pipeline kind on the same spine |
| FeTS (OpenFL), Flower, NVIDIA FLARE | federated learning in medical imaging (FeTS: 71 sites, glioma segmentation) | FL is a pipeline kind, not a core feature; the design leaves the hook |
| EUCAIM | a hub plus federated nodes for cancer imaging; data stays local; federated query and processing | a small, blind hub is acceptable; the node stays the unit of trust |
| Mainzelliste, Bloom-filter PPRL | privacy-preserving record linkage across institutions | cross-node subject overlap is unknown by default; PPRL is an explicit later addition |
| Five Safes | safe projects, people, settings, data, outputs | "safe outputs" is disclosure control at the door; "safe projects" is the federation agreement |
| GA4GH Passports | signed claims about a user carried across organisations | users cross as claims signed by their home node; roles are always mapped locally |

The imaging databases in daily use (XNAT, LORIS) are single-site by design;
federation in imaging came from consortia and from the FL toolkits. There is no
open, self-hostable neuroimaging registry that federates at the query level with
a shared classification vocabulary. That is the gap NILS would fill, and it is the
same gap the pack design was already aimed at.

## 3. The architecture

### 3.1 Vocabulary

- **Node**: an engine in server mode (either storage backend, auth `token` or
  `oidc`) plus the node daemon (`nils node`). The unit of data custody and of trust.
  An organisation may run several nodes.
- **Federation**: a named set of nodes under one agreement: purpose, members and
  contacts, disclosure floor per entity, pack version range, approval policy, audit
  cadence, exit. A node may belong to several federations with different floors.
- **Peer**: another node in a federation you belong to.
- **Request**: an AST (or a pipeline run spec) sent to a peer with a disclosure
  level and a purpose. At the receiving node it is a review item.
- **Disclosure level**: `boolean | count | aggregate | record`, in Beacon's order
  with `aggregate` added between count and record.
- **Node profile**: a suppressed coverage summary a node publishes to its peers
  (counts per axis value, modality, field strength, observation type, pack
  versions, registry epoch), so "which nodes have 7T MP2RAGE" is answered from the
  catalogue without a request.
- **Cluster**: two readings, both supported (3.5). A compute cluster behind a node
  (SLURM, Apptainer, a shared filesystem), and a cluster of nodes under one
  governance that federates internally under a looser floor and presents one
  manifest outward.

### 3.2 Local first, by construction (D25)

Federation off is the standalone or server engine, byte for byte. Without the daemon
running, `GET /api/capabilities` advertises no federation, so there is no scope chip
in the notebook, no federation tool in MCP, no endpoint, no error (principle 2).
Turning it on is a config block and a trust exchange: the peer's manifest and public
key, and yours to them.

The daemon is **a client of its own engine**. It holds a service token whose role
(`federated-reader`, plus per-federation grants) bounds what any peer can ever
obtain, and it reaches the engine only through the public contracts like every other
app (principle 1, rule 2 of [01](01-architecture.md)). The engine gains no
federation code path; it gains generic primitives (3.6) that are useful locally too.
Three reasons: the one door stays one door; a compromised daemon has the powers of a
reader under disclosure limits and nothing more; and federation standards move
(Beacon, PHT, EHDS), so the daemon can wear adapters without the engine noticing.

It ships in the same binary, `nils node serve`, as a separate process with its own
token and its own audit principal, so installing NILS is the whole entry ticket for a
new site. That is the second reason the single binary matters: a node can be a small
server in a hospital basement.

### 3.3 Identity, trust and transport

- **A node is a keypair.** Its signed **manifest** carries name, organisation,
  contact, engine and contract versions, pack versions, the disclosure floor it
  offers, and its endpoints. Trust is explicit, bilateral and pinned, the
  `known_hosts` model: no PKI to run, a revocation is deleting a line. A federation
  directory is an optional convenience later, never a requirement.
- **Requests are signed at the application layer** by the node key (with mutual TLS
  on top where the network allows), so the design is independent of the network:
  public HTTPS, a WireGuard mesh, or a relay all work.
- **Users cross as claims.** The home node signs `(node, subject, groups)`; the
  receiving node maps `(federation, node)` to a local role and never trusts a remote
  role. The audit principal on both sides is `user@node`. Each user logs in at home;
  there is no federated SSO, no shared directory, no cross-node password anything
  (principle 7).
- **Outbound-only nodes are the normal case.** Hospital networks accept no inbound
  connection; Stockholm's own node sits behind one today and we reach it over a
  tailnet. Two patterns cover it: a self-hosted WireGuard mesh (Headscale with a
  DERP relay on 443, which is what we already run), or a **blind store-and-forward
  relay** that holds signed, encrypted envelopes and sees only metadata. The mesh is
  the recommendation for two or three nodes; the relay is for a member that cannot
  join a mesh. Vantage6 solved the same constraint the same way: nodes dial out.

### 3.4 The federated query

The same AST, no second language. From the notebook, the CLI or the agent, a request
is `{ast, federation, nodes?, level, purpose}` submitted to the home daemon. It is
always a job (the two tempos of C22: remote is never synchronous), and it proceeds:

1. **Validate at home** against the federation catalogue: the intersection of the
   peers' catalogues at compatible pack versions, restricted to fields marked
   `federated` (3.6). A query that uses a local-only field never leaves.
2. **Fan out** signed requests to the chosen peers.
3. **At each peer**: the request lands as a review item of kind
   `federation.request`; the per-kind policy decides (auto-approve `count` above
   the floor from a trusted node; a human for `record`, always); the AST executes as
   `user@node` in the reader role; the **result handle stays at the peer**; the
   response is the disclosure projection of that handle: a count with small-cell
   suppression, aggregates under the aggregate rules, rows only under an agreement.
4. **Merge at home** into a result handle whose provenance lists, per node: engine
   and pack versions, registry epoch, AST hash, level, suppression applied,
   approver. Per-node results are always shown; a sum row appears only for counts,
   with "overlap unknown" stated; aggregates combine as a meta-analysis (n-weighted
   means where the AST asked for mean and n; histograms whose bins are declared in
   the AST's ref options and therefore align across nodes by construction).

What the merge cannot do, said plainly: anything that needs individual-level
alignment across nodes. Subject overlap between sites, a temporal window spanning
two nodes' events, a join across nodes: impossible without record-level data or a
privacy-preserving linkage step, and the notebook says so instead of approximating.
Grain (D20) is what makes suppression well-defined: k applies to the stage's grain,
and a session or stack count must also clear k at the subject grain, because five
sessions of one person is one person.

**Safe outputs (D27).** Per node and per federation: a minimum cell size k (default 5
for imaging metadata, 10 for clinical entities) with complementary suppression so a
total cannot reveal a suppressed cell; optional rounding; no exact dates or ages at
`aggregate` level, only bins; `sensitive` fields and free text never; value chips only
above k; a query budget per requester per day and a log-based differencing guard
(near-identical ASTs differing by one predicate are rate-limited and flagged); every
request and response logged at both ends. No technical measure defeats every
differencing attack; the agreement carries the rest, and says so.

**Node profiles** refresh per registry epoch and are suppressed like any aggregate;
the notebook's catalogue chips show per-node availability from them, which is the
"as if the data had grown" experience without a single request.

### 3.5 Compute travels, data stays (D29)

A pipeline run at a peer is the same run as at home: a digest-pinned image, a
`nils.job.yml`, a selection AST. Derivatives are registry rows at the peer; what
comes back is the declared QC metrics and aggregate outputs, under the same
disclosure policy as a query. A peer never runs unknown code: images must be on its
allowlist or signed by a publisher it trusts, they run rootless with no network (D18),
and the run is a `federation.run` review item under policy. That is the Personal
Health Train's safety model, and the Dutch platforms enforce exactly this.

**Clusters.** In the first reading, Amsterdam's cluster is compute: the SLURM and
Apptainer executor that [09](09-pipelines.md) declared as a seam becomes a named
deliverable with a first target (C31): rootless by construction, GPU partitions as
resource leases, staging on the shared filesystem, one array job per unit set. In the
second reading it is several nodes under one governance: a federation whose internal
floor may allow `record` among its own members under their joint controllership, and
which presents a single manifest to the outside. Both fit; which one Amsterdam is, I
have to ask (section 7).

**Federated learning** is an aggregator node plus rounds of `federation.run` requests
that reference a run id. The math belongs to FeTS, Flower or FLARE inside the
container; the design leaves the hook and builds nothing until someone needs it.
**Harmonisation** (ComBat and successors) needs per-site statistics: one `aggregate`
request per node feeding a pipeline, later.

### 3.6 What the engine must have, from Wave 4

The primitives are generic; each is useful without federation, which is the test
that they belong in the engine:

- `GET /api/capabilities` reports engine and contract versions, loaded pack versions,
  the **registry epoch** (a counter advanced by every ingest batch and
  classification run, so a result is reproducible "as of"), and the federation
  endpoint when the daemon is configured (C26).
- Catalogue visibility gains `federated` beside `sensitive` (C20): a field crosses a
  node boundary only if allowlisted. Free text, paths, exact dates of birth and
  identifiers are `local` by default (C27).
- **Result projections by disclosure level** are an API feature: any result handle
  can be read at `count` or `aggregate` with the suppression rules applied. Locally
  this serves teaching, students and external readers; federation only ever asks
  for projections (C28).
- Review-item kinds `federation.request` and `federation.run`, with policies in the
  same configuration as every other kind (C29):

```yaml
federation.request:
  - when: {federation: sthlm-vienna, level: [boolean, count]}
    auto: approve
    suppression: {k: 5, round: 5}
  - when: {federation: sthlm-vienna, level: aggregate, entities: [session, stack]}
    auto: approve
    suppression: {k: 5, bins: {age: 5y, date: month}}
  - when: {level: aggregate, entities: [event]}
    require: human
    suppression: {k: 10}
  - when: {level: record}
    require: human
    agreement: required
```

- Principals of the form `user@node`; a `federated-reader` role; verification of
  signed claims from pinned peer keys (C30).
- Result handles carry node, pack version, registry epoch, level and suppression
  applied (C26, extending C21).
- Identity is node-local (C32): the pseudonym domain of D13 is per node; a
  record-level result carries the peer's pseudonyms only under agreement; linkage
  across nodes exists only through an explicit PPRL step, later.
- The remote-executor seam of [09](09-pipelines.md) gets its first implementation,
  SLURM with Apptainer (C31).

### 3.7 The apps (C34)

- **nils-query**: a scope chip (this node, a federation, chosen nodes); catalogue
  chips with per-node availability from profiles; results with a node column and
  suppression marks (`<5`); "overlap unknown" on sums; a request's approval state
  as job status ("waiting for Vienna"). The AST, the notebook steps and the draft
  selection loop are unchanged.
- **nils-agent**: the daemon has its own MCP server in the shape of
  [05](05-contracts.md) §4 (tools only, JWT per request); the agent connects to
  both servers; "across the federation" is a scope on execute; approvals appear as
  job states, so the conversation never blocks on a human elsewhere.
- **CLI**: `nils node init | peers | serve | requests`, and
  `nils query --federation sthlm-vienna --level count`.
- **nils-segment**: unaffected; annotation is local. A work on a peer's data is a
  compute-travel run there.

### 3.8 Operating a federation

- **The agreement** is a document we ship as a template with the docs: purpose,
  members and contacts, disclosure floor per entity, pack version range, approval
  policy, audit review cadence, incident process, and exit. Exit is revoking a key;
  nothing individual-level was shared, so nothing needs recalling beyond aggregate
  results and logs.
- **Versions.** Packs use semantic versions and a vocabulary change is a major. A
  request declares the pack version it was written for; a peer on an incompatible
  version answers `incompatible` with a diff, never an approximation. Contract
  versions likewise (D26).
- **Monitoring.** Node liveness, request latency, approval backlog. Nothing else
  leaves a node.

## 4. Threats and what answers them

| Threat | Answer |
|---|---|
| A malicious or compromised peer | reader role plus disclosure floor at the answering node; `record` needs a human; keys revocable by deleting a line |
| Differencing attacks through repeated near-identical queries | k with complementary suppression, rounding, a per-requester budget, the log-based guard; the agreement carries the residual risk explicitly |
| Exfiltration through `record` | agreement required, human approval, audit at both ends, minimisation to `federated` fields only |
| Free text or PHI in a result | `local` visibility by default for free text, paths, dates of birth, identifiers; `sensitive` never; the D13 domain is per node |
| Unknown code at a peer | image allowlist or trusted signatures, digest-pinned, rootless, no network in the container (D18) |
| Relay compromise | envelopes signed and encrypted end to end; the relay stores and forwards and sees metadata only |
| Version drift between nodes | pack and contract compatibility checked per request; refuse with a diff |
| Expensive or flooding queries | everything is a job with timeouts, queue caps and budgets per federation |
| Identity spoofing | claims signed by pinned node keys; roles mapped locally; no remote role is ever honoured |
| Replay | signed timestamps, nonces and request ids; expiries on every request |

## 5. What this changes in the folder

In-place additions, marked as proposals until they were ratified on 2026-09-02: [00](00-vision.md) (a fourth sentence
and a note under principle 3), [01](01-architecture.md) (the map, the optionality
matrix, a deployment shape), [03](03-registry.md) (identity is node-local; the
registry epoch), [04](04-classification-packs.md) (the pack as common data model and
its versioning rule), [05](05-contracts.md) (capabilities, catalogue visibility,
projections, review kinds, principals, provenance), [06](06-app-query.md) and
[08](08-app-agent.md) (scope), [09](09-pipelines.md) (compute-to-data, the cluster
executor, allowlists), [10](10-repos-licensing.md) (`nils node` in the engine repo,
a relay repo only if built, the agreement template), [11](11-order.md) (Wave 4
primitives, the executor in Wave 7, a federation wave with a pilot gate).

## 6. Decisions and amendments register (continued from 13)

Proposed decisions:

All five ratified 2026-09-02 as direction; nothing is built before Wave 8.

- **D25, local first, node optional.** A node is an engine plus `nils node`;
  federation off is the standalone engine; adding a node is a trust line; the
  daemon is a client of its own engine bounded by a reader role.
- **D26, the pack is the common data model.** Cross-node comparability is a
  pack-version contract; the vocabularies (axes, image-type tokens, observation
  types, identifier namespaces, roles) are the interoperable layer; incompatible
  versions refuse with a diff.
- **D27, disclosure levels and safe outputs at the door.** `boolean | count |
  aggregate | record`; k per node and federation; `record` only under a written
  agreement and a human approval; `sensitive` and free text never; both ends audit.
- **D28, requests are review items.** `federation.request` and `federation.run`
  under the per-kind policies of D7; no second approval machinery.
- **D29, compute travels, data stays.** Pipelines run where the data is;
  derivatives stay; QC and aggregates return under D27; peers run only allowlisted
  or signed images; the SLURM and Apptainer executor is the first remote executor.

The amendments, all accepted 2026-09-02 ([15](15-ratification.md) §9):

| Id | Affects | Proposal | Status |
|---|---|---|---|
| C26 | 01, 05 §1, C21 | Capabilities report contract, engine and pack versions, the registry epoch and the federation endpoint; result handles carry node, pack version, epoch, level and suppression | accepted 2026-09-02 (15 §9) |
| C27 | 05 §2, C20 | Catalogue visibility `local` (default for free text, paths, exact dates, identifiers) and `federated` (allowlist) beside `sensitive` | accepted 2026-09-02 (15 §9) |
| C28 | 05 §1 → D27 | Result projections by disclosure level with small-cell suppression, complementary suppression, rounding and binning as a generic API feature | accepted 2026-09-02 (15 §9) |
| C29 | D7 → D28 | Review-item kinds `federation.request` and `federation.run`, policies per federation, level and entity | accepted 2026-09-02 (15 §9) |
| C30 | D8 | Principals `user@node`; `federated-reader` role; signed claims verified against pinned peer keys; no remote role honoured; node identity is a keypair with a signed manifest | accepted 2026-09-02 (15 §9) |
| C31 | D9, D18 | The SLURM and Apptainer executor named as the first remote executor, Amsterdam as first target; image allowlists and signatures at every node | accepted 2026-09-02; the executor is built only when Amsterdam confirms a compute cluster (15 §9) |
| C32 | D4, D13 | Subject identity and the pseudonym domain are node-local; cross-node linkage only through an explicit PPRL step; overlap reported as unknown | accepted 2026-09-02 (15 §9) |
| C33 | 11 | Wave 4 ships the primitives of 3.6; Wave 7 the executor; a federation wave after Wave 5 with the two-node pilot as its gate | accepted 2026-09-02 (15 §9) |
| C34 | 06, 08, 02 | Scope chip, per-node results and suppression marks in the notebook; a second MCP server on the daemon for the agent; `nils node` and `--federation` in the CLI | accepted 2026-09-02 (15 §9) |

## 7. What I could not verify, and what to ask

- **Amsterdam's cluster.** Compute cluster, cluster of sites, or both. The executor
  is the answer to the first; a federation with an internal floor to the second.
- **Vienna's data and network.** What they hold, who the controller is, whether a
  node can sit in their network with outbound access only, and whether they would
  run NILS v1 (a node needs a v1 registry: the pack must classify their data, so
  the single binary and the digest at their site come first). A two-node pilot
  needs both sites past Wave 4.
- **The legal template.** Whether the group's approvals already permit aggregate
  sharing (they usually do), and a review of the agreement template by KI's data
  protection office before the first request crosses a border.
- **Names.** Decided 2026-09-02 (D31, [15](15-ratification.md) §10): the
  federation has no name of its own. It is "the federation" in prose, `nils node`
  on the command line and `nils-relay` for the relay. The suggestion this design
  made, Yggdrasil (the brand mark already carries the tree that joins the realms),
  was withdrawn the same day: Yggdrasil Network is an existing open-source
  encrypted mesh network, the kind of thing a NILS federation could run over, and
  the collision would confuse exactly the people the name was meant to help. The
  brand mark keeps its tree.

**The pilot, when both sides are ready:** Stockholm and Vienna as a two-node
federation over a mesh; the composition and protocol questions of
[13](13-query-and-agent-study.md) §2 (sessions with a 3D FLAIR and an MP2RAGE at
7T; protocol spread per role) answered at both nodes at `count` with k=5; one
request that needs a human; audit read at both ends; a researcher in Stockholm gets
"how many such sessions exist across both sites" from the notebook without sending
an email. Amsterdam's part is the executor running the starter pipeline on a
selection at their node. Nothing individual-level moves in either.
