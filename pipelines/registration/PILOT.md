# The registration family's pilot (S4, gate G11)

The family runs a pilot on the research side, graded by its checks, before SAMSEG and segcsvd depend on it in production (Wave 7b spec, section 7 and G11). This file holds the exact commands. The pilot runs the real containers on archive copies; nothing here runs on a laptop or pulls an image onto one.

## What G11 asks

1. Each task's rule picks its method (the first half, proved offline by the engine's tests on fixture sessions and by the replay below).
2. The transform, the aligned image and the checks are written for every unit.
3. On the moved-FLAIR fixtures, the alignment check (the brain-mask Dice at 0.85 of SAMSEG and segcsvd) passes after registration where the header path failed it, 100 %.
4. The development install first, then the research side on archive copies.

## 1. Validate and add the family

```sh
nils pipelines validate pipelines/registration
nils pipeline add pipelines/registration
nils pipeline show registration
```

The images are pulled by the first run, by digest, or installed into the software place first. The FreeSurfer licence is the site file the FreeSurfer members read (`freesurfer_license`, the one recon-all reads). TemplateFlow's `tpl-MNI152NLin2009cAsym_res-01_T1w.nii.gz` for the ANTs member's template task is fetched by its sha256 when the ANTs member is installed (a secret is capped at 64 KiB, so it cannot be set as one); the SynthMorph member falls back to FreeSurfer's own copy of the template:

```sh
nils pipeline secret set freesurfer_license --file /path/to/license.txt
nils software install registration-ants@ants-2.6.2
```

## 2. The first half of G11: the routing, read-only

Over the registry's main-scan picks, per task, counting per member and the units skipped in the family's words:

```sh
nils pipeline replay pipelines/registration --switch task=within_session
nils pipeline replay pipelines/registration --switch task=across_visits
nils pipeline replay pipelines/registration --switch task=to_template
nils pipeline replay pipelines/registration --switch task=to_template --goal fast
```

The pre-flight of a run says the same per unit, over a selection:

```sh
nils run registration --select main@dataset:<name> --switch task=within_session --preflight
```

## 3. The dry run of each entry script

Before any real unit, each member's script runs once by hand on one pair, so the tool names and options of the pinned builds are confirmed (SynthMorph's `-m rigid` in FreeSurfer 8.2.0; `ImageIntensityStatistics` and `ConvertTransformFile` in ANTs 2.6.2; `mri_robust_register --halfmov --halfdst`). The script is the descriptor's command, with the placeholders filled:

```sh
sed 's|\[InputDataset\]|/input|g; s|\[OutputLocation\]|/output|g; s|\[THREADS\]|4|g; s|\[TASK\]|within_session|g; s|\[LESIONS\]|off|g' pipelines/registration/ants/entry.sh > /tmp/ants-within.sh
podman run --rm -v /path/to/one/bids/session:/input:ro -v /path/to/out:/output -v /tmp/ants-within.sh:/entry.sh:ro docker.io/antsx/ants@sha256:d53ce1c7db686c5a3791fb42b917aaee819c1f34327dd5171ebc3d2a8520297e bash /entry.sh

sed 's|\[InputDataset\]|/input|g; s|\[OutputLocation\]|/output|g; s|\[THREADS\]|4|g; s|\[TASK\]|within_session|g' pipelines/registration/synthmorph/entry.sh > /tmp/synthmorph-within.sh
podman run --rm -v /path/to/one/bids/session:/input:ro -v /path/to/out:/output -v /tmp/synthmorph-within.sh:/entry.sh:ro docker.io/freesurfer/freesurfer@sha256:5648104ca75a24f6ee5a8786146f99b405c2bda1da12bff723a15056da8ddf6b bash /entry.sh

sed 's|\[InputDataset\]|/input|g; s|\[OutputLocation\]|/output|g; s|\[THREADS\]|4|g' pipelines/registration/robust/entry.sh > /tmp/robust.sh
podman run --rm -v /path/to/one/bids/subject/tree:/input:ro -v /path/to/out:/output -v /tmp/robust.sh:/entry.sh:ro docker.io/freesurfer/freesurfer@sha256:5648104ca75a24f6ee5a8786146f99b405c2bda1da12bff723a15056da8ddf6b bash /entry.sh
```

A tool that is not in the pinned build is fixed in the script and the descriptor together (the engine's test keeps the two equal), and the descriptor's new digest goes into the member's pack.

## 4. The pilot runs

One run per task over the pilot dataset, through the family (one member run each):

```sh
nils run registration --select main@dataset:<pilot> --switch task=within_session --goal standard
nils run registration --select main@dataset:<pilot> --switch task=across_visits --goal standard
nils run registration --select main@dataset:<pilot> --switch task=to_template --goal standard
```

Then the grading, per run:

```sh
nils pipeline runs --json | jq '.runs[] | select(.pipeline | startswith("registration-"))'
nils pipeline runs <id> --json | jq '.units[] | {unit, status, variant, family, flags, caveats}'
nils pipeline numbers <id>
```

What is read: every unit has a transform, an aligned image and a metrics row (G11, the second half); the flags of the policy checks (determinant, translation, rotation, folding, inverse consistency) and their distribution, from which the thresholds are calibrated (spec section 22, question 3); the SynthSeg quality refusals and whether they are right.

## 5. The moved-FLAIR fixtures (G11, 100 %)

The fixtures of Wave 7a's alignment check (a FLAIR moved 15 mm against its T1w, Dice 0.80 to 0.86 under the header path) go through `registration` with `task=within_session`, then through SAMSEG with lesions on, which reads the aligned FLAIR as its typed input (`aligned_flair`):

```sh
nils run registration --select selection:moved-flair@1 --switch task=within_session
nils run samseg-lesions --select selection:moved-flair@1 --switch lesions=on
nils pipeline numbers <samseg run> | grep flair_t1_brain_dice
```

The bar: every moved session's `flair_t1_brain_dice` at or above 0.85 after registration, where it failed under the header path; `samseg_flair_source` reads `registration` on every unit.

## 6. What the pilot reports back

- The thresholds: the observed distribution of each policy metric per member and task, and the values proposed in their place.
- The tool options confirmed or changed per member (step 3).
- The template: whether FreeSurfer's `mni_icbm152_nlin_asym_09c` and TemplateFlow's `tpl-MNI152NLin2009cAsym_res-01` agree closely enough to be one space in the key, or the SynthMorph member should read the site file alone.
- The two members not shipped (SynthMorph across visits with a changed protocol or 2D; ANTs SyN in the halfway space at research grade): how many pilot units the family skipped for them.
