# Pipelines

A pipeline is a container image that takes a frozen selection and writes files the engine registers as derivatives. Its descriptor, `nils.job.yml`, says what it is; the contract is `contracts/job/v1`. The engine runs one pipeline at a time, rootless, with no network.

## Turn pipelines on

1. Bind a working place, where a run's input and its outputs go:

   ```sh
   nils place add scratch /srv/nils/working --role working --fast
   ```

2. Check that a container runtime is found:

   ```sh
   nils pipeline runtime
   ```

   Rootless podman is taken first, then apptainer. With neither, pipelines are off and the command says why; nothing else is an error. A runtime that does not answer within 30 seconds, as `podman info` can be slow on a busy host, is reported as unknown for now rather than as not rootless: run the command again.

> **Warning:** rootless podman keeps its images under the invoking user's `HOME` (`~/.local/share/containers/storage`). A process that runs with another `HOME`, as a test or a lab with a scratch `HOME` does, sees an empty store of its own, so an image built or pulled by the user is not found there. Link the real store into the scratch `HOME`, or point podman at it with `CONTAINERS_STORAGE_CONF`:
>
> ```sh
> mkdir -p "$HOME/.local/share"
> ln -s /home/<user>/.local/share/containers "$HOME/.local/share/containers"
> ```
>
> A run that podman itself refuses exits 125, and the run's error names the image store it looked in and the `HOME` it ran with.

> **Warning:** docker's daemon is root on the host, so docker is never found on its own. An operator who accepts that chooses it:
>
> ```sh
> nils pipeline runtime --set docker
> ```
>
> `--set auto` goes back to looking; `--set off` turns pipelines off.

## Add a pipeline

```sh
nils pipeline add pipelines/n4-bias-correction/nils.job.yml
```

The descriptor is checked and kept whole with its digest. The image must be pinned by its registry manifest digest, `repository@sha256:<hex>`; a tag or an image id is refused. Adding the same descriptor again changes nothing; one that differs becomes the name's next version.

```sh
nils pipeline list
nils pipeline show n4-bias-correction
```

## Run one

1. Save the selection to run over, as a question (`nils ask selections save`), or name a frozen handle.

2. Run it:

   ```sh
   nils run n4-bias-correction --select selection:every-t1@1 --param shrink_factor=2
   ```

   The selection is frozen into a handle the run pins. A parameter not given takes its default, and every parameter is recorded on the run.

3. Read what it did:

   ```sh
   nils pipeline runs
   nils pipeline runs <run>
   nils derivative list --run <run>
   nils review list --kind pipeline:qc
   ```

Through the engine's doors a run is a job, queued by someone who holds `pipelines:work` at detail quasi:

```sh
curl -X POST http://127.0.0.1:8437/api/jobs \
  -H 'content-type: application/json' \
  -d '{"command": ["run", "n4-bias-correction", "--select", "selection:every-t1@1"]}'
```

## Run a model and take its proposals

1. Run the pipeline with the registered model it reads:

   ```sh
   nils run bodypart-infer --select selection:every@1 --model bodypart-head@h1a2b3
   ```

   The model's card sets the threshold its proposals are staged at. To stage fewer, raise it for the run with `--threshold 0.95`; a lower one is refused.

2. Commit the confident part, or review the rest:

   ```sh
   nils review list --kind body_part:model
   nils review commit --min-confidence 0.95
   ```

   A proposal is never in force until a person commits it. A newer run of the same model supersedes what its earlier runs left untaken on the stacks it proposes again; every other stack keeps its earlier proposal.

3. Commit one change of the change matrix, one model's answers, or some stacks:

   ```sh
   nils review commit --axis body_part --from neck --to spine
   nils review commit --model bodypart-head@h1a2b3
   nils review commit --stacks 12,14,19
   ```

   `--from` is what the axis holds on a stack now: the decision in force there, else the classifier's value. A model's group whose stacks do not all match is split: the matching stacks each get a decision of their own by the same model, put in force, and the group's decision stays staged for the rest. The door is `POST /api/decisions/commit` with `model`, `axis`, `from`, `to` and `stacks`.

## Curate a run's seeds

1. Read the seeds a run suggested and save the stacks as a selection:

   ```sh
   nils pipeline seeds <run> --save seeds-to-curate
   ```

2. Start a campaign from it:

   ```sh
   nils campaign create curate-body-part --axis body_part --select selection:seeds-to-curate@1
   ```

## Build the pictures of a selection

A review picture is drawn from the stack's pyramid in a working place. Build a selection's pyramids as one job, which skips the stacks that have one:

```sh
nils pyramid build --select selection:every-t1@1
```

The job's result counts what it built, skipped and failed, with why for each failure. Run it again after a failure and it builds only what is missing. A campaign made from a selection says how many of its stacks have their picture (`pictures {have, missing}`) and names the job that builds the rest, `pyramid build --handle <id>`. Each manifest names the stack's `orientation`, `origin` and `frame`, so a viewer draws the planes where they are in the patient.

## Ask several axes of a stack at once

1. Make an `axes` campaign. The served pack's legal combinations are frozen into the question:

   ```sh
   nils campaign create classify-review --axes base,technique,modifier \
     --select selection:every-t1@1 --raters-per-item 2 --closes-into decision
   ```

