<!-- SPDX-License-Identifier: Apache-2.0 -->

# The job contract

A pipeline as the engine runs it (decision record 09, D9; record 43): the
descriptor that says what it is, `nils.job.yml`, the manifest the runner
hands it in the stacks layout, and the results it hands back. A pipeline
image is built against this contract and nothing else of the engine.

`VERSION` is the contract version. **It changes only by a pull request that
says so**, in its title, and that bumps `VERSION` and adds a new `vN/`
directory beside the old one, which stays.

| version | documents | since |
|---|---|---|
| 1 | [`v1/`](v1/) | Record 43 slice S1, 2026-09-24; amended in place and additively by record 49 A3 (the table kind, the checks, the roles a unit needs, a unit's minutes) and by record 55 H3 (a mask's grid and parts, a derivative input's checked), and on 2026-10-12 (a unit's disk, `x-nils.needs.disk-gb`, held against the output place's free space before a run is queued; the earlier versions a version replaces because it fixes them, `x-nils.replaces`, which a new run no longer takes; what made each derivative a run takes, `made_by` in `/inputs/manifest.json`), and by record 55 E5 (scoring at arrival, `x-nils.on-arrival`; the slice cache, `x-nils.cache`, with each file's `identity` in `stacks.json` and the files a unit read whole, `read`, in `results.json`) |

## What version 1 fixes

