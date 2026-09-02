# 00 — Vision and principles

## What NILS v1 is

NILS v1 is the reference tool for turning raw DICOM into curated, classified,
anonymized, BIDS-organized data — at any scale from a handful of sessions on a laptop
to a registry of tens of millions of instances — with intelligence available at every
decision point, and required at none of them.

On names: **v0** is the 0.x line running in production today (0.5.3); it never reached 1.0
and this record treats it as the ground, not the product. **v1** is the rewrite
described here, released as 1.0.0. There is no v2.

Three sentences define the product:

1. **`nils` is a single binary** you install like any good CLI tool. It digests,
   classifies, anonymizes and bidsifies without a database server, a container
   runtime, a browser, or a network connection.
2. **The same engine scales up** — as a server it carries the multi-user registry,
   the web UI, the review workflows, and the contracts every other app builds on.
3. **Everything else is optional.** The query notebook, the segmentation app, the
   agent: each adds a capability when present and takes nothing away when absent.

A fourth, from [14](14-federation.md), ratified 2026-09-02 (D25): **the same engine can
be a node.** Two groups each running NILS can connect their engines; a question
asked in one place then reaches both, and only what each side allows comes back,
because individual-level data never leaves a node unless its owner says so. This is
optional twice over: off by default, and adding a node is adding a line of trust.

The ambition is explicit: the tool the field remembers as the gold standard for this
job, in the age where curation decisions can be assisted by agents. Gold standards
are won on three fronts at once — being the fastest and most correct, being trivially
adoptable (one binary, open development, a contract others can extend), and being the
first tool whose *judgement points* were designed for AI participation rather than
retrofitted.

## The ten principles

Every design argument in this folder should end at one of these. When a proposal
fits none of them, that is the signal to re-examine it.

1. **The engine stands alone** (D1). No feature of the core pipeline may depend
   on another app existing. Optional apps discover the engine; the engine discovers
   nothing. The other half (C37): every knob of a judging step is data the engine
   exposes, so an agent, when one is present, can tune it and the step runs again.
2. **Absence is silence, not failure.** A deployment without the agent has no agent
   tile, no agent endpoints, no agent errors. Capability discovery over configuration
   flags wherever possible.
3. **One governed door.** Data questions go through the query AST; work goes through
   jobs; judgements go through review items. UI, CLI, and agents are three clients of
   the same doors — never three implementations. A request from another node is a
   fourth client of the same doors, asked from outside and answered under the
   same policies ([14](14-federation.md), D28).
4. **State lives in the registry; steps are idempotent set operations.** A stage's
   input is a predicate over the database, never a list carried in memory or JSONB.
   Everything is resumable because nothing is in flight.
5. **Bounded memory, small machine first.** v0 was tuned for write stability on
   its production host, a large shared server (Postgres parallelism disabled
   "during extraction", 100-row commits, a 48 GB database cap), and has never been
   measured on a small machine.
   v1 makes an 8-core/64 GB host the design target and Asgard the bonus, with the
   budget enforced in CI (D6) and a measured baseline first
   ([12](12-review-devils-advocate.md), C6).
6. **The science is data, not code.** Classification rules are YAML packs, queries
   are ASTs, pipelines are descriptors, QC weights are files. Code executes;
   knowledge is declarative, versioned, diffable, and contributable.
7. **Buy the platform, build the product.** Identity is OIDC (Authentik here), the
   gateway is a stock reverse proxy, the agent harness is maintained by someone else
   (Flue). We spend our maintenance budget on DICOM, not on SSO cookies. This is the
   single largest lesson of v0.
8. **Every automated judgement leaves evidence** (D7). Whether a rule, a model,
   or an agent decided, the decision carries its evidence and lands in an auditable
   queue with a policy that says who may confirm it.
9. **Open development** (D10). Public products live in public repos with real
   history. Trust in a standard is built in the open.
10. **The live system is the oracle.** Production runs v0 against 37.5M real instances with
    518k classified stacks. Every v1 stage ships only after it reproduces (or
    knowingly improves on) v0's output on that corpus. We never lose this asset by
    decommissioning it early.

## What we are explicitly not building

- Not a PACS, not a DICOM router, not an archive. NILS reads sources and writes
  curated outputs; it does not want to be where data lives.
- Not another auth system, portal framework, or agent harness.
- Not a workflow engine for arbitrary science — pipelines are containers under a
  contract (D9), and the contract is the field's, not ours.
- Not a data-sharing platform. Moving individual-level data between sites is a
  transfer under an agreement, and Bifrost does it; federation
  ([14](14-federation.md)) moves questions, containers, counts and aggregates, and
  nothing else by default.
