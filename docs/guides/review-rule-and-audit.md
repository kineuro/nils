<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# System 1 in the review rule, blind first, and the audit

How the engine runs System 1 beside the rules, asks people blind first where
a read trains or measures, and measures the error of what it accepted every
round (spec `wave7a-ready-for-production.md`, section 11).

## 1. Serve System 1

System 1 is a site model, never part of the engine. It runs as the pipeline
`system1-score` (`pipelines/nils-system1`), given the model chosen for the
task `system1`.

1. Build the site's bundle: one tar file whose `system1.json` names every file
   it holds by sha256 and its predictor (`pipelines/nils-system1/src/nils_system1/bundle.py`).
2. Register its card (task `system1`, kind `pass`, digest the bundle's sha256,
   `gate.axes` with `lambda1` and `lambda2` per axis), admit it, and keep the
   bundle:

   ```
   nils model register --card card.json --artifact system1.tar
   nils model admit <id> --check check.json
   nils model keep <id> --artifact system1.tar
   ```

3. Add the pipeline to the catalog and run it over a dataset, or over every
   scan:

   ```
   POST /api/datasets/<name>/steps/system1/run
   POST /api/review/system1/run
   ```

A bundle that brings its own code runs only with the pipeline input
`trust_code` true, after its bytes match. The run's opinions are kept beside
the rules' answers; they never become decisions.

> **Warning: the gate waits for a certificate.** On a dataset flagged as
> having damaged headers the gate accepts the rules' answer only where System
> 1 agrees above its thresholds. Until a recorded certificate names the chosen
> model, the gate raises nothing and Review shows System 1's opinion marked as
> not confirmed.

## 2. Review blind first

1. Set the share of questions read blind first (one in five by default):

   ```
   PUT /api/settings  {"review": {"blind_share": 0.2}}
   ```

2. A blind-first question is read without NILS's answers. The person answers
   from the pictures and the header text:

   ```
   POST /api/review/<id>/provisional  {"value": "T2w"}
   ```

3. The suggestions then show, both with their reasons:

   ```
   GET /api/review/<id>/suggestions
   ```

4. The person decides with the apply door. The decision is marked blind only
   when its value is the provisional one.

Only blind answers become training labels. A campaign whose answers train or
measure is made with `purpose` `train` or `measure` and is read blind first
always.

## 3. Run a round's audit

1. Draw the audit: 1 % of the round's accepted stacks, by a seed keyed by
   DICOM identity, and the campaign that reads them blind:

   ```
   POST /api/audit/draws  {"raters": ["rater-a", "rater-b"], "adjudicators": ["settler"]}
   ```

2. The raters read each item through the campaign doors, as the reference
   reads were run, each with its own token and `X-Nils-Actor` naming an agent:

   ```
   POST /api/campaigns/<id>/claim                         {"role": "rater"}
   GET  /api/campaigns/<id>/items/<item>/why
   GET  /api/campaigns/<id>/items/<item>/header
   POST /api/campaigns/<id>/assignments/<a>/answer        {"value": "{\"base\": ...}"}
   ```

3. The adjudicator settles every item the raters disagree on:

   ```
   POST /api/campaigns/<id>/claim                         {"role": "adjudicator"}
   POST /api/campaigns/<id>/assignments/<a>/answer        {"value": "..."}
   ```

4. Close the campaign, and grade the draw:

   ```
   POST /api/campaigns/<id>/close
   POST /api/audit/draws/<draw>/grade
   ```

The grade is the share of read stacks where any axis the settled answer names
differs from what the rules accepted, with its exact one-sided Clopper-Pearson
bound at 95 %. An audit's reads never train.

## Reference

| Door | Grant |
|---|---|
| `GET /api/review/{id}/suggestions` | review:see |
| `POST /api/review/{id}/provisional` | review:work |
| `GET /api/review/rounds`, `/rounds/{id}`, `/settings` | review:see |
| `POST /api/review/gate` | review:work |
| `POST /api/review/system1/run` | pipelines:work, at quasi |
| `GET`, `PUT /api/datasets/{name}/headers` | data:see or review:see; data:work |
| `POST /api/audit/draws` | review:work and campaigns:work |
| `GET /api/audit/draws`, `/draws/{id}` | review:see or campaigns:see |
| `POST /api/audit/draws/{id}/grade` | review:work |
