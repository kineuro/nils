<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# System 1 in the review rule, blind first, changing the rules, and the audit

How the engine runs System 1 beside the rules, asks people blind first where
a read trains or measures, changes the rules with their effect shown before a
person decides, and measures the error of what it accepted every round (spec
`wave7a-ready-for-production.md`, section 11).

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
`trust_code` true, after its bytes match. A run given an admitted or
promoted model is given it true by the model's admission (the descriptor's
`trusted-by-admission`), so System 1 scores at arrival with no hand step;
the engine first checks that the kept file has the digest the model was
admitted with, and refuses a file changed since. The run's opinions are kept beside
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

A dataset is flagged by its signs at the end of every sort, for each dataset
holding a scan the sort judged, with the sort's own pack. Three signs are
read, each held to the pack's threshold (`review.damaged_headers`, pack
contract 11), and the dataset is flagged when one reaches it:

- its MR scans with neither a series description nor a protocol name;
- its named MR scans none of whose words the pack knows;
- its MR scans by a maker the rules were not built on that no dataset
  brought in before it holds, or by none.

The Data card says which sign flagged it, with the share and the threshold.
A person flags a dataset, or marks it as usual, with a why, from the card or
with `PUT /api/datasets/<name>/headers`; that word wins over the signs until
the person lets the signs decide again (`damaged: null`).

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

## 4. Answer the questions

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

## 5. Change the rules

An error is mended in the rules, not scan by scan (record 56). A change is a
patch of typed operations, its effect on the sorting is shown before anyone
decides, and a person adopts it. The desk does the same on Review, Rules, whose
tabs are Proposals, Quality, Change a rule and Words.

1. Read where the pack is weak: the rules it cannot rank, which rule wins over
   which, and, once a System 1 model serves, where System 1 confidently
   disagrees. Add `?dataset=<name>` for one dataset:

   ```
   GET /api/packs/mri/quality
   ```

2. Write the change as a patch. The operations are listed in
   `contracts/patch/v1`, with an example, and each carries a scope: the whole
   pack (a pack edit, shipped as a rules release) or `site:NAME`,
   `dataset:NAME` or `scanner:KEY=VALUE` (an overlay, which never leaves its
   scope):

   ```
   patch: 1
   pack: mri
   operations:
     - op: add_words
       axis: post_contrast
       value: given
       words: [mdc]
       scope: site:partner
       reason: the partner writes mdc for contrast given
   ```

3. Check that it applies, and see what it does. Nothing is written:

   ```
   nils pack apply packs/mri --ops change.yml
   nils pack rehearse --ops change.yml --scope dataset:<name>
   ```

   The same is `POST /api/packs/mri/rehearse`. The report says, per axis, how
   many scans move from what to what; which names and main-scan picks change;
   which Review questions appear and go; and, against the answers people
   settled, the right answers it would turn wrong and the wrong ones it would
   turn right. Through the door, a scope of more than 2,000 scans is queued as
   a job.

4. Keep it as a proposal, with its effect report and the card a person decides
   on. A proposal is a pack edit or a scoped change, not both:

   ```
   POST /api/packs/mri/proposals  {"title": "mdc at the partner", "operations": [...]}
   ```

5. A person adopts or rejects it, with review:work and data:work:

   ```
   POST /api/packs/mri/proposals/<id>/decide  {"decision": "adopt", "why": "..."}
   ```

   The answer is a 409, with its reason, for a proposal that is stale because
   the pack or the scoped changes in force have moved since it was rehearsed;
   rehearse it again and read its card first:

   ```
   POST /api/packs/mri/proposals/<id>/rehearse
   ```

   Adopting a pack edit writes the pack's next version beside the pack, the one
   it replaces kept whole under `<packs>.versions`, and re-sorts the registry;
   the version is then released as a rules release. Adopting a scoped change
   re-sorts its scope alone. What the re-sort did is written on the proposal.

6. A word overlay goes the same way: `POST /api/overlays` keeps it as a
   rehearsed proposal, and `POST /api/overlays/<id>/adopt` goes through the
   proposal's decision, with its stale check. A newer adopted version of an
   overlay retires the older. To go back to an older one, propose it again.

