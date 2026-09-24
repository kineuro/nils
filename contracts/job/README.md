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
| 1 | [`v1/`](v1/) | Record 43 slice S1, 2026-09-24 |

## What version 1 fixes

| document | what it fixes |
|---|---|
| `nils.job.schema.json` | the descriptor: v0's Boutiques 0.5 subset with an `x-nils` block. The image by its registry manifest digest; the parameters (name, type, range, choices, default, unit); the command line with its value-keys; the analysis level (`participant`, `session` or `stack`); the input layout (`bids` or `stacks`); the typed inputs (`model`, `label_set`, `derivative:<kind>`); the outputs by derivative kind with path templates, each one per unit or, since the wave's rulings, one for the whole run (`level: run`), such as a model (`kind: model`, with the `card` beside it); the needs (`gpu: none, optional or required`, memory, cores); and the axes it may propose values on |
| `stacks.schema.json` | `/input/stacks.json` in the stacks layout: each stack's files under the source places mounted read-only, and the frames of a multi-frame file; each stack's orientation, its `body_part` and `technique` as the registry holds them now, and its slice count |
| `results.schema.json` | `/output/results.json`: each unit's status, files, metrics and error; the proposals (`proposals.schema.json`); the cards of the models the run used or made; and the `seeds` and `selection` it suggests a person curate, which are never proposals |
| `proposals.schema.json` | a model's proposals, per stack: the axis, the value, the probability of every class and the registered model |
| `embedding.md` | the embedding file a pipeline writes, one per stack and encoder |

## What a container meets

| where | what |
|---|---|
| `/input` | read-only: the BIDS tree (bids layout), or `stacks.json` (stacks layout) |
| `/source/<n>` | read-only, stacks layout only: the source places the manifest's paths are under |
| `/inputs` | read-only: `manifest.json` (the run, the pipeline, every parameter, the units, the models and the label set) and one folder per typed input |
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
