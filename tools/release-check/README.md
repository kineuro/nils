# The release gate

Wave 3's gate (`docs/specs/wave3-anonymize-and-bids.md`, §12): everything from a
tree of DICOM to an encrypted archive somebody could actually be sent, asserted
rather than eyeballed.

```sh
tools/release-check/gate.sh /scratch/nils/release-gate
```

It builds its own registry from its own corpus and refuses to write into a
directory that already exists, because a gate run that writes into another run's
directory costs an hour of bisecting.

## What it runs on

`nils-dicom`'s `reference` example: one person, two occasions, and one of every
case the release has to get right. A contrast BIDS names and one it does not, a
scanner derivative it names, a scout, a screenshot, two echoes, a second
acquisition of the same thing, a post-contrast repeat, and a functional series
nobody has said a task for.

It is synthetic, so nothing in it derives from any registry and anyone can
regenerate it (C10). **Its right answers are in `reference.toml`**, read off a
run and checked one at a time. The generator does not compute them, so a change
in the pack, in either grammar or in the routing shows up as a difference
rather than moving with the code. When a difference is right, that file is the
thing to edit, and the edit is the record of the decision.

## The bars

The first, the repairs, is `tools/repair-check/gate.sh`: it is about digest and
this is about release. The rest are §12's, in `check.py`, and every one of them
reports rather than stopping, because a run that says one thing is wrong when
four are wastes three runs.

| | |
|---|---|
| 2 | every name in the raw tree, against the schema the engine carries |
| 3 | the reference answers, in both layouts |
| 4, 5 | every stack placed, the counts reconciling, and nothing unnamed |
| 6 | one stack per session and role, with a margin so a tie can be seen |
| 7 | every file to its stack, every value to who decided it |
| 8 | no value the source carried appears in anything released |
| 9 | a second run of one release writes nothing |
| 10 | the time in the standard's column is the registry's |
| 11 | the handover verifies and accounts for every file |
| 12 | the budget, measured and printed |

Bar 2 is structural and runs everywhere: the official `bids-validator` needs a
network and a node, and a gate that only runs where those exist is a gate that
does not run. The schema it checks against is `bids-schema.json`, written by
`tools/bids-schema/extract.py` from the published schema, the same generator
that writes the engine's copy, so the engine and the thing that checks the
engine cannot drift apart.

Bar 8 reads bytes rather than tags, because the claim is about what leaves: no
value the source carried appears in anything released, wherever it might have
been copied to. One file per stack directory, since every file of a stack went
through one scrub with one plan.

## What it found the first time it ran

Three things, all of them in code that had tests and passed them:

- **`post_contrast` never reached a filename.** The axis stores `1`, after v0's
  integer column, and the release compared it to `yes`. No released file, in
  either layout, had ever carried `_CE` or `ce-`.
- **The orientation never reached an `acq-` label**, because the pack's tokens
  were the abbreviations §9.1 renders (`Ax`) and the column holds the word
  (`Axial`).
- **`RawRecon` was in every `acq-` label**, saying nothing: it is the
  provenance axis's default, so it holds wherever nothing else claimed the
  stack.

None is the kind of thing a unit test sees, because each is a disagreement
between two things that are separately right.

## Skipping

`dcm2niix` and `7z` are prerequisites of a deployment, not of a checkout. When
one is absent the gate says so and skips the bars that need it, so the tool is
useful on a laptop and complete in CI.
