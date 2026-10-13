# Certificate 2: the plan

**REGISTERED, 2026-10-04.** Nima gave his go on 2026-10-04: MRI pack 0.25.0 goes to certificate 2, with the draft's recommendations as his choices (section 5). Every blank below is filled. The registration is the commit that puts the exclusion lists' sha256 into `config.json` and changes this header to REGISTERED, before the draw runs. Nothing is drawn, sealed or read.

> **Warning: once registered, this plan is fixed.** Any later change is a deviation, written in the last section before the step it touches.

## In plain words, for Nima

**What it will say, if it passes:** "Of the clean stacks NILS keeps, outside the sessions any header read has touched, at most 1 % have any of the ten header axes wrong, at 95 % confidence; and at most 3 % for each of Siemens, Philips and GE." Kept-uncertain and void-kept get their own bounds. The rare cells, the localizers, body part and post-contrast are reported apart.

**Beside it, registered in advance:** "Of the clean stacks rules plus System 1 lets through without review, at most 1 % are wrong, and they are at least 85 % of the clean frame." It is not a fallback for the main claim.

**Your choices** (2026-10-04, the draft's recommendations):
1. Main sample 5,000.
2. The go condition: met. Pack 0.25.0 has 6 wrong of p-go's 1,497 clean reads (at most 9 allowed).
3. Kept-uncertain 600, supplement 250, void-kept 300, localizers 100.
4. The selective claim: `combined`'s frozen score as the review trigger, coverage bar 85 %. Its threshold, chosen on the protocol sweep before the draw, is **0.95** (section 6).

**What changed while preparing it:**
- **The pack** is 0.25.0 as stored by job `pack-0250-a76` (id 400, engine alpha.76). The real frame shows every stored row from that job.
- **The prompts** are v3.4. They carry every ruling since the final certificate, Nima's eleven answers of 2026-10-03 included.
- **The correction record** is final-32: final-31 with one rule mended to match round 6's own wording. It equals final-31 on the sweep.
- **The packets** carry the header fields pack 0.25.0 reads.
- **The stored answers** are fetched read-only at the draw, before the seal, as the sweep and the go read did. So no `--unsealed-access` fetch is needed at the grade.

**Your part:** the same two commands as for the final certificate (snapshot, seal; section 10).

## 1. What is the same as the final certificate

Everything not named in sections 2 to 11:
- the ten axes, and empty is wrong;
- the strata (the registered stratum, the void by today's disposition);
- the picture-read stacks out by stack (decision 1);
- the reference method: three llm-1 raters, llm-2 dissenting at 6, two adjudicators, the 15 % audit, isolation, the correction record;
- the seal kit and its checks;
- Hajek with Korn-Graubard, the 1 % clean bar and the 3 % per-make bars (intersection-union);
- the lines (`lines_final.py`), graded once.

The tools are in `tools/`, copied from `../2026-10-01-final-certificate-read/tools/` on 2026-10-02 and changed only where this plan says.

## 2. The pack and the go condition

**The pack:** MRI pack **0.25.0 as stored**, from job **`pack-0250-a76`** (id 400, engine **1.0.0-alpha.76**). It finished on 2026-10-03 at 23:29:46 UTC on the pre-production registry: 558,874 read, 558,405 written, 0 broken stacks, the same counts as job 397 (`pack-0250-timing`, alpha.75), which the go read graded. The engine moved from alpha.75 to alpha.76 with the same pack: kineuro/nils #359 and #361 changed engine code, no pack file.

**The go condition, met** (choice 2). Go when the pack has at most 9 wrong or empty of p-go's 1,497 clean reads.
- Pack 0.25.0 as stored (job 397) has **6** under the current final record final-31: Hajek 0.40 %, Korn-Graubard 0.79 %, exact 0.79 %.
- By make: GE 1 of 221, Philips 1 of 227, Siemens 4 of 1,049.
- Under the sweep's record alone it has 11 (0.73 %, bound 1.21 %). The 5 between the two lines are final-31's X10 cells on p-go.
- Registered in nils-design `8b99fc1`, graded in `3f2e026` (`../2026-10-04-pack-0250-go/grade.md`). The evidence runs are `pack-0250-go-rules` (`3d9133db…`), `pack-0250-go-grade` (`4ecd4d98…`) and `pack-0250-go` (`9a8d422c…`, sealed and unread).

## 3. The frame

`tools/frame.sql` and `frame.py` are the final certificate's. `tools/runF.sh setup` adds the final certificate's draw (`final-cert-read`) and the protocol sweep's draw (`protocol-sweep`) to the header reads (`subst.py`), out by session. They run in the research folder in the research container.

**The frame, run for real on 2026-10-04** (`runF.sh setup` and `runF.sh frame` with code run `final-cert-2-code-5`, prereg; the frozen-pack check on):
- **every stored row is pack 0.25.0 from job `pack-0250-a76`:** 544,964 of 544,964 unsealed classified rows, none unclassified in the frame;
- the frame query reads 550,680 stacks;
- the dry frame of the same day, run while every row was still job 397, gave the same counts to the stack.

| final stratum (registered) | archive |
|---|--:|
| clean (clean) | 417,570 |
| void (clean, excluded by pack 0.25.0) | 1,072 |
| kept-uncertain (kept-uncertain) | 50,894 |
| void-kept (void, kept today) | 80,129 |
| void (void, excluded today) | 1,015 |

| the frame | non-localizers | Siemens | GE | Philips | other | localizers |
|---|--:|--:|--:|--:|--:|--:|
| clean | **150,517** | 107,593 | 22,890 | 20,034 | 0 | 50,870 |
| kept-uncertain | **3,076** | 571 | 2,444 | 61 | 0 | 14,488 |
| void-kept | 38,484 | 16,500 | 21,888 | 85 | 11 | 490 |

- 20,133 sessions of 4,608 subjects.
- **Pack 0.25.0 excludes 1,072 registered clean stacks** that pack 0.23.0 kept. They are void by today's disposition and leave the frame. On the frame, clean goes from 150,690 (2026-10-02, pack 0.23.0) to 150,517.
- **Two frame stacks (void-kept) are classified with an empty technique.** Pack 0.25.0's rule set `cant_tell` leaves a technique to a person. They are drawn in the cell `empty` and scored as empty (deviation 9).
- **The exclusion lists' sha256** (`logs/inputs.sha256` of `runF.sh setup`, 2026-10-04): the file's own sha256 is **`52ceee2df9528dd6570f46372291403f4dca7c45ea2e02980341df8c1429fd4b`**. The new lines against the final certificate's are `excl/final-cert-read.stacks` (3,750, `6e6b6243…`), `excl/protocol-sweep.stacks` (5,530, `d694c9ae…`), `final-cert-read/MANIFEST.json` (`8d97073d…`) and `protocol-sweep/MANIFEST.json` (`d229f482…`). The rest equal the final certificate's section 1.

## 4. The draw

`tools/draw.py` takes its sizes from `config.json` `draw.sizes`, and `weights.py` reads the same sizes. Seed **`final-cert-2-draw-2026-10-04`**.

| sample | from | size |
|---|---|--:|
| main (the claim) | clean non-localizers | **5,000** |
| supplement (apart) | the rare cells (under one expected main read) | 250 |
| kept-uncertain | kept-uncertain non-localizers | 600 |
| void-kept | void-kept non-localizers | 300 |
| localizers (apart) | localizers of the three kept strata | 100 |

**The dry draw on this frame** (2026-10-04, throwaway seed `dry-g`, deleted after):

| | dry-g |
|---|--:|
| unique stacks (main and supplement shared) | 6,241 (9) |
| main: Siemens / GE / Philips | 3,573 / 761 / 666 |
| supplement: GE / Philips / Siemens; rare-cell frame | 81 / 93 / 76; 1,394 stacks in 174 cells |
| kept-uncertain: GE / Siemens / Philips | 477 / 112 / 11 |
| void-kept: GE / Siemens / Philips | 171 / 128 / 1 |
| localizers: clean / kept-uncertain / void-kept | 77 / 22 / 1 |
| sessions (with 2 or more drawn; most) | 5,221 (858; 5) |
| subjects (most stacks on one) | 2,889 (11) |
| drawn stacks that are campaign items | 0 |

- Every weight was checked by `weights.py` against the frame alone. Kish's n for main is 4,999.6.
- Every make has 299 or more main reads, so all three are held to 3 %.
- The seal would add 1,108 subjects, with 81,620 kept non-localizers on them. The drawn sessions hold 78,051 archive stacks.

## 5. Nima's choices (taken 2026-10-04)

Nima, 2026-10-04: pack 0.25.0 goes to certificate 2, "with the draft's recommendations as his choices". The numbers below are the draft's (`tools/power2.py`, exact binomial arithmetic on a self-weighting sample; no answer read).

### Choice 1: the main size, 5,000

The chance that the clean bound is at or under 1 % (the largest passing count in brackets):

| true clean rate | 2,500 (15) | 3,000 (19) | 3,500 (23) | 4,000 (27) | **5,000 (36)** | 6,000 (44) |
|---|--:|--:|--:|--:|--:|--:|
| 0.3 % | 99.5 % | 99.9 % | 100 % | 100 % | 100 % | 100 % |
| 0.4 % | 95.2 % | 97.9 % | 99.1 % | 99.6 % | 100 % | 100 % |
| 0.5 % | 80.7 % | 87.6 % | 92.0 % | 94.8 % | **98.6 %** | 99.4 % |
| **0.6 %** | 56.8 % | 65.1 % | 71.6 % | 76.8 % | **88.1 %** | 91.9 % |
| 0.7 % | 32.7 % | 38.4 % | 43.2 % | 47.5 % | 61.0 % | 65.9 % |
| 0.8 % | 15.5 % | 17.9 % | 19.9 % | 21.5 % | 29.5 % | 31.2 % |
| 1.0 % | 2.2 % | 2.1 % | 2.0 % | 1.9 % | 2.3 % | 1.9 % |

At the go read's 0.40 %, main 5,000 passes almost surely.

### Choice 2: the go condition, at most 9 of p-go's 1,497

Met (section 2).

| go when wrong is at most | rate | exact bound | go at 0.3 % | 0.4 % | 0.5 % | 0.6 % | 0.8 % | 1.0 % | 1.28 % |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| 7 | 0.47 % | 0.96 % | 91.4 % | 74.6 % | 52.7 % | 32.5 % | 9.0 % | 1.8 % | 0.1 % |
| **9** | **0.60 %** | **1.14 %** | 98.3 % | 91.7 % | 77.9 % | 59.0 % | 24.3 % | 7.0 % | 0.8 % |
| 11 | 0.73 % | 1.31 % | 99.8 % | 98.0 % | 92.2 % | 80.5 % | 46.4 % | 18.5 % | 3.1 % |

### Choice 3: kept-uncertain 600, supplement 250, void-kept 300, localizers 100

- Kept-uncertain has no bar. 600 is a fifth of the frame's 3,076 and keeps it comparable with the final certificate. Philips (61 in the frame, about 11 drawn) is reported, not bounded.
- The supplement shows whether the rare cells improved (the final certificate's: 20 wrong of 250).
- The localizers have no bar.

### Choice 4: the selective claim, option B

`combined`'s frozen score as the review trigger, the threshold chosen on the protocol sweep before the draw, coverage bar 85 %, option A reported beside. Section 6 registers it.

## 6. The selective claim (registered in advance)

**The system:** System 1 round 9 as frozen (`../2026-09-24-phase-0/learners-r9.md`, `FROZEN.sha256` `f34c3f01…`, checked before and after every use).

**The trigger** (`tools/sel9.py`, `sel_score.py`, `selS.sh`):
1. System 1 (the frozen `system` model) predicts each stack from its own inputs: round 9's export of 2026-10-02, which holds pack 0.23.0's stored answers, votes and confidences, the inputs it was frozen for. Every one of the frame's 150,517 clean non-localizers is in that export; 91,466 of them (61 %) are in round 9's training registry, with the teacher's labels.
2. `combined` is round 9's `predict9.combine`, unchanged, with **the certified pack's stored answers as its rules**:
   - the pack's value stands where it answers at a stored confidence of 0.70 or more (a decided nothing answers `none`; an answer with no stored confidence counts as 0.70 or more);
   - System 1's value is taken where the pack is empty or under 0.70;
   - the raw score is System 1's raw stack score, and 0 where the pack answers one of the five at 0.70 or more and System 1 differs.
3. The score is that raw score through round 9's frozen beta map for `combined` (`calib/maps.json`). A 0 stays 0.
4. **A stack is accepted** when its score is at or above the threshold **and** `combined` keeps the pack's answer on all five axes. Otherwise it goes to review. A stack the export lacks has no score and goes to review.
5. **The answers judged are the pack's stored answers,** as in the main claim.

**The claim:** among the accepted clean main stacks, at most 1 % wrong or empty on the ten axes (Hajek over the accepted domain, Korn-Graubard 95 %). The same per-make bar at 3 % applies where a make has 299 or more accepted reads.

**The coverage bar:** the accepted share of the clean frame, by its Hajek estimate on the main sample, is at least **85 %**. The lower 95 % bound is reported beside.

**The threshold: 0.95.** The rule (choice 4): the lowest threshold of `config.json` `certificate.thresholds` whose accepted error rate on the protocol sweep's p-dev and cover is at or under 0.5 %, with coverage of at least 85 %, on the sweep's settled reference, fixed before the draw.
- **The data:** the sweep's 3,930 read non-localizers of p-dev and cover; p-go is never listed, predicted or fetched. The claim's domain is the 2,647 clean ones.
- **The reference:** settled cells, final-31 and the pack-stands record, as the go read's primary line.
- **The estimate:** frame-weighted (Hajek), each stack weighted by the union inclusion probability of p-dev and cover. Unweighted counts are beside: cover takes one stack per cluster, so it over-represents rare clusters.
- **Read twice, and the higher threshold registered** (deviation 6), since the two readings disagree:

| reading | errors | trigger | the rule gives |
|---|---|---|--:|
| 1, as the rule is written | pack 0.25.0's own, which are seen: 0.25.0 was built from p-dev and cover | B, as above | 0.0 |
| 2, errors no pack had been taught | pack 0.23.0's stored errors on the same stacks (held out for 0.23.0 and for round 9) | round 9's `combined` with 0.23.0 as its rules, exactly as graded on the final certificate | **0.95** |

The numbers on the 2,647 clean stacks (Hajek, plain counts beside):

| threshold | reading 1: accepted, wrong | coverage | reading 2: accepted, wrong | error | coverage |
|--:|--:|--:|--:|--:|--:|
| 0.99 | 1,414, 0 | 82.9 % | 1,423, 9 | 0.30 % | 83.1 % |
| 0.98 | 1,808, 0 | 91.1 % | 1,831, 18 | 0.43 % | 91.5 % |
| 0.97 | 1,938, 0 | 92.5 % | 1,970, 27 | 0.45 % | 93.1 % |
| **0.95** | **2,099, 0** | **95.0 %** | **2,143, 38** | **0.48 %** | **95.6 %** |
| 0.9 | 2,241, 0 | 96.7 % | 2,298, 51 | 0.63 % | 97.6 % |
| 0.8 | 2,337, 3 | 97.3 % | 2,417, 73 | 0.79 % | 98.3 % |
| 0.0 | 2,632, 9 | 99.8 % | 2,647, 123 | 0.92 % | 100 % |

- **At 0.95 on pack 0.25.0:** 2,099 of 2,647 clean stacks accepted with none wrong. Coverage is 95.0 % weighted, 79.3 % plain. By make: GE 90.8 %, Philips 89.4 %, Siemens 97.1 %. Across all 3,930 stacks of p-dev and cover, 3,084 are accepted with 1 wrong.
- **Reading 2 at 0.95:** the trigger sent 85 of pack 0.23.0's 123 clean errors on these stacks to review (69 %, the final certificate's figure again) while reviewing 19 % of them, and halved the frame-weighted error (0.92 % to 0.48 %).
- 1,798 of the 3,930 stacks are in round 9's training registry. Round 9 froze before the sweep's draw, so they are teacher-labelled only.
- **The evidence run** is `final-cert-2-selective`, manifest sha256 `8f8c043bd4789262e44f5464a94ce291624194af4d8cfa4d7b08b483e6794ddb`. It holds the code, the parties, the fetch (job 400's values equal job 397's on all 41,635 cells), the trigger per stack and the tables.

**Beside it, described:**
- **A (`rules-err`):** round 9's frozen error model's calibrated score, its threshold set on the main sample to the coverage B reaches there; its accepted error rate, no bar.
- B's catch (the rules' clean main errors sent to review) and the review share.

**When:** at the grade, after the reference and its correction record are sealed. System 1 comes from round 9's export, so no registry read is needed. `combined`'s rules come from `final-cert-2-rules` (section 9).

## 7. The prompts v3.4

Composed by `../2026-09-27-header-judge/tools/compose_prompts_v34.py` from the v3.3 files the final certificate registered, each checked first by its sha256 (all five matched). The composer is reproducible: run again, it writes the same bytes. Against v3.3:
- **The Conventions section** carries the reading guide (`../2026-09-27-read-r1-notes/reading-guide.md`) as ruled through 2026-10-03:
  - record 48 after the final certificate's grade (C1 by round 4, C2 to C5, X9-IRREAL, X9-MEANMAG, X9-DANTE, the narrowed D3, V12, DR9 and DR6);
  - round 4 (with N6 beside R8);
  - round 5;
  - the protocol sweep's settlements and the narrowed D10, V28 and R3-FEEPI;
  - Nima's eleven answers of 2026-10-03, worded as the record's X10-TFE to X10-RGB;
  - the new header fields.

  The guide gains those lines in a dated section, and six older lines are amended in place.
- **The traps** are amended where those rulings supersede them: MT and SFS, the Philips TFE, a BRAVO reformat named Cube FLAIR, SyMRI and MAGiC, diffusion sets and reversed references, echo combinations, FastView and `scout`, what a reformat keeps from its source.
- **"What the packet holds"** names the new fields.
- **`constraints.json`** relaxes two implications to pack 0.25.0's:
  - "MP2RAGE means T1w" no longer applies to a T1 or R1 map;
  - "comb-ME-GRE means T2\*w" no longer applies to a MIP, QSM, R2starmap, B0map or SWIRecon output.

  Without this, answers under rounds 5 and 6 would fail legality. The file is changed in place, since `legal.py`, `validate.py`, `judge.py` and `check_adjudication.py` load that fixed name. v3.1 was changed the same way, and the old file stays in git.
- **The schema** is v3.1's, unchanged: pack 0.25.0 answers no value it lacks. `contrast_mix` and `base_basis` are not asked.

The prompts are copied into the run's `prompts/` and checked against `tools/PROMPTS.sha256` before any rater reads a packet:

| file | sha256 |
|---|---|
| `prompt-v3.4.md` | `f6ab8afcc01cdf8016ee1395e5382f1b3f8e22471d7293e1c2f72cd2e8135c33` |
| `prompt-v3.4-family.md` | `0067905ded6659ea8b68383c8c4fb8f188bde6cb0d55e8a402c4341b3a00ab38` |
| `prompt-v3.4-adversarial.md` | `00f7f4c27bc23963ecfd15d30288303bbdfe6d58b5338367ef46833b8475f2d3` |
| `schema.json` | `7b595f512eac6a745b46055ba7c505721ed5265c5ed81134a92cb114ce38d410` |
| `constraints.json` | `0e283111bcf4a3e31bca31ac76dc1ca2872f9085076ea3a7ad46caf52fc0309f` |
| `adjudicator-v3.4.md` | `e9350c2372342fec7c37dd0f1488f150b60689c5e37dc73152e98d04f66cc464` |
| `adjudicator-2-v3.4.md` | `32ed735bbd2b094f02764184ab9d48d1cff5a55b1ce94de763536df5bc647dcb` |
| `adjudicator-schema.json` | `603cbc6dd831f8036bbf24bd5f3f59f43f18407cdd754ca194fb8f0ed9e309f5` |

`agents.py` and `chain.sh` name the v3.4 files. llm-2's run is `27b-v34` (`config.json` `panel.dissent`).

## 8. The correction record and the scorer

**The correction record: final-32.** That is `rulings.py build --record final` (`../2026-09-28-certificate/tools/rulings.py`) as it stands at the registration commit, sha256 **`83d220db92bf6cbd1936193e2dd5edf734a4ded18f111ce294b1dc47bc46be18`**, with 64 tests passing and 1 skipped. It is final-31 (`69186ffa…`, the go read's) checked rule by rule against every convention made after it (history.md, section 8, item 31's owed fix):
- **One conflict, mended:** R3-TFE's NO clause (round 3: a Philips 3D TFE with InversionRecovery NO is FSP-GRE) still turned a settled MPRAGE into FSP-GRE outside X10-TFE's reach. Round 6's answer 1 says InversionRecovery NO alone never makes FSP-GRE, and X10-TFE's own text says the clause is replaced. The clause is removed, and a test of the escaping case is added: a Philips 3D FFE with MP and no private block. That test fails on final-31.
- **Its effect:** on dev-1 to dev-8 and the sweep outside p-go, one dev-6 cell, where the pack and every reader say MPRAGE. On the whole sweep R3-TFE took no cell, so final-32 equals final-31 there, and the go read's count stands.
- **The rest:** every other rule either has no newer convention that touches it, or agrees with the newer ones (a later rule overrides it, or it was already narrowed). An empirical check found no other cell where a rule moves a settled value that the pack and every reader agreed on.

The record corrects the read's reference in `final-cert-2-read-corrections`. It is built from the settled reference and the packets alone, and sealed before any rule answer is read. The localizers' record is beside it. Then come the pack-stands record (`rulings.py pack --record final`), built against final-32, and N8, as the go read's primary line.

**The scorer:**
- **`grade7.py`** (`../2026-09-24-phase-0/tools/r7/grade7.py`), sha256 `6941e2344ebb1ddbf684073d8a57a52787f57657eda1b59ac7f6c39f7d5b4d0a`, as the final certificate, the sweep and the go read ran it.
- **`cert25`:** the sealed checkpoint's `cert/` whose `derive.py` also derives `contrast_mix` and `base_basis` in its one `nils pack derive` call. From pack 0.24.0 on, the role rule reads `contrast_mix`. It is built and checked as the go read's `run.sh bin` does: only that line differs.
  - `derive.py` sha256: `cf2e502977004e62c8c3365ba7b0ac3a0a2aaf1e4ce14aae1a75940741d5e8e5`;
  - the tree, as the sha256 of its sorted file list: `610d4b4ad5be31d62b3eaab844c95130cb7036caa84690bcbbb3277bbe5331c9`.
- **The engine it derives with:** the v1.0.0-alpha.76 release, checked against its SHA256SUMS:
  - `nils-linux-x86_64`: `6b590e07557117fb1dae4b6a96c38580e84680ac3340a1300fcac7f6aafef3a4`;
  - `packs.tar.gz`: `0b5c804effb32a6d153e06a65bbada88b444e358b869d617b895b4aa348f2b87`, which holds MRI 0.25.0.
- **The conversion:** the go read's `views_to_fetch.py`, with the votes decoded by the pack's voters and a base that only `cant_tell` decided as nothing scored empty. Then dev-5's `stored.py`, unchanged (`17496d63…`).

## 9. The fetch at the draw, and the packets

**The stored answers, fetched at the draw** (deviation 3). Right after the draw and before the seal, I read the frozen job's stored answers on every drawn stack through the research views, read only:
- the commands are `selS.sh ids-draw`, then `JOB=400 selS.sh fetch`;
- the fetch stops unless every drawn stack holds job 400, before the fetch and after it;
- it takes each axis's value, confidence and tier, the values as one list, the votes, the evidence and the pack's voters;
- they go to the evidence run `final-cert-2-rules`, sealed and unread until the reference and its correction record are sealed.

The fetch was tested on 40 of the sweep's stacks on 2026-10-04. The seal's own check confirms that every drawn stack still holds job 400 when it is sealed.

**The packets** (`tools/hdr.sql`, `sess.sql`, `build_packets.py`, `shown.json`, `prep.sh`) are the final certificate's, with what pack 0.25.0 and registry schema 79 (engine alpha.74 on, fingerprint revision 7) show a reader:
- Philips' Prepulse Type and Prepulse Delay (2001,xx1C and 2001,xx1B), which pack 0.25.0 newly shows;
- the pixel storage (PhotometricInterpretation, SamplesPerPixel);
- AngioFlag, AcquisitionContrast and the temporal resolution;
- `physics.diffusion_directionality` (the first image's, as the record reads it) and `diffusion_directionality_all` (every image's).

A field is marked not read where its series was not read again since schema 79. The key names are the ones `rulings.py` reads. The research role reads every new field: revision 7 is on all 558,874 stacks. The session query reads no new field.

A dry build on 2026-10-04 made 479 packets with none left out: 400 of the dry-g draw, plus 80 picked for a prepulse, colour storage or ISOTROPIC. The identifier, personnummer, 8-digit date and answer-key checks passed on every packet. `rulings.py`'s readers found the new values. The scratch packets were deleted.

`build_packets.py` is now the sweep's builder: a stack whose packet or session list holds identifier-shaped text is left out and listed by id, never redacted or swapped. Such a stack is counted wrong (deviation 8).

## 10. The seal

One selection, `final-cert-2`, holds every drawn stack once and is sealed once, as `selection:final-cert-2@1` (`tools/seal_export.py`, `tools/seal-server.sh`). The kit reads the frozen job, pack and engine from `config.json` `frozen`.

`seal-server.sh` stops with exit 3 and writes nothing unless:
- every kit file matches `KIT.sha256`;
- the engine is the frozen engine (new);
- the installed MRI pack is the frozen pack;
- every drawn stack's stored answer is the frozen pack from the frozen job;
- no drawn stack is sealed or excluded;
- no selection of that name exists, and no classify job is unfinished.

It exits 4 if the seal does not cover exactly the drawn stacks.

**Tested on 2026-10-04** with `test_seal.sh`, on a scratch registry: `nils synth` with the alpha.76 release (checked against SHA256SUMS), 898 stacks classified with pack 0.25.0 as job `pack-test`, in the test container, with HOME, the XDG folders and TMPDIR in a scratch folder on disk.

| case | result |
|---|---|
| A, a wrong frozen job | exit 3, nothing written |
| A2, a wrong frozen engine | exit 3, nothing written |
| B, one stack sealed before | exit 3, nothing written |
| C, a clean kit of 60 | exit 0, exactly the 60 sealed under `selection:final-cert-2-c@1` |
| D, C again | exit 3 |

The development laptop's NILS install was unchanged: the same units and the same paths, checked before and after. The database was dropped and the folder deleted.

> **Warning: the engine check.** The seal stops if the NILS server's engine moves past alpha.76 between the registration and the seal.

**Nima's commands** (after I say the draw is sealed in evidence and give `KITSHA`, the sha256 of `KIT.sha256`):

1. On the development laptop, snapshot the registry's datasets:

   *(The operator's command is kept in the private record.)*

2. On the development laptop, check the kit on the compute server, push it into the NILS server and seal:

   > **Warning: `NILS_PACK_DIR` is required.** Without it the CLI finds a stale pack.

   *(The operator's command is kept in the private record.)*

   It prints:

   `text
   engine nils 1.0.0-alpha.76; pack 0.25.0; job pack-0250-a76 0.25.0 <n>; selection final-cert-2@1; drawn <n>; sealed now <n>; by selection selection:final-cert-2@1 <n> ; sealed outside the draw 0; stderr lines 0
   rc=0
   `

   `<n>` is the number of drawn stacks with a header, about 6,240 (`seal/summary.json`). `rc=3` means a check stopped it and nothing was written. `rc=4` means the seal does not cover exactly the drawn stacks: tell me before anything else.

## 11. From the registration to the read

1. ~~**The go condition, the pack, Nima's choices.**~~ Done (sections 2 and 5).
2. ~~**The prompts, the record, the scorer, the seeds, the seal kit, the selective threshold.**~~ Done 2026-10-04 (sections 6 to 10). The seeds are `final-cert-2-{draw,batches,ab,audit,second}-2026-10-04`.
3. ~~**The frame and the setup.**~~ Done 2026-10-04 with code run `final-cert-2-code-5`, prereg (section 3).
4. **The registration:** the exclusion lists' sha256 go into `config.json`, this header changes to REGISTERED, then commit as nima-ch and push. That commit is the registration and the frame commit.
   - *Done (post-registration note, 2026-10-04):* registered by Nima in nils-design `c75f44a`. He confirmed the 0.95 threshold, the two relaxed constraints and final-32.
5. **The committed tools to the research container, then the frame, the draw and the kit:**

   *(The operator's command is kept in the private record.)*

   *(The operator's command is kept in the private record.)*

   The frame stops unless every stored answer is job 400's. The draw prints its counts, the weights' check, the impact and the disjointness counts, which must be 0 where required, or I stop.
   - *Done (post-registration note, 2026-10-04)* with code run `final-cert-2-code-6` (`c75f44a`):
     - the frame showed every stored row from job `pack-0250-a76`: 544,964 of 544,964, none unclassified; `frame.tsv` is byte-identical to the frame of section 3;
     - drawn: 6,245 unique stacks (main 5,000: Siemens 3,574, GE 760, Philips 666; supplement 250, 5 shared with main; kept-uncertain 600: GE 477, Siemens 112, Philips 11; void-kept 300: GE 171, Siemens 128, Philips 1; localizers 100: clean 77, kept-uncertain 22, void-kept 1);
     - 5,223 sessions (857 with two or more, at most 5), 2,908 subjects (at most 13); Kish's n for main 4,999.6; every weight checked;
     - disjointness: sealed stack, sealed session, header-read stack and header-read session all 0; no drawn stack is a campaign item;
     - the seal would add 1,134 subjects, with 84,201 kept non-localizers on them;
     - the kit: 6,245 stacks, the id list's sha256 `b02f2ed2…`.
6. **The fetch at the draw** (section 9), before the seal:

   *(The operator's command is kept in the private record.)*

7. **Into evidence:** on the development laptop, the draw run and the rules run go through Bifrost, are sealed, and are mirrored back:
   - `mirror.sh pull final-cert-2-read the research folder`, then `evidence.py init` and `seal`, then `mirror.sh push`;
   - the same for `final-cert-2-rules` from the research folder, sealed unread.

   Then I give Nima `KITSHA`.
   - *Done (post-registration note, 2026-10-04):* the fetch at the draw found job 400 on all 6,245 drawn stacks before and after (66,490 axis rows, 12,490 vote rows, 66,571 evidence rows, 1,406 voters). `final-cert-2-read` is sealed (99 files, manifest `8db9563a50cadf459f9c956504dd8c628e2b00ad99eac066d71f9a566512390d`), and so is `final-cert-2-rules`, unread (32 files, manifest `757e12b05deec381f273a065bebccbeda57a87a7ee8279475160ac1019c491c3`). Both are mirrored. `KITSHA` is `aa53faf12b9356d17abdb6f7164d451cd0715656ca05883d5cdacf5b86960e8a`; the kit holds 6,245 stacks.

8. **The seal:** Nima's two commands (section 10).
   - *Done (post-registration note, 2026-10-04)* with the two-part kit of deviation 16 (`KITSHA` `f2ffbea80f28042d85b30c6973411b8743c4c840f5b27d486ea35e10ce75c949`).
     - The snapshot was `<snapshot>`, taken before the seal (the first, `<snapshot>`, was taken before the refused single-selection kit).
     - The seal printed `engine nils 1.0.0-alpha.76; pack 0.25.0; job pack-0250-a76 0.25.0 6245; selections final-cert-2-a@1 3122 and final-cert-2-b@1 3123; drawn 6245; sealed now 6245; ... sealed outside the draw 0; stderr lines 0` and `rc=0`.
     - Checked independently on `sealed_stack`: 3,122 under `selection:final-cert-2-a@1`, 3,123 under `selection:final-cert-2-b@1`, and the earlier samples unchanged.
9. **The packets and batches:** `tools/prep.sh` on the development laptop, after `source/reread.json` is brought up to date for the re-read of schema 79.
10. **The read runs:** llm-2 (`CONC=6 tools/chain.sh`), the three rater waves, routing, the adjudicators, settling, the flags, the manifests, then the correction record (section 8).
   - *Done (post-registration note, 2026-10-04),* with deviations 17 to 19; counts in `reference.md`.
     - Every rater answered 6,244 of 6,244 stacks; the stack left out for an identifier shape (deviation 8) is skipped by `to_inputs.py`, the sweep's line.
     - Routing sent 773 stacks to adjudicator-1 and 1,071 to adjudicator-2; 62 adjudicator agents answered every item.
     - The reference is settled; the audit found no joint error before the record and 2 against the corrected reference (bounds at most 0.54 %).
     - `final-cert-2-read` is resealed (manifest `4fa1b974…`) and mirrored.
     - `final-cert-2-read-corrections` (final-32: 127 cells on 103 stacks; localizers 0) is sealed (manifest `da916921…`) and mirrored before `final-cert-2-rules` was opened.
     - The grade has not started.
11. **The grade:** `grade.md` part 1 with its tools is committed before any stored answer is opened. The main lines come from `final-cert-2-rules`, and the selective claim from section 6.
    - *Part 1 written 2026-10-04 (deviations 20 to 23); the tools are `tools/grade/`, `tools/selG.sh`. Graded once the same day: see `grade.md`, part 2 (the certificate passes).*

## Privacy

- The packets carry no personnummer, patient name, patient id, birth date or address. This is checked on every packet before any rater sees one.
- No UID leaves a query. The frame, the draw, the trigger and the seal kit hold production ids, the cohort, the make and model, the md5 key and the cell parts. They stay in the research container, the evidence runs and the NILS server's kit folder.
- Every command over the registry writes its rows to a file, never to a terminal. Nothing per stack enters git.

## Files

| file | what |
|---|---|
| `plan.md`, `config.json` | this plan and its config |
| `tools/frame.sql`, `subst.py`, `frame.py` | the frame: header reads out by session (the final certificate's and the sweep's draws among them), the strata by today's disposition, the frozen-pack check |
| `tools/draw.py`, `weights.py`, `impact.py`, `campitems.sql`, `disjoint.sql` | the five samples with exact probabilities (sizes from `config.json`); the weights' check; what the seal takes; disjointness |
| `tools/runF.sh`, `installF.sh`, `q.sh` | the the research container steps in `p0/final2`; the code run through Bifrost; the read-only runner |
| `tools/seal_export.py`, `seal-server.sh`, `test_seal.sh` | the seal kit with certificate 2's names and the engine check; its test |
| `tools/selS.sh`, `sel9.py`, `sel_score.py`, `sel_cut.py` | the selective claim's trigger and threshold; the fetch at the draw |
| `tools/hdr.sql`, `sess.sql`, `build_packets.py`, `shown.json`, `prep.sh`, `PROMPTS.sha256` | the packets with pack 0.25.0's fields; the prompts v3.4 checked by sha256 |
| `tools/batches.py`, `agents.py`, `agent_plan.py`, `chain.sh` | batches, one folder per agent, the waves, llm-2 (`27b-v34`) |
| `tools/to_inputs.py`, `route.py`, `route.sh`, `adj_batches.py`, `settle.py`, `check_adjudication.py`, `triage_flags.py` | routing, adjudication and settling |
| `tools/lines_final.py`, `stats_kg.py`, `simulate.py`, `power2.py`, `selective2.py`, `selective2b.py` | the lines and bars; the power and the selective claim's planning |
| `../2026-09-27-header-judge/tools/compose_prompts_v34.py` and the v3.4 files | the prompts v3.4 |

**Evidence runs made while preparing:**

| run | what | manifest sha256 |
|---|---|---|
| `final-cert-2-code-4` | the tools for the dry frame (prereg) | `076e13b6d27620055bca54911c0914d19423c67790b8cb61130f4953dc723441` |
| `final-cert-2-code-5` | the tools for the real frame and the setup (prereg) | `4e1a264160568f896ab0f8ad4b66189c4ae751b18f98a3acb983c222883ae77f` |
| `final-cert-2-sel-code`, `-sel-code-2` | the selective tools as installed (prereg) | `43a321b7…`, `a0f38a5d…` |
| `final-cert-2-selective` | the threshold on the sweep (section 6) | `8f8c043bd4789262e44f5464a94ce291624194af4d8cfa4d7b08b483e6794ddb` |

## Deviations from the final certificate

Written before the registration, 2026-10-02 to 2026-10-04:
1. **The frame:** the final certificate's and the protocol sweep's draws join the header reads, out by session.
2. **The sizes** come from `config.json`, with main 5,000 (choice 1).
3. **The stored answers are fetched at the draw,** through the research views and before the seal, sealed unread in `final-cert-2-rules` (section 9). The final certificate fetched them with `--unsealed-access` at the grade. The protocol sweep and the go read did it this way. The data are the same, the frozen job is checked before, during and after, and no answer is read before the reference and its correction record are sealed.
4. **The go condition** reads the protocol sweep's held-out go read: at most 9 of p-go's 1,497 (choice 2). It is met.
5. **The scorer's `derive.py` also derives `contrast_mix` and `base_basis`** (`cert25`, section 8).
6. **The selective claim's threshold was read twice, and the higher one registered.** As written, the rule reads pack 0.25.0's errors on p-dev and cover, and gives 0.0. That accepts every stack `combined` does not fill, so the selective claim would repeat the main claim. Those errors are seen: pack 0.25.0 was built from them. The reading that reviews more uses errors no pack had been taught: pack 0.23.0's stored errors on the same stacks, scored against round 9's `combined` exactly as graded on the final certificate. It gives 0.95. I registered 0.95, the conservative reading. The rule is read on the frame-weighted estimate, since the claim and its coverage bar are frame-weighted; the plain counts are beside, and on reading 2 no threshold meets the rule plain. 0.95 sits beside the final certificate's 0.98, which the draft named as the expected neighbourhood.
7. **The selective claim's trigger uses the certified pack as `combined`'s rules** and accepts only stacks where `combined` keeps the pack's answer (section 6). System 1 keeps the inputs it was frozen for. The draft named `combined` as frozen without saying which pack's answers it reads. On the final certificate `combined`'s answers equalled the rules' on every graded stack, so the question did not arise.
8. **A drawn non-localizer without a packet** (no header, or left out for identifier-shaped text) **is counted wrong** on every claim line and listed. The lines with it as missing are beside. The final certificate left such a stack out (it had none). The protocol sweep counted it as missing with the frame as denominator.
9. **`frame.py` reads "unclassified" from the stored job, not from the technique.** Pack 0.25.0's `cant_tell` leaves the technique empty on some classified stacks. The old check counted them as unclassified, and on 2026-10-04 it would have stopped the frame on two void-kept stacks. They are drawn in the cell `empty` and scored as empty. `frame.tsv` gains the stored job id.
10. **The prompts are v3.4,** not v3.3 (section 7), and `constraints.json` relaxes two implications to pack 0.25.0's.
11. **The correction record is final-32** (section 8): R3-TFE's NO clause is removed to match round 6's answer 1.
12. **The packets gain pack 0.25.0's fields** (section 9). The builder is the sweep's, which leaves out identifier-shaped stacks.
13. **The seal kit checks the engine,** and its names are certificate 2's (section 10). The snapshot is named `before-final-cert-2-seal-*`.
14. **The read tools default to the run `final-cert-2-read`** (`prep.sh`, `chain.sh`, `route.sh`), and `to_inputs.py` names the sample `final-cert-2`. Before, they pointed at the final certificate's sealed run.

Written after the registration (2026-10-04), each before the step it touches:
15. **`selS.sh out` copied System 1's set even where none was built.** At the fetch at the draw (step 6), the fetch itself ran as registered: every drawn stack held job 400 before and after. But its `out` step stopped, since no System 1 set exists for the draw (System 1 runs only at the grade). `out` now copies the set only where it exists; nothing else changes. Installed as code run `final-cert-2-code-7`.
16. **The seal is two selections, `final-cert-2-a` and `final-cert-2-b`, not one** (written 2026-10-04, before the new kit runs).
    - **What happened:** Nima ran the registered kit (section 10). Every check passed, and `nils ask selections save` saved the selection `final-cert-2@1` (id 149, hash `b1be2ede…`, epoch 44713).
    - **Why it failed:** `nils labels seal --select selection:final-cert-2@1` then refused, with "reaches more than one answer holds". The engine freezes a selection for a seal under a row cap of 5,000 (`freeze_selection`, `Caps::default().sync_max_rows`, hard-coded in alpha.76). The final certificate's 3,750 fit under it; certificate 2's 6,245 do not. The script stopped with rc 2, and no stack is sealed. The registry snapshot `<snapshot>` exists.
    - **The leftover:** the unsealed selection `final-cert-2@1` (id 149) stays as it is. It holds the 6,245 drawn stacks, it is not a seal, and nothing reads it. The old kit's folder on the NILS server keeps its `out/` and cannot run again.
    - **What changes** (the coordinator's decision, Nima informed): the same 6,245 drawn stacks are sealed as two selections, with the frozen engine (alpha.76; no engine change).
      - The split is deterministic: the sorted drawn ids, the first 3,122 as `final-cert-2-a` and the other 3,123 as `final-cert-2-b`. Each is well under the cap.
      - The kit is `final-cert-2-split` (`tools/seal_split.py`, `tools/seal-split-server.sh`). Its the NILS server folder is the NILS server's working folder.
      - It keeps every check of section 10, applied to all 6,245 stacks. It also checks that the two parts are disjoint, that their union is exactly the drawn list, that each part is under the cap, and that neither name exists.
      - Then it saves and seals `-a`, then `-b`. It exits 4 unless exactly the 6,245 drawn stacks are sealed, under the two selections and nothing outside them.
      - If `-a` fails it stops with rc 5. If `-a` is sealed and `-b` fails, it prints plainly what is sealed and stops with rc 6.
    - **The test:** `tools/test_seal_split.sh`, on a scratch registry with more than 5,000 stacks. It shows that one selection over the cap is refused, and that the two-part seal of the same set succeeds. The stop cases are tested too.
    - **What reads both:** the sealed sample is the union of `selection:final-cert-2-a@1` and `selection:final-cert-2-b@1` in every later step that names it:
      - the seal's checks;
      - the read's isolation checks (that every drawn stack is sealed before any packet is built);
      - the certificate's unseal record (record 48);
      - the grade.

      `config.json` `seal` names both. The design is unchanged: one draw, one frame, one claim; only the registry's bookkeeping of the seal is split.
    - **Tested on 2026-10-04** (`tools/test_seal_split.sh`), on a scratch registry: alpha.76's `nils synth` with 640 subjects made 11,334 stacks, classified with pack 0.25.0 as job `pack-test`, in the test container, with HOME, the XDG folders and TMPDIR on disk in scratch. The cases:

      | case | result |
      |---|---|
      | CAP: one selection of 5,100 stacks (the registered kit) | refused by `labels seal` with the same message as on the NILS server, rc 2, nothing sealed |
      | A, a wrong frozen job | rc 3, nothing written |
      | A2, a wrong frozen engine | rc 3, nothing written |
      | K, a part list changed after the kit was hashed | rc 3, nothing written |
      | NX, the names exist already | rc 3, nothing written |
      | C, the two-part kit of the same 5,100 stacks | rc 0: 2,550 under `-a`, 2,550 under `-b`, all 5,100 sealed, none outside |
      | D, C's kit again | rc 3 |
      | B, the same stacks, now sealed | rc 3 |
      | P6, part a (100) under the cap, part b (5,100) over it | rc 6, "PARTLY SEALED": 100 sealed under part a's selection, part b not sealed |
      | P5, part a (5,050) over the cap | rc 5: nothing sealed, part b not tried |

      The development laptop's NILS install was unchanged: units, paths and databases identical before and after. The scratch database was dropped.
    - **Nima's command** is section 10's step 2 with the new folder names: `K=the research folder`, `D=the NILS server's working folder` and the script `seal-split-server.sh`. A new snapshot is taken first (step 1). The kit's `KITSHA` and the evidence run (`final-cert-2-read-seal-split`, a supplement to the sealed `final-cert-2-read`) are recorded when the kit is written.
    - **The kit, written 2026-10-04** in the research container with code run `final-cert-2-code-8` (`5989954`), in the run folder's `seal-split/`:
      - `final-cert-2-a` holds 3,122 stacks (ids sha256 `fc594c8f…`), and `final-cert-2-b` 3,123 (`165c9e8e…`);
      - together they are the registered kit's 6,245 stacks, the same id list (`b02f2ed2…`);
      - the largest part allowed is 4,999.
      - `KITSHA` is `f2ffbea80f28042d85b30c6973411b8743c4c840f5b27d486ea35e10ce75c949`.
      - The evidence run is `final-cert-2-read-seal-split`: 17 files, manifest `3fe0398164b05c2d9c9ef0e96db4ba9e4b25e2aba313f0ca284add72f16c2f93`, sealed and mirrored.
17. **Each agent is started with a one-line pointer to its own task text** (written 2026-10-04, before the first rater). The sweep's task texts (`tools/tasks.py`, the same words) are written into each agent's folder as `TASK.md`, hashed in `logs/agent-prompts/raters-task-files.sha256.json` (and `adjudicators-*` later), and the agent is started by the Agent tool with: "You are an independent rater of MRI series. Your whole task is written in <folder>/TASK.md: read it fully first and follow it exactly. Work only inside that folder." The sweep started its agents from a workflow script with the text inline; that tool is not in this session. At most 19 agents run at once (the harness's limit), one rater after the other, the next rater's first batches starting as the last batches of the one before finish.
18. **The account's usage limit stopped the read once** (2026-10-04, about 12:30 UTC), part-way through the `claude-family` wave; the development laptop was then rebooted and the session moved to another account. `claude-tags` was complete (6,244 of 6,244, collected, no schema or legality failure). Of `claude-family`, 46 batches were complete and are kept; 19 were part-way: their outputs are moved unread to `raters/_first/claude-family/` and each batch is answered once more in full by a fresh agent with the same task text, so that no batch mixes two agents' answers (the final certificate's and the protocol sweep's rule); 40 had not started. llm-2 was stopped by the reboot at about 1,032 answers and restarted the registered way (`CONC=6 tools/chain.sh`, user unit `cert2-llm2`); judge.py skips answered stacks. The design is unchanged. Checkpoints go to the run's `logs/checkpoints.log` and to `status.md` in this study.
19. **llm-2 was restarted at concurrency 8, not the registered 6** (2026-10-04, 14:27 UTC, at Nima's request: run more in parallel), at 2,474 of 6,244 answered, as the same user unit `cert2-llm2` (`CONC=8 tools/chain.sh`). The model server's `--max-running-requests` for the 27B is 8, and dev-4 ran it at 8. The chain's registered note says the answers do not depend on the concurrency (temperature 0, one request per stack), and judge.py skips answered stacks, so no stack is answered twice. Only one copy runs.

20. **The stack left out for an identifier shape is counted wrong on the claim lines, and on the selective claim in the conservative way** (written 2026-10-04, before the grade opens any stored answer). It is one Siemens stack of the main sample.
    - **Main, per make, the lines:** counted wrong (deviation 8). Beside it, the lines with it carried as not wrong (the frame as the Hajek denominator, the sweep's line).
    - **The selective claim:** the registered counting is "wrong on every claim line", but the stack may or may not be accepted by the trigger. The primary reading counts it **wrong in the accepted domain whether or not the trigger accepts it** (the reading that does not favour the pack). Beside it: the trigger decides it ("follows"), and it carried as not wrong ("as missing"). Coverage, catch and review share use the trigger's own decision for it in every reading.
21. **The selective claim's grade is `tools/grade/sel_grade.py`, written before the grade and dry-run on a synthetic trigger** (written 2026-10-04, before the grade). The plan registered the claim, the threshold 0.95 and the coverage bar 85 % but not their estimators; they are fixed here, before any trigger exists for the draw:
    - **Accepted:** a main stack with a trigger row, a score at or above 0.95 and `keeps`; a stack the trigger lacks goes to review.
    - **Error:** Hajek over the accepted domain with the Korn-Graubard 95 % bound over make x field (`lines_final.kg`, the main line's estimator), at 1 %; per make at 3 % where 299 or more are accepted. Exact bound beside.
    - **Coverage:** the accepted share by its Hajek estimate on the main sample, at least 85 %. **The lower 95 % bound** beside is one minus the Korn-Graubard upper bound of the review share, the same estimator on the complement. The plain share is beside.
    - **Pass:** the error bound at 1 %, every held make at 3 %, and coverage (the Hajek estimate) at 85 %.
    - **A (`rules-err`), described:** the smallest set of top-scored stacks (ties together) whose Hajek coverage is at least B's; its accepted n, wrong, Hajek rate, Korn-Graubard bound and make lines. No bar.
    - **Catch and review share:** the wrong main stacks (ten axes) that the trigger sends to review, of all wrong (plain and weighted), and the review share, for B and for A.
    - **The grid:** B at every threshold of `certificate.thresholds`, information only. The threshold stays 0.95.
22. **The grade reports counts, and does not read the error lists** (written 2026-10-04, before the grade). The final certificate read each wrong stack and gave it a kind. This grade does not: it reports the counts by axis, make, field and cohort, the cells wrong and empty, and the lenient reading (`counts.py`). The stack-level lists go to a sealed evidence run (`final-cert-2-grade-private`) and stay unread. If the certificate fails, they are development evidence for the next pack and their reading is Nima's decision.
23. **The read run's `logs/checkpoints.log` holds one line more than its manifest** (found 2026-10-04 at the grade's verification). The 18:13:54Z checkpoint was appended after the reseal. The file's sealed content is the manifest's (`577e6755…`, equal to the mirror's copy): the local file with its last line removed hashes to it. Every other file of the 35,436 matches. The grade appends its own checkpoint lines to the same log, as the read did; the mirror's copy is the sealed one.
