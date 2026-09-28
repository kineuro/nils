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
| 0.6.0 | Record 48: the constraints between axes, checked against public sources (`studies/2026-09-25-pack-constraints/` in the design repository, 74 sources, below), and the round 4 contrast phrases. Pack contract 6. **Vocabulary:** technique `DCE` is a value of its own (a T1-weighted 3D spoiled gradient echo, not EPI), and takes the word `dce` from `Perfusion-EPI`, which is now the DSC series; technique `ASL-EPI` is `ASL`, readout-neutral, and `ASL-EPI` stays an alias, so a campaign, an overlay or a decision that names it reads as `ASL`; the stored label `ASL` did not change, and its BIDS `acq-` token is `ASL` for `ASLEPI`. **Rules:** base runs after construct, so a base rule may read it; an ADC, eADC, FA, MD, trace or isoDWI construct is base DWI, a CBF, CBV, MTT, Tmax or TTP construct PWI, a SWI construct SWI; INV2 is PDw, not T1w, and an MP2RAGE is T1w except its INV2; an INV1, INV2 or UNI construct is technique MP2RAGE where nothing decided it; a water or fat image holds modifier Dixon; a DSC or DCE series is post-contrast; a multi-echo GRE is T2*-weighted only where the file says no Dixon or in and opposed phase, and no longer a constraint. **Exclusions (hard):** a SWI construct or base, a T2*w base and a QSM or R2* map rule out the spin-echo family; a TOF rules out base T2w; a MIP or MinIP rules out provenance RawRecon and Localizer. **Hints (soft):** BOLD and multi-echo GRE usually T2*w; STIR and ASL usually before contrast; a synthetic contrast usually SyMRI; PSIR usually T1w; DIR usually T2w (one source); HASTE usually T2w. **Words:** terms corrected (TIRM off STIR, bare TFL off MPRAGE, T2* map off R2* map, 3D DRIVE off CISS, `hyperecho` off RESTORE, bare `dual echo` off MESE) and some 40 Canon, Hitachi and Fujifilm names added; the keyword `3d drive` left CISS, and Inhance Inflow IR is no TOF. The contrast buckets gain the round 4 phrases in the spelling the normalizer leaves (`pre - contrast`, `f re k` for `före K`), including `kontrast` and `efter k`; `med k` does fire on a series' own words (a case in `corpus/` says so), so why the training sites' stacks that carry it came out empty is a question for those stacks, not the word. FatSat gains `fettsat` and `fat supp`. |
| 0.7.0 | Record 50 R2: `body_part` gains `other`, the unknown: whatever is none of the five, or cannot be named from the stack. Like `chest`, no rule writes it: it is a person's answer or a model's proposal, so the rules' verdicts, and the comparison with v0, do not change. `brain` and `brain-neck` gain the description the raters read them by: brain-neck covers the brain and reaches below C2. No BIDS `acq-` token, since an unknown part names nothing. |
| 0.8.0 | Body part. Philips' `VERTEBRALCRANIUM`, stated for its neuro exams, spine and brain alike, and no DICOM defined term (D7), no longer decides: its `vertebral` made every stack under it whose own words said nothing a spine, the brain protocols among them, so the stated-anatomy normalizer drops it and the series' words, the coil or the geometry decide, or nothing does. The spine rule gains the German `WS` for the spine (G5), spelled as a word by the anatomy normalizer, and a range of vertebral levels (`C2-Th1`, `Th4-9`, `C4-L1`), the German thoracic letter being `Th` (G4); `HWS`, `BWS` and `LWS` were there already and now cite G1 to G3. Estimated on the archive (2026-09-27): of 1,090 `VERTEBRALCRANIUM` stacks, 264 that were a spine by the stated body part alone name no part now, 92 stay a spine by their portrait sagittal geometry and 57 by the new words; 6 spinal series in exams registered as a brain or a head become a spine. |
| 0.9.0 | Nima's rulings of 2026-09-27 and 28 (`studies/2026-09-27-read-r1-notes/` and `studies/2026-09-27-header-judge/` in the design repository). **MP2RAGE:** both inversions, INV1 and INV2, take base none, and the construct carries the meaning, as BIDS files them `inv-1` and `inv-2` of the suffix `MP2RAGE` (B1, P9, MQ6); an exclusion rules every base out beside one; the uniform image stays T1w. This replaces 0.6.0's "INV2 is PDw". The BIDS mapping was already right and is now tested against the pack, and a suffix's `when_technique` is checked at load. **Dual-echo PD/T2:** a dual-echo FSE or TSE, split into one stack per echo, is technique TSE, never ME-SE (M1, MQ20, RP14, B1); its base is its echo's, PDw under 40 ms and T2w from 40 ms, v0's cut, which the published echoes clear; ME-SE keeps a conventional multi-echo spin echo of 3 or more echoes or with no echo train, and GE's `memp` with one echo is a plain spin echo; a dual-echo single shot stays SS-TSE. **Provenance:** an acquisition's own images are RawRecon and what the scanner computed from them takes the product. An SWI's magnitude and phase, a BOLD or ASL series as acquired and MAGiC's MDME images are RawRecon; the SWI image, QSM, R2* and MIP stay SWIRecon, an ASL's CBF map and subtraction ASLRecon (no longer PerfusionRecon), an fMRI map or motion-corrected run BOLDRecon, the synthetic contrasts and maps SyMRI. The SWI and SyMRI routes decide the provenance again for those outputs (the engine's new `redecides`), and the MDME images stay the working scan. **SWI base:** the SWI image SWI, the magnitude T2*w, the phase none; its minIP ProjectionDerived with base SWI. **GE ScanOptions:** `FSA_GEMS`, `FSP_GEMS`, `FSS_GEMS`, `FSI_GEMS`, `FSR_GEMS` and `FSL_GEMS` are the readout direction, never FatSat; FatSat's GE evidence is the defined term FS, and corpus cases hold it. Estimated on the archive (2026-09-28, 558,405 stacks classified on 0.7.0, a read-only query rather than a rerun): 1,469 MP2RAGE inversions lose their base; 24,812 dual-echo stacks move from ME-SE to TSE and 692 single-echo ME-SE to a spin echo, 38 change base by their echo; provenance moves on 8,806 SWI magnitudes and phases, 3,751 SWI minIPs, 1,433 BOLD and 314 ASL series, 33,997 MDME images, and 5 ASL CBF maps; 4,394 SWI magnitudes become T2*w and 4,440 phases lose their base; none of the 28,843 stacks with a GE readout token had FatSat from it. 75,310 stacks change on some axis. |
| 0.10.0 | fMRI, perfusion and the gradient-echo family (`studies/2026-09-28-sequence-research/` in the design repository: two independent research reports, `our-research.md` and `deep-research.md`, reconciled in `reconciled.md` and checked read-only against the archive). Only changes both reports support, or that the archive settles, and no new vocabulary. **Order:** the EPI time series run diffusion, ASL, DSC, BOLD, then SE-EPI and GRE-EPI (both reports: `PERFUSION` covers ASL, DSC and DCE, and DSC shares BOLD's gradient-echo EPI); the gradient-echo family runs angiography, the magnetisation-prepared, the coherent steady states, combined multi-echo before multi-echo, VFA, VI-GRE, FSP-GRE, SP-GRE, SS-GRE, GRE. **BOLD** from the header: an EPI time series (10 or more time points, a Siemens mosaic of 10 or more images) with no diffusion, perfusion or ASL token, no contrast and no perfusion word; a single-band reference (one volume, named `sbref` or on Siemens numbered 1000 above its run, dcm2niix issue 816) is BOLD-EPI too, as evidence only. **DSC** from a perfusion image type that is not ASL's, a map token only DSC makes (rCBV, MTT, TTP, Tmax, leakage, PBP, K2; not CBF, which ASL makes too), or an EPI time series given contrast (the tag, or `gd`, or the contrast words) or named for perfusion; a map named rCBV, rCBF or rMTT whose image type says nothing. **DCE** from the header: a 3D spoiled gradient echo of 10 or more time points that is no angiography run (TWIST, TRICKS, MRA words) and no PRESTO (QIBA DCE profile: at least 5 baseline phases, 40 to 80 phases; the archive: BOLD 100 or more time points, DSC 40 to 90, multiphase liver at most 5, and every 3D gradient echo of 10 or more time points a contrast-enhanced angiography run). **Words:** `bold`, `task`, `asl`, `pasl`, `casl`, `pcasl`, `dsc`, `pwi` and `dce` count only as whole words (`asl` inside `basline`, `bold` inside VenBOLD); VenBOLD, Philips' SWI, is SWIRecon's word; the technique axis reads `technique_text`, which leaves GE's exam-level ProtocolName out (the engine's `unless_manufacturer`); an EPI technique's word is refused where the header states a readout that is not EPI, and BOLD's where the file says fewer than 10 time points. **Names before ScanningSequence:** a Siemens gradient-echo stem is never a spin echo (`*ci3d1` and `*tfi2d1` are written SE), `epfid` is GRE-EPI and `epse` SE-EPI whatever ScanningSequence says (dcm2niix issue 309); the stems are anchored (`fl2d` is inside `tfl2d`); Siemens `fi` is FISP, `de` DESS, `ps` PSIF, and SequenceVariant TRSS is PSIF (the Siemens brochure's legend, DICOM PS3.3). **Philips (2001,1020):** read at last: T1FFE spoiled, T1TFE and TFE the turbo field echo, B-FFE and B-TFE balanced, T2FFE PSIF, FEEPI and SEEPI the EPI readouts. **VI-GRE** by name only: no header field marks interpolation, so an unnamed 3D spoiled GRE is SP-GRE (both reports). **GE `MP_GEMS`** is no magnetisation preparation (117 of its 255 stacks are EPI). **MoCo:** Nima's ruling (2026-09-28), "if Motion Correction is ORIGINAL\PRIMARY then it is not derived": a motion-corrected copy keeps modifier MoCo, and its provenance and disposition follow value 1 of its image type, BOLDRecon or PerfusionRecon and scanner_derived only when DERIVED; a computed map is still decided by its own token, never by value 1 (Siemens writes some maps ORIGINAL, dcm2niix issue 243). Estimated on the archive (2026-09-28): the rules of 0.9.0 and 0.10.0 replayed over the 558,405 MR fingerprints, without the passes; 9,675 stacks change on some axis: technique on 7,894 (3D spoiled GRE from VI-GRE to SP-GRE 4,042; Philips TFE and GE FSPGR to FSP-GRE 3,184; CISS and TrueFISP out of SE 147; Philips balanced to bSSFP 37; Philips FE-EPI SWI to EPI 129; BOLD lost to VenBOLD 18, to GE reverse-phase field maps 33 and to GE exam-protocol words 11; DSC gained 17 and lost 10 to exam-protocol words; ASL lost 16 to `basline`; no stack became DCE), provenance on 1,790 (BOLD MoCo to RawRecon 1,759, DSC MoCo to PerfusionRecon 22), base on 308; and 1,780 motion-corrected copies written ORIGINAL are no longer scanner_derived. |
| 0.11.0 | The decisions ruled on 2026-09-28 (`studies/2026-09-28-sequence-research/decisions.md` in the design repository) and the rule bugs of two header re-measures (`studies/2026-09-28-judge-remeasure/` and `-remeasure-2/`). **Constructs:** `DeltaM`, the ASL label-control difference (Siemens `ASL` with `SUB`, `SUBTRACTION` or `TTEST`; GE `PERFUSION_ASL` or (0043,xxA3) PSEUDOCONTINUOUS; Philips' `PERFUSION` image of an ASL), base PWI, ASLRecon, BIDS `perf/asl` with `aslcontext` `deltam`; `M0`, GE's proton-density pass (`ORIGINAL\PRIMARY\ASL` or (0043,xxA3) CONTINUOUS) or a series named M0 beside an ASL readout, base PDw, BIDS `perf/m0scan`; `SBRef`, the single-band reference (named, or on Siemens numbered 1000 above its run, dcm2niix issue 816), BIDS `func/sbref`; `FieldmapRef`, an EPI of a few volumes named for a reversed phase encoding or GE's `epi_pepolar` (0019,xx9C), BIDS `fmap/epi`, and no longer a B0 map. ATT, Ktrans, Ve, Vp, Kep, iAUC and Mean are deferred until a dataset holds one; one PWI base stays for all perfusion. **Private elements** read from the next digest on: GE (0019,xx9C), (0019,xx9E), (0043,xxA3) to (0043,xxA5), (0043,xx2F), (0043,xxB6); Philips (2001,xx81) and (2005,xx29). **Rule bugs:** Nima's ruling 7 (2026-09-27), an ADC, eADC or trace from a plain DWI is RawRecon and only a tensor's outputs are DTIRecon, never the acquisition; an ADC is not also Trace (`3scan_trace`) or eADC; the localizer words `3 - pl`, `- sikt` and `surstar` as the normalizer spells them, and a tri-plane scout by its shape (split by orientation, at most 5 slices, no projection); a 3D spin echo with a train of 100 or more is 3D-TSE, and a CUBE reformat keeps it though excluded; `DIS3D` and `DIS2D` stand for the acquisition type only where the file states none; Siemens `tir` is a turbo IR; on GE a b value alone is no diffusion on a stated non-EPI readout; a Siemens `fldyn3d` phase with `TTC` in its name is DCE; GE `MT_GEMS` is MT; a `PROJECTION IMAGE\VASCULAR` is a MIP; SyMRI acquisition images keep ND; a projection named MPR is an MPR; `T1W_FFE` is SP-GRE; a gradient echo named T2 with TE over 15 ms is T2*w [P8, MQ8]; post-contrast words are read from `technique_text`, without GE's exam-level ProtocolName; the physics vote does not fill the base of a phase image or an MP2RAGE inversion; an EPI SWI with a gradient-echo readout is GRE-EPI. Counted read-only on the archive (2026-09-28, 558,405 stacks on 0.10.0; stored verdicts and header fields, not a replay): DTIRecon on a plain DWI map 20,950 and on an acquisition 856; ADC with Trace 7,263, with eADC 347; localizers missed by the words 3,983 and by the shape 4,198 (overlapping); 3D TSE reformats 43,990 (12,226 already SPACE; 4,158 voted TIRM, 12 other and 27,594 with no technique now SPACE); 2D written DIS3D taken as SPACE 734; Siemens `tir` written SE 1,882; GE stray b value called DWI-EPI 1,782; `fldyn3d` phases with TTC 652; GE MT_GEMS without MT 221; vascular projections without MIP 76; MDME without ND 6,383; projections named MPR 743; phase images given a base by the vote 5,120; GRE named T2 written T2w 272; `T1W_FFE` written GRE 334; GE post-contrast given only by the exam ProtocolName 15,339; EPI SWI written EPI 1,320; DeltaM 365, GE M0 15, M0 by name 2, SBRef 0, FieldmapRef about 69 candidates. |

## The sources of record 48

The ids that `rules/base.yml`, `rules/post_contrast.yml`, `rules/implied*.yml`, `excludes.yml`, `hints.yml`, the axis files and, from 0.8.0, `rules/body_part.yml` and the two anatomy normalizers cite, and from 0.9.0 the SWI route and the dual-echo rules. `WS` (G5) rests on one publisher and on the archive, where the token stood only in spinal series. Every rule a campaign holds an answer to rests on at least two independent publishers; a hint may rest on one, and cites its counter-examples. The full quotes are in the study.

| id | source |
|---|---|
| D1 | DICOM PS3.3 C.8.13, Enhanced MR Image, Image Type values (NEMA, current) |
| D2 | DICOM PS3.3 C.8.13, Table C.8-81, the former Value 4 terms (NEMA, 2018c) |
| D3 | DICOM PS3.3 C.8.13.3, Table C.8-86, Acquisition Contrast (NEMA, current) |
| D4 | DICOM PS3.16 CID 7180 (NEMA, current) |
| D5 | DICOM PS3.16 CID 7263, 7271, 7272, diffusion (NEMA, current) |
| D6 | DICOM PS3.16 CID 4107, 4108, 4109, perfusion (NEMA, current) |
| D7 | DICOM PS3.16 Annex L, Correspondence of Anatomic Region Codes and Body Part Examined Defined Terms (NEMA, current) |
| B1 | BIDS specification schema, suffixes and anat |
| S8 | Ellingson et al., Brain Tumor Imaging Protocol consensus, Neuro-Oncology 2015 |
| MQ1 to MQ19 | mriquestions.com (A. D. Elster): BOLD sequences, DSC v DCE v ASL, trace and ADC, exponential ADC, making an SW image, MP-RAGE v MP2RAGE, TOF artifacts, multi-echo GRE, MERGE/MEDIC/M-FFE, TOF MRA, Dixon, STIR, HASTE, driven equilibrium, relaxation rate, PROPELLER, PSIF v FISP, DESS, synthetic MRI |
| MQ20 | mriquestions.com (A. D. Elster): dual-echo FSE, and SE versus multi-echo SE versus FSE |
| RP14 | Radiopaedia: dual echo |
| RP1 to RP13 | Radiopaedia: BOLD, MR perfusion, DCE, DSC, ASL, SWI, TOF angiography, DWI, Dixon, MIP, PSIR, DIR, pulse sequence abbreviations |
| IM1 to IM4 | IMAIOS e-MRI: first-pass perfusion, the 180-degree pulse, STIR and FLAIR, sequence acronyms |
| M1 | MR-TIP, a comparison of MRI acronyms used by manufacturers |
| W1 | SIIM, maximum intensity projection |
| V1 | Siemens Healthineers, MRI acronyms, cross-vendor comparisons v2 (2018) |
| V2 | Fujifilm, MRI acronym guide (2024) |
| V3 | Siemens, syngo MR pulse sequences application brochure (2013) |
| V4 | GE HealthCare, Inhance suite |
| T1 | dcm2niix source (rordenlab) |
| T2 | BrainVoyager user's guide, MP2RAGE background noise |
| P1 | Wang et al., spin-echo BOLD fMRI, NeuroImage 2021 |
| P2 | Zhang et al., Quant Imaging Med Surg 2022 |
| P3 | EADC and ADC, Appl Magn Reson 2013 |
| P4 | Haller, Haacke, Thurnher, Barkhof, SWI technical essentials, Radiology 2021 |
| P5 | Docampo et al., Neuroradiol J 2013 |
| P6 | QSM consensus (Bilgic et al.), MRM 2024 |
| P7 | Langkammer et al., fast QSM with 3D EPI, NeuroImage 2015 |
| P8 | Chavhan et al., T2*-based MR imaging, RadioGraphics 2009 |
| P9 | Marques et al., MP2RAGE, NeuroImage 2010 |
| P10 | MP2RAGE v MPRAGE morphometry in focal epilepsy, PLoS One 2024 |
| P11 | Brant-Zawadzki et al., MP RAGE, Radiology 1992 |
| P12 | van der Kouwe et al., MEMPRAGE, NeuroImage 2008 |
| P14 | Duffy et al., Pediatr Radiol 2021 |
| P15 | Liu et al., IDEAL T1 bias, MRM 2007 |
| P16 | Merkle and Nelson, dual gradient-echo in and opposed phase, RadioGraphics 2006 |
| P17 | Asiri et al., J Med Radiat Sci 2021 |
| P18 | Held et al., J Neuroradiol 2003 |
| P20 | Alsop et al., ASL consensus, MRM 2015 |
| P21 | Warntjes et al., MRM 2008 |
| P22 | Hagiwara et al., SyMRI of the brain, Invest Radiol 2017 |
| P23 | McCullum et al., J Appl Clin Med Phys 2025 |
| P24 | Tanenbaum et al., MAGiC trial, AJNR 2017 |
| P25 | Direct contrast synthesis from MR fingerprinting, arXiv 2022 |
| P26 | Fischer et al., Skeletal Radiology 2021 |
| P27 | Grade et al., a neuroradiologist's guide to ASL, Neuroradiology 2015 |
| P28 | Poonawalla et al., Radiology 2008 |
| P29 | Reis, Radiol Bras 2023 |
| P30 | Naganawa et al., Jpn J Radiol 2026 |
| P31 | Tang et al., IR-HASTE, J Magn Reson Imaging 1998 |
| P48 | Byun et al., Korean J Radiol 2008 |
| P49 | Headley et al., J Appl Clin Med Phys 2020 |
| P50 | Tamada et al., Sci Rep 2022 |
| G1 | Wiktionary (de), HWS: Halswirbelsäule |
| G2 | Wikipedia (de), Brustwirbelsäule: "Als Brustwirbelsäule (BWS) wird der Abschnitt der Wirbelsäule zwischen Hals- und Lendenwirbelsäule bezeichnet" |
| G3 | Wiktionary (de), LWS: Lendenwirbelsäule |
| G4 | Wikipedia (de), Brustwirbel: the thoracic vertebrae are named Th1 to Th12 |
| G5 | med-serv.de, Medizinische Abkürzungen, WS: Wirbelsäule |
