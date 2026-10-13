# 50 · Body part from the image

Opened 2026-09-26. Follows the study `studies/2026-09-26-body-part-image/study.md` and Nima's answers to it. It amends [48](48-the-reader-and-two-learners.md) for body part.

## The ask

> Nima, 2026-09-26: "we had it as QC (a prototype) but it can be part of our engine in v1 instead of relying only on LightGBM or rule or other model. it is specialized to look at the pixel. but v0 was good in a sense that you could add a new category by technically defining your category and a small post train on only few samples from your data and are very specialized. there might be a better solution or we might not need to have two models like in v0. or even an architecture far more simpler and faster to the point that also can run on cpu. so we need to explore all these ... create a campaign to label chest, brain, brain-neck, neck, spine and other to cover the unknown. as you see in v0 the way we treat each orientation also is different (ax, sag, cor) but maybe there is a better solution for this. a smarter one."

## Rulings

| | Ruling |
|---|---|
| R1 | Body part from the pixels becomes part of the engine: one image model, not two, and CPU-capable if the bake-off allows. It is fused with the header's view in System 1's body-part head |
| R2 | **The labels:** chest, brain, brain-neck, neck, spine and other, each with a confidence. Other covers the unknown. New categories can be added later from a few examples of our own data, as v0 allowed |
| R3 | **Gold, not silver.** v0's committed labels (the legacy storage server and the production host) and every model proposal go into a campaign as suggestions. A person checks about 100 at a time, corrects the wrong ones and accepts the rest in one move. What a person accepted is gold |
| R4 | **The data:** our own (the legacy storage server's QC'd labels, everything on the NILS server, the other cohorts in the source pool), open labelled datasets, and CT scans too |
| R5 | Open data lives in the compute server's shared place |
| R6 | **The architecture is found by a bake-off** for the fastest model that is accurate: v0's design is one arm among several, including far simpler ones. Orientation is handled by the smartest scheme the bake-off finds, not by separate treatment per ax/sag/cor unless that wins |
| R7 | **The loop:** the model's per-class confidence drives the next campaign (the least certain first), then retraining |

## Open

- The pre-registered bar for "beats v0" is the study's five criteria, applied to the winner of the bake-off.
- The privacy rules of record 39 hold for any weights trained on patient pixels.

## The first gold campaign (2026-09-26)

`bodypart-gold-r1` opened on the production host's reader with 2,000 stacks, shown with v0's committed answers as suggestions:
- 1,082 brain-neck cases;
- 192 stacks a person set in v0;
- 222 where v0 was below 0.70;
- 41 from rare strata;
- the rest from the large strata, least certain first.

The campaign closes into nothing, so nothing becomes a decision until Nima decides. 220 items are held back to be read alone.

**Same subject as a sealed stack.** 992 of the items belong to subjects who also have a stack in the sealed certification samples, whose body part is usually the same. Their gold labels are kept, but marked, and never train a model that a certificate grades. The certificate stays independent of the stacks it measures.

**Round 5 confirmed why brain-neck fails from the header.** No rule and no learner ever answers brain-neck (0 of 81 true cases on study H). Study H reaches 0.91 when brain-neck is folded into brain. Philips spine fails from the header, because the names do not say spine. Body part needs the image.

## To come (Nima, 2026-09-26)

- **A body-part report,** like the learners' report (`studies/2026-09-24-phase-0/learners-report.md`): what was tried, the data, the bake-off's designs compared, the gold labels and what they changed, the winner with its model cards. It is written after the gold campaign and published as a page.
- **The research scratch registries on the NILS server are not the production digest,** so the data there stays free to change or augment. The photon-counting CT study is left out as too homogeneous (brain only, 14 constructs per session), and diverse CT is still wanted.

## After the first gold campaign (Nima, 2026-09-26)

