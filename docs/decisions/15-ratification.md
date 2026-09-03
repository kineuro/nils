# 15 — Ratification: every open item, with a recommendation

Nima is the decision owner for NILS; I advise. This sheet lists every open item
in the register chain ([12](12-review-devils-advocate.md) §5 and §6,
[13](13-query-and-agent-study.md) §6, [14](14-federation.md) §6), the licensing
decision of [10](10-repos-licensing.md), and the repository restart, each with my
recommended verdict and the reason in one breath. Written 2026-09-02.

> **Status: ratified in full.** Nima answered in two messages on 2026-09-02: the
> first batch is in section 7, the items he raised with it in section 8, and the
> rest, license included, in section 9. No item in this sheet is open; what
> remains waits for evidence or for other people, not for a decision.

**How to answer.** "Accept all as recommended" is a complete answer. Otherwise
name the ids you change and what to. Verdicts use five words:

- **accept**: as written in the register.
- **accept, changed**: as written, with the change stated here.
- **reject**: the register line is withdrawn.
- **evidence**: stays open until a named measurement or prototype, with an owner
  and a wave; the work to produce the evidence is approved now.
- **defer**: not needed before the wave named; parked, not lost.

**What happens after.** In one commit I flip the Status column of every ratified
line in 12, 13 and 14, update the README register, and amend the doc text where a
verdict changed it. From then on the folder speaks with one voice and the repo
restart can begin.

## 1. The decisions that need you

These are the items where the recommendation is a real choice, not housekeeping.

### License (10, open since the folder was written)

**Recommendation, revised 2026-09-02: AGPL-3.0-only for the engine, the apps, the
relay and the first-party packs; Apache-2.0 for the contracts and the client
libraries; a CLA on the AGPL repositories and a DCO on the Apache ones; the name
registered as a word mark.** The full analysis is in [10](10-repos-licensing.md).
The first version of this sheet said Apache-2.0 for everything, on the argument
that the moat is the corpus and the oracle and a standard wins by being freely
implementable. Two things Nima then put on the table change the answer: the fear
that a permissive license lets a company take the best tool of its kind and charge
for it, and the wish to keep open a company of our own on the Metabase pattern.
Against those, three facts decide: GPL-3.0, the v0 license, has the hosting
loophole and so does not deliver the protection it was chosen for, while AGPL-3.0
does; no open-source license forbids charging and we intend to charge for hosting
ourselves, so the principle we want is "whoever builds on it shares back", which
is AGPL in one sentence; and the door swings one way, since AGPL can become Apache
later in one commit while Apache can never be taken back from what is already
released. The contracts stay Apache-2.0 so that a rival implementation of them is
the standard winning, not a threat. v0 stays GPL-3.0 where it is.

**Decided 2026-09-02: accepted as recommended, in Nima's word, "totally".**
[10](10-repos-licensing.md) records it as the decision; the public repository's
`LICENSE` is AGPL-3.0-only from its first commit (R6).

### D13, PHI custody and the pseudonymization domain

**Recommendation: accept, with these specifics written into 12 §5 so that Wave 1
builds them and not an interpretation:**

- The registry holds **no direct identifiers**. Names never enter; the source
  PatientID and every other identifier live only in a **linkage store**, a separate
  schema with its own key file and a read audit, filed under their ID type. Birth
  date, sex and exact dates stay in the registry as quasi-identifying fields under
  class, with the clinical timeline beside them (C35, section 8; the first version
  of this bullet said "birth date becomes age at study", which was wrong).
- The **pseudonym scheme is declared per registry** (C36, section 8): `blake2b-8`,
  v0's keyed digest, for the KI registry with the v0 key, so that every existing
  subject keeps its code; for new registries the default is a keyed digest with the
  key held outside the database (a file, mode 600; a KMS later), the full digest
  stored, a 12-character display code derived from it, and a collision detected at
  insert and never silent. v0's four-digit truncation goes in both cases.
- **Sensitivity classes** on every catalog field (identifying, quasi-identifying,
  clinical, technical), enforced in query results, review-item evidence and MCP
  responses alike, which is the same mechanism C27 and C28 build on.
- The anonymizer does **keyed UID remapping**, **per-subject date shifting**
  (one offset per subject, drawn uniformly within ±180 days, held in the linkage
  store; ages computed before the shift), a **private-tag policy** that drops by
  default with an allowlist, a burned-in-pixel check that emits a review item, and
  an audit that never sits beside the originals. `preserve_uids` defaults to false.
- The migration of the live registry (Wave 4) keeps birth dates and the clinical
  timeline, drops names, and moves every identifier to the linkage store (C35).
- The domain is **per node** (C32).

**Accepted 2026-09-02, as amended by C35 and C36 (section 9).**

### C1, the language spike

