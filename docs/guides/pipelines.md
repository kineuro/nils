# Pipelines

A pipeline is a container image that takes a frozen selection and writes files the engine registers as derivatives. Its descriptor, `nils.job.yml`, says what it is; the contract is `contracts/job/v1`. The engine runs one pipeline at a time, rootless, with no network, in a lane of its own beside every other job, so a long run never holds up a digest; the units of a run run side by side within the lane's budget.

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

3. Where the engine runs in an unprivileged container, as the group's NILS guest does, take apptainer before podman, and keep images as sandbox folders where there is no `/dev/fuse`:

   ```sh
   nils pipeline runtime --set apptainer-first --apptainer-image sandbox
   ```

   Apptainer 1.1 or later is taken, since an older one cannot keep an unprivileged container off the network. Each image is built once from its pinned digest into `<working>/images/<digest>.sif` (or `.sandbox`) and run from there; the build's log is the run's `image.log`.

## Set the lane

Pipeline runs have a lane of their own: `nils serve --worker` runs them in a worker beside the one that runs every other job. The lane holds up to 48 cores and 512 GB of memory for all the units it runs at once, and never more than this machine, or the container the engine runs in, offers.

1. Read the lane:

   ```sh
   nils pipeline lane
   ```

2. Set its budget and the card a GPU unit leases:

   ```sh
   nils pipeline lane --cores 48 --memory-gb 512 --gpu-card 1
   ```

   `--gpu-card none` uses no card: a unit that needs a GPU is refused, and one whose need is optional runs on the CPU.

> **Warning:** the budget is the engine's own bookkeeping. Keep the container's own memory cap below the host's memory, so that what the lane is allowed is really there.

A unit starts only when the cores and memory its descriptor declares (`x-nils.needs`) fit in what the running units leave. A unit that could never fit is refused before the run starts. A GPU unit waits until the card's free memory, as `nvidia-smi` reads it, less what the lane's own running units there declared, covers its `gpu-memory-gb`; the run's progress says what it waits for. Each unit holds its lease until its container ends, and is given that card alone. Podman and docker hold each container to the cores and memory its unit declares (`--cpus`, `--memory`), and apptainer does where the host's cgroups delegate those controllers; elsewhere the budget is the engine's bookkeeping alone. A descriptor names the parameter its tool is told the threads by under `x-nils.needs.cores-input` (and the memory under `memory-input`): left out of a run it is the declared value, and asked above it the run is refused.

## Use the starter catalog

The engine seeds the first analyses of record 49 into its catalog each time it starts, when they are not there: N4, SynthStrip, SynthSeg, SAMSEG with lesions, MRIQC and FreeSurfer recon-all. Each is marked as a starter, and each image is pinned by its registry manifest digest. Each runs its sessions apart (`x-nils.units: apart`), a container a session, so the lane runs as many at once as its budget holds.

1. See what the catalog holds of each:

   ```sh
   nils pipeline starter
   ```

2. Seed them now, without restarting the engine:

   ```sh
   nils pipeline starter --seed
   ```

3. Turn the seeding off, or back on:

   ```sh
   nils pipeline starter --off
   nils pipeline starter --on
   ```

A version a person added is never gone over, and a starter a person retired stays retired. A newer engine with newer pins adds them as the name's next version only where the newest version is the engine's own starter.

> **Warning:** the starters' images are large (FreeSurfer 8.2.0 is about 14 GB, MRIQC about 5 GB) and are pulled on a run's first use. FreeSurfer recon-all reads the lab's licence as a secret input (record 49 R3) and does not start without it.

> **Warning:** segcsvd, record 49's fourth analysis, is not a starter: it ships only as an image archive on Hugging Face, with no public registry image to pin by digest.

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

2. Check what the run would do, without running it:

   ```sh
   nils run samseg-lesions --select selection:ms-baseline@1 --preflight
   ```

   The pre-flight counts the units, names the units that lack an input and why (a session with no FLAIR picked, a stack with no file or no derivative its input needs), counts the stacks no unit takes, estimates the time from the pipeline's own past runs here or else from its descriptor, says whether it wants a GPU and what it would run on, and holds what a unit needs against the lane's budget. `ready` false lists the blockers. Nothing runs.

3. Run it:

   ```sh
   nils run n4-bias-correction --select selection:every-t1@1 --param shrink_factor=2
   ```

   The selection is frozen into a handle the run pins. A parameter not given takes its default, and every parameter is recorded on the run.

4. Read what it did:

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

