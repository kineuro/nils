# 02 — The engine core

## Language: a compiled Rust core (D2)

The stated targets — one dependency-free installable binary, and the fastest
anon→digest→classify→bidsify there is — describe a compiled language. v0's process
pools, pickled per-subject batches, and adaptive batching are all machinery built to
work around per-file parsing in interpreted Python; v1 removes the cause instead of
managing the symptom.

- **Rust**, with `dicom-rs` for metadata parsing. The group's compiled-language
  prior art is Go (Bifrost's CLI); `dicom_reorganizer` holds only a 60-line pyo3
  stub. The Rust-versus-Go question is C1 in
  [12](12-review-devils-advocate.md), accepted 2026-09-02: a Wave 0 parsing spike
  settles it on speed and maintainability together, not this sentence. Distribution mirrors Bifrost either way: six static binaries +
  checksums per release, an install script, `nils update`.
- **Risk, honestly**: `dicom-rs` has met fewer cursed vendor files than pydicom. The
  mitigation is our own corpus — 37.5M live instances become the parser regression
  suite (parse-and-compare against v0's extracted values before anything else is
  built on top). Files the parser cannot handle are *recorded* per file (a review
  item, D7), never silently skipped, with a quarantine list as output.
- **What stays Python**: ML sidecars (the body-part CLIP worker pattern was right:
  an optional HTTP sidecar owning heavy deps, the core owning all results), and
  nothing else in the core path. `dcm2niix` remains the conversion engine, shelled
  out, exactly as the field expects.

## Storage: one schema, two backends (D3)

- **Standalone**: an embedded registry — SQLite (WAL) as the system of record,
  DuckDB attached for the columnar passes (it reads SQLite natively and spills to
  disk, which is what makes the memory budget honest). A registry is a directory:
  `registry.db`, config, outputs. Backup is copying a file.
- **Server**: the same schema on Postgres 16, for real multi-writer concurrency
  (review sessions, imports, jobs) — but tuned normally, not survival-tuned: v0 ran
  with parallel query disabled and a 48 GB cap to survive extraction; v1's writer
  discipline (below) removes the reason.
- **The cost is real and accepted**: the dialect layer stays thin, the schema is
  declared once, and the full test suite runs against both backends in CI. Any
  feature that cannot be expressed on both does not go in the schema.

This is per-registry, not per-cohort — see [03-registry.md](03-registry.md) for why
cohorts stopped being containers.

## The pipeline model: predicates, not handovers

The single most important architectural change from v0:

- A stage's input is a **predicate** (`fingerprinted AND NOT classified`), evaluated
  as a query — never an ID list in memory or in a JSONB handover. v0's largest
  handover row today is 3.1 MB of compressed JSONB holding two lists of 458k ids;
  at 10M stacks that is ~180 MB of JSON text per step row, materialized as Python
  lists on every read, written best-effort, and rewritten on every boot. v1's grow
  to zero.
- Progress is **columns on the data** (`digested_at`, `classified_at`, pack version,
  evidence ref), so every stage is resumable and re-runnable by construction, and
  "what state is this registry in" is a query, not a ledger reconstruction.
- Ingest streams: parser workers emit Arrow batches → bulk appends in transactions
  of thousands of rows (v0 committed every 100 rows through one writer — a 50M-file
  cohort was ~500k transactions). Backpressure end to end; no unbounded queues; no
  per-subject materialization (v0 pickled a whole subject's instances in one shot).
- Classification compiles the YAML packs ([04](04-classification-packs.md)) to
  vectorized columnar expressions where the rule structure allows, and runs the
  remainder as parallel batch evaluation. The rules stay declarative; the per-stack
  single-threaded Python loop does not survive, for structure rather than speed:
  v0 classifies 454k stacks in about four minutes, and the measured bottleneck is
  extraction (150-460 files/s) and the write path, not the classifier.

## Jobs: the CLI is the only truth

Every heavy verb is a **job**: recorded in the registry, resumable, cancellable,
progress-reporting. The CLI runs jobs in-process; the server *enqueues the same
jobs* onto worker processes and streams progress out of the job record. Nothing
heavy ever executes inside an HTTP handler again — v0 ran 12-hour extractions inside
a request and the whole sort pipeline inside one SSE generator, and both died with
their connection.

## CLI surface (sketch, spec per wave)

```
nils init | status | doctor
nils digest <src>          # walk+parse+extract+fingerprint (resume by default)
nils classify [--pack ...] # packs versioned; re-run is a diff, not a repeat
nils anonymize | bids
nils query <ast.json|expr> # the same AST the server executes (D5)
nils review list|show|apply# the review-item queue, scriptable
nils serve                 # the thin server over this registry
nils custody               # every store: where, what class, how long, the command that changes it (C38)
nils node init|peers|serve|requests   # the federation daemon (14, D25, ratified 2026-09-02):
                           # a client of this engine under a reader role, off by default
nils query --federation <name> --level count   # the same AST, fanned out (14, C34)
```

## The performance budget (D6)

Written as a requirement, not an aspiration: full digest of a 30M-instance registry
on 8 cores / 64 GB in under a working day, peak RSS under 16 GB, and a scaled
benchmark (1M-instance synthetic corpus) run in CI with a hard regression gate.
Numbers may be tuned when the first real measurements exist; the *existence* of the
gate may not. There is no baseline yet: v0 has only ever run on its production
host, a large shared server, at 150-460 files/s, so "a working day" on eight cores
means roughly
1,000 files/s sustained; the baseline host and the v0 measurement on it are a
Wave 0 deliverable ([12](12-review-devils-advocate.md), C6).

## What carries over from v0, by name

The stack-signature field set (`extract/stack_utils.py`), the fingerprint
normalization semantics (`sort/fingerprint.py` — the Polars/COPY design is the
blueprint for every v1 columnar pass), DWI enrichment, gap-filling physics, the QC
weighting files, the anonymization strategies and audit shape, BIDS naming and
cross-cohort resolution rules. These are specifications to preserve, whatever the
implementation language.
