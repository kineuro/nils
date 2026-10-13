# The final header certificate: the grade

2026-10-02. `plan.md`, sections 8 and 9. **Result (part 2): the certificate does not pass** (main 32 of 2,500, Korn-Graubard upper bound 1.72 % against the 1 % bar; every make passes 3 %). The final certificate grades MRI pack 0.23.0 by its stored answers (job `pack-0230`, engine 1.0.0-alpha.71) on the 3,750 stacks drawn and sealed as selection `final-cert@1`. This file holds counts only. Stack ids, values and the grouped error lists stay in the evidence runs.

## Part 1: the registration

**This part and `tools/grade/` are committed before any rule answer on a drawn stack is fetched, stored or replayed. The commit is the grade's registration.** At that commit:
- the final read's reference is sealed (`final-cert-read`, `reference.md`);
- its correction record is sealed (`final-cert-read-corrections`);
- pack 0.23.0's stored answers on the drawn stacks have not been fetched, and no pack has been replayed over the final read's packets;
- the scorer, `locbad.py` and `lines_final.py` have been dry-run on a synthetic party made from the corrected reference itself (part 1, "The dry run").

> **Warning: graded once.** Nothing found on this grade is fixed and graded on it again (record 48).

### What is graded

| line | pack | its answers | source |
|---|---|---|---|
| the certificate | MRI 0.23.0 | stored by production, job `pack-0230` (engine alpha.71) | `final-cert-rules-0230` |