The door `POST /api/pipelines/{name}/preflight` answers the same pre-flight, under `pipelines:see`; below detail quasi it names no unit by its subject or session:

```sh
curl -X POST http://127.0.0.1:8437/api/pipelines/samseg-lesions/preflight \
  -H 'content-type: application/json' \
  -d '{"select": "selection:ms-baseline@1", "params": {"lesion": "on"}}'
```

The pre-flight's budget check reads the lane's own budget (see *Set the lane*).

## Take a run up again

A run whose units run apart (`x-nils.units: apart`) keeps each unit it finished. When the engine goes away mid-run, the pipeline lane's worker finds the run, marks it `interrupted`, and queues it to go on under what its job recorded; it does so three times at most, then leaves the run to a person.

1. Cancel a run; the units in flight stop, and those finished stay:

   ```sh
   nils jobs cancel <job>
   ```

2. Go on where it stopped:

   ```sh
   nils run --resume <run>
   ```

   The units in flight when it stopped run again from a clean folder; a unit whose container had ended is taken in from what it left, and nothing is registered twice. `nils jobs resume <job>` of the run's job does the same. A run whose units run together goes on from the start, unless its container had ended, when it is taken in from what that left.

## Give a pipeline a secret

A pipeline that needs a licence, such as FreeSurfer, declares it under `x-nils.secrets`. The site keeps the file; the engine keeps its path.

1. Set the file:

   ```sh
   nils pipeline secret set freesurfer_license --file /srv/nils/secrets/license.txt
   ```

2. Check that the engine can read it:

   ```sh
   nils pipeline secret list
   ```

The file is read when a run starts and mounted read-only into that pipeline's containers alone, at the path its descriptor names (`/secrets/<id>` by default), with the variable it names (such as `FS_LICENSE`) pointing there. A run that needs a secret the site has not set is refused before anything runs.

> **Warning:** a container can print what it was given. After each container the engine removes every file it left that holds the secret, and refuses it as a `pipeline:qc` item, and writes its log and `results.json` again with the secret replaced by `[secret <id>]`. The run records the secret's id, never its path or its bytes. Gzip streams and tar members are read inside, the secret is looked for as base64 too, and an archive the sweep cannot open is removed and refused; units stopped by a cancel are swept as well.

## Read a run's numbers in the ask

A table output (`kind: table`) is a file of numbers a unit or a run writes, with declared and typed columns. The runner registers the file as a derivative of kind `table`, reads its rows, and loads each as the unit's measures. The ask then reads each one as a field of the unit's grain, `measure.<pipeline>.<column>`, the value of the newest run that measured the unit, and `measure.<pipeline>.run` names that run.

1. Run the pipeline, then ask per session:

   ```json
   {"ast_version": 1,
    "sets": {"s": {"grain": "session"}},
    "out": {"set": "s", "level": "record", "columns": [
      ["field", {}, "id"],
      ["field", {}, "measure.synthseg.left_hippocampus"],
      ["field", {}, "measure.synthseg.run"]]}}
   ```

2. Below detail quasi, ask for totals over a group instead (record 49 R4):

   ```json
   {"ast_version": 1,
    "sets": {"s": {"grain": "session"},
             "g": {"grain": "group", "group": {"of": "s", "by": [["field", {}, "subject.sex"]]},
                   "bind": {"mean": ["avg", {"set": "s"}, ["field", {}, "measure.synthseg.left_hippocampus"]]}}},
    "out": {"set": "g", "level": "aggregate", "columns": [["field", {}, "subject.sex"], ["field", {}, "mean"]]}}
   ```

> **Warning:** a measure is quasi identifying. Below detail quasi the ask refuses it in a column, an order, a group's key and any binding but a total (count, distinct, sum, avg, min or max) of a group set, and allows it in a predicate. A group's totals of a measure, its `_rows` and its `_subjects` show below detail quasi only for a group of 5 scans or more, and so does a count of scans filtered on a measure (other than none); a smaller group's are withheld. A list of the scans a measure filter keeps is refused below detail quasi.

A descriptor also declares its checks under `x-nils.qc`, as `snr >= 8` or `{metric, op, value}`. Each unit that succeeded is held to them, the metric read from its `results.json` metrics or else from its tables; a breach is one `pipeline:qc` review item, status `breach`, whose error names the metric, its value and the check. A breach does not make a run partial. A metric a check reads from `results.json` is kept as a measure too.

