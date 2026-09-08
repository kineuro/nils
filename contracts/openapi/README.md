<!-- SPDX-License-Identifier: Apache-2.0 -->

# The HTTP API contract

The one door of the engine (decision record 05 §1): resource-shaped,
versioned, job-based for anything heavy, with a bounded synchronous path
beside the jobs. `GET /api/capabilities` names the engine's version and the
contract versions it speaks.

`VERSION` is the contract version. **It changes only by a pull request that
says so**, in its title, and that bumps `VERSION` and adds a new `vN/`
directory beside the old one, which stays.

| version | document | since |
|---|---|---|
| 0 | [`v0/openapi.yaml`](v0/openapi.yaml) | Wave 4a: the document exists and is empty until `nils serve` fills it (Wave 4a §11) |
| 1 | [`v1/openapi.yaml`](v1/openapi.yaml) | Wave 4a slice 14, 2026-09-06: `nils serve`, the nineteen doors the command line has (capabilities, status, custody, audit, jobs, releases, handovers, select, review, decisions, events) |
| 2 | [`v2/openapi.yaml`](v2/openapi.yaml) | Wave 4b slice 9, 2026-09-07: version 1 plus the ask doors (schema, catalog, validate, run, jobs, explain, options, apply, diagnose, preview, describe, documents, selections, handles, values) and the session rebuild job; `capabilities.ask` carries the caps, the schema digest and the epoch |
| 3 | [`v3/openapi.yaml`](v3/openapi.yaml) | Wave 4c slice A4, 2026-09-09: version 2 plus the security scheme, the three headers (ceiling, actor, idempotency), the caller's roles and result on a queued job, the authorised and audited handle read, the capped event stream, the declaration on every answer, the guide, the draft, the diff, the value sampler, and the doors of sections 6.5 and 6.6 (packs, batches, quarantine, classify signals and try, overlays, the ingest probe), which the slices after A4 serve |

Version 0 is deliberately empty: a skeleton that a test reads, so that the
first route lands in a file that is already checked in, already versioned and
already tested, rather than in one somebody has to remember to create.
