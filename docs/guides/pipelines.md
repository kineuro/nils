# Pipelines

A pipeline is a container image that takes a frozen selection and writes files the engine registers as derivatives. Its descriptor, `nils.job.yml`, says what it is; the contract is `contracts/job/v1`. The engine runs pipelines rootless, with no network, in a lane of its own beside every other job, so a long run never holds up a digest. Runs whose stacks do not meet go on side by side, and the units of every run share the lane's one budget.

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

   The desk chooses the runtime on Pipelines > Catalog, under Runtime, through `GET` and `PUT /api/pipelines/runtime`; choosing is an admin's (`install:work`) and is audited.

   Apptainer 1.1 or later is taken, since an older one cannot keep an unprivileged container off the network. Each image is built once from its pinned digest into `<working>/images/<digest>.sif` (or `.sandbox`) and run from there; the build's log is the run's `image.log`.

## Set the lane

Pipeline runs have a lane of their own: `nils serve --worker` runs them in a worker beside the one that runs every other job. The lane holds up to 48 cores and 512 GB of memory for all the units it runs at once, of every run on this machine together, and never more than this machine, or the container the engine runs in, offers.

1. Read the lane:

   ```sh
   nils pipeline lane
   ```

2. Set its budget and the card a GPU unit leases:

   ```sh
   nils pipeline lane --cores 48 --memory-gb 512 --gpu-card 1
   ```

   `--gpu-card none` uses no card: a unit that needs a GPU is refused, and one whose need is optional runs on the CPU.

3. Name the working place a run's outputs go to, and the one its scratch goes to:

   ```sh
   nils pipeline lane --output-place results --scratch-place scratch
   ```

   The outputs are the derivatives and tables a run registers, under `derivatives/<pipeline>/<run>/` in the output place. The scratch is the run's input, each unit's folder, its log and apptainer's images, under `runs/<run>/` and `images/` in the scratch place. Each must be an active working place (`nils place add <name> <path> --role working`). Unset, or set to `default`, each is the first working place, as before. The desk reads and sets the lane through `GET` and `PUT /api/pipelines/lane`, the change an admin's (`install:work`). A run records both, so a run taken up again goes on in the places it started in, and a run reads derivative and model inputs from the output place first and the scratch place next.

> **Warning:** the budget is the engine's own bookkeeping. Keep the container's own memory cap below the host's memory, so that what the lane is allowed is really there.

A unit starts only when the cores and memory its descriptor declares (`x-nils.needs`) fit in what the running units of every run on this machine leave, as their rows in the registry say. A unit that could never fit is refused before the run starts. A GPU unit waits until the card's free memory, as `nvidia-smi` reads it, less what the running units of every run there declared, covers its `gpu-memory-gb`; the run's progress says what it waits for. The units of a run whose engine went away hold nothing, though their rows still say running. Each unit holds its lease until its container ends, and is given that card alone. Podman and docker hold each container to the cores and memory its unit declares (`--cpus`, `--memory`), and apptainer does where the host's cgroups delegate those controllers; elsewhere the budget is the engine's bookkeeping alone. A descriptor names the parameter its tool is told the threads by under `x-nils.needs.cores-input` (and the memory under `memory-input`): left out of a run it is the declared value, and asked above it the run is refused.

## Use the starter catalog

The engine seeds the first analyses of record 49 into its catalog each time it starts, when they are not there: N4, SynthStrip, SynthSeg, SAMSEG with lesions, segcsvd, MRIQC and FreeSurfer recon-all. Each is marked as a starter, and each image is pinned by its registry manifest digest. Each runs its sessions apart (`x-nils.units: apart`), a container a session, so the lane runs as many at once as its budget holds.

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

The desk shows the same on Pipelines > Catalog, under Starters, and an admin seeds them now or turns the seeding off there (`GET` and `PUT /api/pipelines/starters`, `POST /api/pipelines/starters/seed`, `install:work`).

A version a person added is never gone over, and a starter a person retired stays retired. A newer engine with newer pins adds them as the name's next version only where the newest version is the engine's own starter. A newer starter that fixes the older one replaces it (see *Choose a version*).

> **Warning:** the starters' images are large (FreeSurfer 8.2.0 is about 14 GB, MRIQC about 5 GB) and are pulled on a run's first use. FreeSurfer recon-all reads the lab's licence as a secret input (record 49 R3) and does not start without it.

> **Warning:** segcsvd (white-matter hyperintensities on the FLAIR, enlarged perivascular spaces on the T1w) reads each session's SynthSeg label map, so run `synthseg` over the selection first: a run of segcsvd with none is refused, and a session without one fails. Its image, segcsvd rc03 as its authors publish it (GPL-3.0), is about 13 GB. A session without a FLAIR gets the perivascular spaces alone. Its WMH model is validated for hyperintensities of vascular origin at 3 T, not for MS lesions.

> **Warning:** SAMSEG with lesions and segcsvd place each FLAIR on its T1w by the image headers alone; nothing registers them yet. Each unit compares a brain mask of the two images and writes their agreement (Dice) to its table as `flair_t1_brain_dice`. Below 0.85 the session is flagged with a `pipeline:qc` item, "FLAIR and T1 may not be aligned", and its results stand: look at the two images together before you use them.

> **Warning:** SAMSEG separates the pallidum from the white matter (`pallidum_separate`, on by default) only where it reads the FLAIR, with lesions on; from the T1w alone (lesions off) it never does, since a T1w cannot tell the two apart. Separating it changes the pallidum and white-matter volumes, so each session's table says which it had in `pallidum_separate` (1 or 0): compare only results with the same value.

> **Warning:** segcsvd's results depend on the SynthSeg they read: robust and not are different networks, and the WMH model was trained with a SynthSeg of the FLAIR, while segcsvd reads the T1w's. Each session's table names it: `synthseg_run`, `synthseg_pipeline` (name@version), `synthseg_robust` (1 or 0) and `synthseg_contrast` (the contrast that SynthSeg read, as the engine tied its label map to a scan). Compare only results that read the same kind of SynthSeg.

## Add a pipeline

```sh
nils pipeline add pipelines/n4-bias-correction/nils.job.yml
```

The descriptor is checked and kept whole with its digest. The image must be pinned by its registry manifest digest, `repository@sha256:<hex>`; a tag or an image id is refused. Adding the same descriptor again changes nothing; one that differs becomes the name's next version.

The models' pipelines (body part, post-contrast's cascade and System 1) come from the engine's release, not from a checkout. A release builds a model's image from its tag only when the package's files changed since the release that last shipped it, and otherwise ships that release's image, by its digest; either way its descriptors name every image by digest, in `pipelines.tar.gz` beside the binaries, listed in `IMAGES` and checked by `SHA256SUMS`, and the release's notes say for each image whether it was built or taken, and from which release. Take one by its name from the release this engine is:

```sh
nils pipeline add --release bodypart-infer-fusion
nils pipeline add --release postcontrast-cascade
nils pipeline add --release postcontrast-full
nils pipeline add --release system1-score
```

