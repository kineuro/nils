<!-- SPDX-License-Identifier: Apache-2.0 -->

# The pipeline pack contract

What NILS adds to a pipeline beside its descriptor (record 57, P1 to P4):
`nils-pack.yaml` beside `nils.job.yml`. The descriptor stays the knob layer:
the image, the command, the inputs as Boutiques knobs with their constraints
(`requires-inputs`, `disables-inputs`, `value-choices`, groups), the outputs
and `x-nils`. The pack holds what the descriptor cannot say: what a session
needs to run, what each knob does and whether it breaks comparability, the
rules that choose each knob (or the tool's variant) per session from the
scan's facts and the person's goal, the checks that mark a unit flagged or
failed, the licence, the resources, the comparability key, the tests and the
source. It pins the descriptor by the digest the catalog already computes
(`pipeline.descriptor_sha256`).

**A pipeline version is the pair** (descriptor, pack). Changing either file
makes the name's next version, and `replaces` decides, as today, whether the
older stays selectable. A pack that does not load refuses the version at the
catalog, with the loader's words, as a descriptor with an unpinned image is
refused today. A version with no pack keeps the behaviour it has and reads
`pack: none` in the catalog.

`VERSION` is the contract version. **It changes only by a pull request that
says so**, in its title, and that bumps `VERSION` and adds a new `vN/`
directory beside the old one, which stays.

| version | documents | since |
|---|---|---|
| 1 | [`v1/`](v1/) | Wave 7b slice A1, 2026-10-18 (record 57 P1 to P4, spec sections 5.1 to 5.8) |

## What version 1 fixes

| document | what it fixes |
|---|---|
| `nils-pack.schema.json` | the pack: `nils_pack: 1`; `pipeline` (the name and `descriptor_sha256`); the sections below. The file is YAML; the schema is of the value it parses to. A reader keeps a key it does not know, so a later version can add one without breaking an older reader; a key this version names is checked. |
| `examples/samseg-lesions/nils-pack.yaml` | the SAMSEG example of the spec (section 5.2), which the engine's tests load |

### The sections

| Section | What it holds |
|---|---|
| `needs` | Per role, `hard` and `soft` needs. Each need: `when` (an expression over facts), `words` for the person, `source` (a paper, a maintainer's statement, or `policy`). `hard_if` makes a role's hard needs hard only under a goal or switch. Needs on links (`aligned(flair, t1w)`, under the role `link`) and on the host (`host.memory_gb`, `host.gpu_memory_gb`, a licence file, under the role `host`) live here too. |
| `knobs` | For each Boutiques input: `what`, `default`, `cost`, `changes` (the outputs it changes), `comparability` (`none`, `additive`, `breaks`) and `set_by` (`person`, `rule`, or `fixed`, never shown). |
| `goals` | The person's choices: `goal` (`fast`, `standard`, `research`) and the pack's own switches (`lesions: [on, off]`). |
| `variants` | The sibling pipelines a rule may route a session to, inside one family. |
| `rules` | Ordered; each `id`, `when`, `set` (knob values) or `route` (a variant), `why`, `source`. First match per knob wins. |
| `checks` | Each `metric`, `op`, `value`, `action` (`fail`, `flag`, `drop`), `source`; fixed thresholds as today's `x-nils.qc`; `cohort: true` with `stratum` and `min_sessions` for the cohort checks. |
| `licence` | `spdx`, `redistributable`, `site_file` (`env`, `mount`), `terms` (address and version), `accept` (whether an administrator must accept). |
| `resources` | Memory and minutes per unit as the input drives them, `gpu` (`none`, `optional`, `required`) and its memory. |
| `comparability` | The knobs and facts that enter the comparability key. |
| `tests` | A small public input and the outputs expected, run when the tool is installed or registered. |
| `source` | The image's OCI reference with `@sha256`, the version `probe` (a command and the text expected) and the `key_files` the fingerprint reads. |

### The expression language

A small language with no side effects: comparisons (`==`, `!=`, `<`, `<=`,
`>`, `>=`, `in`), `and`, `or`, `not`, and the named tests `present(role)`,
`aligned(a, b)`, `min(...)` and `max(...)`. No loops, no user functions, no
arithmetic beyond `min` and `max`. A fact is written `<role>.<fact>`
(`t1w.dimension == '3D'`); inside a role's own needs a bare fact is that
role's (`dimension == '3D'` under `needs.t1w`); a unit of the stacks layout
is the role `stack`. The goal and the pack's switches are bare names
(`goal == 'fast'`, `lesions == 'on'`); the host's facts are `host.cores`,
`host.memory_gb`, `host.gpu`, `host.gpu_memory_gb` and `host.runtime`; a
session's `age_years` and a cohort's `diagnosis` are bare.

The engine parses and type-checks every expression when the pack is loaded:
an unknown fact, a wrong type, or a value outside a fact's vocabulary refuses
the pack at load. One evaluator serves the pre-flight, the runner, the desk's
doors and the assistant. After the rules run, the engine validates the
invocation against the descriptor's Boutiques constraints: a rule can never
produce an invocation the descriptor forbids, and a rule whose `set` would is
refused at load.

### The facts

Facts come from the registry, never from a pack's own reading of a header.

| Level | Facts |
|---|---|
| A stack in a role | `base`, `dimension` (2D or 3D), `voxel_max_mm`, `voxel_min_mm`, `slice_thickness_mm`, `field_strength_t`, `post_contrast` (given or not_given), `vendor`, `body_part`, `skull_stripped`, `defaced` |
| A session | the roles present (`present(role)`); `aligned(a, b)` from a registration step or a check; `age_years` where detail allows |
| A cohort | `diagnosis` from the clinical pack's imports |
| The host | `host.cores`, `host.memory_gb`, `host.gpu`, `host.gpu_memory_gb`, `host.runtime` |
| The person | `goal` and the pack's switches |

A fact the registry cannot tell is `unknown`. A hard need on an unknown fact
skips the session with "cannot tell: <fact>", and a person may let it run
(the unit then records the person as the one who chose). A soft need on an
unknown fact adds its caveat.

### Who chose

Every unit records, for each knob, `{value, by, rule, facts}`, where `by` is
`default`, `rule:<id>`, `person:<user>`, or `person:<user>` with
`proposed: assistant` when a person accepted the assistant's proposal, and
`facts` are the facts the rule read. Replaying the pack's rules on the
recorded facts gives the recorded values.

### The two keys

Every result carries two keys the engine computes from the pack and the run:
the **comparability key** (the tool, its version, the image digest or the
registered install's fingerprint, the pack's digest, the variant, every knob
whose comparability is `breaks` with its value, the input contrast set, the
converter and its version for a BIDS input, and, recursively, the
comparability keys of the steps it read) and the **result key** (the
comparability key plus every remaining knob value and the input stacks by
content). A measure over results with more than one comparability key is
answered per key, or refused in the same words at every door.