**Recommendation: evidence, with a stated prior of Rust, ten working days, in
Wave 0.** The spike measures three things on the baseline host: files per second
and resident memory on one million production instances with `dicom-rs` and with a Go
DICOM library; the count of vendor files each library fails on; and whether a
static binary that embeds SQLite and DuckDB cross-compiles for the six release
targets from CI. The third is where Go is likely to lose, because DuckDB in Go
means CGO and CGO makes "six static binaries" a fight; Rust's `duckdb-rs` bundles
the engine and cross-compiles with known tooling. Go wins only if it matches
throughput within 20 percent, fails no more vendor files, and clears the
cross-compile test, in which case the group's Go prior art (Bifrost) tips it. The
spike lives in the public repo under `spikes/` and its report closes D2 either
way. **Accepted 2026-09-02, with maintainability as the second criterion (section 7).**
**Closed 2026-09-02: Rust**, on the first corpus and the CI matrix alone, because
the gap (2.8× the throughput at a third of the CPU, 1,771 vendor files the Go library
fails and Rust reads, six of six static targets against five) left nothing for the
baseline host, the one-million-instance run or the mix corpus to reverse; those runs
join the report as evidence when they exist. The decision and its numbers are in
[02](02-engine.md), the report in `spikes/lang/README.md` of the public repository.

### C11, the pack format

**Recommendation: evidence, decided by a prototype before Wave 2, with a stated
prior of "declarative grammar, sandboxed expressions as the escape hatch, never
Python".** The 10k Python lines are mostly tokenizers over series descriptions:
token tables, ordered tiers, exclusion groups, regexes and thresholds, which are
data. The prototype expresses the three hardest pieces of v0 (the five parsers
with their 138 flags, the MP2RAGE TI rule, the SWI, SyMRI and EPIMix branches) in
the draft format during the second half of Wave 1. If 90 percent or more of the
grammar is declarative, the pack ships without a code hatch; what is left gets a
small sandboxed expression language (predicates over fingerprint fields, no I/O,
no loops), not a plugin API. This keeps principle 6 true and keeps packs
contributable by people who do not write Rust. **Accepted 2026-09-02, with the
modality-extension criterion (section 7).**
**Closed 2026-09-03: the grammar is 100 percent declarative and the pack ships
with no code hatch.** The prototype (`spikes/pack/` in the public repository)
expresses the five parsers with their 220 predicates, all 138 unified flags and
the seven helpers v0 keeps as context methods, the SWI branch with its own
taxonomy, and the physics-vote pass, and is checked against v0's own code over
the live corpus: 518,365 stacks × 138 flags, 17,054 branch verdicts and 518,353
votes, **no disagreement in any field**. It reads the pack in 5.5 s where v0's
hand-written Python takes 30.0 s, so the format costs nothing to run. The
prototype swapped the SyMRI and EPIMix branches for the physics vote, which
makes the test stricter: those two are structurally the same as SWI, and the
vote is the one piece that is not an expression at all. The language is ten
atoms and three combinators over a stack, plus two atoms that read a pass's
**candidate**; a pass remains a configured instance of an engine-provided kind,
which is the boundary written into the Wave 2 spec (§5.1, §6, §7.2). Eight
findings about v0 came with it, four of them bugs: three flags that can never be
true and cover 11 percent of the corpus, two functions that decide an SWI output
by different rules, a vote whose tie is broken by the database's row order, and
a dead `or` in the synthetic test. The first three are declared differences in
the wave's gate rather than behaviour to reproduce; the fourth is dead code that
costs nothing, since the clause in front of it catches the same values, and
knowing which of the four is which is the point of naming a cause rather than a
class.

### C6, the baseline host

**Recommendation: accept, built in Wave 0; the budget number is restated after
the measurement.** An 8 vCPU / 64 GB VM on Asgard over NFS, v0 deployed as it runs
on the production host, a defined sample corpus digested and timed. The only dependency is a raw
DICOM sample on the storage server, which follows the migration. Nothing in v1 may
claim "a
small machine" before this number exists. **Accepted 2026-09-02 (section 7).**
**Built and measured 2026-09-03**: an 8 vCPU / 64 GB VM on Asgard reading the
corpus over NFS, v0 0.5.3 deployed on it and measured first (507 files/s,
5.78 GiB), then v1 (5,713 files/s, 0.83 GiB) and the gate of §12, which passes
there. The numbers and their caveats are in 11 under slice 8 and in the wave
spec's §12.5.

### C10, what the public corpus may contain

**Recommendation: accept, and make the review a named step of the repo restart
(R7 below).** The live-derived corpus stays on the production host. The public corpus is synthetic
or transformed, and the transformation is reviewed once, in writing, before the
first public commit. The same review covers this folder. **Accepted 2026-09-02
(section 7).**