`--release=<version>` takes the descriptor of another release, and `--channel <url>` (or `NILS_RELEASES`) another place releases come from. A release's tarball that is not what its sums say is refused. A descriptor in a checkout pins the image of an earlier release, for development; a build that is not a release adds from there as above. Post-contrast's fallback, `postcontrast-full`, comes the same way, but the release does not build its image: the build pulls FreeSurfer's image (41 GB unpacked) and writes 34 GB, more than a hosted runner holds. Its image is published beforehand to `ghcr.io/kineuro/nils-postcontrast-full`, from a machine with the room, with `scripts/publish-postcontrast-full.sh`, and the release ships its descriptor pinned to the digest published there, provided anyone can pull it without credentials (see *Run a model registered by its card*).

> **Warning:** the models' images run on the CPU and on a card. A pipeline that declares the card optional (`x-nils.needs.gpu: optional`) takes the lane's card where the runtime offers one (`nils pipeline lane --gpu-card <n>`; card 0 where the lane names none), under the lease its `gpu-memory-gb` asks, and the CPU where the lane names `none`, the machine has no card, or the card is smaller than a unit's need. Check the lane's card before you add a model's pipeline on a machine whose cards other work uses.

The desk adds one the same way, under Pipelines, through `POST /api/pipelines` with the descriptor's text; it is an admin's (`install:work`), since it brings a container image onto the site.

A descriptor of the bids layout finds each pick's image by the name the release gives it. The suffix is the base contrast and FLAIR is always a modifier, so the T1w is `*_T1w.nii.gz` and the FLAIR is a T2w with `FLAIR` among the `+` tokens of its `acq-` (`sub-01_ses-01_acq-Ax+2D+FLAIR+IRTSE_T2w.nii.gz`), never `*_FLAIR.nii.gz`, which only older releases wrote. A T2w without the token is the `t2w` pick. The segcsvd and SAMSEG starters find their FLAIR this way.

```sh
nils pipeline list
nils pipeline show n4-bias-correction
```

## Give a pipeline a pack

A pack, `nils-pack.yaml` beside `nils.job.yml` (the contract is `contracts/pipeline-pack/v1`), holds what the descriptor cannot say: the needs a session must meet, what each knob does and whether it breaks comparability, the ordered rules that choose knob values (or a variant) per session from the registry's facts and the person's goal, the checks, the licence, the resources and the comparability key's members. It pins the descriptor by its digest. A pipeline version is the pair: adding a folder adds both, the same pair again is the version it is, and a version with no pack reads `pack: none`.

1. Check the folder offline. The loader's words say what is wrong (an unknown fact, a wrong type, a word outside a fact's vocabulary, a knob the descriptor has not, a rule whose `set` breaks a Boutiques constraint), and the answer prints the descriptor's digest the pack must pin:

   ```sh
   nils pipelines validate pipelines/samseg-lesions
   ```

2. Add the pair, and read it back:

   ```sh
   nils pipeline add pipelines/samseg-lesions
   nils pipeline show samseg-lesions --json | jq .pack
   ```

3. Run with a goal, the pack's switches, and the pre-flight first:

   ```sh
   nils run samseg-lesions --select selection:ms@3 --goal standard --switch lesions=on --preflight
   ```

The pre-flight judges every unit with the pack (`units_judged`): a hard need it lacks skips it in the pack's words, a hard need on a fact the registry cannot tell skips it with `cannot tell: <fact>` (`--let-run` lets it run as your choice, recorded on the unit), a soft need is a caveat that travels with the unit's results, and each knob says `{value, by, rule}` where `by` is `default`, `rule:<id>` or `person:<user>`. A knob the pack marks `set_by: rule` or `fixed` refuses a `--param` of yours. The run records the same per unit, with the facts each rule read, the flags its checks raised (a flag keeps the result and raises a `pipeline:qc` item of status `flag`; a fail fails the unit; a drop drops the one value) and the two keys of its results; `GET /api/pipeline-runs/{id}` returns them per unit and `GET /api/pipelines/{id}/pack` the pack as loaded with the facts it reads. Cohort checks (`cohort: true`) run within a stratum of site, vendor, field strength, sequence and post-contrast state once it holds `min_sessions` (30 by default) and flag a robust z above 3; below that they read `not yet: 12 of 30 sessions`, and a flag that comes after the run closed says when.

## Choose a version

A name keeps every version added under it. A version that fixes the earlier ones says so in its descriptor, and a new run no longer takes them:

```yaml
x-nils:
  replaces:
    why: "It checks that each FLAIR sits on its T1w; the earlier versions never checked it."
```

Without `versions`, it replaces every earlier version of the name in the catalog it is added to. Name them, as `versions: [1, 2]`, to replace those alone. A version that changes what a pipeline makes without fixing the earlier one leaves `replaces` out, and the earlier one stays selectable.

1. See each version and how it stands:

   ```sh
   nils pipeline list
   ```

   | standing | what it means |
   |---|---|
   | `current` | the newest version; `nils run <name>` runs it |
   | `selectable` | an older version nothing replaces; `nils run <name>@<version>` runs it |
   | `replaced` | a later version fixes it; a new run is refused, naming that version and why |
   | `retired` | retired by a person; it replaces nothing |

2. Run an older version that nothing replaces by its number:

   ```sh
   nils run n4-bias-correction@1 --select selection:ms-baseline@1
   ```

3. Run a replaced version only where you must, at the keyboard:

   ```sh
   nils run samseg-lesions@1 --replaced --select selection:ms-baseline@1
   ```

   The desk and the doors never run one: the jobs door refuses it with 409 and the pre-flight names it as a blocker.

The runs, files and checks of a replaced version keep its name and read as before.

Of the starters, recon-all's memory, MRIQC's `--notrack`, and the FLAIR alignment check of SAMSEG and segcsvd fix their earlier versions, so those are replaced; so do SAMSEG's next version, which separates the pallidum only beside a FLAIR, and segcsvd's, which names the SynthSeg it read (record 57). N4's spline distance and bias field change what it makes, so its earlier version stays selectable.

## Run one

1. Save the selection to run over, as a question (`nils ask selections save`), or name a frozen handle. A run can also go straight over a dataset's or a cohort's main scans of a role, with no selection made first: see *Run over a dataset's or a cohort's main scans*.

