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

> **Warning: the gate waits for a chosen model.** On a dataset flagged as
> having damaged headers the gate accepts the rules' answer only where System
> 1 agrees above its thresholds. The gate runs once a System 1 model is
> admitted and chosen for its task: its admission stands for it (record 55,
> ruling 27), and no certificate is required. With no model chosen the gate
> raises nothing.

Once System 1 stands, the gate on a flagged dataset stages System 1's
answer where the rules name nothing and System 1 is confident (at its λ1 or
above), on a single-valued axis its card gates and in a value of the pack's
vocabulary. The answer waits in Review as a staged decision of System 1's
model: a person confirms it with one commit, or answers otherwise. System 1
never fills an answer on its own, and a dataset without the flag stages
nothing.

## 2. Record System 1's certificate (for the paper)

A certificate is not required for System 1 to stage answers (record 55,
ruling 27); it serves the paper, and the model's page shows it where one is
recorded. A model graded on research evidence is certified by citing its graded run,
since the site's registry holds no sample sealed for it (record 55, ruling 22
b). One person records it, under their own sign-in and models:work: nothing is
unsealed, so no second person is needed. The audit keeps it as
`labels.certificate`.

1. Sign in at the keyboard, or have the person's token at hand:

   ```
   nils login --issuer https://sign-in.example.org/application/o/nils/ --client nils-cli --username <you> --password-stdin
   ```

2. Record the certificate at the engine's door, from the run's manifest:

   ```
   nils model certify <name>@<version> --server http://127.0.0.1:8437 \
     --evidence MANIFEST.json --plan-commit <commit> \
     --model-digest sha256:<the registered artifact> \
     --bar 0.01 --n <graded> --errors <wrong> --bound <reported bound> \
     --test T19 --claim "wrong among accepted stacks under held-out damage"
   ```

   Where the manifest is not at hand, give `--manifest-sha256` and `--run`
   instead of `--evidence`. In the desk the same is Pipelines, Models, the
   model's page, Record a certificate.

3. Check that the certificate is shown beside the chosen model:

   ```
   GET /api/review/settings
   ```

The engine works the exact one-sided Clopper-Pearson bound out again from the
counts. A stated bound that differs is refused, and so is a result above its
bar. The model's page shows it as certified, with the run, its manifest and
the plan's commit.

> **Warning: the digest is the registered artifact's.** A certificate that
> cites a graded run certifies the one model whose artifact digest it names.
> Where the served artifact is a repackaging of the file the run graded, cite
> the run that showed it answers the same with `--equivalence MANIFEST.json
> --equivalence-class I`.

## 3. Review blind first

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

## 3. Answer the questions

The few questions the rules leave (record 56) are answered a question at a
time, each as a grid of its scans.

1. Read a dataset's questions, each with its distinct scans and the two
   answers its keys give:

   ```
   GET /api/review/questions?dataset=<name>
   ```

2. Read a question's scans, a page of 50, each with its picture, NILS's answer
   and the rules' reason (a key is percent-encoded: `base%3Amissing`):

   ```
   GET /api/review/questions/<key>/scans?dataset=<name>&page=1
   ```

3. Answer the page with one value, and give the scans set apart their own:

   ```
   POST /api/review/questions/<key>/answer  {"dataset": "<name>", "value": "T2w", "stacks": [12, 13, 14], "except": [{"stack": 13, "value": "PDw"}]}
   ```

   Each scan gets a decision at its own scope, all in one transaction, and the
   scans are re-sorted at once. A scan the page did not hold, or one answered
   meanwhile, refuses the whole answer.

## 4. Run a round's audit

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
| `GET /api/review/questions`, `/questions/{key}/scans` | review:see |
| `POST /api/review/questions/{key}/answer` | review:work |
| `POST /api/review/{id}/provisional` | review:work |
| `GET /api/review/rounds`, `/rounds/{id}`, `/settings` | review:see |
| `POST /api/certificates` | models:work, a person |
| `POST /api/review/gate` | review:work |
| `POST /api/review/system1/run` | pipelines:work, at quasi |
| `GET`, `PUT /api/datasets/{name}/headers` | data:see or review:see; data:work |
| `POST /api/audit/draws` | review:work and campaigns:work |
| `GET /api/audit/draws`, `/draws/{id}` | review:see or campaigns:see |
| `POST /api/audit/draws/{id}/grade` | review:work |
