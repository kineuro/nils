# 09 — Pipelines as plugins (D9)

## What v0 got right

The foundation is sound and carries over: declarative descriptors, containers as
the execution unit, selection → materialized BIDS tree → work units → derivatives +
`results.json`, content-addressed descriptor versions, docker/apptainer runtimes.
Real N4 ran on real ALS data in development (the live application database holds
zero pipeline runs); segment's prep consumes the contract. Output registration was
deferred in v0 (`INGEST_DB_WRITE_ENABLED = False`, and intentionally no result
cache), so v1 has to build it, not strengthen it: every derivative is a registry
row with a content hash and its producing run. v1 strengthens the contract rather
than replacing the idea.

## Adopt the field's contract

The gold-standard move is to stop having a private container format: the v1 runner
speaks **BIDS-Apps** natively (a container that takes a BIDS tree and a
participant/session scope and writes derivatives). With that, fMRIPrep, QSIPrep,
FreeSurfer wrappers and the whole existing ecosystem are NILS pipelines on day one,
and our own starter catalog (dcm2niix → N4 → brain-extraction → pre-segmentation)
ships as ordinary well-behaved citizens of the same contract.

`nils.job.yml` remains — as the **metadata layer**: image + pinned digest, the
BIDS-Apps invocation shape, resource needs (GPU class, memory), and everything the
raw contract cannot say (below). A plain BIDS App with no descriptor still runs
with defaults; a descriptor makes it a first-class, tunable, QC-aware plugin.

## Tunable means declared

The descriptor declares each parameter: name, type, range/enum, default, unit,
human description. From that single declaration the engine derives the UI form, the
JSON Schema agents read through MCP, CLI flags, and — non-negotiable — **full
parameter provenance on every run**. A run you cannot reproduce from its record is
a bug in the contract, not in the pipeline.

## QC hooks: where pipelines meet review items

The descriptor may declare QC outputs: named metrics the pipeline emits per unit in
`results.json` (SNR, registration cost, mask volume, custom scores) with optional
thresholds. The engine turns threshold breaches — and unit failures — into **review
items** (D7) carrying the metrics and artifact refs as evidence. That is the
whole "agentic decision point" story for pipelines: a registration-QC agent is a
policy on `pipeline-qc` items plus a skill for reading the evidence — zero new
machinery per pipeline, which is the test that the base was envisioned correctly.

## Scheduling, honestly scoped

v1.0 runs pipelines through the engine's job system on the local runtime
(docker/apptainer), with GPU allocation as a first-class resource lease in the job
scheduler (v0's known debt). Remote executors (SLURM and friends) are a declared
seam on the same job model — designed for, not built, until a real deployment needs
one. A real deployment now may: the Amsterdam collaboration described its setup as
a cluster, and in the compute reading of that word the seam gets its first
implementation, a **SLURM executor over Apptainer** (C31 in
[14](14-federation.md)). It is rootless by construction (D18 holds
without exceptions), leases GPU partitions the way the local scheduler leases
devices, stages the materialized BIDS tree on the cluster's shared filesystem, and
submits one array job per unit set; the engine sees the same job states it sees
locally. Wave 7 is the slot.

## Compute travels, data stays

D29 in [14](14-federation.md), ratified 2026-09-02. The same run can execute at another
node: a digest-pinned image, a `nils.job.yml`, a selection AST, sent as a
`federation.run` review item and approved under that node's policy. Derivatives
are registry rows at the node that ran it; what returns is the declared QC
metrics and aggregate outputs, as disclosure projections. Two rules make it safe.
A node runs only images on its allowlist or signed by a publisher it trusts, so
the "train" of the Personal Health Train never carries unknown code; and a
descriptor declares which outputs are aggregates fit to return, so a pipeline
cannot smuggle rows out in `results.json`. Federated learning (an aggregator node
and rounds of runs referencing one run id) and site harmonisation (per-site
statistics as `aggregate` requests feeding a pipeline) are later pipeline kinds on
this spine; the hook is the run id, and nothing is built for them now.

## Independence

Pipelines are optional twice over: the engine core pipeline (digest→bids) never
requires the analysis-pipeline subsystem, and each pipeline is a plugin the
deployment chooses. Absence of a container runtime disables the subsystem with a
capability flag, not an error — per D1.