> **Warning:** a check's breach says a scan's measure against a bound. Below detail quasi a run's doors, its job and its `pipeline:qc` items say the checks and the failures as counts, `summary.breaches_by_check` and `summary.failures_by_reason`, a count of 1 to 4 scans withheld, and name no unit, no value and no tool's error text. The review list shows a run's `pipeline:qc` items as one entry a check or a reason with its count, never one a unit.

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
| lane | a run is one job in the pipeline lane; its containers start while the cores and memory each declares fit in the lane's budget, a GPU one only under a lease on the lane's card. Units that run apart (`x-nils.units: apart`) each have a container of their own that sees its own input alone: the dataset's top-level files and its subject's or session's folder (bids), or a `stacks.json` of its stack and the folders of its files (stacks), with `[ParticipantLabels]` its subject and `NILS_UNIT` its id; each writes `derivatives/<pipeline>/<run>/<unit>/` and a `results.json` of its own. Each container is told `NILS_CORES` and `NILS_MEMORY_GB`. A unit is `queued`, `running`, `registering` or `over`, and `nils pipeline runs <run>` lists them |
| container | `/input` read-only, `/source/<n>` read-only in the stacks layout (one per folder that holds the selection's files, or the source places' roots past 2,000 folders, which the run's `summary.scope` says), `/inputs` read-only (`manifest.json` and the typed inputs), `/output` the one folder it writes, `<working>/derivatives/<pipeline>/<run>/`. No network. Podman runs with `--userns keep-id` and `--user`, and docker with `--user`, so the process is the engine's user on the host even where the image names a `USER` of its own, and a later run can link the files it wrote |
| GPU | the lane's card alone, passed through CDI (`nvidia.com/gpu=<card>`, podman), `--nv` with `CUDA_VISIBLE_DEVICES=<card>` (apptainer) or `--gpus device=<card>` (docker) where the descriptor needs one, the host has one and the lane names a card; a pipeline whose need is `optional` runs on the CPU otherwise, and the run records `device cpu`; one whose need is `required` is refused |
| apptainer | `apptainer run --containall --cleanenv --no-home --net --network none`, each folder bound with `--bind`, so the image's ENTRYPOINT runs before the descriptor's command line as it does under podman, the image built once from its digest into `<working>/images/` |
| secrets | each declared secret's file mounted read-only for this pipeline's containers alone; what a container left is swept of it |
| results | `/output/results.json`, one entry per unit; without it, a unit's files are the ones the descriptor's path templates find, and a container that exits with an error registers nothing |
| derivatives | every file hashed by the engine and registered, naming the run; a file outside `/output`, one reached through a link out of it, or one a unit's own templates do not name is refused and raised as a `pipeline:qc` item. An embedding is kept under its stack, encoder and preprocessing version, and a file for a key the registry holds already is not registered again |
| run-level outputs | a file of the whole run (`level: run`). A `model` output is registered as a model in state registered from the card beside it: the card must name the artifact's digest, it is trained on the label set the run was given, and its encoders are the card's |
| seeds | the `seeds` and `selection` of `results.json`, kept as the run's one derivative of kind `seeds`, never as proposals |
| proposals | on the axes the descriptor declares, grouped into `<axis>:model` review items and staged at the model card's threshold, or refused whole when the file says what the contract does not |
| tables | a `table` output's rows, read by its declared columns (a column by the header its `from` names, or by the header whose folded form is its name), are the unit's measures; a unit's table holds one row, and a run's table names each row's unit in its `unit-column`. The run's `summary.numbers` counts the files, rows and measures, the declared columns a file lacked, the values refused, and the checks held |
| checks | the declared checks (`x-nils.qc`), each held against each unit that succeeded; a breach is a `pipeline:qc` item with status `breach`, unless the unit has an item already |
| review | a unit that failed, or that `results.json` did not name, is one `pipeline:qc` review item. A container that failed as a whole, exiting with an error and no `results.json`, or writing one that does not read, is one item of the run (unit `run`), not one per unit; so is a run whose every unit's container did |
| run | every parameter, the runtime and its version, the host, the device, the models, the label set, the handle, the summary and a digest of the results, which a re-run that makes the same files repeats. A run whose container exited 0 is `done`, or `partial` when units failed or went unreported or a file it made was refused |

A bids input's release is marked as the run's input and left out of `nils release --history`; `--runs` lists it.

The container's log is `<working>/runs/<run>/log.txt`, or `<working>/runs/<run>/units/<unit>/log.txt` for a unit that runs apart, and a bids input's release log `release.log` beside it.