- **The answers are fetched by Nima** on the NILS server with `tools/grade/fetch-server-final.sh` (dev-8's fetch with the job, the outbox and the purpose changed, and `--unsealed-access` with an audited reason, since the stacks are sealed; plan section 9).
  - It runs as `nils` with `NILS_PACK_DIR=the engine's install folder`: one ask, `nils explain` for the stacks the ask leaves out, and the stored votes cut to the stacks.
  - Before it reads anything it checks that every one of the 3,750 stacks holds job `pack-0230`, that all 3,750 are sealed under `selection:final-cert@1`, and that no classify job is unfinished. It stops otherwise.
  - The folder goes to the research container's inbox, comes to the development laptop by `mirror.sh pull`, and is sealed and mirrored as `final-cert-rules-0230`.
- **The stored answers become a rules party** by dev-5's `stored.py`, unchanged (labels mapped by pack 0.23.0).
- **System 1 round 9** is graded on its own registration once it is trained after the seal (`../2026-09-24-phase-0/learners-r9.md`); this part grades the rules alone. The rules-plus-System-1 line of kept-uncertain and void-kept waits for it.

### The reference

1. The final read's settled cells (`final-cert-read`, `scoring/out-reference/private/cells.tsv`).
2. **The correction record `final-cert-read-corrections`** (`rulings.py build --record final` as registered, X7-* and X8-BNR included).
3. **The localizers' record** (`final-cert-read-corrections`, `scoring/corrections-localizers.jsonl`), read by the localizers' line only.
4. **The pack-stands record:** `rulings.py pack --record final` from the pack's own answers (N7).

### The scoring, as dev-8's grade

- **The scorer** is `grade7.py` as dev-7's and dev-8's grades ran it, with the sealed checkpoint's `cert/` and `config-certificate.json`, `--strata-coverage`, and no `--subjects`. The derived five come from alpha.71's binary with pack 0.23.0's tree.
- **The ten axes:** provenance, technique, modifier, construct and base, and the derived role, directory type, disposition, convertible and quality. A stack is wrong when any of the ten is wrong or empty. Technique `Unknown` is empty. A base decided as nothing is `none`. N8: a stack settled Localizer is scored on technique by family.
- **The localizers apart** (`tools/grade/locbad.py`): provenance and technique by family against their settled cells as the localizers' record leaves them. No bar.

### The lines and the pass reading (plan section 8, `tools/lines_final.py`, unchanged)

- **Main, the claim:** the Hajek estimate on the 2,500 main reads with the Korn-Graubard one-sided 95 % bound over the design strata (make x field). Beside it: exact Clopper-Pearson, Wilson at Kish's n, Kish's n and n*, the Jeffreys aggregates, and the session-clustered bootstrap.
- **Per make:** each make's domain of the main sample; a make with 299 or more main reads is held to 3 %.
- **The supplement, apart:** per rare cell and cell group, and its own Hajek line.
- **Kept-uncertain and void-kept:** each its own Hajek and Korn-Graubard bound, by make beside.
- **The localizers, apart.**
- **The pass condition** (intersection-union): the main bound at or under 1 % and every make of 299 or more main reads at or under 3 %.

### What is reported

1. The certificate's statement: pass or not, with the main line and every per-make line.
2. Kept-uncertain, void-kept, the supplement and the localizers, apart.
3. Per axis: cells, wrong and empty.
4. Every error, read and grouped, one kind per stack, as dev-8's grade (a rule bug or gap, the reference stale, a new dispute, the header cannot tell, an artefact; kind f, convention after pack, is empty, since no ruling in the record is newer than pack 0.23.0).

### The code

`tools/grade/`: `run.sh` (bin, stored, stands, grade, lines, errors, pack), `fetch-server-final.sh`, `locbad.py`, `errors.py`, `dryparty.py` (the dry run only). From elsewhere, unchanged: dev-5's `stored.py`, `../2026-09-28-certificate/tools/rulings.py`, `../2026-09-24-phase-0/tools/r7/grade7.py`, the sealed checkpoint's `cert/`, and this plan's `tools/lines_final.py` and `stats_kg.py`.

### The dry run

On the development laptop, 2026-10-02, in a scratch folder deleted after (`W=… run.sh`): alpha.71's binary and `packs.tar.gz` checked against the release's `SHA256SUMS` (pack 0.23.0), and a synthetic rules party made from the corrected reference itself (`tools/grade/dryparty.py`).
- **The corrected reference itself:** 0 of 3,650 non-localizers wrong or empty; the pack-stands record empty; `lines_final.py` gives main 0 of 2,500 (Korn-Graubard upper 0.12 %), every make and every line apart at 0, the localizers 0 of 100, and the pass reading. N8 applied on 32 stacks.
- **Three planted technique errors:** counted 3 of 3,650 by `grade7.py`, and the one that falls in the main sample moves the main line to 1 of 2,500 (Korn-Graubard upper 0.19 %). `errors.py` lays them out.

### Nima's fetch (a production operation on the NILS server)

After this part is committed, the tools reach the compute server as the sealed code run `final-cert-grade-code` (with `seal/stack-ids.txt` of `final-cert-read`), mirrored to the research container. Then Nima runs, on the development laptop:

*(The operator's command is kept in the private record.)*

It prints three count lines (the job check with `sealed 3750, under final-cert@1 3750`, the engine and pack, and the answer, vote and stderr counts) and `rc=0`. `rc=3` means a check stopped it before anything was read. The folder is then handed to the research container's inbox (research-container skill, step 7), pulled to the development laptop and sealed as `final-cert-rules-0230`.

## Part 2: the results (2026-10-02)

**The answer: the certificate does not pass.**
- **The main line fails its 1 % bar.** Pack 0.23.0 gets 32 of the 2,500 clean main reads wrong: Hajek 1.28 %, Korn-Graubard upper bound **1.72 %**. To pass, the main sample could have held at most 16 wrong stacks.
- **Every make passes its 3 % bar** on its own:
  - GE: 5 of 363 (1.38 %, upper bound 2.88 %).
  - Philips: 4 of 377 (1.06 %, upper bound 2.42 %).
  - Siemens: 23 of 1,760 (1.31 %, upper bound 1.85 %).
- **The intersection-union reading is therefore "no".** Under the registered pass condition, both the main line and every make had to pass.
- **The result does not hinge on the reference or on open questions.** Of the 32 main errors:
  - 24 are the pack's own (kind a);
  - 1 is a reference fault (kind b);
  - 7 are new disputes (kind c).
  
  Two sensitivity lines, computed afterwards as information only:
  - without the reference fault: 31 wrong, Korn-Graubard upper bound 1.67 %;
  - without the fault and the disputes: 24 wrong, Hajek 0.96 %, upper bound 1.35 %.
  
  Both still fail.
- **The lines kept apart:**
  - kept-uncertain: 8 of 600 (1.33 %, upper bound 2.39 %);
  - void-kept: 6 of 300 (1.99 %, upper bound 3.90 %);
  - the supplement's rare cells: 20 of 250 (Hajek 7.64 %, upper bound 11.19 %);
  - the localizers: 0 of 100 (upper bound 2.95 %).
- **Against dev-8.** On dev-8, pack 0.22's clean rate was 0.66 % (upper bound 1.55 %), and 0.23 modelled on dev-8 was at 0 of 900, which is seen data. On fresh, archive-wide draws, 0.23's clean rate is 1.28 %, inside dev-8's interval for 0.22 but twice its point estimate.
  - Most of the main errors come in groups that no development read drew. One example is six Siemens MPRs of a real inversion recovery (`t1_space_ir`), all at 3 T.

> **Warning: graded once.** Nothing found here is fixed and graded on this read again (record 48). The errors below go to the next pack and to the record. A further certificate needs a new sealed draw.

### The checks before scoring

- **The fetch** (`final-cert-rules-0230`, Nima on the NILS server, 2026-10-02, 13:11:46Z to 13:11:53Z, rc 0):
  - Every one of the 3,750 stacks holds job `pack-0230` (pack 0.23.0), the same before and after the fetch.
  - All 3,750 are sealed under `selection:final-cert@1`.
  - The fetch returned 3,750 answer rows; the ask excluded no stack and explained none.
  - It returned 61,970 vote rows on 3,750 stacks, with 0 stderr lines.
- **The engine was alpha.72 at the fetch, not alpha.71.** The reader update of 12:39 UTC changed no pack:
  - the installed pack is 0.23.0;
  - alpha.72's `packs.tar.gz`, checked against its `SHA256SUMS`, carries an MRI pack tree identical to alpha.71's (sha256 of the tree listing `766d6cbb…`).
  - The stored answers are job `pack-0230`, made on alpha.71 and unchanged.
  - The derived five are computed, as registered, with alpha.71's binary.
- **"unfinished jobs 2"** counts two (kind, state) lines, each one job:
  - job 370 (`worker`, lane main) and job 371 (`pipeline-worker`, lane pipelines), both `running`;
  - both were started at 12:39:17Z by the engine's restart into alpha.72, and their predecessors failed at that restart;
  - each has run 0 jobs, and the only job started after them is 372, the fetch's own ask.
  
  No classify job was unfinished, and neither job is a picture build. Stored answers are written only by classify jobs, and the stored jobs were the same before and after the fetch. Neither job can have touched job `pack-0230`'s stored answers. (Read only as `nils_research` in the research container; `logs/fetch-checks.txt`.)
- **The rules party** (dev-5's `stored.py`, unchanged) covers 3,750 stacks, with 0 excluded and 0 filled from `explain`. The rules decided "no base" on 377 stacks.
- **The pack-stands record** (N7) holds 7 cells, all construct: 5 `Magnitude` to none and 2 `Magnitude+ND` to `ND`.
- **The scorer** gave 0 of 3,650 on the corrected reference itself (part 1).

### The lines (`tools/lines_final.py`, unchanged)

| line | n | wrong | Hajek | Korn-Graubard upper | n* | exact upper |
|---|--:|--:|--:|--:|--:|--:|
| **main, the claim (bar 1 %)** | 2,500 | 32 | **1.28 %** | **1.72 %** (fails) | 2,500 | 1.72 % |
| main, GE (bar 3 %) | 363 | 5 | 1.38 % | 2.88 % (passes) | 363 | 2.87 % |
| main, Philips (bar 3 %) | 377 | 4 | 1.06 % | 2.42 % (passes) | 374 | 2.41 % |
| main, Siemens (bar 3 %) | 1,760 | 23 | 1.31 % | 1.85 % (passes) | 1,756 | 1.85 % |
| supplement (apart) | 250 | 20 | 7.64 % | 11.19 % | 228 | 11.41 % |
| kept-uncertain | 600 | 8 | 1.33 % | 2.39 % | 600 | 2.39 % |
| void-kept | 300 | 6 | 1.99 % | 3.90 % | 300 | 3.91 % |
| localizers (apart, no bar) | 100 | 0 | 0 | 2.95 % | 100 | 2.95 % |

- **Beside the main line,** the other upper bounds:
  - Jeffreys over makes: 1.74 %.
  - Jeffreys over make x field: 1.86 %.
  - Session-clustered bootstrap: 1.68 % (2,354 sessions).
  - Wilson at Kish's n: 0.91 % to 1.81 %.
  
  Every reading of the main line is above 1 %.
- **Main, by make and field** (wrong of read):
  - GE: 1.5 T 0 of 218, 3 T 5 of 144, 1 T or less 0 of 1.
  - Philips: 1.5 T 0 of 29, 3 T 4 of 341, 1 T or less 0 of 7.
  - Siemens: 1.5 T 8 of 1,118, 3 T 10 of 550, 1 T or less 1 of 4, 7 T 4 of 88.
- **Main, by cohort:** study A 27 of 1,995, study B 4 of 175, study L 1 of 22, study H 0 of 209, study K 0 of 61, study E 0 of 32, study D 0 of 6.
- **Kept-uncertain, by make:**
  - GE: 5 of 367 (1.36 %, upper bound 2.85 %).
  - Philips: 3 of 36 (8.27 %, upper bound 21.0 %).
  - Siemens: 0 of 197 (upper bound 1.51 %).
- **Void-kept, by make:**
  - GE: 5 of 166 (3.00 %, upper bound 6.24 %).
  - Philips: 0 of 1.
  - Siemens: 1 of 133 (0.75 %, upper bound 3.56 %).
- **The supplement:** 165 rare cells were read and 20 have a wrong stack. No supplement stack is also in the main sample.
- **The localizers:** 0 of 100 wrong on provenance and on technique by family. The localizers' record corrected 0 cells.

**Per axis** (all non-localizers; wrong cells, none empty):

| axis | provenance | technique | modifier | construct | base | role | directory type | disposition | convertible | quality |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| wrong | 12 | 23 | 13 | 26 | 17 | 9 | 2 | 10 | 0 | 0 |
| cells | 3,639 | 3,645 | 3,617 | 3,613 | 3,608 | 3,582 | 3,582 | 3,582 | 3,582 | 3,650 |

In all, 66 of 3,650 non-localizers are wrong (112 cells): 52 of 2,750 clean (main and supplement), 8 kept-uncertain and 6 void-kept.

**`grade7.py`'s certificate reading** is unweighted, over main and the supplement together. At 1 % and at 2 %, the clean stratum fails accept-the-rules: 2,750 stacks, 52 wrong, upper bound 2.38 %. 0 of 3,650 are certified, and kept-uncertain decides nothing by the rules alone. The registered pass reading is `lines_final.py`'s above.

### Every error, by kind

The 66 wrong stacks were read one by one. Each reading covered:
- the header and the session;
- the four readers' and the adjudicators' values and reasons;
- the pack's stored votes;
- the corrections applied.

Each stack was put in one kind. The material is in `final-cert-grade`:
- the list, with stack ids: `scoring/errors/private/kinds-0230.tsv`;
- the reading: `readout.md`, made by `tools/grade/readout.py`;
- the counts: `scoring/errors/kinds-0230-summary.json`.

| kind | stacks | main | supplement | kept-uncertain | void-kept |
|---|--:|--:|--:|--:|--:|
| (a) a rule bug or gap the header shows | 51 | 24 | 14 | 8 | 5 |
| (b) the reference stale or inconsistent | 6 | 1 | 4 | 0 | 1 |
| (c) a new dispute no ruling covers | 9 | 7 | 2 | 0 | 0 |
| (d) the header cannot tell | 0 | 0 | 0 | 0 | 0 |
| (e) a scoring or pipeline artefact | 0 | 0 | 0 | 0 | 0 |
| (f) convention after pack | 0 | 0 | 0 | 0 | 0 |

Main by make: GE 4 (a) and 1 (c); Philips 2 (a) and 2 (c); Siemens 18 (a), 1 (b) and 4 (c).

**(a), the pack's own, 51 stacks in twelve groups and twelve single stacks** (main, supplement, kept-uncertain, void-kept):

| group | stacks | where | what the pack does |
|---|--:|---|---|
| A1 | 6 | 6 main (Siemens 3 T) | A Siemens MPR (`t1_space_ir ... _MPR_*`, `*spcir_257ns`) of a real inversion recovery (its source in the session is written component `R`) is PSIR by DR3. The pack reads IR: the MPR's own ImageType drops the `R`. |
| A2 | 4 | 1 main, 3 supplement | GE `Sag 3DSPGR MT=on/off` with `VASCTOF_GEMS` and pulse `3dtof` is SP-GRE (case 2: an SPGR name rules out TOF). The pack gives TOF-MRA, and so base T1w for MTw on three. |
| A3 | 4 | 2 main, 1 supplement, 1 kept-uncertain | A GE MAGiC or MDME acquisition with private image type 2 is Real (R3-GEPROP's exception covers PROPELLER only). The pack gives no construct. |
| A4 | 4 | 1 main, 1 kept-uncertain, 2 void-kept | A technique word in the name outranks the sequence. `TSE` over Philips T1TFE, `Cube` over GE BRAVO, `tse` over Siemens `*se2d1` (SE) and over a position display's `*fl2d1` (GRE). |
| A5 | 3 | 3 main | A keyword found inside another word or label: `MToff` read as TOF, a phase-encoding `phase` read as construct Phase (base then empty), `trace` on a b=0 image read as Trace. |
| A6 | 3 | 1 main, 2 void-kept | "T1" in a reformat's name outranks long-TE T2 or FLAIR physics (TE 122 to 290 ms; GE Cube, CubeT2flair, Philips TIR). |
| A7 | 3 | 3 main (Siemens 7 T) | DANTE in the source's sequence name (`wip-spc-t2p+ir-dante`) is BlackBlood; the pack reads FLAIR only. |
| A8 | 4 | 2 supplement, 1 kept-uncertain, 1 void-kept | A map not recognised: GE's `Exponential Apparent Diffusion Coefficient` (RawRecon, eADC; the pack gives DTIRecon with ADC and MPR); GE's `MCFA` (DTIRecon, FA); a Philips third-party `rMTT ... Lupe` (PerfusionRecon); Siemens MOLLI's inline T1 map (T1map, base none). |
| A9 | 2 | 1 supplement, 1 kept-uncertain | A GE `swi_ks` SWI output (`SWIFilt`, `... SWI`) is SWIRecon, SWI, base SWI. The pack's acquired-magnitude rules win. |
| A10 | 2 | 1 main, 1 supplement | A Philips view of the SWIp (`PROJECTION IMAGE\SW_M`, `VSWIp`) is ProjectionDerived, MPR with SWI. The pack's SWIRecon outranks the projection. |
| A11 | 2 | 1 main, 1 supplement | GE's multi-echo QSM (`psd/QSM/me`, 10 echoes): one echo is ME-GRE and its SWI minIP comb-ME-GRE. The pack gives GRE. |
| A12 | 2 | 2 main | CE-MRA not read: `MRA-CE Vibe` with an agent, and a `vibe-twist ... dyn` series. The pack gives VI-GRE. |
| singles | 12 | 3 main, 5 supplement, 4 kept-uncertain | Main: a Siemens MEAN magnitude of a multi-echo GRE without Magnitude; a Siemens `tirB` (BLADE) without Radial; a SyMRI `T1W_FLAIR` synthetic given T2w. Supplement: a Siemens XA parallel MPR of an MPRAGE with no sequence name (GRE, PDw); a Philips `Patient Aligned MPR` of SmartBrain not Localizer (R3-LOC); MT from "MTC" in a name (MT comes from the tokens only); a GE SWAN's `ORIG` dropped by the SWI route's construct; a Philips `Du-IR` read FLAIR, not DIR. Kept-uncertain: a GE `B0map` given TOF-MRA and T1w with no rule vote (base none, R3-CAL); a Philips `subtraktion` not SubtractionDerived; a Philips MAXIMUM of 32 slices of 4 mm left RawRecon (R3-MAX); a GE `ksepi2` SWI phase given ME-GRE, not EPI (D12). |

**(b), the reference, 6 stacks.** The pack and every reader agree; the correction record moved the reference away from them.
- **B1, D3 against case 27 (2: 1 supplement, 1 void-kept).** D3 makes the source of a GE diffusion map a single-shot EPI. Round 3's case 27 makes `epi2muse` (MUSE) MS-EPI, and the readers and the pack say MS-EPI.
- **B2, DR9 on relCBF maps (2, supplement).** DR9 makes a derived motion-corrected DSC series RawRecon, and it took two `RELCBF_LOCAL ..._MoCo` maps with it. Maps stay PerfusionRecon by DR9's own words.
- **B3, V12 against case 28 (1, supplement).** V12 makes EPIMix's FLAIR blocks SE-EPI. Case 28 makes its T1-FLAIR EPI.
- **B4, a localizer's technique left after DR6 (1, main).** DR6 took the Localizer away from a one-slice `t1_tse_tra_repositionering`. Its technique stayed settled by family (`family:SE`), so the pack's TSE scores wrong.

**(c), new disputes, 9 stacks in five questions:**
- **C1 (3 main, Siemens 1.5 T):** a STIR named T2 (`t2_tirm`) under TE 40 ms. The guide's TE rule gives PDw; the pack and llm-2 give T2w. The adjudicators settled PDw and called the guide silent on STIR.
- **C2 (1 main, 1 supplement):** GE `ksepi` 3D T1 (`3DEPI`, TE 3 to 6.6 ms). Is it GRE-EPI by its physics, or EPI as the header states nothing more?
- **C3 (2 main, Philips):** a Philips diffusion stack holding more than one kind of image. The reference has b=0 with the isotropic image as Trace, and the whole set with private direction `I` as none; the pack has it the other way round.
- **C4 (1 main):** a Siemens `Image Filter` copy (`_FIL`, DERIVED). Is it Filtered, by analogy with GE's `FL:` (DR8)?
- **C5 (1 supplement):** a Siemens QSM sequence's acquired magnitude echo. Is it Magnitude, on the QSM route as the guide lists it, or none (N7)?

### What this means

- **Pack 0.23.0 is not certified.** Its archive-wide clean rate is about 1.3 %, not the 0.5 % dev-8 suggested. Its bound is 1.72 %.
- **The misses are the pack's own and are clustered.** 24 of the 32 main errors are rule bugs or gaps the header shows, and most come in groups:
  - the Siemens real-IR MPRs (6);
  - three keyword traps (3);
  - DANTE at 7 T (3);
  - MAGiC Real (2 in main).
  
  Several groups reach protocols no development read drew. 900-stack development reads could not see them.
- **The reference held, with six faults:** its audit showed 0 joint errors. The six faults are corrections in the record that conflict with a newer convention (D3 against case 27, V12 against case 28), that reach too far (DR9), or that leave a stale localizer reading (DR6). They should be mended in the record before the next read.
- **For the next pack** (measured on a new sealed read, never on this one): the twelve groups and twelve singles of kind (a).
- **For the record and the guide:**
  - rulings for C1 to C5;
  - mend D3 for MUSE, DR9 for maps, V12 for EPIMix's T1-FLAIR and DR6's family cell;
  - a ruling on whether an MPR of a real IR takes construct Real. On A1's six stacks the adjudicators added Real on five and called it a convention gap.
- **System 1 round 9** is graded on its own registration (`learners-r9.md`). Its rules-plus-System-1 lines for kept-uncertain and void-kept are not part of this grade.

### Evidence

| run | what | files | manifest sha256 |
|---|---|--:|---|
| `final-cert-read` | the read: draw, seal, packets, raters, llm-2, adjudication, audit, reference | 21,504 | `8d97073da08f720e32d5547e6dc5597b2b85dcd46eff952c97ec43ac74fd1425` |
| `final-cert-read-corrections` | the correction record and the localizers' record | 7 | `8e408c1e21e8b882c1b86b529e4e5720538747e5da32c27a3f712b69d051bd4a` |
| `final-cert-grade-code` | the fetch kit carried to the compute server | 5 | `1ddb6f39b3013cf214b067982e5b00058e9ad770352df12343c030cbececca3c` |
| `final-cert-rules-0230` | pack 0.23.0's stored answers and votes on the 3,750 stacks | 25 | `a12345511251ee553c14a1e8941d6ae55f2e58c8673944944303259018afa91e` |
| `final-cert-grade` | the party, the pack-stands record, the grade, the lines, the error list, the kinds and the reading | 32 | `68db229eaa677231177b41591ca853ce7e362f932bbaa8e76c2a4449d4fb65e9` |

Each run was mirrored to the research container and verified against its manifest.

The sensitivity lines above are information only and are not in a sealed run. They are reproducible from `kinds-0230.tsv`: drop the stacks of kind (b), or of kinds (b) and (c), from the grade's wrong list and run `lines_final.py` as registered.