### C23 and D23, the agent pilot and its time-box

**Recommendation: accept, changed: the pilot is six weeks from the day Wave 4's
MCP server answers.** The exit criteria stand as written in 08; a pilot without a
calendar end is not time-boxed. **Accepted 2026-09-02 (section 7).**

### C24 and D24, transcript retention

**Recommendation: accept, changed: full transcripts are kept 90 days by default,
configurable per deployment; ASTs, result hashes and scores are kept indefinitely
as the `nils-evals` dataset (C25); deletion on request applies to both.** Ninety
days is long enough to debug a pilot and short enough that the chat log never
becomes a shadow registry. **Accepted 2026-09-02 (section 7).**

### D25 to D29, federation

**Recommendation: accept all five as direction, build nothing before Wave 8, and
put the Wave 4 primitives (C26 to C30) on the Wave 4 gate.** D27's defaults: k of
5 for imaging metadata, 10 for clinical entities, complementary suppression on,
rounding off unless a federation's agreement asks for it. C31's executor is built
only when Amsterdam confirms a compute cluster; until then it is the declared seam
it already was. Names wait, as you said. **Accepted 2026-09-02 (section 7); the
defaults confirmed with the rest (section 9).**

## 2. Amendments I recommend accepting as written

All accepted 2026-09-02 as written here, C18 staged as its row says (section 9).

| Id | Proposal in one line | Verdict | Why |
|---|---|---|---|
| C2 | Overrides are provenance-scoped pack overlays, never per selection | accept | the same stack must classify the same for everyone who asks; per-selection overrides break principle 4 |
| C3 | Registry-wide pseudonym key outside the DB; subject codes and CSV linkage maps imported as linkage records in Wave 1 | accept | retrofitting identity is the misery 11 warns about; the D13 specifics above make it concrete |
| C4 | The ten `nils-data` query patterns become AST fixtures in the Wave 4 gate | accept | absorbed into C16, kept as the older, smaller half of the same gate |
| C5 | Staged result versions with commit; grouped items; bulk decisions; emission thresholds per kind | accept | 435k flagged stacks; a queue without grouping and thresholds is unusable on day one |
| C7 | Decisions exportable as labelled datasets; models registered with training provenance | accept | the verified corpus and `nils-evals` both need it, and it costs a schema, not a system |
| C8 | Wave 3 gate = validator-clean tree plus reference selections; main acquisition per session and contrast is a registry fact | accept | v0 exports are not valid BIDS, so they cannot be the oracle; the main acquisition as a fact is what picks (C19) compute |
| C9 | Rootless runtime by default; Docker socket opt-in | accept | D18; also the precondition for a peer ever running our containers (D29) |
| C12 | Two named corpora, v0-parity and verified; the gate reports against both | accept | naming them stops machine output from being mistaken for truth |
| C13 | `off` mode binds loopback only | accept | one line of code, removes a whole class of accidents |
| C14 | Gap filling deterministic against a versioned reference, or replaced by review items | accept | a pass that votes against the whole registry makes sorting one cohort change another; the reference is the ingest batch plus pack version, and weak votes emit items |
| C15 | Decision precedence human > agent > rule; decisions keyed at their scope; re-classification emits new items, never overwrites | accept | otherwise the next sort reverts human work, which is what happens in v0 today |
| C16 | The 28 gold tasks and the ten families are the AST gate across Waves 4 and 5 | accept | the only acceptance test we have that came from real use |
| C17 | The AST adopts Metabase's shape, one external dialect, generated JSON Schema, `ast_version`, repair pass | accept | proven at scale by a team that spent years on it; the JSON Schema is what makes agents reliable |
| C18 | Grain per stage; `nearest`, `within`, `pairs`, `age_at`; set algebra; values sources; identifier projection; derived fields | accept, changed: staged | Wave 4 ships grain, summarize, pick, `within` and `nearest`; `pairs`, `age_at`, set algebra and derived fields ship with the notebook in Wave 5, so the Wave 4 gate does not balloon |
| C19 | Roles and picks as catalog objects; image-type tokens structured; main acquisition = default role set | accept | the one pattern in the traffic that no reference product has, and the one every study needs |
| C20 | Engine-served affordances `options`, `describe`, `preview`, `diagnose`; field records with `sensitive`, `ai_context` | accept | the notebook cannot compute these client-side, and the agent must not guess them; `preview` runs on the bounded sync path |
| C21 | Result handles paged by continuation token; exports and send-to consume handles | accept | the concrete form of D14; every downstream consumer gets one object to name |
| C22 | Bounded sync path beside jobs; MCP tools from endpoint metadata, tools-only, JWT per request, bounded results, query handles; OAuth resource metadata in `oidc` | accept | written to the one client we will use and to any other; the sync cap is what keeps the notebook alive |
| C25 | `nils-evals` from gold tasks and trajectories, versioned with the corpus; fine-tuning waits for it | accept | without it no claim about the agent is measurable; the private data stays on the production host, the harness goes public |
| C26 | Capabilities report pack versions, registry epoch, federation endpoint; handles carry node, epoch, level, suppression | accept | a counter and four fields; the "as of" of every result |
| C27 | Catalog visibility `local` (default for free text, paths, exact dates, identifiers) and `federated` (allowlist) beside `sensitive` | accept | the same sensitivity mechanism D13 needs anyway, one more value |
| C28 | Disclosure projections with suppression, rounding and binning as a generic API feature | accept | useful for students and external readers on day one; the only thing a peer ever sees later |
| C29 | Review-item kinds `federation.request` and `federation.run`, policies per federation, level and entity | accept | two kinds and a policy block; no second approval machinery |
| C30 | Principals `user@node`; `federated-reader` role; signed claims against pinned peer keys; no fourth auth mode | accept | the `known_hosts` model is the one every admin already understands |
| C32 | Subject identity and the pseudonym domain node-local; cross-node linkage only by explicit PPRL | accept | anything else is a linkage we did not agree to |
| C33 | Wave 4 ships the primitives; Wave 7 the executor; Wave 8 federation with the two-node pilot as gate | accept | cheap now, misery later, and the daemon waits for a registry people can query locally |
| C34 | Scope chip and per-node results in the notebook; a second MCP server on the daemon; `nils node` and `--federation` in the CLI | accept | nothing changes for a deployment without a node |

