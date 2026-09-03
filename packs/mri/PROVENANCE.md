<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Where this pack's vocabulary came from, and what was left behind

Every value, keyword and flag here was transcribed from NILS v0 0.5.3, which
had accumulated them over years of clinical research data. The transcription is
mechanical, because the vocabulary is data on both sides and the transformation
is a rename; what makes it trustworthy is not the care taken over it but the
check afterwards, which is that both classifiers are run over the whole live
corpus and their answers diffed row by row (`tools/pack-check/`).

**The result, on 518,365 stacks of the live corpus, is zero differences on
every axis**, with nothing of v0's output in the input: the pack's own
normalizer builds the search text from the six raw DICOM text fields, its
parsers and flags read that, and its rule sets decide.

| axis | stacks | differences |
|---|---:|---:|
| provenance | 518,365 | 0 |
| technique | 518,365 | 0 |
| base | 518,365 | 0 |
| modifier | 518,365 | 0 |
| construct | 518,365 | 0 |
| body_part | 518,365 | 0 |
| post_contrast | 518,365 | 0 |
| the normalizer | 386,488 series | 0 |

Four of the seven are the compact axis form, where v0's own detector is
value-major. Three are written longhand, because v0's are not: **base** scans
six tiers and tries every value inside each; **body_part** collects every
category that matches and then applies a precedence; **post_contrast** lets the
DICOM tags settle it and only then listens to the text, where a word saying no
beats a word saying yes.

## What v0 carries and this pack does not

Each of these was found by transcribing, and each is recorded rather than
quietly dropped. None of them changes an answer, because v0 does not read them
either; that is the point.

| in v0 | why it is not here |
|---|---|
| `technique-detection.yaml`'s `confidence_thresholds: {high, medium, low}` | the detector uses its own constants, named `exclusive`, `keywords`, `combination`; the YAML block is read by nothing |
| `acceleration-detection.yaml`, all 364 lines | the acceleration detector's lists are Python constants; the file is loaded by nothing |
| `requires_derived` on all 43 constructs | the branch that would narrow the match is a literal `pass`, with a comment saying the author decided to allow it anyway |
| `provenance:` on six constructs | never read by the construct detector; those constructs come from the SyMRI and SWI routes instead |
| a per-value `confidence` on all 12 provenances | the detector takes its confidence from the tier, not from the value |
| `SWIProcessed` and `PhaseMap` in the construct priority order | no such constructs exist in the vocabulary, so nothing can ever match them; both were written **zero** times in 518,365 stacks, which makes every intent rule that tests for them unreachable |
| `exclusion_groups.IR_CONTRAST.fallback: IR` | the resolution reads each value's own `group` and `priority`; the top-level block is documentation |

## The one rule that was fixed, tried, measured and put back

v0's only conditional token replacement waits for `t1`, which an earlier
unconditional replacement has already turned into `t1w`, so it can never fire.
Writing it against what is actually there turns `mpr` into `mprage` on 15,858
series. That was tried on the whole corpus and it loses more than it gains:
**7,693 stacks lose the MPR construct and 7,725 lose the ProjectionDerived
provenance, against 1,207 gaining MPRAGE as their technique**, because in this
corpus `mprage` is usually already in the text and the separate `mpr` means the
reformat.

Whether `MPR 3D T1` with no `mprage` beside it is an MPRAGE is a radiological
question and not a programming one, so it goes to the verified corpus and not
into a pack on an engineer's say-so. The mechanism is in the format and tested;
the rule is out, and the numbers are here so that the decision can be taken on
them.
