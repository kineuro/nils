# The MRI pack

The first-party modality pack for MRI: the series classification rules, the
roles and the vocabulary that NILS v0 accumulated over years of clinical
research data, carried into v1 in meaning rather than by copying code (D12).
The pack format was decided by a prototype on the hardest v0 pieces before the
second wave (C11, `spikes/pack/`), and it is data: no code, no escape hatch.

## What is here today

| file | what it holds |
|---|---|
| `pack.yml` | identity (name, semantic version, contract, modality) and the buckets a site may amend |
| `parsers.yml` | five parsers and their 220 predicates, one DICOM string in, named booleans out |
| `flags.yml` | v0's 138 unified flags, and the seven booleans it keeps as methods on its classification context |
| `corpus/` | what this pack's author says it does; the engine will not load the pack unless every case holds |

The parser and flag layers were checked against v0's own code over the whole
live corpus, 518,365 stacks by 138 flags, with no disagreement. The axes, the
routes, the intent cascade and the passes land in slices 3, 4 and 6 of Wave 2
(`docs/specs/wave2-fingerprint-and-classify.md`, §13).

## How to change it

Anything that changes a verdict changes the version, and a **vocabulary**
change (an axis value, a directory type, an identifier namespace) is a major
bump, because a federated question asked for pack 2 must not be answered by
pack 3's vocabulary (D26). Every classified row records the pack name, version
and contract that judged it, which is what turns a re-classification from a
blind overwrite into a diff.

If the change is one site's word for the same thing, it is an **overlay** and
not a change to this pack: provenance-scoped, applied at load, recorded on the
row, and carrying its own cases (§5.3). A site never forks the pack to add a
contrast agent.

    nils pack validate packs/mri
    nils pack show mri --pack-dir packs

Packs are AGPL-3.0-only like the engine. Third-party packs for other modalities
follow the specification in `contracts/pack/`.

## The passes

`passes/physics_vote.yml` is the pack's one pass: v0's physics vote, as a
configured instance of the engine's `nearest_neighbour_vote` kind. Every
number the algorithm uses is in the file, including the rounding mode, because
Python rounds a half to even and Rust rounds it away from zero and a TR of
exactly 50 ms would otherwise fall in a different bin.

Checked against v0's own `sort/gap_filling.py` over the whole live corpus, with
the reference held equal so that the algorithm is compared and not the pool:
**518,057 of 518,353 stacks get the same answer by the same method**. Every one
of the 296 differences is a tie, where two answers are equally popular: v0
takes whichever the database returned first, and this pack takes neither.

Run as it will run, against the reference v1 declares, 8,020 stacks differ.
The extra 7,724 are the reference itself: v0 votes against its own table, which
already holds the answers earlier votes wrote, so 34,497 of its 397,712
reference stacks were themselves filled by voting. v1 votes against what the
rules decided.