## 3. Missing decisions I recommend accepting as written

All ratified 2026-09-02: D23 to D29 with the first batch (section 7), D14 to D22
with the second (section 9).

| Id | Decision | Verdict | Why |
|---|---|---|---|
| D14 | Staged results and bulk decisions | accept | see C5 and C21; one object, two uses |
| D15 | Labels and models registered with provenance | accept | see C7 and C25 |
| D16 | The BIDS oracle is the validator plus reference selections | accept | see C8 |
| D17 | The walker groups by DICOM tags only, records every path, quarantines refusals as a listed output | accept | live layouts are heterogeneous; the batch root path in the text is our deployment's default, not the product's |
| D18 | Rootless container runtime by default | accept | see C9 |
| D19 | The deployment glue disappears; `nilsctl` is not ported | accept | 13,675 lines with no job left; the private glue repo is created when Wave 4 needs it, not before |
| D20 | Grain and denominators explicit in the AST | accept | the 242/244/271 dispute, settled by construction |
| D21 | Roles and picks are registry facts | accept | see C19 |
| D22 | Results are first-class objects | accept | see C21 |
| D23 | The harness is replaceable and the contract is the product | accept | with the six-week time-box above |
| D24 | Transcripts are inside the custody boundary | accept | with the 90-day default above |
| D25 | Local first, node optional | accept | federation off is the standalone engine byte for byte |
| D26 | The pack is the common data model | accept | vocabulary change is a major; incompatible refuses with a diff |
| D27 | Disclosure levels and safe outputs at the door | accept | k of 5 and 10, complementary suppression on |
| D28 | Federated requests are review items | accept | see C29 |
| D29 | Compute travels, data stays | accept | executor built only on Amsterdam's confirmation |

## 4. What stays open on evidence

Accepting these on 2026-09-02 approved the work; each line closes when its
evidence lands, and the report says what it found.

| Id | Waits for | Owner | By |
|---|---|---|---|
| C1 | the language spike report | me | Wave 0, ten working days from the repo restart |
| C6 | the baseline measurement on the Asgard VM | me | Wave 0, after a sample corpus is on the storage server |
| C11 | ~~the pack-format prototype on the three hardest v0 pieces~~ **done 2026-09-03; the format is data, no code hatch (section 3, C11)** | me | second half of Wave 1 |
| C31 | Amsterdam's answer on what their cluster is | you | an email, any time before Wave 7 |
| Federation pilot | Vienna's answers of 14 §7 | you | before Wave 8 |
| Names | decided 2026-09-02: the name stays NILS, the federation is unnamed (D31, section 10); the trademark search (TMview, PRV, classes 9 and 42) is yours, in a browser, and gates the word-mark filing, not the repository | you | the search: before the filing |

## 5. The repository restart, as you decided it

A new repository, fresh history, you as the only author. My recommendations on
the details, all accepted 2026-09-02 (section 9); v0 is frozen from that day (F1):

