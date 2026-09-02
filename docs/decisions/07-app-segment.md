# 07 — nils-segment: rebased on the contracts

## What it is, unchanged

The lease-based multi-rater annotation control plane — works, units, assignments,
questionnaires, inter-rater agreement, adjudication, DICOM-SEG export, the NiiVue
viewer — is v0's second-most-mature product and its domain design survives whole.
The COLD/HOT/DURABLE storage split and the refs-only database stay.

## What changes underneath

Segment reaches the engine through three channels today: a 425-line HTTP client on
the analysis-pipelines API and `/api/export/resolve-text` (the main path,
authenticated with a service token), one read-only JOIN over three tables for
subject/session grouping, and the engine's scratch volume mounted read-only at
`/cold` for derivative promotion. Each gets a contract to lean on instead:

- **Subset definition** (today: pasted stack-id manifests, the seam its own
  docstring calls "LATER"): a work's subset becomes a **selection** — picked from
  saved ones or authored ad hoc — resolved by the engine (D5).
- **The read-only DB role** (today: `seg_readonly` SELECT on three tables, used for
  one grouping query): replaced by catalog + AST reads over the API. The database
  stops being an integration surface entirely, which retires the provisioning
  machinery with it.
- **Derivative transfer** (today: the shared volume): the contract must name a
  transport, a download URL per derivative or a declared shared-volume capability;
  "refs only in the database" does not settle how bytes cross.
- **Prep** stays "a pipeline run on the work's frozen selection" — now against the
  BIDS-Apps-compatible runner ([09-pipelines.md](09-pipelines.md)) and its seeded
  starter catalog (convert → bias-correct → brain-extract → pre-segment).
- **AI assist** (nnInteractive/MedSAM2-class interactive servers) stays an outbound
  client to an external capability-discovered server — that design was right and is
  the sidecar pattern already.
- **Adjudication meets review items**: rater disagreement above threshold can emit
  a review item (D7), which puts segmentation QC into the same queue and policy
  system as every other judgement in NILS — including, eventually, agent-proposed
  pre-adjudication under a strict propose-only policy.
- **Auth**: verify-only against the engine's mode; the copied-verifier machinery and
  its drift tests retire with nils-identity.

## Independence (D1)

- **Absent**: no segmentation features anywhere; pipelines and selections are
  unaffected.
- **Present**: consumes contracts only; nothing else knows it exists. The App
  Center tile becomes a portal link like any other.

## Repo

Its own repo, own version, own releases, per D10 — it is the pilot case proving
an app can live entirely on the public contracts, and its port is scheduled as the
validation wave for those contracts in [11-order.md](11-order.md).
