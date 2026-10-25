<!-- SPDX-License-Identifier: Apache-2.0 -->

# The workflow contract

A workflow chains pipelines (record 57 P7, Wave 7b spec section 8):
`nils.workflow.yml`, kept in the catalog beside the pipelines and versioned
as a pipeline is. Its `steps` are each a pipeline or a family of the catalog,
at a version or `current`, with the goal and the knobs a person may set; its
`links` wire an output of one step to an input of a later one. Outputs are
typed by the descriptor that declares them (`x-nils.outputs[]`): the
derivative kind (`mask`, `image`, `transform`, `table`), the `role` and
`contrast` they stand for and the `grid` or `space` they are on; a typed
input (`x-nils.inputs[]`) may declare the `role`, `contrast` and `grid` of
the result it takes. Those typings live in the job contract
(`contracts/job/v1`), added in place and additively.

**The links are checked when the workflow is planned**, before anything
runs (section 8.3, G12), against the catalog's descriptors and packs, and
each gets one of five verdicts with words: `satisfied` (the output meets a
need of the next step, as registration meets `aligned(flair, t1w)`),
`missing` (a need no step meets, said when the plan still accepts the step
with its own check on), `redundant` (a preprocessing the next step does
itself, or one that may harm it), `refused` (a type that does not meet the
input, both types named) and `fed` (a typed input fed by a step, the plan
naming what the key carries). What a pack says of its links is the pack's
`links` section (`contracts/pipeline-pack/v1`): `provides`, `redundant`,
`harmful`.

**A change makes the document's next version** in the catalog, and
`replaces` decides whether the older stays selectable, as for a pipeline.
A family the catalog does not hold refuses the plan, never the document:
the registration family lands beside this contract.

`VERSION` is the contract version. **It changes only by a pull request that
says so**, in its title, and that bumps `VERSION` and adds a new `vN/`
directory beside the old one, which stays.

| version | documents | since |
|---|---|---|
| 1 | [`v1/`](v1/) | Wave 7b slice A7, 2026-10-25 (record 57 P7 and P8, record 58 P5, spec section 8) |

## What version 1 fixes

| document | what it fixes |
|---|---|
| `nils.workflow.schema.json` | the document: `nils_workflow: 1`, `name`, `title`, `what`, `replaces`, `steps` and `links`. A reader keeps a key it does not know; a key this version names is checked. |
| `examples/cortical/nils.workflow.yml` | the smallest workflow: SynthStrip, then recon-all |
| `tests/links.yaml` | the link fixture set of G12: workflows over the shipped pipelines, each link's expected verdict and the words it carries; the engine's contracts test plans every case and holds the plan to it, 100 % |

The two workflows the engine ships are in the repository's
`pipelines/workflows/` folder (`flair-lesions` and `cortical`), seeded into
the catalog as the starter pipelines are.

### A step

| key | what it holds |
|---|---|
| `id` | the step's name within the workflow, which links name |
| `pipeline` or `family` | a catalog name; one of the two |
| `version` | `current` (the default) or a version number |
| `goal` | `fast`, `standard` or `research`; the pack judges the step under it |
| `knobs` | the knobs a person may set when the workflow runs; the rest are the rules' or the default |
| `switches` | the pack's switches fixed for the step, such as `lesions: on` |

### A link

`from: <step>.<output>` names an output the step's descriptor declares;
`to: <step>.<input>` names a pick role of the later step (`flair`, `t1w`) or
one of its typed inputs (`synthseg`, `brain_mask`). A link runs forward
through the steps, and an input is fed by one link at most.

### What the plan answers

`POST /api/workflows/{id}/plan` (and `nils workflow plan`) answers, per
link, `{link, verdict, words, from: {step, output, type}, to: {step, input,
type}}`, and per step the pre-flight of its pipeline over the scope, with
each unit's status from the pack. A step that names a family the catalog
does not hold, or a pipeline it does not hold, is `refused` with the name,
and so is every link to and from it.