| Id | Recommendation | Why |
|---|---|---|
| R1 | Create the new repo under `kineuro` the day the name is decided; until then this record lives in `nils-design` and v0 in `nils_private`. Creation is a minute; the name is the gate. Decided 2026-09-02: NILS (section 10) | a rename later works (GitHub redirects) but the first tag, the first release and the first external link should carry the final name |
| R2 | Fresh history; every commit authored by nima-ch, including the ones Claude Code makes; no trailers, ever; conventional commit messages as already used (`docs(v1): …`) | the rule of 2026-08-31, now the repo's rule from commit one |
| R3 | The public `kineuro/nils` mirror (14 commits, all yours, 0 stars, 2 forks, last push 2026-06-11) is renamed `nils-legacy` and archived, not deleted | frees the name if you want it, keeps the public record for the two forks, costs nothing; v0's real home stays `nils_private`, the NeuroGranberg archives stay as they are |
| R4 | `main` protected: pull request and green CI required for code, linear history (rebase), direct commits allowed for `docs/` only; tags `v1.0.0-alpha.N` onward; releases like Bifrost (tag builds binaries, checksums, changelog section) | solo development still wants a green main; the docs exception keeps the decision record cheap to amend |
| R5 | Layout: the engine only (`crates/` or `cmd/` after C1), `packs/mri/`, `contracts/` (Apache-2.0, its own LICENSE), `docs/decisions/` (the scrubbed copy of `nils-design`), `spikes/`, `evals/` (public harness), `.github/workflows/` | one repo per product (D10); apps arrive with their waves, never as directories here |
| R6 | Files at commit one: `LICENSE` (AGPL-3.0-only, decided 2026-09-02, with Apache-2.0 `LICENSE` files in `contracts/` and the SDK directories, SPDX headers in every source file), `README` (the three sentences and "pre-alpha; v0 lives in nils_private"), `CONTRIBUTING` (CLA for the engine, DCO for the contracts, per 10), `TRADEMARKS.md`, `SECURITY.md` (a group address for reports), `CODEOWNERS` (you), `CHANGELOG.md` (keep-a-changelog), minimal issue and PR templates, `.editorconfig` | the standard set for a project that wants to be a standard; each is a page |
| R7 | A copy of this record lands as `docs/decisions/` after a **scrub pass** with a written list: the live system's exposure details in 12 §2 and §5 (D13's premise paragraph is rewritten to state the v1 requirement, not what the production host holds today), host specifications, network topology, storage and source paths, anything that overlaps the admin SOP. The C10 corpus review runs in the same pass. `nils-design` stays the full record (moved there 2026-09-02, history intact) and never goes public: a repository that flips to public takes its whole history with it, scrubbed or not | the folder was written for the team; a public repo is read by strangers, and a description of a live clinical system's internals is not something to publish about ourselves |
| R8 | CI from commit one: format, lint and tests on every pull request; release on tag; the benchmark harness job added when the spike settles the language | a gate that exists from the start is never "added later" |
| R9 | `nils-deploy` (private glue, D19) is created when Wave 4 needs a compose file, not now; `nils-query`, `nils-segment`, `nils-agent` are created with their waves | empty repos are promises nobody keeps |
| F1 | v0 on the production host is feature-frozen from today: bug fixes only, in `nils_private`, tagged | it is the oracle (principle 10); every change to it moves the target of Waves 1 to 3 |

## 6. Order of work

1. Done 2026-09-02: the register statuses flipped and the texts amended, one
   commit per batch of verdicts in `nils-design`.
2. Done 2026-09-02: the name, NILS (section 10); the mirror archived as
   `nils-legacy` (R3); `kineuro/nils` created per R1 to R8, with the scrub pass
   (R7) and the corpus review (C10) as its second commit (section 11).
3. The spike (C1) and the baseline VM (C6) start the same day; ten working days
   later D2 closes and the CI skeleton follows.
4. Wave 1 opens by writing its spec: registry schema on both backends, ingest
   batches, the linkage store of D13, the pseudonym scheme and the identity rule
   (C36, C37), the walker of D17, `nils digest` with resume, and the
   parse-and-compare gate against v0 in production.

## 7. Verdicts, 2026-09-02

Nima answered the first batch the same day, in a message that also raised four
items the sheet did not have (section 8).

| Item | Verdict | What changes |
|---|---|---|
| C1 | accept | the spike judges performance and maintainability together: files/s, RSS and vendor failures beside lines of code for the same parse path, dependency count, build time and how each library treats vendor deviations; "as fast as possible, with the right compensation" is the criterion, and the report says what was traded |
| C6 | accept | the baseline VM is built in Wave 0 |
| C10 | accept | the corpus review is a named step of the repo restart (R7) |
| C11 | accept | the format is decided by a prototype before Wave 2, with a second criterion: adding a modality (CT, PET, a new MRI technique) has to be easy for us and need no code from a user; where the axes differ (CT has no base contrast) the vocabulary finds common ground or the modality gets its own axes on the same flow; users tune detection through knobs (C37), never through code |
| C23, D23 | accept | Flue pinned, seams owned, six-week pilot |
| C24, D24 | accept | transcripts inside the custody boundary, 90-day default |
| D25 to D29 | accept | federation as direction; nothing built before Wave 8 |