2. Check what the run would do, without running it:

   ```sh
   nils run samseg-lesions --select selection:ms-baseline@1 --preflight
   ```

   The pre-flight counts the units, names the units that lack an input and why (a session with no FLAIR picked, a stack with no file or no derivative its input needs), counts the stacks no unit takes, estimates the time from the pipeline's own past runs here or else from its descriptor, says whether it wants a GPU and what it would run on, and holds what a unit needs against the lane's budget. `ready` false lists the blockers. Nothing runs.

   The units that cannot run are said one line a reason, in a few words, with the units under it, and a run whose units can all run says so:

   ```text
     units            4 (1 ready, 3 missing an input)
     cannot run       3 sessions: no FLAIR picked
       sub-7a8b9c0d_ses-1, sub-1e2f3a4b_ses-1, sub-5c6d7e8f_ses-2
   ```

   The answer's `cannot` holds the same, `{why, units, named}` a reason, and each of `missing` its `lacks`. A run none of whose units could run would make nothing, so it is not ready (`no session can run: 4 of 4 lack an input (no FLAIR picked)`), and `nils run` and the jobs door refuse it before anything is made or queued, with the same words; the door answers 409 with `failure` (input), `units` and `cannot`.

   It also checks what the run needs of this machine (`requirements`), each check with what it found:

   | check | what it holds |
   |---|---|
   | `converter` | a bids input: dcm2niix is found and answers its version (see *dcm2niix* below) |
   | `runtime` | a container runtime is usable |
   | `image` | the runtime holds the image already, or can take it; an image not pulled yet is information, and the run pulls it first. Rootless podman whose account has no subordinate ids in `/etc/subuid` and `/etc/subgid` cannot unpack an image, and is refused |
   | `memory` | a unit's `memory-gb` against this machine's memory and the lane's budget |
   | `gpu` | a pipeline that requires a GPU has one the lane leases |
   | `disk` | where the descriptor says what a unit's outputs take (`x-nils.needs.disk-gb`), the units against the output place's free space |

   A check that fails is a blocker, and `failure` says its kind and what to do. `nils run`, the jobs door and a dataset's step refuse such a run before anything is frozen or queued, with the same words; the door answers 409 with `failure` and `requirements`. No check is slow: nothing is pulled.

3. Run it:

   ```sh
   nils run n4-bias-correction --select selection:every-t1@1 --param shrink_factor=2
   ```

   The selection is frozen into a handle the run pins. A parameter not given takes its default, and every parameter is recorded on the run. A run takes a selection of any size: it is frozen whole, however many stacks it reaches, where an answer to a question stops at its cap.

   A selection too large for one container's input, such as every stack of the registry, runs in parts:

   ```sh
   nils run bodypart-infer-fusion --select selection:every@1 --chunk 20000 --model bodypart-tiny@r7 --model bodypart-fusion@r7
   ```

   `--chunk N` freezes the selection in stack order into parts of at most N stacks, each a handle of its own, and runs each part as a run of its own, one after another; where a part's units run together in one container on the CPU, as System 1's do, as many parts go at once as the lane's budget holds of the cores and memory a part declares (`--parts-at-once N` caps it, and 1 keeps them one after another), their runs made in part order and each container still admitted by the lane. The parts together are the selection, each stack once. A handle splits the same way (`nils run <pipeline> --handle <id> --chunk N`), each part a handle of its own pinned as the handle is; a handle that fits one part runs as it is, and a handle of a scope's main scans of several roles is split only as it is frozen, by `--select`. Where a part fails or is stopped, the ones after it are not run, and the error names their handles and how to go on: `nils run --resume <run>` takes up the part that stopped and then runs the parts after it that no run holds yet, and `nils run <pipeline> --handle <id>` runs one. Each part's job records every part's handle (`part.handles` in its args), so a part taken up by `nils jobs resume` or by the pipeline lane after its engine went away goes on with the rest too. Under a worker the parts are one job, which stays running until the last part ends. A run queued at the jobs door takes `--chunk N` as the keyboard does, a whole number of stacks above 0.

4. Read what it did:

   ```sh
   nils pipeline runs
   nils pipeline runs <run>
   nils derivative list --run <run>
   nils review list --kind pipeline:qc
   ```

   A run is read by what its units made, beside its status: `4 of 4 made`, `3 of 4 made · 1 skipped: no FLAIR picked`, or, for a run that ended and made nothing, `nothing made · 4 of 4 skipped: no FLAIR picked`. The doors answer it as `made` on the run, in the listing and on the run's job, with the counts (made, failed, skipped, left), each reason with its count, `nothing` and the `line`; each unit of a run says its own status, and a skipped one what it lacked (`lacks`).

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

## Run over a dataset's or a cohort's main scans

1. Check what the run would do over a dataset's main T1w scans:

   ```sh
   nils run synthseg --select main:t1w@dataset:study-anon --preflight
   ```

   The pre-flight says how many scans the list holds, of how many visits, and under which rules they were picked. It counts the visits that have no main scan of the role, by why, and names each at detail quasi: it holds no scan of the role; it holds one and none is picked (`nils pick run` picks it); a person said none stands for it; or, of a dataset, its main scan is one only another dataset's folder holds.

2. Run it:

   ```sh
   nils run synthseg --select main:t1w@dataset:study-anon
   ```

   `main:<role>@cohort:<name>` names a cohort instead. The role is one the pack's picks name (`t1w`, `flair` and `t2w` in the MRI pack, listed as `main_roles` among the engine's capabilities). Through the doors the same text goes where a selection does: `{"select": "main:t1w@dataset:study-anon"}` to the pre-flight door, and `"--select", "main:t1w@dataset:study-anon"` in a run queued at the jobs door. `--chunk N` splits the list as it splits a selection.