> **Warning: an agent proposes and a person decides.** The decision door refuses
> a caller that is not a person. The assistant's skill that teaches the sorting
> new words writes `add_words` and `remove_words` and proposes them; no station
> can adopt.

> **Warning: a free-form edit of a pack's files gets no effect report.** The
> report is for typed operations and overlays. A pack release that is edited by
> hand is replayed over the archive first (`nils pack replay`), and every scan
> that moves is listed and checked before it is released.

## 6. Run a round's audit

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

## 7. Run the monthly audit

Each month the audit reads a fixed 150 of the stacks accepted since the last
draw, with two local raters of different weights, and a person settles what
matters against the accepted answer in verify mode. A disagreement between the
raters alone decides nothing.

1. Draw 150 stacks accepted since the last draw (or since a time you give), by
   a seed you register first:

   ```
   POST /api/audit/draws  {"count": 150, "seed": "<registered seed>", "raters": ["<rater-a>", "<rater-b>"]}
   ```

   Give `"since": "2026-11-01T00:00:00Z"` to start the frame at a time. The
   same seed over the same frame draws the same stacks in any registry; the
   draw keeps its frame's size and digest and each stack's accepted answer,
   vendor and acquisition year.

2. The two raters read each item blind through the campaign doors, as in
   section 6. Their campaign is never adjudicated.

3. Once both raters have answered every stack, make the settle:

   ```
   POST /api/audit/draws/<draw>/settle  {"settlers": ["<person>"]}
   ```

   It is a verify campaign over every stack where a rater differs from the
   accepted answer and a seeded 10 % of the stacks where both agree with it. The
   person sees the accepted answer filled in, and where a rater differs, the
   accepted and the differing answer as A and B, unlabelled. Planted values
   guard against accepting without looking.

4. The person reads the settle campaign, then close it and grade the draw:

   ```
   POST /api/campaigns/<settle>/close
   POST /api/audit/draws/<draw>/grade
   ```

   The grade counts every settled difference from the accepted answer as a
   rules error; a stack not settled stands as the raters' agreement. It
   reports the wrong count, the exact bound, the wrong by axis, counts by
   vendor and year, and the registered rule `{max_wrong: 1, passed}`.

5. Pool the rounds (six rounds are 900 stacks):

   ```
   GET /api/audit/draws/pooled?ids=<d1>,<d2>,<d3>,<d4>,<d5>,<d6>
   ```

> **Warning: a month over the rule opens a rule review, never a certificate.**
> The grade makes a campaign over the round's wrong stacks that shows the
> rules' answer and evidence and closes into nothing (`rule_review` in the
> grade, `campaigns.rule_review` on the draw). Fixes go through the proposals
> of section 5. The audit's and the settle's reads never train a model.

## Reference

| Door | Grant |
|---|---|
| `GET /api/review/{id}/suggestions` | review:see |
| `GET /api/review/questions`, `/questions/{key}/scans` | review:see |
| `POST /api/review/questions/{key}/answer` | review:work |
| `POST /api/review/{id}/provisional` | review:work |
| `POST /api/packs/{name}/rehearse` | pipelines:work or review:work |
| `GET /api/packs/{name}/quality`, `/proposals`, `/proposals/{id}` | review:see |
| `POST /api/packs/{name}/proposals`, `/proposals/{id}/rehearse` | pipelines:work or review:work |
| `POST /api/packs/{name}/proposals/{id}/decide` | review:work and data:work, a person |
| `POST /api/overlays` | review:work |
| `POST /api/overlays/{id}/adopt` | review:work and data:work, a person |
| `GET /api/review/rounds`, `/rounds/{id}`, `/settings` | review:see |
| `POST /api/certificates` | models:work, a person |
| `POST /api/review/gate` | review:work |
| `POST /api/review/system1/run` | pipelines:work, at quasi |
| `GET`, `PUT /api/datasets/{name}/headers` | data:see or review:see; data:work |
| `POST /api/audit/draws` | review:work and campaigns:work |
| `GET /api/audit/draws`, `/draws/{id}` | review:see or campaigns:see |
| `POST /api/audit/draws/{id}/grade` | review:work |
| `POST /api/audit/draws/{id}/settle` | review:work and campaigns:work |
| `GET /api/audit/draws/pooled` | review:see or campaigns:see |