Still open after this batch were the license (10), D13 (with the revision C35
makes to my own recommendation), C2 to C5, C7 to C9, C12 to C22, C25 to C34, D14 to
D22, the repository restart R1 to R9 and the freeze F1; all were accepted later the
same day (section 9).

## 8. Raised with the verdicts

Four things Nima stated that the sheet did not have. Each is written into the doc
it changes and registered here with my recommendation; he accepted all four,
wording included, later the same day (section 9).

### C35, birth dates and the clinical timeline stay in the registry (amends D13; 03, 12 §5)

My D13 recommendation above said "birth date becomes age at study" and "the
migration strips birth dates". Nima's answer: the registry needs them, and the
clinical layer around them is integral. He is right, and I was solving the wrong
problem. v0's registry is not an image index with a subject table attached: it
carries demographics (`patient_birth_date` on nearly every subject, sex), identifiers by
type, eight seeded diseases with their subtypes (the MS courses, the ALS onsets, the
Parkinson phenotypes), typed events (diagnosis, disease onset, SP transition,
treatment, scans, anthropometrics) and measures (EDSS, SDMT, FSMC, FSS, HADS, MSI),
each entered through the UI or imported with a preview-then-apply importer, and
Nima filled them subject by subject. Every question in 13 §2 that has a time in it
(age at session, nearest scan to an event, within a window of a treatment) runs on
that layer, and age at any event added later needs the birth date, not an age
frozen at one study.

So the rule is not absence but class. Direct identifiers (name, the source
PatientID, any other identifier) stay out of the registry proper and live in the
linkage store with their ID type. Quasi-identifiers (birth date, sex, exact event
dates) and clinical data (diagnoses, measures, treatments) are registry fields,
classed as such, visible to the roles that need them, filtered from review-item
evidence, MCP responses and federated results by class (C27, C28), and shifted or
dropped at export by the anonymizer, never in storage. The migration keeps birth
dates and drops names unless a name field carries an identifier, in which case it
moves to the linkage store. **Recommendation: accept.** Accepted 2026-09-02.

### D30, the clinical timeline is core registry (new; 03)

Stated by Nima on 2026-09-02 and recorded as a decision: subjects carry
demographics, identifiers by type, diseases and subtypes, typed and dated events,
and measures; one declarative importer (map, preview, validate, apply) and the UI
edit them; and everything later (queries with `age_at`, `nearest` and `within`,
cohorts, pipeline inputs, federated aggregates) may depend on them. A deployment
that only sorts and bidsifies leaves the layer empty, and the engine never requires
it to be filled; it is the thing that makes everything after sorting possible.
**Ratified**, on his words. The vocabulary seeds (diseases, subtypes, observation
types) become part of the pack's vocabulary so that a second site starts with the
same words (D26).

### C36, the subject code keeps its key (amends D13, C3; 02, 03)

v0 derives a subject code as a keyed BLAKE2b digest of the source PatientID,
eight bytes, hexadecimal, with a StudyInstanceUID fallback when the ID is missing
and an optional CSV of fixed codes on top (`extract/subject_mapping.py`). The key
is typed into the extract stage form per digest and saved with the digest's
configuration; left empty, the cohort name is the key, so the same person gets a
different code in every digest, which is the trap the fixed key exists to avoid.
Nima has used one key throughout so that a person who comes back with the same
personal number gets the same code, and those codes are in every BIDS tree,
export and collaboration that exists.

v1 keeps them. The registry declares a **pseudonym scheme** once, at creation: the
algorithm and a key reference. `blake2b-8` is a built-in scheme, byte for byte
v0's, and the KI registry is created with it and the v0 key, so every existing
subject keeps its code and every new session of a known person lands on the known
subject. New registries default to the stronger scheme of D13 (full digest,
display code, collision detected at insert); a 64-bit digest is comfortable at
registry scale, so the KI registry loses nothing by staying. The key lives in the
engine's key store (a file, mode 600, or a KMS), is referred to by name, and is
never in a digest's configuration, in the database, in a document or in a chat.
The per-digest seed field goes away. Re-keying means re-deriving from the sources
and is written down as exactly that expensive, which is why the key is also backed
up under the same custody. **Recommendation: accept.** Accepted 2026-09-02. The
key itself appears nowhere in this record. *Spec review, 2026-09-02:* the Wave 1
spec's first draft named two keys, the pseudonym key and a separate linkage-store
key; Nima closed it to one. A registry has one key, set by the user, and for the
KI registry it is the v0 key; the linkage store's lookup and encryption subkeys
are derived from it (Wave 1 spec §7.2), so there is one secret to set, back up
and guard.