| document | what it fixes |
|---|---|
| `nils.job.schema.json` | the descriptor: v0's Boutiques 0.5 subset with an `x-nils` block. The image by its registry manifest digest; the parameters (name, type, range, choices, default, unit); the command line with its value-keys; the analysis level (`participant`, `session` or `stack`); the input layout (`bids` or `stacks`); the typed inputs (`model`, `label_set`, `derivative:<kind>`); the outputs by derivative kind with path templates, each one per unit or, since the wave's rulings, one for the whole run (`level: run`), such as a model (`kind: model`, with the `card` beside it); the needs (`gpu: none, optional or required`, memory, cores); and the axes it may propose values on. Record 49 A3: a `table` output with its format (`csv`, `tsv` or `json`) and typed columns (`number`, `integer` or `text`, with a unit), one row per unit or, at run level, a `unit-column`; the checks each unit is held to (`x-nils.qc`, as `snr >= 8` or `{metric, op, value}`); the pick roles each unit of a bids input needs (`x-nils.input.roles`); whether a stacks input carries each stack's header facts (`x-nils.input.header`, record 50) each file's geometry (`x-nils.input.geometry`, record 55 E2) and, with both, the session facts the post-contrast model reads (`x-nils.input.contrast`, record 55 E3); and a unit's typical minutes on the CPU (`x-nils.needs.unit-minutes`). Record 55 D3: whether a stacks input with its header carries the registry's rows a header model reads (`x-nils.input.rows`), and the axes it may give System 1's opinion on (`x-nils.opinions`). Record 55 H3: a mask output's `grid`, the input role whose stack its grid is on, and its `parts`, the part sets a check reads it as (`components` of named labels or `labels`, each value a part with its name); a `derivative:<kind>` input's `checked` (`accepted`, `not_rejected` by default, or `any`). 2026-10-12: the earlier versions of its name it replaces because it fixes them (`x-nils.replaces`, `why` and, where not every earlier one, `versions`) |
| `stacks.schema.json` | `/input/stacks.json` in the stacks layout: each stack's files under the source places mounted read-only, and the frames of a multi-frame file; each stack's orientation, its `body_part` and `technique` as the registry holds them now, and its slice count; with `x-nils.input.header` (record 50) each stack's header facts too; with `x-nils.input.geometry` (record 55 E2) each stack's orientation, slice thickness and spacing, its study (to group a session by), image type, SOP class, instance count and whether it is sealed now, and each file's position, pixel spacing, matrix, number and frame count; and, under `context`, the other unsealed MR stacks of each session the run holds part of, with that geometry and no path, never scored; with `x-nils.input.rows` (record 55 D3) the `rows` a header model reads: each stack's whole fingerprint row and votes, and each series's frame of reference as a digest and private elements, for the run's stacks and the other MR stacks of their sessions; with `x-nils.input.contrast` (record 55 E3) each stack and context entry carries its `contrast` facts (its series' contrast tags, its clock, its physics, the post-contrast rules that fired), each context entry its series and header, and each header the `datasets` holding a file of the stack |
| `results.schema.json` | `/output/results.json`: each unit's status, files, metrics and error; the proposals (`proposals.schema.json`); System 1's opinions (`opinions.schema.json`, record 55 D3); the cards of the models the run used or made; and the `seeds` and `selection` it suggests a person curate, which are never proposals |
| `proposals.schema.json` | a model's proposals, per stack: the axis, the value, the probability of every class and the registered model |
| `opinions.schema.json` | record 55 D3: System 1's opinions, per stack and axis: the answer, its calibrated probability, the rules' reliability on the header, the fields that mattered and the registered model; a second opinion beside the rules', never a decision |
| `embedding.md` | the embedding file a pipeline writes, one per stack and encoder |

## What a container meets

| where | what |
|---|---|
| `/input` | read-only: the BIDS tree (bids layout), or `stacks.json` (stacks layout) |
| `/source/<n>` | read-only, stacks layout only: the source places the manifest's paths are under |
| `/inputs` | read-only: `manifest.json` (the run, the pipeline, every parameter, the units, the models, the label set, and the derivatives each typed input takes, each since 2026-10-12 with `made_by`: the run that made it, its pipeline's `name` and `version`, the `params` it ran with, and the scan it is tied to, `grid_stack_id`, with the file name its release gave it, `grid_stem`; null where no run made it) and one folder per typed input |
| `/output` | the only place it writes; the engine's `<working>/derivatives/<pipeline>/<run>/` |

The container has no network, and its process is not root on the host:
podman runs rootless with `--userns keep-id`, apptainer as the user by
construction, and docker only where an operator opted in, as the user's
uid. A GPU is passed where the descriptor asks for one and the host has it
(CDI for podman, `--nv` for apptainer); otherwise a pipeline whose need is
`optional` runs on the CPU and the run records `device cpu`.

## The rules both sides keep

1. An image is pinned by its registry manifest digest, `repository@sha256:<hex>`, or the descriptor is refused. A tag moves, and an image id is a config digest no registry can serve.
2. Every parameter is recorded on the run, the defaults filled, so a run is reproduced from its record.
3. The bids layout is a release with the picks applied (record 43 R4): a BIDS App meets one image per role and session.
4. A unit is `sub-<s>`, `sub-<s>_ses-<t>` or `stack-<id>`, as `/inputs/manifest.json` names it. A failed unit, and a unit `results.json` does not name, is one `pipeline:qc` review item.
5. A file is registered as a derivative only from under `/output`, hashed by the engine. Without `results.json`, a unit's files are the ones its declared path templates find.
6. A proposal is evidence for the review spine, never a fact, and only on an axis the descriptor declares. It is staged as the model's decision at or above the threshold on the model's card, which a run may raise and never lower; a newer run of the model supersedes what its earlier runs left untaken on the stacks it proposes again, and every other stack keeps its earlier proposal.
7. A run whose container exits 0 while units failed or went unreported is `partial`, not `done`; those units are `pipeline:qc` review items.
8. A run-level model output is registered as a model, in state registered, from its card: the card names the artifact's digest, the label set is the one the run was given, and the encoders are the card's.
9. Seeds and a suggested selection are kept as the run's one derivative of kind `seeds`; `nils pipeline seeds <run> --save <name>` makes the selection a campaign starts from.
10. A table's rows are its units' measures (record 49 A3): a unit's table holds its one row, a run's table names each row's unit, a column is read by the header its `from` names or by the header whose folded form is its name, and the ask reads each as `measure.<pipeline>.<column>` on the unit's grain, the newest run's value. A column's name is unique across a pipeline's tables, and `run` is the ask's own.
11. A declared check is held against each unit that succeeded, its metric read from the unit's metrics in `results.json`, else from its tables; a breach is one `pipeline:qc` review item, status `breach`, naming the metric, its value and the check, and does not make the run partial.
12. A mask is registered with its grid, read from its NIfTI header (the sform, else the qform), and the scan whose grid it is on (record 55 H3): the stack the unit was given for the output's `grid` role, else a stack unit's own stack, else the unit's input whose grid it matches within 0.01 mm and 0.001 in direction. The tie is `same` where the grids match, `world` for another grid in the scan's frame, `none` where no scan is known, and a mask tied by `none` is never drawn. Its parts are made in its own grid as it is registered, and nothing of the file is changed.
13. A version that says it replaces earlier versions of its name (`x-nils.replaces`, 2026-10-12) fixes them: a new run of one of them is refused, naming the version that replaces it, unless asked for at the keyboard by name and version with `--replaced`; its runs, derivatives and checks keep its name. Without `versions` it replaces every earlier version of its name in the catalog, since a version's number is the catalog's own. A retired version replaces nothing, and a version that only changes what a pipeline makes leaves the earlier one selectable.
14. Record 55 E5: a pipeline that declares `x-nils.on-arrival` and is the run of a step of its own with the chosen model (body part, post-contrast) is queued by a dataset's bring-in over the stacks its batch brought, as the step's door queues it, with the parameters the declaration gives, under the grants and detail the bring-in was queued with; post-contrast after the body part's run, since it reads its answers. Amended in place on 2026-10-16 by record 55, ruling 35 (b): a bring-in queues System 1's run alone; body part and post-contrast are runs a person starts from the dataset's step, and their declaration gives the parameters of a run `nils score --step` asks for. A pipeline that declares `x-nils.cache` is given the working place's `slice-cache/` read-write at `/cache` and a `stacks.json` naming each file's SOP Instance UID and, where a run reported reading the file whole and the file has not changed since, the sha256 of its bytes; an image looks in the cache before it reads a file, keys what it keeps by those identities and the input's version, never a registry id, and names the files it read whole in `results.json` (`read`). An input version no admitted or promoted model reads is dropped 30 days after it went unread.
