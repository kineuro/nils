# 10 — Repositories, releases, licensing (D10)

## The repo model

One repo per product, each with its own version, changelog, and releases. The
private-superset-plus-public-mirror pattern is retired: it doubled release
mechanics, left the public repo stale, and severed the history and issue continuity
a standard needs.

| Repo | Visibility | Contents |
|---|---|---|
| `nils` | public, developed in the open | the engine: core, CLI, server, web UI, contracts, MRI pack, starter pipelines, docs; the node daemon (`nils node`, 14) and the federation agreement template in `docs/` |
| `nils-relay` | public, only if a member cannot join a mesh | the blind store-and-forward relay of [14](14-federation.md) §3.3: a few hundred lines that see metadata only |
| `nils-segment` | public when ported | the annotation app |
| `nils-query` | public when built | the notebook |
| `nils-agent` | private until stable, then public | the Flue app |
| `nils-design` | private, permanent | this record: decisions, reviews, ratification, the page builder, `scripts/scrub.py`; the public engine repo carries the scrubbed copy it produces as `docs/decisions/`, with the removals listed in the copy's `SCRUB.md` |
| `nils-legacy` | public, archived 2026-09-02 | the v0 mirror, formerly `kineuro/nils`: 14 commits and 2 forks, read-only (15 R3) |
| deployment glue | private (ours) | our compose files, Authentik/Traefik config, site specifics |

Development happens on the public repos directly — branches and tags are the
dev/stable split; the corpus fixtures that would leak anything clinical stay out by
construction (synthetic and fully-anonymized fixtures only; the live-DB regression
harness runs on the production host against the private data and reports pass/fail upstream).

`nils_private` remains the v0 home until decommissioning, feature-frozen since
2026-09-02 (F1 of [15](15-ratification.md)): bug fixes only, tagged. This record
lived there as `docs/v2/` until 2026-09-02, when it moved to `kineuro/nils-design`
with its history; `nils_private` keeps a pointer. The private record and this public scrubbed copy
are the one allowed exception to the retired private-superset pattern, and the
reason is not code: the record describes the live system in a detail that belongs
to its operators. What this copy leaves out is listed in `SCRUB.md`.

## Releases

The engine releases like Bifrost: tag → CI builds the six static binaries +
checksums → release with changelog section → installers serve latest. Apps release
independently on their own cadence. Contract versions
([05-contracts.md](05-contracts.md)) are declared, never inferred from product
versions.

## Licensing: decided 2026-09-02

v0 is GPL-3.0, chosen for two reasons Nima stated on 2026-09-02: NILS is meant to be
the best tool of its kind and smaller, weaker projects are already being
commercialised, so a permissive license invites a company to take it and charge for
it; and we may want a company of our own later, on the Metabase pattern (an open
community edition and a hosted edition with more). Both reasons are legitimate. The
analysis below replaces the Apache-2.0 recommendation of [15](15-ratification.md)
§1, which was written before the second reason was on the table. Nima accepted
the recommendation the same day; the table under "The decision" is the license of
every repository from its first commit.

### What each option does, against the two reasons

| Option | Stops a vendor embedding NILS in a closed product | Stops a hosted, modified NILS with the changes kept closed | Stops hosting the unmodified engine for money | Open source by the OSI definition | Keeps our own commercial option |
|---|---|---|---|---|---|
| Apache-2.0, MIT | no | no | no | yes | yes, and everyone else's too |
| GPL-3.0 (v0) | yes | **no**: hosting is not distribution, so the copyleft never fires | no | yes | yes |
| AGPL-3.0 | yes | yes (section 13: network use triggers the source offer) | no | yes | yes, with the ownership rules below |
| BSL, SSPL, FSL, Elastic License (source-available) | yes | yes | yes | **no** | yes |

Three facts decide it:

1. **GPL-3.0 does not do what it was chosen for.** A company can modify NILS, run
   it as a service, charge for it and publish nothing, because no copy is ever
   distributed. AGPL-3.0 closes exactly that door and is otherwise the same
   license; it is compatible with GPL-3.0 code and with every dependency we plan
   on (DuckDB MIT, SQLite public domain, dicom-rs MIT/Apache, dcm2niix BSD, Flue
   Apache-2.0, read from its LICENSE in the clone).
2. **No open-source license forbids charging, and we do not want one that does**:
   we intend to charge for hosting ourselves. The principle we actually want is
   "whoever builds on it shares back; nobody but us ships a closed version", and
   AGPL is that sentence as a license. The one thing it leaves open, hosting the
   unmodified engine for money, is closed only by the source-available licenses,
   which are not open source, are kept out of Debian and Fedora, and turn "open
   source" on a grant application or a hospital procurement form into a
   discussion. For a clinical-research engine the realistic threat is a vendor
   embedding the classifier in a proprietary product, not a hyperscaler hosting
   the engine; AGPL stops the realistic threat. Against the other, the defences do
   not come from the license: the name (below), the private corpus that keeps our
   packs better than any fork's, and federation itself, since a node joins a
   federation by agreement between institutions and a closed fork does not get
   one.
3. **The door swings one way.** With every commit ours, AGPL today can become
   Apache tomorrow in one commit. Apache today can never become AGPL for anything
   already released, and the forks continue on the old terms: Elasticsearch and
   OpenSearch, Terraform and OpenTofu, Redis and Valkey are the 2021 to 2024
   record of tightening after the fact. Take the strict door while the copyright
   is in one pair of hands; loosen when adoption asks for it, not when the fear
   has passed.

### The decision