### C37, every knob is a contract, and the agent sits beside each step (amends D1, D7, D12, C2; 00, 01, 04, 03)

D1 says the engine discovers nothing, and Nima confirms it, with the other half
stated: at every step that judges, the engine exposes its knobs, so that an agent,
when one is present, can tune them and the step runs again. His examples: a digest
from Italy where "senza mdc" means no contrast, so the keyword set for that digest
is tuned rather than the rule rewritten; a source whose PatientName holds an
identifier and a date, where the engine should recognise the pattern, ask for the
ID type, and keep the date apart so that one person across sessions stays one
subject.

The amendment: every judging step (subject identity resolution, classification,
gap filling, anonymization, QC) declares its knobs as data, scoped to the digest
(C2's provenance-scoped overlays are the classification case; the **identity
rule**, which fields identify a subject, how they are parsed and under which ID
type, is the ingest case, and v0's `subjectIdTypeId` plus CSV mapping is its seed),
editable in the UI and the CLI and served through the affordance API (C20) as
`describe` and `diagnose`. Without an agent the engine runs the step, writes a
diagnostics report (conflicts, missing values, low-confidence decisions, identity
fields it could not parse) and stops there, which is v0 today, made legible. With
an agent, the agent reads the diagnostics after the first pass, proposes knob
changes as review items (D7: proposals with evidence, never silent edits), and the
step re-runs under the accepted proposal. **Recommendation: accept**; this is D1
read correctly, and it is what "intelligence available at every decision point,
and required at none" in 00 means in practice. Accepted 2026-09-02.

### C38, custody is visible (amends D13, D24; 02, 05, 08)

Nima's requirement in his words: be very honest, and let the user know where
things live, for how long, and how they can manage, change or take control. The
amendment: `nils custody` in the CLI and a Custody page in the UI list every store
the deployment writes (registry, linkage store, key store, staged results and
result handles, transcripts, caches, logs, the anonymization audit, backups the
engine knows about), and for each: where it lives, which classes of data it holds,
how long it is kept, and the command that changes the retention, exports it, or
deletes it. The public documentation carries the same table for a default
deployment, and the rule is that nothing is retained that the page does not list.
**Recommendation: accept.** Accepted 2026-09-02.

## 9. Verdicts, 2026-09-02, the rest

Nima's second answer, the same day, in full: "I accepted your suggestion on
license. totally. so that settles it down. also the rest. all is accepted and
ok." Recorded item by item, so that nothing later rests on a reading of "the
rest":

| Item | Verdict | What it settles |
|---|---|---|
| License (10) | accept | AGPL-3.0-only for the engine, the apps, the relay and the first-party packs; Apache-2.0 for the contracts and the client libraries; CC-BY-4.0 for the documentation; a CLA on the AGPL repositories and a DCO on the Apache ones; the name registered as a word mark; sell operation, not science; KI's copyright position settled before the first commercial license. v0 stays GPL-3.0 |
| D13 | accept, as amended | the specifics of section 1 with C35 and C36 written in: direct identifiers only in the linkage store; quasi-identifying and clinical fields in the registry under class; the pseudonym scheme declared per registry, `blake2b-8` with the v0 key for the KI registry; the migration keeps birth dates |
| C2 to C5, C7 to C9, C12 to C15 | accept | as written in section 2 and [12](12-review-devils-advocate.md) §6 |
| C16, C17, C19 to C22, C25 | accept | as written in section 2 and [13](13-query-and-agent-study.md) §6 |
| C18 | accept, staged | grain, summarize, pick, `within` and `nearest` in Wave 4; `pairs`, `age_at`, set algebra and derived fields with the notebook in Wave 5 |
| C26 to C34 | accept | as written in section 2 and [14](14-federation.md) §6; C31's executor is built only when Amsterdam confirms a compute cluster |
| C35 to C38 | accept | the four items of section 8, wording included |
| D14 to D22 | ratified | as written in section 3 |
| D30 | ratified | confirmed |
| R1 to R9 | accept | the repository restart as recommended in section 5; R6's `LICENSE` is AGPL-3.0-only |
| F1 | accept | v0 on the production host is feature-frozen from 2026-09-02: bug fixes only, in `nils_private`, tagged |
| D27 defaults | accept | k of 5 for imaging metadata, 10 for clinical entities, complementary suppression on, rounding off unless an agreement asks for it |

With this, every id from C1 to C38 and D1 to D30 has a verdict, and the record
speaks with one voice. What remains waits for evidence or for other people, not
for a decision: the three reports of section 4 (the spike, the baseline, the
pack-format prototype, all approved work), Amsterdam's answer on its cluster
(C31), Vienna's answers of [14](14-federation.md) §7, and the name (R1), which
was the gate of the repository restart and was decided later the same day
(section 10).

## 10. The name, 2026-09-02

Asked what the name gated, I explained that "the name" of R1 is the product name
of the rewrite: the repository `kineuro/<name>`, the command, the crate or module,
the container image, the word mark. I recommended keeping NILS and withdrawing
Yggdrasil as the federation's name. Nima's answer, verbatim:

> yes we keep nils and keep it simple on fedaration. proceed with first step.
> I grant you all permission you need

| Id | Decision | Ratified |
|---|---|---|
| D31 | The rewrite keeps the name NILS (Neuroimaging Intelligent Linked System, as v0 spelled it out). The federation has no name of its own: "the federation" in prose, `nils node` and `nils-relay` in code. Yggdrasil is withdrawn as a federation name because Yggdrasil Network is an existing open-source mesh network ([14](14-federation.md) §7) | 2026-09-02 |

The checks that went into the recommendation, all made 2026-09-02: no
neuroimaging, DICOM or BIDS tool named NILS exists; `nils` is free on crates.io
and on PyPI and taken on npm (a JavaScript client would be `@kineuro/nils`);
`kineuro/nils` on GitHub is our own stale mirror, freed by R3; `nils.org`,
`.io`, `.dev`, `.se` and `.app` are all registered, and the product lives at
`nils.kineuro.se` in any case. The trademark search (TMview for the Union, PRV
for Sweden, "NILS" in classes 9 and 42) is Nima's step in a browser, since both
services refuse non-browser clients; it gates the word-mark filing of
[10](10-repos-licensing.md), not the repository. Nima's direction for the logo,
recorded in 10, is not a decision yet.

"First step" is item 2 of section 6: the repository, per R1 to R8, with the
mirror archived (R3). What was done is recorded in section 11. Next ids: C39
and D32.

## 11. The repository, 2026-09-02

Done the same day, every commit authored by nima-ch without trailers (R2).

| Step | What was done |
|---|---|
| R3 | The v0 mirror `kineuro/nils` got an archival notice at the top of its README (commit `2bac175`), was renamed `kineuro/nils-legacy` and archived. Its 2 forks and its GitHub Pages branch stay with it; the Pages URL moved with the name and is not redirected |
| R1, R2 | `kineuro/nils` created public with fresh history. Commit one `2e227eb`, "chore: commit one, per the design record (R1 to R8)". Local clone at `~/Projects/nils-v1`; `~/Projects/nils` holds Nima's old clone of the v0 upstream (uncommitted changes in it) and was left untouched |
| R6 | `LICENSE` (AGPL-3.0-only), `contracts/LICENSE` (Apache-2.0), `docs/LICENSE` (CC BY 4.0), `README.md`, `CONTRIBUTING.md`, `CLA.md` (an adaptation of the Apache ICLA 2.2, to be read by a lawyer before the first external pull request), `TRADEMARKS.md`, `SECURITY.md` (admin@kineuro.se; private vulnerability reporting on), `CHANGELOG.md`, `.github/CODEOWNERS`, `.editorconfig`, issue and pull request templates |
| R5 | `contracts/`, `packs/mri/`, `spikes/lang/` (the C1 question and its four criteria, written before the work), `evals/`, `docs/decisions/`, `.github/workflows/` |
| R8 | `ci.yml` (text hygiene, SPDX headers, DCO on commits that touch `contracts/` or `sdk/`; the job `ci` is the required check), `release.yml` (a `v*` tag publishes the matching changelog section as the release), `cla.yml` (CLA Assistant Lite; signatures on the orphan branch `cla-signatures`; skipped for pull requests that touch only `contracts/` and `sdk/`). First run on main: green |
| R4 | Ruleset "main": pull request required, linear history, no deletion, no force push, status check `ci`. Repository admins bypass it, which is how `docs/` gets its direct commits (CONTRIBUTING says so). Squash and rebase merges only, head branches deleted on merge, wiki and projects off |
| R7, C10 | `docs/decisions/` is the scrubbed copy of this record, commit `bb98a0b`, produced by `scripts/scrub.py` in this repository (every substitution asserts its count; the pass fails if a removed term survives). Its `SCRUB.md` lists the removals and records the corpus review: no corpus and no fixture at commit one, and the transformation behind the first one is written down before it lands |
| org profile | The NILS row of `kineuro/.github` points at v1 and its design record, and at `nils-legacy` for the version in production |

The `TRADEMARKS.md` claim ("registration is in progress") waits on Nima's search
(section 10). Item 3 of section 6, the spike and the baseline VM, starts next.
