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

   Rootless podman is taken first, then apptainer. With neither, pipelines are off and the command says why; nothing else is an error.

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

   A proposal is never in force until a person commits it. A newer run of the same model supersedes what its earlier runs left untaken.

## Curate a run's seeds

1. Read the seeds a run suggested and save the stacks as a selection:

   ```sh
   nils pipeline seeds <run> --save seeds-to-curate
   ```

2. Start a campaign from it:

   ```sh
   nils campaign create curate-body-part --axis body_part --select selection:seeds-to-curate@1
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
| container | `/input` read-only, `/source/<n>` read-only in the stacks layout, `/inputs` read-only (`manifest.json` and the typed inputs), `/output` the one folder it writes, `<working>/derivatives/<pipeline>/<run>/`. No network. Podman runs with `--userns keep-id` and docker with `--user`, so the process is the engine's user on the host |
| GPU | passed through CDI (podman), `--nv` (apptainer) or `--gpus` (docker) where the descriptor needs one and the host has one; a pipeline whose need is `optional` runs on the CPU otherwise, and the run records `device cpu`; one whose need is `required` is refused |
| results | `/output/results.json`, one entry per unit; without it, a unit's files are the ones the descriptor's path templates find, and a container that exits with an error registers nothing |
| derivatives | every file hashed by the engine and registered, naming the run; a file outside `/output` is refused. An embedding is kept under its stack, encoder and preprocessing version, and a file for a key the registry holds already is not registered again |
| run-level outputs | a file of the whole run (`level: run`). A `model` output is registered as a model in state registered from the card beside it: the card must name the artifact's digest, it is trained on the label set the run was given, and its encoders are the card's |
| seeds | the `seeds` and `selection` of `results.json`, kept as the run's one derivative of kind `seeds`, never as proposals |
| proposals | on the axes the descriptor declares, grouped into `<axis>:model` review items and staged at the model card's threshold, or refused whole when the file says what the contract does not |
| review | a unit that failed, or that `results.json` did not name, is one `pipeline:qc` review item |
| run | every parameter, the runtime and its version, the host, the device, the models, the label set, the handle, the summary and a digest of the results, which a re-run that makes the same files repeats. A run whose container exited 0 is `done`, or `partial` when units failed or went unreported or a file it made was refused |

A bids input's release is marked as the run's input and left out of `nils release --history`; `--runs` lists it.

The container's log is `<working>/runs/<run>/log.txt`, and a bids input's release log `release.log` beside it.