| What | License | Contributor terms | Why |
|---|---|---|---|
| The engine (`nils`, with `nils node`), nils-query, nils-segment, nils-agent, nils-relay, the first-party packs | AGPL-3.0-only, `SPDX-License-Identifier: AGPL-3.0-only` in every source file | CLA | copyleft that survives hosting; the CLA keeps the commercial option |
| The contracts others are meant to implement: the AST JSON Schema, the pack format specification and the vocabulary, the OpenAPI document, the MCP tool schemas, the federation protocol, `nils.job.yml` | Apache-2.0 | DCO | a standard has to be freely implementable, including by commercial software; a rival implementation of the contracts is the standard winning |
| Client libraries and SDKs, anything a third party links into a program of their own | Apache-2.0 | DCO | so a hospital's proprietary tool can talk to a NILS server without a legal review; the Grafana pattern, server copyleft and libraries permissive |
| Public documentation, the federation agreement template | CC-BY-4.0 | DCO | the usual choice for text |
| This record | none; private | | |

`-only` rather than `-or-later`: we decide our future terms; a hypothetical AGPL-4
does not. The packs stay with the engine because rules are executable knowledge and
are the part a vendor would most like to lift; the vocabulary is free so that the
words become the field's.

### What the commercial option needs, and needs now

The Metabase pattern, read from its source (`~/Projects/ref/metabase`):
`LICENSE-AGPL.txt` for the code, `LICENSE-MCL.txt` (the Metabase Commercial License)
for the top-level `enterprise/` directory of the same repository, a CLA from every
contributor, one build with the paid features unlocked by a token. What makes it work
is not the directory but the ownership: Metabase can sell a commercial license only
because it holds the rights to all of the code. Hence:

- **Copyright in one pair of hands.** Every commit is Nima's (15, R2), which
  settles today. Outside contributions to the AGPL repositories come under a
  Contributor License Agreement (the Apache ICLA text, which grants the right to
  sublicense, administered by the CLA-assistant app on each pull request); the
  Apache-2.0 repositories use the Developer Certificate of Origin, which is enough
  where nothing will ever be relicensed. This replaces "DCO, never a CLA" in 15
  R6: a DCO grants no right to offer the code under other terms, and a single
  un-relicensable contribution in the engine is a veto on the commercial edition.
- **Whose copyright.** Under section 40a of the Swedish Copyright Act the copyright
  in a computer program written by an employee as part of their duties passes to
  the employer unless otherwise agreed; the teacher's exemption of the 1949 Act on
  employee inventions is about inventions, and Swedish universities extend it to
  software by policy, each in its own words. What KI's policy says about NILS, and
  what KI Innovations expects of a spin-out, is settled before the first commercial
  license is signed, because the licensor has to be the owner. It does not have to
  be settled before the first commit: the AGPL grant is valid whoever the owner
  turns out to be.
- **The name.** A license cannot stop anyone from hosting the unmodified engine; a
  registered word mark stops them calling it NILS. **Decided 2026-09-02: the name
  stays NILS** (D31, [15](15-ratification.md) §10). The checks behind it: no
  neuroimaging, DICOM or BIDS tool of that name exists; `nils` is free on crates.io
  and on PyPI (the npm name is taken, so a JavaScript client would be
  `@kineuro/nils`); the GitHub name is ours (the old mirror became `nils-legacy` on
  2026-09-02, 15 R3); the plain `nils.*` domains are all registered, and the product lives at
  `nils.kineuro.se` regardless. NILS is a common given name and a common acronym,
  which was the strike against it; a common word is still registrable as a word
  mark for software (class 9) and software services (class 42) when nobody holds
  it there, which is what the search has to show. The search (TMview for the EU,
  PRV for Sweden, both refuse non-browser clients) is Nima's step in a browser; it
  gates the filing, not the repository. Register the word mark (PRV for Sweden,
  EUIPO for the Union) once the search is clean. `TRADEMARKS.md` (nominative use
  allowed; no product or service names derived from the mark) is in the repository
  from commit one (15 R6).
  Logo, Nima's direction of the same day, not yet a decision: a mark in the
  family of the Bifrost mark with an N inside it, or an old Nordic character for
  the sound; he named the rune himself, ᚾ, Naudiz in the Elder Futhark (nauðr in
  the Younger; the name means "need"). The mark is drawn when the website page
  for NILS is.
- **Where the edge lives.** Sell operation, not science. The classifier, the query
  language, the packs and the federation primitives are the open edition, always;
  the hosted multi-tenant service, the managed relay and federation operations,
  SSO/SCIM and audit exports for institutions, support and certification are what
  an institution pays for. That split keeps forks pointless and customers honest,
  and it matches the architecture: the engine is complete alone (D1) and
  everything else is an app on public contracts.
- **A clean implementation.** No code copied from Metabase, Flue or any other
  project whose copyright is not ours: ideas, yes; lines, no. Dependencies come
  through package managers under permissive or compatible licenses; an AGPL
  dependency is allowed but listed, because that part can never be dual-licensed.
  v0's engine can be relicensed freely (every commit is Nima's, no vendored code);
  v0's agent carries the DeerFlow fork (MIT, ByteDance) without its LICENSE file,
  which is a notice to restore if v0 is ever distributed again, and none of it
  carries over (nils-agent v1 is a Flue app, 08).

Decided by Nima on 2026-09-02, as recommended ([15](15-ratification.md) §9). The
public repository's `LICENSE` is AGPL-3.0-only from commit one, with Apache-2.0
`LICENSE` files in `contracts/` and the SDK directories and an SPDX header in
every source file (15, R6).