3. A pipeline that needs several roles (samseg-lesions and segcsvd need a T1w and a FLAIR a session, as their descriptors' `x-nils.input.roles` say) goes over the main scans of each, written without a role:

   ```sh
   nils run samseg-lesions --select main@dataset:study-anon --preflight
   nils run samseg-lesions --select main@dataset:study-anon
   ```

   The list holds the main scan of each role of every visit that has a main scan of each, one pick a visit a role; a visit that lacks one is no unit of the run. The pre-flight's `main` names the roles (`roles`), counts the visits with every one (`main_visits`) and the visits that lack one, by role and by why (`without.why`, each `{role, why, visits}`), names those visits at detail quasi (each with its role), and gives each role's own count (`by_role`). The handle records which role each stack was taken for, so the run's bids input names each stack by its own role. `--chunk N` splits such a list by whole visits: a part never holds one role of a visit without the other. A pipeline of one role takes its role this way too; one that names no role refuses the spec and asks for `main:<role>@...`.

What the list is: the scope's scans (a dataset's, the stacks its folder holds a file of; a cohort's, every stack of its open members) that the main-scan pick of their visit names. The picks are the scope's own once a run of its saved rules finished, else the pack's, as the Main scans page shows them. One pick a visit, with every stack it names.

The list is resolved when the run is made and frozen into a handle, as a selection is, and the handle records the picks it was read under: the version of the scope's rules, their digest and the pick run that made them read, or the pack's defaults. A run over the handle takes the same stacks after the rules change, since a bids input is released from the list as it was frozen, each stack the main scan of its role; the input's release records the picks and keeps them with each stack of the list (see *What a release keeps of the main scans* under the reference). `nils pipeline runs <run>` says it under `main`.

> **Warning:** a scope's newer rules replace the picks of the version before once their run finishes, so the pick table no longer holds the picks an older list was read under. The list itself, and what its handle records, are what a run over it reads, and what its input's release kept is what the run's readers read after it.

## When a run fails

A run that failed, and each unit that failed, says why beside its error, which is kept as it was written: `failure` {kind, cause, advice, fix}. `nils run`, `nils pipeline runs <run>` and `nils jobs show <job>` print it as a line; the doors answer it on the run, on each of `units_run`, and on the run's job. The kinds:

| kind | causes | what it means |
|---|---|---|
| `dependency` | `dcm2niix`, `runtime`, `subuid`, `image`, `gpu_driver` | something the engine or a container needs is missing or does not start: dcm2niix, the container runtime, the subordinate ids rootless podman unpacks an image with, an image that will not pull, the GPU's driver. Install or mend it, then run again |
| `resource` | `memory`, `disk`, `gpu_memory`, `lane`, `time` | the machine ran short: a container killed for memory (exit 137, or the kernel's OOM killer), a full disk, a card out of memory, more than the lane holds, a time limit. Give it more, or run fewer units at once, then `nils run --resume <run>` |
| `input` | `input` | a unit's input is missing or was refused: the pre-flight names each unit and why |
| `pipeline` | `pipeline` | the pipeline's own error, and whatever else is not known: the error and the run's log say why |

`advice` is one sentence of what to do, in the engine's own words, so it is shown at any detail; the error, a tool's own words, is not below detail quasi. `fix` is the install the engine offers itself, where it has one: for dcm2niix, `nils software install dcm2niix` (`nils tools install dcm2niix` is its alias for one release) and `POST /api/tools/dcm2niix/install`, an admin's (`install:work`); for a tool whose terms are not accepted (`licence`), `nils software accept <app>@<version>` and `POST /api/software/{app}/{version}/accept`. A run's summary counts its units' failures by cause (`failures_by_cause`), so a partial run says why its units failed. A run that failed before this was recorded is read from its error when it is shown.

## dcm2niix

A run with a bids input is given a release of its selection, which dcm2niix converts. The engine finds dcm2niix in this order: a path given with `--dcm2niix`; the one the site gave setup with `--dcm2niix`; `dcm2niix` on the engine's search path; beside the running `nils`; where the setup record names one setup found or placed; and the registry's `tools/` folder. The first that answers its version is taken. The run's summary says which it used (`summary.converter`: version, path, found_by).

`nils setup` places the pinned build beside `nils` on an install on this machine (see the setup guide). Where none is found, an admin installs it:

```sh
nils software install dcm2niix
nils tools list
```

`nils tools install dcm2niix` is its alias for one more release.

The install downloads the build this version of `nils` pins from rordenlab's releases (or the mirror `NILS_DCM2NIIX_RELEASES` names), checks its sha256, unpacks it into the registry's `tools/` folder and asks its version; it is audited as `tool.install`. One found already is left alone. Where the engine cannot reach GitHub, it says so and gives the way by hand. The desk offers the same install through `POST /api/tools/dcm2niix/install`, and `GET /api/tools` and the capabilities' `tools` say whether dcm2niix is here, where and which build.

Where the engine cannot reach GitHub, fetch the archive `nils tools list` names (its name, its address and its sha256) on a machine that can, and give that file to the engine:

```sh
nils tools install dcm2niix --from dcm2niix_lnx.zip
```

The desk offers the same beside Try again, as Install from a file, through `POST /api/tools/dcm2niix/install-file` with the archive as the body. The archive is checked against the pinned sha256 before anything is unpacked, a file that is not the pinned build is refused with the archive it takes named, and the right one is installed as the download is.

## The software place

The tools the engine runs live in one place, for NILS and for people (record 57 P5): the setup record's place of role `software` (`nils setup --place software=/data/shared/software/nils,role=software`, or `nils place add software DIR --role software`), else `software/` under the pipelines' scratch working place, where apptainer's image cache was. It is laid out as a site's shared software folder is:

```text
<place>/
  <app>/<version>/image            the SIF or sandbox; on docker or podman the manifest names image@sha256
  <app>/<version>/manifest.json    source and digest, pack hash, size, installed or registered, by whom, when, verified when
  <app>/<version>/bin/<cmd>        one command per tool entry point
  <app>/bin/<app>-run              the launcher people use today
  bin/                             links for people's PATH
  modules/<app>/<version>.lua      Lmod modules: module use <place>/modules
  licences/<app>/                  site licence files, readable by the engine's account alone
  externals.yaml                   registered installs
```

The wrappers find the place from their own location, so moving the folder does not break them; `NILS_SOFTWARE_FLAGS` adds flags for the runtime they call (`--nv` for a card under apptainer). Two ways in:

```sh
nils software install freesurfer@8.2.0 --pack pipelines/freesurfer-recon-all/nils-pack.yaml
nils software register freesurfer@8.2.0 --kind sandbox --path /data/shared/software/freesurfer/8.2.0 --pack ...
```

`install` pulls the image by its digest from the pack's `source` (or `--source repository@sha256:<hex>`, or the one the catalog's pipeline of that tool pins), builds it into the place under apptainer or leaves it in the docker or podman store, and fails unless the digest got is the digest asked. `register` records an existing install as Spack records externals (`managed: false`, in `externals.yaml` and the manifest); the engine never updates or removes it, and `nils software remove` on it is refused. Both are verified, and `nils software verify <app>@<version>` verifies again: the pack's version probe, the fingerprint (the sha256 of a SIF; of the pack's `key_files` in a sandbox or a home, so a viewer's libraries change nothing), and the pack's test run on its small public input. A run uses the install whose digest is its pipeline's, builds its apptainer image into the place where there is none, and records "installed (or registered) install, version, fingerprint" in its `manifest.json` and its summary (`summary.software`); a registered install whose key-file fingerprint equals the pinned image's is the same tool for the comparability key.

Licences are recorded acceptances: `nils software accept <app>@<version> [--file PATH] [--terms URL --terms-version V]` records who, when and which terms, and the licence file's sha256, never its content; the file goes under `licences/<app>/`, readable by the engine's account alone, and reaches the pipeline read-only as a secret input with the variable the pack names (`FS_LICENSE`; `FS_ALLOW_DEEP` the same way). Until an administrator accepts the terms, or places the file, a run of the tool is refused before it starts (the `licence` check, 409). The dependencies are checked at install as the pre-flight checks them (runtime, image, memory, GPU, disk, converter, licence), each `ok`, `warn` or `missing` with its fix. `nils software list` shows every package; the doors are `GET /api/software`, `POST /api/software/install`, `POST /api/software/register`, `POST /api/software/{app}/{version}/verify`, `POST /api/software/{app}/{version}/accept` and `DELETE /api/software/{app}/{version}` (install:work; the listing install:see or pipelines:see). The converter is one package among the others: `nils software install dcm2niix`.

## Run several at once

Runs whose stacks do not meet go on side by side, each a job of its own, and all of them within the lane's one budget.

1. Start each run over its own stacks, for example one `--handle` each of the parts `--chunk` named:

   ```sh
   nils run bodypart-infer-fusion --handle 41 --model bodypart-fusion@r7 &
   nils run bodypart-infer-fusion --handle 42 --model bodypart-fusion@r7 &
   wait
   ```

   A run that would take a stack a running run holds is refused before it starts, and the refusal names that run, its job and how many of the stacks it holds. Start it again once that run has ended, or cancel that run with `nils jobs cancel <job>`.

2. Read what each waits for:

   ```sh
   nils jobs list
   ```

   A unit of one run waits for the cores, memory and card memory the units of the others hold. The run's progress says so under `waiting`, and `lane` gives what every run holds beside what this run holds.

The steps that write what every run shares are taken one run at a time: the intake of a run's proposals into review groups, with the supersede of what an earlier run of the model left untaken, the review items, the measures and the audit's epoch that close a run, a model a run registers, an encoder an embedding names, the release of a bids input and the build of an image apptainer keeps. A run beats its job's heart from a thread of its own for as long as it goes on, so a long intake or a wait for a lock never reads as a run whose engine went away, and a claim never fails a job whose own process still runs on this machine, however long since its last beat. The lock is the database's own: an advisory lock on Postgres, and on SQLite a file lock on `registry.db-pipeline-<step>.lock` beside the database. Either goes with the process that held it, however it ends.

`--chunk` runs its parts in one process, several at once only where each part is one container on the CPU, and the pipeline lane's worker still takes the queue's runs one at a time; runs started at the keyboard go on beside them.

> **Warning:** on SQLite a statement waits at most five seconds for another process's transaction. A very large intake of proposals can hold one longer, and a run beside it then fails on its own bookkeeping. Keep a registry that runs many large runs at once on Postgres.

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

   The units in flight when it stopped run again from a clean folder; a unit whose container had ended is taken in from what it left, and nothing is registered twice. `nils jobs resume <job>` of the run's job does the same. A run whose units run together goes on from the start, unless its container had ended, when it is taken in from what that left. A run that is a part of one split with `--chunk` then goes on with the parts after it that no run holds yet, one after another, with the parameters, models and version it recorded; a part a run holds already, one run by hand, is left as it is.

   A run taken up again goes on beside the runs whose stacks it does not share, and is refused, as a new run is, while a running run holds one of its stacks.

   It is checked first as a new run is, for the units it still has to run: those of the bids input it released before, where it did, else those a new run over its handle would have, the units it is over with kept. Its pre-flight says which cannot run and why:

   ```sh
   nils run --resume <run> --preflight
   ```

   The door takes the same as `{"resume": <run>}` at `POST /api/pipelines/{name}/preflight`. `nils run --resume` and the jobs door refuse a run taken up again whose checks fail (dcm2niix only where its input is still to be released), one that could run none of the units it still has and made nothing, and one that cannot be taken up (done, failed after its containers ended, or running), before anything is claimed or queued. A run that made some of its units goes on though none left could run, and closes with what it made. The job that stopped says which later job took its run up again (`taken_up` at the jobs doors, and `nils jobs show <job>`).

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

The desk lists the secrets the pipelines declare and whether each is set (`GET /api/pipelines/secrets`), and an admin pastes a licence's text there (`PUT /api/pipelines/secrets/{id}`): the engine writes it as `secrets/<id>` under its home, mode 0600, and keeps that path. `DELETE /api/pipelines/secrets/{id}` forgets it and removes that file; a file set at the command line is the site's own and stays.

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

Results of one pipeline may hold several comparability keys (a knob that breaks comparability set differently, another pack or image). The ask never pools across them silently: a total over `measure.<pipeline>.<name>` is refused with the words "these results hold N comparability keys (...) and are never summed together" until it is grouped by `measure.<pipeline>.key`, and `GET /api/pipeline-runs/{id}/numbers` (and its `?format=csv` export) answers per key in the same words.

A descriptor also declares its checks under `x-nils.qc`, as `snr >= 8` or `{metric, op, value}`. Each unit that succeeded is held to them, the metric read from its `results.json` metrics or else from its tables; a breach is one `pipeline:qc` review item, status `breach`, whose error names the metric, its value and the check. A breach does not make a run partial. A metric a check reads from `results.json` is kept as a measure too.

> **Warning:** a check's breach says a scan's measure against a bound. Below detail quasi a run's doors, its job and its `pipeline:qc` items say the checks and the failures as counts, `summary.breaches_by_check` and `summary.failures_by_reason`, a count of 1 to 4 scans withheld, and name no unit, no value and no tool's error text. The review list shows a run's `pipeline:qc` items as one entry a check or a reason with its count, never one a unit.

## Run a model and take its proposals

1. Run the pipeline with the registered model it reads:

   ```sh
   nils run bodypart-infer --select selection:every@1 --model bodypart-head@h1a2b3
   ```

   The model's card sets the threshold its proposals are staged at. To stage fewer, raise it for the run with `--threshold 0.95`; a lower one is refused.

   A run takes its proposals in once, as it closes; the `pipeline:qc` items of its failed units do not stop it. A run that closed without taking them, as one with a failed unit did before 1.0.0-alpha.83 (`summary.proposals.taken` is 0 and `refused` says they were in already), takes them in afterwards from the `results.json` files it left:

   ```sh
   nils pipeline ingest <run>
   ```

   It takes what the intake would have taken, at the run's threshold, and records it in the run's summary under `proposals`. Taking in again takes nothing and says so.

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

## Run a model registered by its card

A model registered with `nils model register --card --artifact` is checked against its file, and the file is not kept. Keep it where a run finds it before a pipeline reads it.

1. Keep the artifact in the lane's output place:

   ```sh
   nils model keep bodypart-fusion-coarse@r7 --artifact mode.json
   ```

   The file is copied, never moved, to `derivatives/models/<name>-<version>/<file>` in the output place and registered as a derivative of kind `model` that belongs to the model and to no subject. A file whose sha256 is not the model's digest is refused, and so is a retired model. The same bytes kept again write nothing.

2. Run the pipeline with the model:

   ```sh
   nils run bodypart-infer-fusion --select selection:every@1 --model bodypart-fusion-coarse@r7
   ```

   The kept file's folder is mounted read-only at `/inputs/<input>/`, and `/inputs/manifest.json` names the file in `models[i].artifact` beside the card.

A stacks input may ask for each stack's header facts with `x-nils.input.header: true` (record 50). Each entry of `stacks.json` then carries a `header` object: the stack's fingerprint columns (`fingerprint`, null without a fingerprint row), every classification row by axis (`classification`), the name of its first ingest batch (`batch`) and the cohorts its subject is an open member of (`cohorts`). Without the flag `stacks.json` is as before, and a bids input is refused the flag.

It may also ask for the geometry with `x-nils.input.geometry: true` (record 55 E2), beside the header or alone. Each entry then carries a `geometry` object, the orientation of its images (`image_orientation_patient`, six numbers) its fingerprint's `slice_thickness` and `spacing_between_slices`, and the stack's facts a few-slice model reads beside them: `study` (an id to group a session's stacks by, nothing more), `image_type`, the fingerprint's `sop_class_uid`, `n_instances` and `sealed` (whether it is in a sample sealed now, so an image leaves it out of its session and answers it as sealed), and each of its files a `geometry` object of its own: `image_position_patient` (three numbers), `pixel_spacing` (two), `rows`, `columns`, `instance_number` and `number_of_frames`, each null where the registry holds none. An image chooses which frames to read from these without reading a header first. A multi-frame file's position is its first frame's; the other frames' positions are in the file. With the flag `stacks.json` also lists, under `context`, the other MR stacks of each session the run holds part of that are not sealed now (a page of `--chunk`, a selection that is not whole studies), each with its `modality`, its `geometry` and its files' `geometry` and no path, so an image that reads a stack against its session (body part's few-slice student, against the session's top) reads the same input in any run; a context stack is never scored, a run of whole sessions lists none, and a unit run apart is given the rest of its session. With the flag the file is written compact, without indentation. Without the flag `stacks.json` is as before, and a bids input is refused it.

With the header and the geometry it may also ask for the facts the post-contrast model reads of a session, `x-nils.input.contrast: true` (record 55 E3). Each stack's entry and each `context` entry then carries a `contrast` object: its series' `contrast_bolus_agent` as written and `other_tag` (whether the route, a volume, a dose, a flow rate or a start time is filled), its clock (`acquisition_date` and `acquisition_time`, the fingerprint's earliest acquisition, else the series' date and time), `series_number`, `repetition_time`, `echo_time`, `inversion_time`, `flip_angle`, `mr_acquisition_type`, `derived`, `angio_flag`, `text_all_ci`, and `post_contrast_rules`, the post-contrast rules that fired on it. Each `context` entry also carries its `series_id` and its `header`, and every header the `datasets` that hold a file of the stack. The date and the time stay in the run's input; a pipeline never writes them out. Without the header and the geometry the flag is refused.

The post-contrast step of a dataset or a cohort runs `postcontrast-cascade`, given the promoted post-contrast cascade (a model whose card names its parts) and each part under the name of the model input it fills. When a run of it ends, the stacks whose table says `reason` `deferred` are queued to `postcontrast-full`, where that pipeline is installed, under the grants of the cascade's job; where it is not, they stay "not sure" and the run's summary says why. The full pipeline reads the fusion and the arms (model r1's three image arms with the atlases they were fitted on), each a part the cascade's card names, and the cascade. Its image is `pipelines/nils-postcontrast/Dockerfile.full`, published to `ghcr.io/kineuro/nils-postcontrast-full`, and a release ships the descriptor pinned to its digest (`nils pipeline add --release postcontrast-full`). It holds of FreeSurfer 8.2.0 only the parts the Synth tools run on, copied unchanged from FreeSurfer's own image under the FreeSurfer Software License Agreement, with no licence key and nothing else of FreeSurfer. The notices and licences of its parts are inside the image, in `/usr/local/share/licenses/nils-postcontrast-full`, and a site that wants another build makes it from the same file and repins the descriptor.

The post-contrast label is the engine's own and not a pipeline's (record 55 F2 and F5). Every MR stack carries a state (`pre`, `post`, `by_definition`, `undetermined`), the minutes since injection, where the label came from and the 72-hour flag, set by the time rule over its session's clock between the last pre and the first post T1 whose answer is certified (the image model at or above its threshold, or a person's read), with the checks against the eight failure modes each giving its reason code; `nils contrast show <stack>` reads one and `nils contrast label` labels again. Each label also carries its error level by use, the bar the label is held to where it is used: at most 1 % where it decides inclusion in pooled quantitative analysis (FLAIR segmentation sets, ASL blood flow, quantitative maps), 2 % for ADC, QSM and TOF, and 5 % where it is descriptive (T2, PD, DWI, SWI). A level is a bar for a use, not a claim: the label is certified for a use only where its audit's bound meets it.

## Complete a registered model's card

A card registered without the label set its model was trained on, its tested claims or its speed may gain them later (record 55, ruling 31 (a)). Nothing the card states changes.

1. See what the card lacks:

   ```sh
   nils model show postcontrast-fusion@r1
   ```

   The `card` line says `complete`, or lists what it lacks.

2. Register the label set the model was trained on, if it is not registered yet:

   ```sh
   nils labels register sets/postcontrast-r1-train --name postcontrast-r1-train
   ```

3. Write the claims in the card's own shape, beside the model they are for, and add them:

   ```json
   {"model": "postcontrast-fusion@r1", "digest": "sha256:<the model's digest>", "why": "its training set, its tests and its speed",
    "claims": {"trained_on": {"label_set": "sha256:<the set's digest>", "name": "postcontrast-r1-train", "rows": 2558},
               "tested": [{"claim": "what was measured", "value": "0 of 650", "bound": "0.46 %", "evidence": "<the evidence run>"}],
               "speed": {"cpu": {"stacks_per_second": 3.44, "hardware": "<cores and storage>", "evidence": "<the evidence run>"}}}}
   ```

   ```sh
   nils model add-claims postcontrast-fusion@r1 --claims claims-fusion.json
   ```

   A claim the card states already is refused, and so is any other field: a card that needs another change is a new version of the model. Each claim is checked as registration checks it, the label set among them (registered, and clear of every sample sealed now). The door is `POST /api/models/{id}/claims`, under Models work.

4. `nils model show` lists each addition with who made it, when and why, and the audit keeps it as `model.add_claims`.

## Score a dataset as it arrives

A dataset's bring-in ends with `score` (record 55 E5): System 1 answers its new scans without anyone starting a run. Body part and post-contrast are optional models that add value where the data need them (ruling 35 (b)): a person runs them from the dataset's step, and they never run at arrival.

1. Bring the dataset in, as ever:

   ```sh
   nils bring-in @scans
   ```

   After the digest, the fingerprints, the sort and any pick run, `nils score @scans --name <batch>` runs as the last step. It queues System 1's run over the stacks the batch brought (those whose first batch is the bring-in's), with the model the site chose for the task `system1`, where its pipeline declares `x-nils.on-arrival`. The run goes to the pipeline lane, so it never holds up the next digest. The result names body part and post-contrast under `optional`.

   A batch of more new stacks than one container of the pipeline holds is queued in parts (`--chunk`), each a run of its own in the one job: 1,250 stacks for each GB of memory its descriptor declares a unit needs, 5,000 for System 1's 4 GB, since its header model reads every stack's rows at once and 456,110 stacks did not fit where 5,220 did. A pipeline whose units run apart is never split. A step's run from its door and System 1's run over the archive are queued the same way. A bring-in queued at the keyboard, whole (`nils bring-in`) or a step at a time (`nils jobs enqueue`), records the keyboard's reach, so its score runs System 1.

2. Read what it queued and what it skipped:

   ```sh
   nils jobs show <score job>
   ```

   The result names the queued run, and a step it skipped with its reason: its pipeline does not score at arrival (`not_on_arrival`), nothing here runs it (`no_pipeline`, `no_model`, `no_runtime`, `no_place`), the machine lacks what it needs (`requirements`), or the person who brought the dataset in may not run a pipeline (`no_grant`: a run reads pixels, so it needs Pipelines work at detail quasi). The bring-in itself never fails because of it, and a bring-in that read nothing new queues no score.

3. Where the data need body part or post-contrast, run it from the dataset's step: the desk's Run on the step, or its door, `POST /api/datasets/{name}/steps/{step}/run`. Each step says how many scans its model would add an answer to (`hint` on the dataset's summary): the scans with no body part, and the scans that can be T1-type brain whose contrast is unknown. `nils score @scans --name <batch> --step body_part` runs body part over one batch's new stacks with the parameters its pipeline declares for a run at arrival, the slice cache among them; with `--step post_contrast` as well, post-contrast is queued once the body part's run ends done.

A pipeline scores at arrival with `x-nils.on-arrival: true`, or with the parameters such a run is given, `x-nils.on-arrival: {params: {cache: /cache, overlap: "off", cache_fill: touched}}`, as body part's does: at arrival its stages run one after the other with the slice cache, and a pass over scans the registry holds already runs them overlapped without it.

A unit may ask for every core of the lane, `x-nils.needs: {cores: lane, cores-input: threads}`: the run is given as many as the lane's budget holds (`nils pipeline lane`), its threads parameter is set to them, never above the parameter's maximum, and the pre-flight shows the number. Body part's pipeline does, so a lane of 48 cores runs it with 48 workers.

## Re-score from the slice cache

The slice cache keeps the small pictures a model reads of each scan (record 55 E5), so a re-score after a model update reads them instead of the files.

1. Choose the new model for its task:

   ```sh
   nils model choose bodypart-fusion@r8 --why "the new calibration"
   ```

2. Run the model over the dataset's scans again with the cache:

   ```sh
   nils run bodypart-infer-fusion --select selection:<the dataset's stacks>@1 --param cache=/cache \
     --model bodypart-tiny@r7 --model bodypart-fusion@r7 --model bodypart-fusion-coarse@r7 \
     --model bodypart-student@s1-r7 --model bodypart-defer@d3-r7 --model bodypart-cascade@r7-fast
   ```

   The models are the cascade and its parts, in the order the body-part step gives them. The run reads each scan's inputs from the cache where it holds them and reads the files of the rest. A run from the desk's dataset page or its door (`POST /api/datasets/{name}/steps/body_part/run`) is a pass over scans the registry holds: it takes body part's fast path, the stages overlapped in one worker pool with a worker a core of the lane, and reads no cache (record 55 T24). The cache is filled by a run that asks for it: `--param cache=/cache`, or `nils score --step body_part` over a batch's new stacks.

3. See what the cache holds:

   ```sh
   nils cache show
   nils cache prune
   ```

   A version no admitted or promoted model reads is dropped 30 days after it went unread; `prune` drops what is due now, and every run that used the cache does so when it ends.

A pipeline keeps model inputs in the cache by declaring their versions, `x-nils.cache: {inputs: [bodypart-planes-b24:v1]}`, stacks layout only. Its run is given the working place's `slice-cache/` read-write at `/cache`, which `[Cache]` names on its command line; the pixels are clinical, so the cache stays in the site's working place. Each file of `stacks.json` then carries an `identity`: its `sop_instance_uid`, and its `sha256` where a run reported reading the file whole and the file still has the size and time it had then, on the disk and in the registry (`null` otherwise). Each stack carries the `study_instance_uid` of its session. An image keys what it keeps by those identities and the input's version, never a registry id, so the cache outlives a new registry, and names the files it read whole in `results.json` (`read`, each file by its place in the unit's list with its sha256), which the engine keeps as that file's content. A miss reads the files; a file whose content the engine does not know is always read.

## Curate a run's seeds

1. Read the seeds a run suggested and save the stacks as a selection:

   ```sh
   nils pipeline seeds <run> --save seeds-to-curate
   ```

   In the desk, a run that suggested any says so on its page under Suggested to curate, with Save as a selection (`GET /api/pipeline-runs/{id}/seeds`, then the ask's own `PUT /api/ask/selections/{name}`).

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

A picture is also built when it is first asked for: a reader that opens a stack with no pyramid gets an answer that it is being built, and the engine queues the build itself. Building the pictures of a selection ahead is still worth it before many people read at once.

To build again pyramids that are built, for example after a fix to the reader, add `--force`:

```sh
nils pyramid build --select selection:every-t1@1 --force
```

The job builds several stacks at once, an eighth of its workers (one to eight) and fewer where the stacks are large, each with its share of the workers, so the files of a large selection on a network share are read many at a time. To say how many, add `--at-once N`; `--at-once 1` builds one stack after another:

```sh
nils pyramid build --select selection:every-t1@1 --at-once 4
```

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

## Choose what the raters are shown

A campaign says when it is made what its raters see beside each item as the answer suggested, and it never changes:

- `--suggest none`, the default: nothing. Every item is read blind, as an item of a sealed sample is: the evidence door shows the file and nothing a system said of the stack, no batch is formed, the gallery suggests nothing, and each answer keeps nothing suggested beside it. Use it for a reference read, where a shown answer would lead the rater.
- `--suggest rules`: the rules' answer and System 1's, filled in, with batches of like stacks. Use it to check the engine's own reading fast.
- `--suggest imported`: only the suggestions `nils campaign suggest` brings in, never the rules, and no batches.

```sh
nils campaign create body-parts --axis body_part --select selection:gold@1 --suggest imported
```

At the door it is `suggest` on `POST /api/campaigns`, and every campaign says its `suggest`.

A campaign that suggests none still shows the file's header text beside the pictures: the series description, the sequence name and the physics. Where that text could give the answer away, as for a post-contrast read, make the campaign show the pictures alone, as pair mode does:

```sh
nils campaign create post-contrast-single --axis post_contrast --select selection:singles@1 --hide-header
```

Its reader shows no header text, the evidence door serves none, and the whole-header door is refused. Only an axis or an axes question that suggests none takes it. At the door it is `hide_header: true` on `POST /api/campaigns`, and every campaign says its `hide_header`.

## Read a campaign fast

These steps show the rules' answer, so they are for a campaign made with `--suggest rules`.

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

## Check a hundred suggestions at once

For a campaign that asks one axis, such as body part, suggestions from outside the engine become gold labels once a person has checked them.

1. Bring the suggestions in: v0's committed labels, or a model's proposals with a confidence per class. The file's first line names its columns: `stack_id`, `item` or `SeriesInstanceUID`; `value`; `author`; and one `p:<value>` column per class.

   ```sh
   nils campaign suggest body-parts --file v0-body-parts.tsv --author v0-person
   ```

   At the door it is `POST /api/campaigns/{id}/suggestions`, for the campaign's maker. Only a campaign made with `--suggest imported` takes suggestions. A stack of a sealed sample takes none.

2. Open the gallery: `GET /api/campaigns/{id}/gallery` lists up to 100 open items, each with its suggested value, who suggested it, its confidences and a small picture (`GET /api/instances/{stack}/thumb`), the least certain first.

3. Correct the wrong ones and accept the page in one move with `POST /api/campaigns/{id}/gallery/accept`, `{"answers": [{"item": 12, "value": "brain"}, ...]}`. Each item becomes its own answer by the person, with the suggestion and its author kept beside it. The items the campaign holds back are never in a gallery; the gallery counts them (`held_back_open`, and `alone` with the sealed ones), and a claim with `{"alone": true}` offers only those, to be read one by one. They check how accurate the batches were.

4. Correct a mistake while the campaign is open. `GET /api/campaigns/{id}/mine?value=brain` lists your answers, the latest first, and `POST /api/campaigns/{id}/answers/{answer}/amend` with `{"value": "spine"}` writes the correction. The earlier answer is kept and names the correction in `superseded_by`; agreement and the close read the correction.

5. Add a rater to an open campaign, for example when one person reads another's share:

   ```sh
   nils campaign raters body-parts-b --add reader-a@lab
   ```

   A rater taken off with `--remove` keeps their answers. Keeping the reading blind, and each stack read once, is the operator's: the command warns when the rater added already answered the same stacks in another campaign.

6. Read the results. In the desk, the campaign's page has a Results view: per item each rater's answer, the adjudication, the settled value and the decision the close wrote, with the agreement, the unsure count, the counts by value and the label sets to download. At the door it is `GET /api/campaigns/{id}/results`. While the campaign is open it answers its maker (unless the maker rates in it), its adjudicators and a holder of `review:work`, and the raters are named to the maker and a holder of `review:work` alone.

## Compute a certificate on a sealed sample

While a sample is sealed, nothing a system said of its stacks opens to anyone: the ask leaves them out, explain and the evidence line answer blind, their review items are not there, their timeline keeps only their arrival, a label set leaves their lines out, and no release carries them. That holds for an admin as for a rater. The file itself, its header and its pictures, still opens.

1. Compute the certificate under a token that names `sealed:see`, the one grant that reads a sealed stack. No role holds it, admin's neither, so give it by name to the token the computation runs under and to no person who reads the sample:

   ```sh
   NILS_TOKENS="<token>=certify@lab:reviewer,sealed:see" nils serve ...
   ```

2. At the keyboard, a command that would show what a system said of a sealed stack refuses unless you say why. The reason is written to the audit as `sealed.read`:

   ```sh
   nils ask run --file sample.json --unsealed-access "certificate for cert-p0-clean"
   ```

## Let a certified sample train

A sealed sample trains nothing until the certificate it was drawn for is recorded. Both acts are a person's, at the engine's door with the person's own token, so the two people involved are told apart by the identity the engine verified. The keyboard refuses both.

1. Record what the certification measured with `POST /api/certificates`: `{sample, models, result}`, where the result names `sample`, `sample_digest` (as `nils labels seal --json` gave it), `risk`, `n` and `errors`.

2. Another person unseals the sample with `POST /api/certificates/{id}/unseal`. The seal's rows stay, naming the certificate.

A certificate may instead cite the graded run that measured the model, where the sample was never sealed in this registry (record 55, ruling 22 b). It names the run, the sha256 of its `MANIFEST.json`, the commit that registered the plan and the artifact digest of the model, with the graded result: `{models, evidence: {run, manifest_sha256, plan_commit, model_digest}, result: {bar, n, errors, bound}}`, or `nils model certify --server`. It unseals nothing, so one person records it.

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
nils place set <place id> --share /volumes/nils-working
curl 'http://127.0.0.1:8437/api/derivatives/<id>/content?transport=share'
```

Both are gated at detail quasi and audited. A place that declares no share path answers 409 to `transport=share`, and the bytes still come by the door.

## Reference

What a run does, in order:

| step | what |
|---|---|
| input | `bids`: a release of the selection in the BIDS layout with the picks applied, under `<working>/runs/<run>/input`, so a BIDS App meets the one image a pick chose per role and session, named as the release names it (a FLAIR is a T2w with `FLAIR` in its `acq-`, never `_FLAIR`). `stacks`: `<working>/runs/<run>/input/stacks.json`, each stack's files under the source places |
| lane | a run is one job in the pipeline lane, beside the runs whose stacks it does not share; its containers start while the cores and memory each declares fit in what the running units of every run leave of the lane's budget, a GPU one only under a lease on the lane's card. Units that run apart (`x-nils.units: apart`) each have a container of their own that sees its own input alone: the dataset's top-level files and its subject's or session's folder (bids), or a `stacks.json` of its stack and the folders of its files (stacks), with `[ParticipantLabels]` its subject and `NILS_UNIT` its id; each writes `derivatives/<pipeline>/<run>/<unit>/` and a `results.json` of its own. Each container is told `NILS_CORES` and `NILS_MEMORY_GB`, and one on a card `NILS_GPU_MEMORY_GB`, the card memory it is leased and plans within. A unit is `queued`, `running`, `registering` or `over`, and `nils pipeline runs <run>` lists them |
| container | `/input` read-only, `/source/<n>` read-only in the stacks layout (one per folder that holds the selection's files, or the source places' roots past 2,000 folders, which the run's `summary.scope` says), `/inputs` read-only (`manifest.json` and the typed inputs; each derivative a run takes is listed under its input with `made_by`, the run that made it, its pipeline's name and version, the parameters it ran with, and the scan the derivative is tied to with the file name its release gave it), `/output` the one folder it writes, `<working>/derivatives/<pipeline>/<run>/`. No network. Podman runs with `--userns keep-id` and `--user`, and docker with `--user`, so the process is the engine's user on the host even where the image names a `USER` of its own, and a later run can link the files it wrote |
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

### What a release keeps of the main scans

A release of picked stacks (`nils release --picked`, and every bids input) records which main scans it read: a dataset's or a cohort's rules at their version, each subject's own dataset's, or the pack's defaults. Since 2026-10-12 it also keeps the picks themselves, before it takes a stack: of each visit it took a stack of, every pick of what it read with every stack the pick names, and the scope and version of the rules each was made with. It takes its stacks by that list. A bids input released from a frozen list of a scope's main scans keeps each stack of the list as the main scan of its role as well; of a list over several roles, each stack as the role it was taken for, and the release records the roles (`picks.main` "t1w+flair", `picks.roles`).

Whatever reads a release's picks after it was made reads what it kept, not the picks of today: the run's units, judged again when a run is taken up with `--resume`; a mask's grid by its output's role (`nils derivative parts`, and a layer's build); and a check's panels, the visit's main scans beside the scan a result was made on. `GET /api/releases` and `nils release --history` say what each release read and kept (`picks`, with `frozen`: its visits, picks and stacks).

> **Warning:** a release made before 2026-10-12 kept no picks. Its readers read the live picks of what it recorded while they name the stacks it holds. Once a newer version of the rules it read has replaced them, or they no longer name its stacks, a unit says which of its stacks stands for a role is not known and why, a check's preview counts its results under `panels_gone` and shows their made-on scan alone, and a mask's grid is the input whose grid its file's matches.

The container's log is `<working>/runs/<run>/log.txt`, or `<working>/runs/<run>/units/<unit>/log.txt` for a unit that runs apart, and a bids input's release log `release.log` beside it. A run's folder is its own: a new run that finds `runs/<run>/` there already, left by another run of the same number (a registry made again numbers its runs from 1), moves it aside whole to `runs/<run>.before-<when>`, never deleting it, and says so in its output and on the first line of its first container's log. Its output folder is its own the same way: a `derivatives/<pipeline>/<run>/` an older registry's run left under the output place is moved aside to `derivatives/<pipeline>/<run>.before-<when>` and named in the same words. A run taken up again goes on in its own folders.
