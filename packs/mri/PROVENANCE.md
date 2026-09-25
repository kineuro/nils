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

Against v0's whole pipeline, with its routes and its exclusions:

| axis | stacks | differences |
|---|---:|---:|
| provenance | 518,365 | 0 |
| modifier | 518,365 | 0 |
| construct | 518,365 | 0 |
| body_part | 518,365 | 0 |
| post_contrast | 518,365 | 0 |
| base | 518,365 | 6 |
| technique | 518,365 | 11 |
| directory_type | 518,365 | 15 |
| the normalizer | 386,488 series | 0 |

The 32 remaining have a named cause and not a class label: **every one of the
base and technique differences is an EPIMix stack** (11 of the corpus's 437),
where the route's choice between a single-shot and an echo-planar readout
differs from v0's in a detail; the intent differences are 6 of those and 9
stacks no route touches. They are open items for the wave's gate, which does
not pass with open items.

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
| two of the ten `technique_inference` entries | `SWI` and `DSC-EPI` are not values of v0's own technique axis, so they can never fire |
| the perfusion guard on the anat intent rule | `DSC`, `DCE` and `ASL` are not values of v0's modifier axis, so the guard can never block |
| the `PhaseMap` rule of the intent cascade | no construct of that name exists and none was ever written |
| `is_dixon_water`, `is_dixon_fat`, `is_dixon_in_phase` and `is_dixon_out_phase` | v0 reads none of the four either; the construct axis reads `has_water`, `has_fat`, `has_in_phase` and `has_out_phase`, and has carried the Dixon part on every stack that states it since it was written. Removed by record 37 rather than wired, because a second spelling of a decided fact is how two parts of one program come to disagree about it |
| the two `FatFraction` tests of the intent cascade | no rule writes `FatFraction`, in v0 or here, so the Dixon rule's third alternative and the dual-echo field map's third exclusion could never hold. Removed by record 41; the value stays in the vocabulary |

## What this pack has that v0 does not

v0 is the source of the vocabulary and not its limit. A survey of 437,069
classified stacks of the legacy archive on 2026-09-19 found 17,065 of them
stating an identity in `ImageType` that no axis read, thirteen of those
identities sitting in flags v0 parses and reads nowhere. Record 37 slice S5
gives each of them an axis. What is new here, and so is nobody's transcription
of v0:

| value | axis | what it says |
|---|---|---|
| `Composed` | construct | one image built out of several, `COMPOSED` and the `COMP_*` spellings; 2,792 stacks, telling 2,525 colliding names apart |
| `EchoCombined` | construct | the image written beside the echoes it combines, `MEAN`; 1,154 stacks, 308 colliding names |
| `TTestMap` | construct | a statistic over a series rather than an image of a person; 357 stacks, 209 colliding names |
| `Qmap` | construct | a quantitative map that does not say of what |
| `MAVRIC` | technique | the metal artefact technique, and its composite is a composed image of one |
| `Distorted`, `InputUnavailable`, `Encrypted` | quality | what a file says is wrong with its own image: no distortion correction, a missing input, unreadable pixels |

The `quality` axis itself is new. v0 has no concept of it, and neither had
this pack: three tokens that answer neither what an image is nor how it was
made, and that are the whole reason two otherwise identical stacks are not
interchangeable.

## The pattern behind half of these

Seven of the entries above are the same mistake: **a name used in one place
that exists nowhere else**. v0 has no check that a value named in a priority
order, an inference map, or an intent rule is a value some axis actually
declares, so each of them fails silently and forever. A pack refuses to load
when a rule names a value outside its axis's vocabulary, and that one check
found all seven, in an afternoon, without anybody looking for them.

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

## Where this pack departs from v0 on purpose

Record 41 read five Siemens scanner protocols against their printouts
(`studies/2026-09-24-v1-on-the-protocols/` in the design repository, 23,136
stacks) and found rules v0 wrote, and v1 transcribed, that the printouts
prove wrong. Each is changed here, with a case in `corpus/` that says so, and
v0 still gives the old answer:

| rule | in v0 | here, and why |
|---|---|---|
| technique `MS-EPI` (RESOLVE) | also by the combination segmented k-space plus EPI | only by the readout-segmented sequence name (`*re_b`) or its words. Siemens writes `SK` on every EPI; the combination called 2,916 single-shot diffusion stacks, 462 BOLD stacks and 403 ASL stacks RESOLVE |
| modifier `FlowComp` | also by the words `flow comp`, `flowcomp`, `gmn` and `fc` | only by ScanOptions: `FC`, and GE's `FC_SLICE_AX_GEMS` and `FC_FREQ_AX_GEMS`. A protocol named `0 flow comp` with flow compensation off was FlowComp on 212 stacks |
| body part | one text of the series' names and `BodyPartExamined`, a spine word anywhere winning | the series' own names first, then a spine receive coil, then `BodyPartExamined`, which is written from the exam's registration and says the same on every series |
| construct `TTestMap` (v1's own) | not in v0 | not on an ASL series, where Siemens writes `TTEST` on the perfusion-weighted image |
| the SWI route | its last resort calls every unnamed output the SWI image | an MPR plane is a reformat first, as every other MPR is |
| disposition | no disposition in v0 | a `MOCO` copy and a `SUB` subtraction, both computed by the scanner, are `scanner_derived` |

## The private dictionary

`private-dictionary.tsv` is generated by `tools/private-dict/extract.py` from
pydicom's `_private_dict.py` (tag v3.0.1), which pydicom itself generates from
the GDCM project's private dictionary. It is data about what vendors call
their private elements, 449 creators and 10,507 elements (38 wildcard entries
that name a range rather than an element are left out; 45 more are case
variants of one creator and fold together on load, so a loaded pack reports
442 creators and 10,462 elements), and it travels here
as pack data so that `(0019,xx0C)` reads as `B_value` and the bytes of an
implicit VR file are read as the number they are.

pydicom is distributed under the MIT licence, copyright the pydicom
contributors; GDCM under the BSD 3-clause licence, copyright Mathieu Malaterre
and the Insight Software Consortium. Both permit redistribution of the data
with their notices, which this file is. Nothing in the dictionary comes from a
scanner or a cohort of ours.

Two things the dictionary and the pack disagree on, kept in the open rather
than resolved by picking one: it names `GEMS_PARM_01 (0043,xx30)` a vascular
collapse flag where v0 and Wave 3 section 6 read the number of gradient
directions from it, and it names the number of diffusion directions at
`GEMS_ACQU_01 (0019,xxE0)`; and it has no name for four of the Siemens group
`0051` elements the surveys found varying on every acquisition, which the
pack names from what the console prints. Wave 4a slice 3 settles both against
the surveys.

## Versions after the transcription

| version | what changed |
|---|---|
| 0.4.0 | `body_part` gains `chest`, which only a model's proposal writes. Values later gained `terms` and `description`, display only, which change no verdict and so no version. |
| 0.5.0 | Role `t1w` does not take a T1-weighted FLAIR (a T1w whose modifier holds FLAIR): it is rare and special and is not used as a session's main T1w for analysis. A T1-FLAIR is a candidate for no role. |