- **Brain-neck is confirmed as labelled:** "I select all brain that included any part of neck brain neck. so yes. most of them were below c2 anyway."
- **The first round was unbalanced:** brain-neck 1,521, brain 406, spine 43, neck 30, chest 0, other 0. Its draw followed v0's labels, which never say chest or other.
- **Spine must come from study B and study H** ("we must have way more spine in study B or study H"). Study B's pixels are on the NILS server, not the production host.
- **Round 2 is drawn by the models:** the bake-off's teacher and the tiny model score every unlabelled stack, and the draw takes mostly spine, neck, chest and other, plus the uncertain stacks. It aims at about 300 per rare class, on the production host for study H and on the NILS server for study B and the newly digested cohorts.

## After the gold campaigns (2026-09-27)

Report: `studies/2026-09-26-body-part-image/gold-report.md`; model card `models/bodypart-tiny-gold-2026-09-27.md`.

- **`bodypart-gold-r2-mms` is closed** into none on production: 957 of 960 answered, 0 unsure, 765 of the shown suggestions changed. The open-data model agreed with 20 % of the answers, the study B model with 50 %, the rules with 49 %. The 97 held-back items read alone agree with the models as the batches do.
- **Gold is 2,955 stacks from 688 subjects** (r1 1,998, r2-mms 957; r2-fg2 closed empty): brain-neck 1,662, brain 722, spine 278, neck 200, chest 90, other 3. 502 share a subject with a sealed or drawn stack and never train (the record's earlier 992 is to be reconciled).
- **The tiny network trained on gold beats the old model and the rules on six labels** at the held-out study B sites (accuracy 0.736 against 0.539 and 0.622; macro-F1 0.749 against 0.267 and 0.352) and learns brain-neck (9 confusions in 220). It is weaker on brain against spine there (0.68 against the rules' 0.97).
- **Open data** helps the open test and study H and costs 11 points at the held-out sites; **v0's silver** fixes brain against spine and erases brain-neck, neck and chest, because v0's labels mean something else.
- **Nothing is registered.** Proposed next: silver as partial labels in the new label set, a definition of neck against cervical spine, a gold round on thin strata (study B brain and spine across sites, GE, our own "other"), and a CT round of about 200 the photon-counting CT study subjects in the research scratch (Nima allowed the photon-counting CT study on 2026-09-27; about an hour of machine time). Nima's decisions G1 to G5 are in the report.

## After the gold report (Nima, 2026-09-27: "agreed on all")

| | Ruling |
|---|---|
| G1 | **Register nothing yet.** No body-part model is registered until one passes the bar at the held-out sites (ECE at most 0.05, non-inferior to the rules on brain against spine) |
| G2 | **Neck against cervical spine.** A stack is *spine* when its field of view is centred on the vertebral column: sagittal spine protocols, HWS, the axial slices of a cord or spine exam. It is *neck* when its field of view is centred on the soft tissue of the neck: larynx, thyroid, the vessels. As with brain-neck, the label follows what the image is centred on, not the coil or what the exam was registered as |
| G3 | **The same-subject rule excludes the union of every sealed set ever made.** A gold stack whose subject has a stack in any of them never trains a model that a certificate grades |
| G4 | **The CT round waits for a de-identified tree** of the photon-counting CT study's second site (`dcm-anon`), made the NILS way. Nothing digests `dcm-original` |
| G5 | **Label sets get their own export place** instead of `bids`. The current export stays where it is until that place exists |
| G6 | **Train on augmented, noisy and bad data too** (Nima: "we need to train the model on augmented noisy and bad data too") |
| G7 | **The header helps** (Nima: "i agree that header might help"). The image and header fusion is the main candidate, and the image-only model is its component |

**G2 applied to the gold set.** I relabelled only where the header settles it without doubt: a neck answer whose own words (description, protocol, sequence name, comments) name the spine (the pack's spine words, such as HWS, cervical, "c col", halsrygg) and no neck soft tissue became spine. The same rule the other way (a spine answer whose own words name neck soft tissue) found none. The tool is `r3_g2.py`.
- **68 neck answers became spine:** 67 at five sites of study B (20 sagittal) and 1 in study H. Gold is now brain-neck 1,662, brain 722, spine 346, neck 132, chest 90, other 3.
- **50 more are listed and kept:** neck answers whose own words say nothing and whose exam is registered as a spine (mostly axial cord slices in study D and at one site of study B). The header does not settle these. They go into round 3's campaign.
- The held-out sites now hold brain 156, spine 194, neck 20, brain-neck 64, chest 39.

**G3 computed.** The union is the production host's three sealed samples (1,981 stacks: the whole of the production host's `sealed_stack`), the read-r1 selections, every certification draw carried by the move (including the 800-stack void uncertain draw, of which 381 were sealed), the production host's forbidden list, the move-check scratch's 60-stack test seal, production's `sealed_stack` (empty on 2026-09-27), and the move package's own the production host-subject mark. Mapped onto the research registry, that is **1,569 subjects** (203,272 of their stacks). It marks **650 gold stacks** (r1 612, r2-mms 38), against 502 before, and leaves 2,305 free to train. The tool is `r3_sealed.py`.
- Record 50's earlier 992 could not be reproduced. On the production host's own subjects, r1 has 408 items that share a subject with a sealed stack, 408 with the forbidden list added, and 493 with every draw added (53 by study). The 992 is withdrawn, and 650 is the count now.

**G4 planned, not run:** the de-identification plan in `studies/2026-09-26-body-part-image/`. The dataset is declared identified with its originals tree, the map comes from Nima and never enters a session, then `nils pseudonymize` writes `dcm-anon` (about 3.8 million files and 4.9 TB, about 95 minutes), and the research digest reads `dcm-anon` only. The plan found that one of production's de-identified places already covers this identified tree, and it proposes narrowing or retiring that place.

**G5 proposed, not changed:** `studies/2026-09-26-body-part-image/g5-labels-place-proposal.md`. It proposes one `nils place add labels ... --role export`, either on a new dataset of the storage server (recommended) or inside the results place. With two export places, the desk's export needs the place named.

## Round 3 (2026-09-27)

Report: `studies/2026-09-26-body-part-image/round3.md`, plan `round3-plan.md`, 56 model cards in `models/r3/`. Nima, 2026-09-27: explore architectures ("simple, fast and accurate"), train on augmented, noisy and bad data, and let the header help.

- **Nothing passes the bar, so G1 holds.** The candidate chosen by the registered rule is the LightGBM header fusion over the tiny 2D network, trained with the corruption suite and the SigLIP2 teacher's pseudo-labels. At the held-out sites it reaches accuracy 0.856 and macro-F1 0.715, with ECE 0.155, and brain against spine at 0.889 against the rules' 0.960. For comparison, round 2's gold-only network on the G2 labels reaches 0.647, 0.645 and 0.144, and the rules 0.742 and 0.480.
- **Abstention does not rescue it.** At the threshold chosen on the pool, it answers 49 % of stacks with accuracy 0.935 and ECE 0.097, and brain against spine is -0.020 [-0.056]. It meets the bar only below 10 % coverage.
- **The header carries the transfer (G7).** The header alone reaches 0.899 accuracy at the held-out sites, within 0.009 of the rules on brain against spine, and finds 86 % of brain-neck. Every image model alone stays at 0.55 to 0.82 there. Fusion adds 7 to 30 points to each image model.
- **The corrupted and noisy data helps (G6)**, on corrupted copies (+0.06 to +0.10) and on clean held-out stacks (+0.02 to +0.08). Physical extent matters: without the geometry the network loses 12 points.
- **Architectures:** eight arms were compared. The pretrained 2D networks are within 5 points of the 52k-parameter tiny network, at 4 to 50 times the CPU time. The Pareto front is the tiny network (0.13 ms, ONNX), LightGBM on profiles (0.27 ms), MobileNetV3-small (0.54 ms) and the SigLIP2 reference (2.5 s).
- **The registered selection by leave-one-site-out chose nothing.** Its intervals overlapped for every variant, because the pool is 73 % study H. Round 4 needs a registered change there (decision 3 in the report).
- **Round 3's campaign is drafted and not opened:** `bodypart-gold-r3`, 400 stacks on production. It holds 120 disagreements with the rules, 80 brain against spine, 100 neck against cervical spine (G2) and 100 brain-neck outside study H, from 12 sites and 4 makes. The files are in the install's working place, and the report says how to open it for the rater account.
- **G3 rechecked:** production sealed 1,970 stacks during the move on 2026-09-27; they add no subject to the union of 1,569.

## Gold round 3 and round 4 (2026-09-28)

Report: `studies/2026-09-26-body-part-image/round4.md` (part 1 the pre-registration, committed before any training; part 2 the results), 13 model cards in `models/r4/`.

- **`bodypart-gold-r3` is closed** into none on production and exported (answers 350 rows, settled 349): 349 of 349 answered, 0 unsure, 0 can't tell, 1 amended, 200 changed from the suggestion (57 %). Labels: brain-neck 116, neck 101, brain 93, spine 35, chest 3, other 1. The drawing candidate agreed with 43 %, production's rules (pack 0.10.0) with 26 %; the 24 held-back items agree with the candidate on 38 %. 51 drafted stacks were excluded by the pack as secondary reformats. Gold is now 3,304 stacks.
- **Round 4 followed the round 3 decisions:** header first (the image a secondary input where the series' own words name no anatomy or name a localizer, and for chest), per-site calibration, the choice on gold round 3 (V), the tiny network and MobileNetV3-small with and without open data, augmentation on, G2 labels, the G3 union, production's rules as the comparator.
- **Nothing passes the bar, so G1 holds.** V chose header first over the tiny network with open data. At the held-out sites it reaches accuracy 0.664, per-site ECE 0.062, and brain against spine -0.343 [-0.402, -0.285]. V, drawn where models were unsure at new cohorts, favoured open data, which costs the tiny network 12 to 14 points at the study B sites.
- **Per-site calibration works:** it halves ECE at the held-out sites for every model. The best reference, full fusion over the tiny network without open data, reaches accuracy 0.873, per-site ECE 0.029 (0.056 with only 25 labelled stacks per site), and brain against spine -0.074 [-0.114, -0.042], with 28 spine stacks called neck. It meets the bar at 35 % coverage and below.
- **The gate of header first is on for 97 % of training stacks:** series names rarely name the anatomy, so header first is not a separate model in practice.
- **Post hoc, the cause:** 21 of round 3's neck answers name the spine in their own words, while G2's relabelling turned 68 such r1 and r2 answers into spine. With G2's word test applied to those 21 too, the same fusion reaches 0.926, per-site ECE 0.026, brain against spine -0.003 [-0.028, +0.026], meeting the bar at 98.5 % coverage. It used the test sites to find the cause, so it sets up round 5 and passes nothing.
- **Decisions for Nima** (report section 8): register nothing; settle neck against cervical spine on those 89 stacks (a short campaign read alone with G2's text); round 5 pre-registered as full fusion over the tiny network without open data, per-site calibration, and a fresh held-out site set; choose on a random gold sample of new sites, not on a campaign drawn where models are unsure; drop open data for our sites; drop the header-first gate; G4 and G5 still wait.

## Body part leaves Phase 0's header certificate (Nima, 2026-09-28)

Nima ruled that a reference read from the header is not valid for body part. His gold rounds showed that the header and the rules match the picture only about half the time: 80 % of round 2's suggestions were changed, and the rules were right on 49 %. So:
- **The sealed read `sealed-1` certifies the header axes only.** Those are provenance, technique, modifier, construct, base and post-contrast, plus the derived five as registered (record 48; `studies/2026-09-28-sealed-reference/README.md`, deviation note).
- **Body part is certified separately, on this record's track, from the pictures:** the image model, Nima's gold for development, and a sealed picture-read test set later.
- The body-part picture step is dropped from sealed-1. The raters' header answers for body part stay in the archive and are not a reference.

## Round 5 (2026-09-28)

Report: `studies/2026-09-26-body-part-image/round5.md` (part 1 the pre-registration, committed before the draw and any training; part 2 the results so far), the G2 settle `studies/2026-09-26-body-part-image/g2-settle/README.md`, 3 model cards in `models/r5/`. Nima agreed on 2026-09-28 to round 4's decisions.

- **G2 settled by agents:** the 89 stacks (68 r1 and r2 relabels, 21 r3 neck answers whose own words name the spine) were read from de-identified header packets by two Claude raters and the local 27B judge under G2's text. All 89 unanimous spine; no adjudication was needed. Evidence run `bodypart-g2-settle` (manifest in `studies/phase0-evidence/runs/`), mirrored on the group's machines.
- **Gold version r5** (gold r4 kept): 3,304 stacks, brain-neck 1,778, brain 815, spine 402, neck 212, chest 93, other 4; 2,654 trainable from 14 sites.
- **Fresh held-out sites:** study K (never read, never in the research registry; Siemens 7 T, 3 T, 1.5 T and GE) and a site of study B. Study I, the other site without gold, is not on production's desk and holds only brain by the rules.
- **A random gold campaign, drafted, not opened:** `bodypart-gold-r5`, 488 stacks, no suggestion shown. S (150, a third of study K's subjects) chooses between the fusion and the header alone and sets the threshold; T (338: 300 from study K, 38 IT3, no subject shared with S) is judged once.
- **The candidate is trained and frozen before the read:** full fusion over the tiny network, no open data, all 14 gold sites, per-site calibration as in round 4. The sha256 of the frozen predictions and models are in `round5.md`.
- **Nothing is judged and nothing is registered (G1 holds).** Descriptive leave-one-site-out: 0.886 accuracy and per-site ECE 0.024 over 14 sites; at the old held-out sites 0.892, ECE 0.058, brain against spine -0.046 [-0.069, -0.021].
- **A warning before the read:** at the fresh sites the frozen fusion agrees with the rules on 65 to 71 % of the stacks they answer, and calls spine far less often than the rules (52 against 95). No 7 T stack is in the training gold.
- **Next:** Nima opens and reads `bodypart-gold-r5` (`campaign-draft.sh`, about 500 answers), then `r5_score.py fresh` judges the bar.

### The read of `bodypart-gold-r5` (2026-09-28)

Report: `round5.md` part 3; evidence run `bodypart-r5-read`.
- **Closed** into none and exported: 399 items (the pack excluded 89 without a rules answer: all of IT3 and 51 of study K), 399 answered, 0 unsure, 107 changed from the suggestion. **Deviations:** held back 0.1 (the engine refuses 0), and the engine showed the rules' answer on every item although the registration said no suggestion. Batches agreed with it on 75 %, items read alone on 55 %.
- **The split held as registered:** S 133 (23 subjects), T 266 (45 subjects, all study K, 32 spine), no subject shared, 0 mismatches with the draw.
- **S kept the fusion** (macro-F1 0.708 against the header alone's 0.564); no threshold met the bar on S.
- **The bar is not met (G1 holds; nothing registered).** On T: accuracy 0.789 [0.736, 0.838] against the rules' 0.756, macro-F1 0.653 against 0.318, per-site ECE 0.052 [0.048, 0.114] (0.062 with S's temperature), brain against spine -0.153 [-0.199, -0.109] (model 0.842, rules 0.995). It meets the bar only at 25 % coverage and below.
- **The failing half is brain called brain-neck** (26 of 170 brain stacks), not spine: brain was never called spine. The rules never say brain-neck. Post hoc, counting brain-neck on a brain stack as right: -0.025 [-0.049, 0.000].
- **Decisions for Nima** (`round5.md` part 3, section 5): register nothing; whether brain against spine should count brain-neck on a brain stack as an error; read test samples without the engine's suggestion; a look at the 26 brain stacks called brain-neck; neck still weak (5 of 20); study K's 399 can join training in a round 6 with another fresh site.

### Brain against brain-neck (Nima, 2026-09-29: "yes on the brain-neck argument")

Evidence: `round5.md` part 4. By measured reach below the vertex, 25 of the 26 brain stacks the frozen fusion called brain-neck reach below C2, and 1 does not (a scout). 21 of 24 other sagittal or coronal brain stacks also reach below C2, so the reader's boundary is not the geometric one.
- **Ruled, from round 6 on:** the brain-against-spine half of the bar is judged on the brain-or-spine question, and a brain-neck answer on a brain stack counts as brain. It applies only to a later round with a fresh test; round 5's result stands (G1 holds).
- **Ruled:** brain against brain-neck is a coverage fact, not a reader's call. It is derived from the stack's reach below the vertex (below C2 means brain-neck), and the gold's sagittal and coronal brain and brain-neck stacks are relabelled by that rule before they train or score. Where the reach is undecidable, the reader's label stands and is marked.
- **With it:** test samples are read without the rules' suggestion (engine #291, `campaign --suggest none`, once released).

### Round 6's fresh site (Nima, 2026-09-29: "on study A never read i also say yes")

No production MR cohort is both never read and outside the research registry. Nima accepted the relaxed criterion: round 6's test site is study A's 19 scanner models that never gave gold, with subjects that have gold, sealed or gold-model stacks excluded (159 subjects and 5,422 MR stacks: Siemens and GE, 1.5 and 3 T, 609 spine by the rules). It is new scanners of an institution that did give gold, never read and never trained on. The report states this limit (round6.md part 1, nils-design ad93a4c). The draw waits for engine alpha.59 and desk alpha.57 (`campaign --suggest none`).

### The read of `bodypart-gold-r6` (2026-09-29)

Report: `round6.md` part 3; evidence run `bodypart-r6-read`. Nima read 610 items blind (`--suggest none`, engine alpha.59): S 150, T 400, TN 60, all at study A's never-read scanner models. Subjects sharing a study with another cohort were excluded before training (123 subjects remain). The model was trained on gold r6 and frozen on the whole pool before the draw.
- **The bar's numbers hold, but T is underpowered, so G1 holds and nothing is registered.** On T:
  - accuracy 0.915 [0.881, 0.943] against the rules' 0.570;
  - per-site ECE 0.026 [0.020, 0.065];
  - brain-or-spine +0.032 [-0.005, +0.086] (model 0.978, rules 0.946).
  - T holds 21 spine stacks by the read, below the registered 30. The rules' 61 spine answers in T were 20 spine, 16 neck, 6 chest and 19 head by the read.
- **S kept the fusion** (macro-F1 0.679 against 0.586). S's threshold is 0.336, which gives full coverage on T.
- **Neck is still weak:** recall 0.41 on T plus TN (34 neck stacks), with 17 called spine. The neck arm's image component alone reaches 0.85 (descriptive). The neck arm's rule on S is not met (10 neck stacks).
- **Blank thumbnails (Nima's report):** no answer is unanswered or unsure because of them. The one unsure answer is the stack whose pictures failed (dropped). Agreement with the model is flat across the six gallery submissions.
- **Decisions for Nima** (`round6.md` part 3, section 7):
  1. register nothing;
  2. a stratified spine draw for round 7's power;
  3. both modes pre-registered with their own bars;
  4. the pipeline for two modes from one model;
  5. `tiny-nw` as round 7's image component for neck;
  6. the neck audit;
  7. Study A's new scanners join training.

### One model, two modes (Nima, 2026-09-29: "we report both mode. we need one model but we can report and function as two")

- **Ruled:** body part has one model, reported and used in two modes.
  - **Fine:** the six classes.
  - **Coarse:** head = brain + brain-neck, spine = neck + spine, chest, other. It is the same model's probabilities summed over the merged classes, then calibrated. It is not a second model.
- **Round 6:** the verdict is the fine mode's registered bar. The coarse mode is reported beside it as descriptive, because it was ruled after the draw.
  - On T, coarse accuracy is 0.973 against the rules' 0.917, per-site ECE 0.014, and coverage 0.90 at 2 % risk.
  - Over the 15 training cohorts (leave-one-site-out), coarse accuracy is 0.950 against 0.884.
- **Proposed from round 7 on:** both modes pre-registered, each with its own bar (coarse: ECE, head against spine non-inferior to the rules, and a coverage-at-risk target), so the coarse mode can be certified too. The pipeline would serve both from one derivative: six calibrated probabilities, the coarse ones computed from them, both answers and confidences, a derived pack axis for the coarse region, and a threshold and route per mode.

### Round 7's pool and neck settle (Nima, 2026-09-29: "yes to both")

- **The pool:** study A's never-read subjects, from any scanner model. No fresh site is left. The patients are new, but the hospital and its scanners have been seen, and the report states this limit.
- **The neck and spine settle:** the test's neck and spine reads get the G2 header settle, blind to the model, as registered in round7.md part 1. Coarse mode does not depend on it.

### The read of `bodypart-gold-r7` (2026-09-29)

Report: `round7.md` part 3; evidence runs `bodypart-r7-read` (manifest a1a9fa0e…) and `bodypart-r7-g2-settle` (be4bc8f0…). The test was 600 blind reads (S 150, T 450) of the untouched subjects of every cohort, stratified by cohort and a blind spine stratum and weighted. One model was chosen on S: `ff-tiny`.
- **Fine mode passes its registered bar at S's threshold** (0.897). T's weighted coverage there is 0.68, with error 1.4 %, per-site ECE 0.018, brain-or-spine +0.011 [+0.003, +0.020], and spine n 137. At full coverage it misses: brain-or-spine -0.017 [-0.034, -0.001]. Accuracy against the rules is 0.887 against 0.558.
- **Coarse mode fails on its coverage part only.** Per-site ECE 0.017, accuracy 0.972 [0.953, 0.986] against the rules' 0.944 (+0.028 [+0.011, +0.046]), and spine n 139 all hold. S's threshold covers everything, and T's error there is 2.8 %, above 2 %. T's own curve gives 0.98 coverage at 2 % risk.
- **Neck:** G2's header settle turned all 72 neck reads into spine, so the registered neck recall is empty. Before the settle, the chosen model found 18 of Nima's 50 T neck reads (0.46), and the neck arm's image component 42 (0.82).
- **Decisions for Nima** (`round7.md` part 3, section 8):
  1. register fine mode with its abstention threshold, or not;
  2. coarse: accept, or register a threshold with a margin in a round 8;
  3. one authority for neck against cervical spine (picture read, G2, or a joint rule);
  4. the desk's black and gray screens (3 answers);
  5. 6 stacks without features.

### Round 7's decisions (Nima, 2026-09-29: "agreed on all")

- **Certified: fine mode.** ff-tiny, answering at or above its S threshold 0.897 and abstaining below it. The limit, stated in every report: new patients, from hospitals and scanners seen in training. The registration is prepared in `studies/2026-09-26-body-part-image/registration-r7.md`. Production steps wait as exact commands.
- **Coarse mode:** a small confirmation round (round 8), with the threshold fixed with a safety margin on the selection set and about 300 fresh reads.
- **Neck against cervical spine, ruled:** a field of view aimed at the cervical vertebrae is spine, even though the neck is visible. Neck is imaging aimed at the neck's soft tissue or vessels (larynx, thyroid, carotids). G2's header settle stands, and the gold is relabelled by it (gold r8).
- **The desk's black or gray screens:** a separate task.
- **The 6 stacks without features:** abstentions in use, registered drops in the test.

### Round 8: coarse mode confirmed (2026-09-29)

Report: `round8.md` part 3; evidence run `bodypart-r8-read` (manifest 66a9ac3e…). 300 blind reads of the untouched subjects of every cohort after round 7 (294 scored). Round 7's frozen ff-tiny in coarse mode, with t\* 0.9152 fixed with a margin on round 7's read.
- **Every bar part holds:**
  - per-site ECE 0.011;
  - accuracy 0.983 [0.965, 0.996] against the rules' 0.978 (+0.005 [-0.009, +0.018]);
  - at t\*, error 1.3 % at coverage 0.92;
  - coarse spine n 78.
- **Gold r8** (for later rounds): 250 of 271 unsettled gold neck labels settle spine under G2, 6 stay neck and 15 are unsettled. Round 7's read joins: 4,903 stacks.
- **Decisions for Nima:**
  1. register coarse mode as the second mode of the same model at t\*;
  2. the pack gains a derived `body_region` axis.