2. Each rater answers every axis in one answer. A combination the pack forbids is refused, such as two modifiers of one exclusion group, or MPRAGE with a base other than T1w:

   ```sh
   nils campaign answer <assignment> --value '{"base": "T1w", "technique": "MPRAGE", "modifier": ["FatSat"]}'
   ```

3. Close it. Each item becomes one decision per axis, and agreement is reported whole and per axis.

## Read a campaign fast

1. Claim the items worth a look first: those where the two systems or the rules disagree, then the least confident.

   ```sh
   nils campaign claim classify-review --order value
   ```

   At the door it is `POST /api/campaigns/{id}/claim` with `{"order": "value"}`.

2. Read the evidence of the item's stack, one line per axis: the value in force and who set it, the rule and clause that decided, the header values that clause read, the other rules and what they said, and System 1's candidates where it asked. `GET /api/campaigns/{id}/items/{item}/why` answers it with the suggested answer; the words a rule matched show only at detail quasi or above.

3. Accept like stacks in one move. `GET /api/campaigns/{id}/batches` groups the open items by the physics that decided them and the suggested answer. `POST /api/campaigns/{id}/batches/{key}/accept` answers every item of a batch with the suggestion, one answer per item, and holds back the campaign's share (a tenth unless its maker set `hold_back` higher) to be read alone. An item of a sealed sample is never in a batch, and is read blind, without a suggestion.

4. See how fast it goes. Every answer keeps the seconds from its claim, the suggestion and whether it was changed:

   ```sh
   nils campaign stats classify-review
   ```

## Let a certified sample train

A sealed sample trains nothing until the certificate it was drawn for is recorded. Both acts are a person's, at the engine's door with the person's own token, so the two people involved are told apart by the identity the engine verified. The keyboard refuses both.

1. Record what the certification measured with `POST /api/certificates`: `{sample, models, result}`, where the result names `sample`, `sample_digest` (as `nils labels seal --json` gave it), `risk`, `n` and `errors`.

2. Another person unseals the sample with `POST /api/certificates/{id}/unseal`. The seal's rows stay, naming the certificate.

3. Write the development labels a training tool reads: every person's decision in force, and nothing of a sample still sealed:

   ```sh
   nils labels export --for-training --to /export/labels/dev
   ```

## Fetch an output

By the door, the bytes with their digest in `X-Nils-Sha256`:

```sh
curl -o out.nii.gz http://127.0.0.1:8437/api/derivatives/<id>/content
```

Where a client shares the working place's volume, the place declares where that client reaches it, and the door answers with a path there instead of the bytes:

```sh
nils place set <place id> --share /mnt/nils-working
curl 'http://127.0.0.1:8437/api/derivatives/<id>/content?transport=share'
```

Both are gated at detail quasi and audited. A place that declares no share path answers 409 to `transport=share`, and the bytes still come by the door.

## Reference

What a run does, in order:

| step | what |
|---|---|
| input | `bids`: a release of the selection in the BIDS layout with the picks applied, under `<working>/runs/<run>/input`, so a BIDS App meets the one image a pick chose per role and session. `stacks`: `<working>/runs/<run>/input/stacks.json`, each stack's files under the source places |
| container | `/input` read-only, `/source/<n>` read-only in the stacks layout (one per folder that holds the selection's files, or the source places' roots past 2,000 folders, which the run's `summary.scope` says), `/inputs` read-only (`manifest.json` and the typed inputs), `/output` the one folder it writes, `<working>/derivatives/<pipeline>/<run>/`. No network. Podman runs with `--userns keep-id` and `--user`, and docker with `--user`, so the process is the engine's user on the host even where the image names a `USER` of its own, and a later run can link the files it wrote |
| GPU | passed through CDI (podman), `--nv` (apptainer) or `--gpus` (docker) where the descriptor needs one and the host has one; a pipeline whose need is `optional` runs on the CPU otherwise, and the run records `device cpu`; one whose need is `required` is refused |
| results | `/output/results.json`, one entry per unit; without it, a unit's files are the ones the descriptor's path templates find, and a container that exits with an error registers nothing |
| derivatives | every file hashed by the engine and registered, naming the run; a file outside `/output`, one reached through a link out of it, or one a unit's own templates do not name is refused and raised as a `pipeline:qc` item. An embedding is kept under its stack, encoder and preprocessing version, and a file for a key the registry holds already is not registered again |
| run-level outputs | a file of the whole run (`level: run`). A `model` output is registered as a model in state registered from the card beside it: the card must name the artifact's digest, it is trained on the label set the run was given, and its encoders are the card's |
| seeds | the `seeds` and `selection` of `results.json`, kept as the run's one derivative of kind `seeds`, never as proposals |
| proposals | on the axes the descriptor declares, grouped into `<axis>:model` review items and staged at the model card's threshold, or refused whole when the file says what the contract does not |
| review | a unit that failed, or that `results.json` did not name, is one `pipeline:qc` review item. A container that failed as a whole, exiting with an error and no `results.json`, or writing one that does not read, is one item of the run (unit `run`), not one per unit |
| run | every parameter, the runtime and its version, the host, the device, the models, the label set, the handle, the summary and a digest of the results, which a re-run that makes the same files repeats. A run whose container exited 0 is `done`, or `partial` when units failed or went unreported or a file it made was refused |

A bids input's release is marked as the run's input and left out of `nils release --history`; `--runs` lists it.

The container's log is `<working>/runs/<run>/log.txt`, and a bids input's release log `release.log` beside it.
