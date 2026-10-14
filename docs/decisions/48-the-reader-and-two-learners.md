# 48 · The reader, and two learners

Opened 2026-09-24. **The reader merged 2026-09-24** as kineuro/nils#221 and kineuro/nils-desk#128, after two reviews that made blind reading truly blind; not yet released. Nima's directions after reading the state of Wave 6. They amend [39](39-two-ways-of-knowing-a-stack.md), [40](40-the-road-through-wave-6.md) and 46.

## The ask

> Nima, 2026-09-24: "you know that manual check we decided to be in a way that also can be use to update the performance by training the model. right? 500 seems good but it should be a very 'next level' easy and fast in terms of UI/UX to speed things up. like smart suggestions, easy information on screen like the flags that mattered or were used during the rule or so on."

> "CLM and LightGBM both should be fine tuned for our work with neurology term and MRI/CT language. I think CLM can help us for unseen languages and abbreviations and mistake. so i like to have both but having different weight for different thing as they are powerful at."

> "segmentation can be for later. we have neuroverifier and analysis pipeline (with agentic help) that can and should be before that since it will sit on top of all." Neuroverify is Adithya Raju Ganeshan's own QC app, not part of NILS. It moves to the group's servers later and joins segmentation after that.

## 1. Every check a person makes teaches the model

A person's answer is a label, and every label goes back into training. The one rule that keeps the paper honest decides **when** a label may train (record 40 R3):

- **Labels from a sealed certification sample** first measure the model they were drawn to test. Once that certificate is computed and recorded, they are unsealed and join the training set for the next model version. The next certificate draws a fresh sealed sample. So no check is ever wasted, and no model is ever graded on its own homework.
- **A sealed item is read blind.** The reader shows no suggestion and no candidates for a stack drawn in a sealed sample, so the reference labels are not anchored to the answer of the model they grade. Such items never enter a batch, and their timing is kept apart from the rest. (Found in review, 2026-09-24.)
- **Raters read a sealed sample under a rater's account:** one that holds the campaign grants and nothing else. The engine hides the classification of a sealed stack from that campaign's raters on every door they can reach. A rater who also held query rights could still look a stack up in the ask, so the procedure closes that gap.
- **Unsealing needs a real certificate recorded by a person.** The certificate names the sample and its digest, the certified risk, the sample size and the errors found. A different person unseals it.
- **Every other answer** trains at once. That covers development draws, cases the model asks about because they would teach it most, and disagreements between the two systems. These sit alongside the sealed draw, never inside it.
- The engine already holds the pieces: `nils labels seal`, a label set's sealed flag, and the rule that refuses to register a model trained on a sealed set (record 42).

## 2. The reader

A person reads a stack in seconds, not minutes. The rating workspace of record 45 becomes the reader. Its bar is the time per decision, measured as it runs.

- **Smart suggestions first.** The answer the rules and System 1 agree on is filled in, and a person confirms it with one key. Where they disagree, both candidates show, legal combinations only, each with its probability. A different answer takes one or two keys.
- **The evidence on screen, short:**
  - the flags that decided;
  - the rules that fired and the clause that held;
  - the header values each clause read (TR, TE, TI, flip angle, sequence name, image type);
  - the words it matched, when the detail level allows them;
  - what System 1 weighed most.

  It is one line per axis, with the rest one key away.
- **The picture at once:** the three planes and the stack view, prefetched for the next items so there is no wait between stacks.
- **Like with like:** stacks that look the same (same sequence, same answer suggested) are offered as a batch. A person checks a few and accepts the rest in one move. Each accepted stack still gets its own decision, and a random few stay in the certificate's draw.
- **Order by value:** first what the model is least sure of or the two systems disagree on, and within that what teaches most.
- **Speed is measured.** Seconds per decision and changes per suggestion are recorded per reader. Those counts feed the effort study of record 39.

## 3. Two learners, each for what it does well

Both are tuned for our work, and **both see the whole picture** (Nima, 2026-09-25: "having whole picture is much more useful and informative for both"):
- the header's numbers: TR, TE, TI, flip angle, geometry;
- the flags and image type;
- the scanner;
- the words: series and protocol names, comments;
- the rules' votes.

Neither is limited to one kind of evidence in advance. Which one to trust for what is measured, not assumed.

- **LightGBM** takes the words as features too: the pack's keywords and flags, and word pieces. We expect it to be strongest where physics and high-value flags decide, such as the edge cases between T1, T2 and T2*.
- **CLM** is a frozen Qwen3-8B encoder with small heads trained on our labels, and it runs on the production host's GPUs. It reads the numbers and flags written out as text beside the words. We expect it to be strongest where language decides, such as contrast and body part, with unseen languages, abbreviations and misspellings.
  - It is first adapted to neurology and MRI and CT terms on public text, where no patients are involved: the DICOM standard, vendor manuals, protocol printouts, public sidecars.
  - Its heads are then trained on our labels.
- **They are compared head to head, per axis.** Both are trained on the same development labels with the same splits, and scored on the same held-out stacks, including a left-out protocol. The scores are accuracy, calibration and how they do on new words.
  - If one is as good or better on every axis, the system keeps only that one.
  - If each wins somewhere, the difference is the reason for an ensemble.
- **The ensemble**, if it is kept, is a small stacker per axis. It learns from the development labels how much to trust each learner and the rules in each situation, for example by axis, by vendor, by whether the words are new, and by which family of rules fired. It is calibrated once at the end. The certificate covers the answer that ships, whether that is one learner or the ensemble.
- Both run outside the engine's deterministic path, as pipelines whose stored scores are pinned by digest (record 39). The fused pass in the engine reads those stored scores.

## 4. The order changes

- **Segmentation (wave 46) moves later.**
- **Before it comes the analysis pipeline with the assistant's help:**
  - the runner of wave 43, grown into choosing and running analyses on a selection;
  - the assistant proposing and explaining a run;
  - results registered as derivatives.

  It sits on top of everything that comes before it, so it goes first. Its record is written after a study.
- **Neuroverify** stays Adithya's app. Its move is a platform task beside NILS, not a NILS wave. It joins segmentation when segmentation's turn comes.

## Slices

| | Where | What |
|---|---|---|
| R1 | desk + engine | **The reader.** Suggestions filled in, evidence in one line per axis, prefetch, batches of like stacks, value ordering, timing per decision. It builds on record 45's workspace and the `classify.asked` shape |
| R2 | engine | **Labels back into training.** Unseal a certification draw once its certificate is recorded; a development-labels view the training tools read; the draw that asks where the model would learn most |
| L1 | Phase 0 study | **Both learners on the whole picture, compared, then one or an ensemble.** CLM adapted on public terms, then its heads on our labels on the production host; LightGBM on numbers, flags and words; a head-to-head per axis; keep one if it wins everywhere, else a stacker per axis; calibration last |
| P1 | study, then a record | **The analysis pipeline with the assistant's help**, before segmentation |

## Found when the reader was set up for Phase 0 (2026-09-25)

A proof on the development laptop, with synthetic data, showed three gaps before the real reading on the production host:
- **Pictures needed the ask's grant.** The image doors needed `query:see`, and with it a rater could read a sealed stack's classification through the ask. So pictures now open through the campaign: a rater may read the images of exactly the stacks in their own open campaigns, and nothing else.
- **Raters saw each other's answers.** A rater could see the outcomes of other open campaigns. Every campaign door a rater reaches is now scoped to their own campaigns.
- **One odd file broke a stack's pictures.** A file without the standard DICOM header failed the whole stack's picture build.

All three are fixed in one engine change before any reading starts.
- **Compressed DICOM had no pictures** (found on the production host the same day). The picture builder read uncompressed files only. Most of the Phase 0 archive is JPEG-family compressed, so 444 of the 500 reference stacks had no pictures. The builder is taught the compressed transfer syntaxes before the reading starts. Until items can be assigned inside one campaign, each rater gets a campaign of their own.

## How the reference is read (Nima, 2026-09-25)

Nima asked: "we can still make a mistake right? so how do we account that? like if there is no clue for a certain axes then we just need to assume." His answer to what follows: "yes."

1. **"Can't tell" is an answer.** Where the data give no clue for an axis, the rater answers can't tell and does not guess. That axis is left out of the certificate for that stack. The count of can't-tell answers is reported per axis, as a finding of its own.
2. **Known truth checks the readers.** A sealed stack from a protocol with a printout (site B1, study C, study D, study E, study F) has exact labels. The rater's answer there is scored against them, which measures the readers' own error.
3. **A random share is read twice** (Phase 2). Nima and Russell both read a random share of stacks. Their agreement (kappa) is the reference's own noise, and the certificate is stated beside it. Their disagreements are settled together. The share is random and is never chosen by disagreement with the model: re-examining only the model's disagreements bends the reference toward the model.
4. **An unsure mark per stack.** A rater who answered but wants a second look marks the stack unsure. Unsure stacks are read twice beside the random share and are reported apart from it.

## Strong on unseen data (Nima, 2026-09-25)

> "we need to make both models very strong and generalizable. they should enhance nils performance specially on unseen data"

The first head-to-head (`studies/2026-09-24-phase-0/learners.md`) had four problems:
- Its truth came from the protocols the pack was fixed on in wave 41, so the rules look near perfect there.
- The label model's posteriors are wrong on five axes, so learners trained on them copy the errors.
- Only 183 held-out stacks had unseen words.

So the learners are judged and trained this way from now on:
1. **Tests shaped like unseen data.**
   - Whole sites and scanner makes are held out.
   - Study B, which spans several sites and includes heavy anonymisation, is an outside test that never trains.
   - Every table reports the stacks whose words were never seen apart from the rest.
2. **Better teachers.**
   - The label model's value encoding on contrast, disposition, provenance, directory type and convertible is fixed.
   - Development labels from ordinary reads train at once.
   - The rules are one voter in the stacker.
3. **Training for the unseen.**
   - Perturbed copies of the text: misspellings, abbreviations, other languages' series names, reordered words.
   - Headers with tags wiped, as heavy anonymisation wipes them.
   - CLM adapted on public neurology, MRI and CT text first.
4. **An honest combination.** Per axis, a learner is used only where it beats the rules on held-out sites. Elsewhere the rules stand. The certificate covers what ships.

## After the first real read (Nima, 2026-09-25)

> "it is very difficult and almost impossible on many axes. like why we don't have series description, sequence name and protocol name? or any text blob? a lot of base can have overlap on physics you need all info to tell. also some axes are deterministic based on high value tags like image type or etc or should be infer based on result of other axes like directory_type, disposition, convertible. the page also is not easy to navigate. it is long, it need scroll and very uncomfortable."

Three corrections, made before the reading goes on (it is paused after one answer):
1. **Blind hides NILS's answers, never the file.** A sealed item shows everything a radiologist reads:
   - the header's text in full: series description, protocol name, sequence name and variant, scan options, image type, and the text fields the pack reads;
   - the physics;
   - the whole header, one key away.

   Patient name, patient id and birth date are never shown. Still hidden: the suggestion, the rules' values, System 1's scores and the matched words.
2. **A rater answers only what needs a person:** technique, base, modifier, construct, body part, contrast (post_contrast) and provenance.
   - Role, directory type, disposition and convertible are computed by the pack's own rules from the rater's answers.
   - Quality is read from ImageType.
   - All five are shown live beside the answer and stored with it as derived. They are the rater's answers carried through the pack, not NILS's opinion, so blindness holds.
   - The certificate's "any axis wrong" covers the seven answered axes and the derived five as computed.
3. **One screen:** pictures left, and on the right the header text, seven compact rows with a key per value, the derived line and the submit. Nothing needs scrolling on a laptop screen.
- **Open, found moving the campaigns to seven axes (alpha.43):** a lease that has run out still counts as held, so `campaign requestion` refused while one item's lease was 25 minutes past its end. The operator cannot release another person's claim either. The rater's own release door cleared it. An expired lease should not block, and an operator should be able to release one.

## After the second real read (Nima, 2026-09-25, alpha.43)

The whole header is "superb in comparison to previous one", and the compact layout works. Next level:
- **Search by any name:** a value is found by its vendor synonyms. The pack's keywords and a reviewed list of display terms per value, so "bravo" finds MPRAGE.
- **Axes constrain each other live:** implied values fill in (MPRAGE sets T1w and 3D), and excluded values close up. They come from the pack's own implications and exclusion groups. This is the rater's choice carried through the pack's grammar, not a system's opinion of the stack, so blindness holds.
- **A whole combination is looked up,** not assembled axis by axis: a search over legal full combinations, like the directory type.
- **Key facts stay visible without hovering:** slices, orientation, matrix, spacing, field strength.
- **The viewer mode persists** from stack to stack.

## Round 4 of the learners (Nima, 2026-09-25)

> "the study B cohort is the best cohort for this. it come from several different site and different language and vendors. Study A is across years. I think they can create a very useful inventory. also we can enhance it by adding noise, abbreviations, language related words like senza c or utan K ... for some axes and for some confident stack we can use their header info ... base, technique and modifier + contrast and also the provenance since it define how the rule see the stack in first place. like if we have SyMRI or SWI or STAGE then this can over rule other axes as for example we have special rules for them."

1. **Study B is split by site, and study A by year.** Most study B sites train. Three sites never train and remain the outside test, chosen to span a vendor, a language and heavy anonymisation. Study A trains on its earlier years and is tested on its later ones, which measures drift.
2. **A vocabulary inventory** of words, abbreviations and phrases from the training sites, counted by site, vendor and year. It feeds augmentation and CLM's tuning, and is shown to Nima as a list he can correct.
3. **Language phrases go into augmentation and into the pack's keywords:** senza c, utan K, mit KM, sans injection, +C, post Gd and their kin, for contrast and the other axes. The pack change is reviewed by Nima before it ships.
4. **Silver labels** for base, technique, modifier, contrast and provenance, from stacks where the header decides clearly: strong rule evidence agrees, the label model is at least 99 % sure, and nothing conflicts. They are used on training sites only, and never scored.
5. **Provenance first.** The models predict provenance, then every other axis given it, under the pack's constraints, as the rules' special rule sets (SyMRI, SWI, STAGE) do.

## After the learners report (Nima, 2026-09-26)

- **The outside test and the models move to the NILS server.** "lets move the model to [the NILS server]. it is much more strong server." Study B is split by site there.
- **Body part:**
  - The legacy storage server's v0 database holds body-part QC labels for study B and study H, which Nima calls very reliable, and they become body part's truth for those cohorts;
  - v0's image body-part model is the baseline;
  - an image study follows, adding open labelled datasets and a body-part campaign, aiming to outperform v0's model and architecture.
- **Pack:** "we absolutely need to update rules and our keyword wherever we need". The cross-verified implications, the hard excludes and hints, the synonym fixes, DCE as its own technique and ASL renamed are approved. TOF keeps base T1w: the physics sources agree, and angiography stays visible through technique and construct. The proposed keywords and the inventory's phrases go in after review.
- **The inventory may be committed** to the private design repo.
- **The reader:**
  - the page and the whole desk must use the full width of a large screen;
  - an oblique stack's sagittal and coronal views must not be tilted (see kineuro/dicom_nifti_viewer);
  - a compact view where every axis is a type-to-find box, beside the expanded one;
  - after a pick the box clears and focus moves on;
  - the loading line stays out of the rater's way.

## The allowlist screened, and one rater for now (Nima, 2026-09-26)

- **Allowlist:** Nima screened all 934 whole words of `lgbm-r4-2026-09-26` ("all word were ok. i checked them all. a lot of them were one two three digits numbers"). None is removed. Record 39 asks for a second screener, and Russell's pass follows when he is available. The allowlist-only model costs nothing measurable (0.942 against 0.943 in round 5), so the winners are retrained without hashed pieces, on the current pack.
- **Nima reads Russell's 250 too** while Russell is unavailable. The first 500 reference reads then come from one rater. The double reads of Phase 2 still need a second.
- **The first gold campaign is done:** 2,000 of 2,000 answered, 1,775 in batches and 225 alone (219 held back by design), a median of 2.7 s per single read, and 469 of v0's suggestions changed (23 %).

## Everything in one place (Nima, 2026-09-26)

> "we can make a rater account. i think we can have them all in one place and it is better to be on beefy server for training and next step too"

Phase 0 moves from the production host to the group's install on the NILS server: the sealed samples, the reading campaigns, the gold campaigns and the learners' training. Raters read the sealed samples under rater-only accounts, one each for Nima and Russell, separate from their own logins, which hold query rights. The move goes in two steps:
1. A read-only check and plan: coverage of every Phase 0 stack in the source pool, the seal carried over by series identity, and the answers given so far kept as label sets with their provenance.
2. The move itself, only once the plan shows that nothing sealed can be seen, lost or duplicated.

## The move's decisions (Nima, 2026-09-26: "ok on all Decisions")

Per the move plan in `studies/2026-09-24-phase-0/`:
- **D1:** the engine makes sealed mean sealed on every door (ask, explain, why, review, pictures) for every token but a named certificate grant, before the move.
- **D2:** the 7T cohort (only on the production host) moves into the source pool through Bifrost.
- **D3:** the 51 sealed stacks without files are dropped from the sample, with the reason written.
- **D4:** study B stays on production as a dataset, renamed from test data.
- **D5:** the 13 answers given on the production host are read again on production; the production host's are kept as a test-retest pair.
- **D6:** reading on the production host stops now.

The rater account exists: it is in a raters' group of the sign-in provider only, and the desk group `Raters` follows that group with `campaigns:see` and `campaigns:work` at detail quasi, nothing else.

## The reference is not assumed perfect (Nima, 2026-09-27)

> Nima: "i can indeed make mistake and nils be right ... how we account rater mistake? this assume rater is perfect gold standard." On the answer below: "this is very sound and defendable."

The certificate measures the reader's error and corrects for it:
1. **The reader's error, measured three ways:**
   - against the protocol truth on the sealed stacks that have it;
   - test and retest: the 13 answers read on the production host are read again on production, and a random 10 % of the rest come back later without the first answer;
   - a second reader (Russell) on a random share when he is available, for kappa. Until then this gap is stated.
2. **Disagreements settled blind:** every stack where the reader and NILS disagree comes back later as "answer A or answer B", without saying which is NILS's, with the protocol document where one exists. The settled answer is the reference.
3. **Agreements sampled too:** a random 10 % of the stacks where the reader and NILS agree get the same blind second look. That measures how often both are wrong, which re-checking only the disagreements would miss.
4. **The correction:** the certificate is stated beside the reference's measured error, with an imperfect-reference correction (a latent-class model of Dawid and Skene's kind, or a correction of the Staquet kind).
5. **Guesses stay out:** can't tell per axis, and unsure per stack.

The reader gains an "A or B" adjudication view for step 2 after the 500 reads.

## read-r1 is a pilot; a fresh certification read follows (Nima, 2026-09-27)

Per `studies/2026-09-27-read-r1-notes/README.md`:
- **read-r1 is paused at 107 answered (390 open)** and stands as the pilot of the reference read. Its 107 answers stay out of the certificate. The campaign stays open and is not closed; closing is Nima's call. The engine has no paused state (a campaign is open, closing or closed), so the pause is that nobody reads it; taking the rater account off with `nils campaign raters read-r1 --remove <principal>` would also stop claims, and keeps every answer.
- **The 5 dual-echo PD/T2 answers are corrected** under the ruling: technique TSE, base PDw below 40 ms and T2w at 40 ms or more, everything else as answered. The correction is an amend by the rater, which supersedes and keeps the earlier.
- **The conventions come first**, released as pack 0.9.0: MP2RAGE inversions (INV1, INV2) take base none, and a dual-echo FSE or TSE is TSE with the base by the echo. The reading guide (`studies/2026-09-27-read-r1-notes/reading-guide.md`) writes the rest down.
- **A fresh certification read is then drawn** from the sealed samples:
  - without localizers at their natural share: localizers are a pre-registered group of their own, capped, read last or apart;
  - sized for the certificate: at least 300 non-localizer stacks per stratum (299 with no error certify 1 % at 95 %), or 150 per stratum for 2 % if time is short;
  - with a second reader (Russell) on part of it, for kappa and the blind adjudication above.

## The reference, read by judges (Nima, 2026-09-27 and 28)

> Nima, 2026-09-27: "we can have 3 or 4 raters this way and more than 500 samples." On the design below: "Agree. This is good. We can have a very strong case for certification." And, 2026-09-28: "The rater can also reason (why), and on a mistake we can direct the problem if it exists."

This replaces the manual reference read of the section before. read-r1 stays a pilot and the fresh certification read is no longer read by hand, stack by stack.

### Why

The pilot (`studies/2026-09-27-header-judge/README.md`) ran two local judges and a Claude rater on the 107 stacks Nima answered in read-r1, and on 268 Prisma stacks with protocol truth:
- **A person cross-checking tags errs more than a judge reading the header.** Where they disagreed with Nima, reading the header cell by cell found the judges wrong on 0 to 9 cells and Nima on 16 to 18. His misses were header facts: +Gd in the words, ScanOptions FS, `me2d` read as ME-GRE, a MIP as RawRecon.
- **The header axes are deterministic from the header.** Provenance, technique, modifier, construct, base and post-contrast follow from the words, the tokens, the physics and the session's order. No one tells MERGE from GRE, or finds a modifier, by looking at the pixels.
- **Pixels decide body part.** 12 to 14 of each judge's disagreements were body part, mostly localizers and spine or neck coverage. Body part comes from the image model (the body-part study) and a person confirms it from the picture.
- **Most of what separated the parties was convention, not error**, and the conventions are now ruled (below).
- **No joint error was found** where a judge and the rules agreed, but about 60 agreeing cells per axis bound it only at about 5 %. The design below closes that to 2 %.

### The design

1. **Independent blind raters of different families:**
   - two local Qwen judges on the group's model server, `qwen38-27b` and `qwen3.8-flash-next`, temperature 0, structured output;
   - Claude sub-agents, each run fresh on the same packets and knowing only what the packet holds.

   Each rater gets only the de-identified header packet, the pack's vocabulary and constraints, and the reading guide. None sees the rules' answers, another rater's answers or a person's. The rules are a voter of their own, not a rater.
2. **Every rater gives its reason.** Per axis: the answer, a short reason that names the header facts it used as tags with their values (ScanOptions FS, TI 2500, SequenceName `*me2d1r3`), and a confidence (high, medium, low), or can't tell. An answer that breaks the pack's constraints is retried once with the violation named.
3. **The reference is the consensus.** Where the raters agree on an axis, that is the reference for it. Where the stack has protocol truth (a printout), the truth is the reference and the raters are scored against it, which measures their error.
4. **Nima adjudicates only the disagreements**, in the A/B view:
   - the candidates show as A and B in random order, never labelled by source, each with its rater's reason beside it, and "neither" and can't tell beside them;
   - the axes that agree are prefilled;
   - the header text, the physics and the pictures show as in the reader.
5. **A pre-registered 10 % audit of the agreements**, drawn with a seed written down before the draw. An audit item looks like any other item. The audit is sized so the joint error is bounded at 2 % per axis: about 150 agreeing audited cells per axis with no joint error (rule of three). Where 10 % of an axis's agreements falls short of 150, that axis is audited at a higher rate.
6. **Body part is confirmed from the picture** on every stack, with the image model's answer and the raters' as candidates.
7. **Localizers take the one-key answer** (reading guide ruling 1): provenance Localizer, body part to confirm, the rest not asked. They are a group of their own, capped, and read apart.

### Every settled disagreement is traced to its cause

Each disagreement Nima settles is triaged from the raters' reasons into one cause, logged with the stack and the axis, and routed:

| cause | what it means | where it goes |
|---|---|---|
| rule bug | the rules read the header wrong | a pack fix |
| convention gap | the guide does not say which answer is right | Nima rules; the guide and the prompt are updated |
| header ambiguity or damage | the header cannot settle it (wiped, contradictory, mislabelled) | can't tell is the right answer; the stack stays out of that axis |
| rater error | a judge misread a header fact its reason names | a prompt fix |
| reader slip | the adjudicating person's own mistake, found on the audit or a second look | corrected; counted as the reference's error |

So the rules, the pack and the prompt get better from each round, and a fix is re-measured on the next fresh draw, never on the stacks that found it. The certificate reports the count per cause, per axis.

### The population

- **The sealed sets:** cert-p0-clean (800) and cert-p0-kept-uncertain (792). Every rater reads all of them, localizers apart.
- **The void draw, cert-p0-uncertain (378), stays apart** and is not in the certificate.
- **read-r1 goes into this.** Its 390 unread stacks are read by the judges with the rest, not by hand. Nima's 107 pilot answers join as a human rater on those stacks, which measures the judges against a person; they are re-scored under today's conventions (N′).
- The seal holds throughout: the raters' answers and the adjudication are labels on a sealed sample, and unsealing still needs a recorded certificate (section 1).

### Privacy

- **The packet is the reader's trimmed header.** Patient name, patient id, birth date and address are stripped before a packet is written, and a check asserts that none of them and nothing shaped like a personnummer is left in any packet. 8-digit dates in names are masked.
- **The product uses local models only.** A judge that ships in NILS runs on the group's own model server.
- **Claude is a rater only for the reference**, under Nima's ruling that DICOM tags other than the direct identifiers may be seen in the session (2026-09-25). The packets stay on the group's machines; only counts leave.

### The order

1. The rulings below, done.
2. Pack 0.9.0 with them, released and reclassified.
3. The prompt updated with the rulings, the GE ScanOptions tokens, MEDIC and MDME, and the per-axis reason and confidence (done, in `studies/2026-09-27-header-judge/tools/`).
4. Re-measured on a fresh unsealed draw: the load, the agreement and the joint error, before any sealed stack is read.
5. The full run of every rater on the sealed sets.
6. The A/B view built and the adjudication, with the audit and the cause triage.
7. The certificate, stated beside the reference's measured error and the counts per cause.

### The conventions, as ruled

**2026-09-28**, on the pilot's recommendations:
1. **Provenance of SWI, BOLD, ASL and MAGiC.** The acquired images are RawRecon: SWI magnitude and phase, the BOLD and ASL time series, MAGiC's MDME images. What the scanner computes takes the pack's derived value: the combined SWI image SWIRecon, CBF maps ASLRecon, fMRI maps BOLDRecon, MAGiC's synthetic contrasts and maps SyMRI.
2. **SWI base.** The combined SWI image is SWI, the magnitude T2\*w, the phase none.
3. **SWI minIP.** ProjectionDerived with construct MinIP, base SWI.

**2026-09-27**, the reading guide's seven:
1. Localizers take a one-key answer.
2. MP2RAGE's inversions INV1 and INV2 take base none; UNI and UNI-DEN are T1w (option B, which Nima found more accurate than PDw).
3. Post-contrast not_given from the words or from the session's order.
4. A localizer's technique by family; a diagnostic gradient echo told apart or can't tell.
5. ND and FlowComp from the header only: ND from ImageType, FlowComp from ScanOptions.
6. brain-neck only when the stack clearly reaches below C2.
7. An FA or other tensor map is DTIRecon; an ADC or trace from a plain DWI is RawRecon with construct ADC or Trace.

**Dual-echo PD/T2 FSE or TSE** is technique TSE, with base PDw below TE 40 ms and T2w at 40 ms or more.

The reading guide (`studies/2026-09-27-read-r1-notes/reading-guide.md`) and the prompt carry all of them. Pack 0.9.0 carries them for the rules.

## Agents read and settle the reference (Nima, 2026-09-28)

> Nima: "We can trust agents, as you saw that a human is no better. Consider them pseudo-humans and treat them as such, since the criterion is look and answer. We can revise mistakes, and since we have the why we can catch any wrong or misbehaviour. No need to involve me." And: "we can make the local models better by providing a better prompt, or use more Claude instead of them."

This amends "The reference, read by judges" above: Nima no longer adjudicates, and the rules no longer vote.

1. **The rules are what is certified, never a voter in the reference.** The reference is built from the raters alone. Where the raters agree and the rules differ, the raters' consensus is the reference and the rules are scored wrong on it; the cell is in the audit's frame like any other agreement.
2. **The voters are independent rater agents of at least two model families.** Claude sub-agents, each run fresh on the same de-identified packet, and the local Qwen judges if their improved prompt brings them close enough; otherwise two or three independently prompted Claude raters plus the best local judge. Their consensus is the reference.
3. **The panel is chosen by measurement, not assumed.** The next fresh draw (`studies/2026-09-28-judge-remeasure-2/`) runs two panels on the same packets and compares their load, each voter's error rate and the pairwise correlation of their errors:
   - **A:** a Claude rater with the two local judges on prompt v2;
   - **B:** two or three Claude raters under different framings (axis by axis from the tags; from the vendor's sequence family; adversarial, asking what would make the obvious answer wrong), plus at most one local judge that clears a pre-registered accuracy bar.
   
   The panel that reaches the lower joint-error estimate at a load we can carry reads the sealed sets.
4. **Disagreements are settled by an adjudicator agent.** It sees the stack's header packet and the candidates as A, B, ... with the raters' reasons, never their source, and answers per axis with its own reason and confidence, or "neither" with a value, or can't tell.
5. **A second, independently prompted adjudicator** reads a seeded 15 % audit of the agreements, raised from 10 % so that each axis reaches the 150 cells the 2 % joint-error bound needs, and a seeded sample of the first adjudicator's cells. Its answers on the agreements settle the audit. On the sample, the agreement between the two adjudicators is reported, and every cell where they differ is listed for review. An audit item looks like any other item.
6. **Every output is kept with its reason.** Every rater's and adjudicator's answer, reason and confidence stays beside the reference, so any settled cell can be traced back and revised. A revision is a second look and is counted as the reference's error.
7. **Misbehaviour is checked, not trusted away.** The report flags a reason that cites a header fact the packet does not hold, an answer that contradicts its own reason, and a can't tell given with high confidence. The flags are counted per agent and each one is read.
8. **Independence has limits, and they are stated.** Agents of one model family share blind spots, so their agreement is weaker evidence than that of independent people. The adversarial framing, the second family, the audit and the misbehaviour checks are there for that reason. The joint-error bound is what carries the claim, not the number of voters.
9. **Disclosure for the paper:** the reference was read and settled by AI raters and AI adjudicators on the de-identified header, the body part confirmed from the picture, and the rules were the system under test. Every reason is kept for review.

**Everything is recorded, so nothing has to be reproduced** (Nima: "record everything so we do not need to reproduce it again; also good for the final report and certificate"). Every run keeps, in the evidence archive on the group's machines (on disk, private to the research account) and in its mirror: the draw (seed, query, stack ids), the packets as given, the prompts byte for byte with their sha256, each model's identity (model id, weights or image hash, server, temperature and settings, date), every rater's and adjudicator's raw output per stack, the audit draw, the scoring, and a `MANIFEST.json` with the sha256 of every file. Git holds the prompts, schemas, code, the manifests and aggregate results, never header text or series names (`studies/phase0-evidence/README.md`). The certificate cites runs by id and manifest hash. The first re-measure is run `remeasure-1`; the pilot's raw answers were lost with a scratch folder held in memory at a 2026-09-28 reboot, and only its counts remain.

**The certificate's bars, as ruled** (the ten questions in `studies/2026-09-28-certificate/README.md`, all as recommended):
- the corrected certificate is stated beside it, not a pass condition;
- the gate applies within the kept-uncertain stratum;
- each make with at least 300 stacks passes on its own, the others are reported;
- an empty answer is wrong, with the lenient line as a sensitivity;
- localizers are apart, reported, with no bar;
- the sealed samples suffice, and per-site lines are reported;
- the reference's own error has no bar and is stated beside;
- truth cells go through adjudication blind, without the document;
- where the rules are empty and every rater agrees, the cell is consensus and audited;
- an audit that can't tell is outside the bound, with a line counting it as a joint error.

## Body part leaves the header certificate (Nima, 2026-09-28)

This amends design 6 of "The reference, read by judges": Phase 0's certificate from the sealed read `sealed-1` covers the header axes only. Those are provenance, technique, modifier, construct, base and post-contrast, plus the derived five as registered.
- **Why:** a header-read reference for body part is not valid. Nima's gold rounds found that the header and the rules match the picture only about half the time: 80 % of round 2's suggestions were changed, and the rules were right on 49 %.
- **Where body part goes instead:** it is certified separately on record 50's track, from the pictures.
- **In sealed-1:** the body-part picture step is dropped, and the raters' header answers for body part are kept but are not a reference. The deviation is noted in `studies/2026-09-28-sealed-reference/README.md` and `studies/2026-09-28-certificate/README.md`.
- **Post-contrast may follow later** (Nima, 2026-09-28). It stays in this certificate for now, but it may move to its own image-based track as body part did. Contrast is sometimes visible only in the pictures, and the header's contrast tags and words are unreliable: an exam-level "MS+GD" can name contrast for series acquired before it.
  - **In sealed-1** (certificate non-localizers):
    - 375 of 1,105 post-contrast cells have no header answer (can't tell);
    - of the 730 settled values, 356 rest on the ContrastBolusAgent tag, 321 on the session order and 53 on words alone;
    - the rules are empty on 397 of the 730.
  - The study README has the table.

## Iterating to the certificate (Nima, 2026-09-28)

> Nima: "a rule + System 1 improvement on iteration so that it meets or even surpasses our certificate".

The first grade of System 1 on the sealed reference (`studies/2026-09-28-sealed-reference/system1.md`) fails the gate. Round 5's LightGBM, taught by pack 0.5.0 on six study B sites, is below the rules on five of six header axes (technique 65.7 % against 84.6 %). At 1 % it decides nothing in kept-uncertain. So I improve the rules and System 1 together, in rounds, until the combination meets the certificate:
- **Each round** fixes the pack from its error lists and retrains System 1 with that pack as teacher. The first is pack 0.11, after tonight's reclassify.
- **Development rounds are graded on a development reference.** It is read the same way as sealed-1 (AI raters, adjudicators, audit) on fresh unsealed stacks. remeasure-1 and remeasure-2 give about 550, and I read more as needed. The per-rule error lists that drive the fixes come from it, never from the sealed set.
- **sealed-1 is graded only at checkpoints.** The pack 0.11 baseline is one, since 0.11's fixes came from the re-measures. System 1 retrained on 0.11 is the next. A checkpoint reports numbers and never feeds a fix back.
- **The final certificate is graded once**, on a fresh sealed sample read the same way.

## Post-contrast leaves the header certificate (Nima, 2026-09-29)

> Nima: "i think post-contrast leave the header certificate but we should still use information but not trust it. you know? like bodypart. we get that info but will not trust it. anyway if we need itterate more we do if there is a gound for improving the rule or the system 1 but if not the imposed error by contrast and body part is not fair as they are mistaken in the header in a first place"

This follows body part (2026-09-28).
- **The certificate's axes:** Phase 0's header certificate covers provenance, technique, modifier, construct and base, plus the derived axes registered for them. Post-contrast is certified on its own image track (`studies/2026-09-28-post-contrast-image/`), as body part is on record 50's.
- **The header is still used, not trusted.** The contrast tag, the words and the session order stay as evidence: rule answers, System 1 features, and the fusion in the image track's head. A header-read post-contrast value is no reference.
- **Why:** on dev-1 (learners round 6), 340 of pack 0.12's 562 bad cells are post-contrast left empty, and System 1 fills them at 89.8 %. The answer is often not in the header at all, so charging those cells to the rules and System 1 measures the header's failure, not theirs.
- **Iteration continues only where there is ground.** Rules and System 1 get another round where the error lists show a fixable cause on the five axes, not to chase errors the header cannot resolve.
- **In sealed-1 and dev-1:** the raters' post-contrast answers are kept as data, but they are no reference for this certificate. The certificate's config and README record the change before the next checkpoint.

## Certify what is sure, review the rest (Nima, 2026-09-29)

> Nima: "we should aim to make this as good as it gets but at the end of the day manual review on "not sure" stuff can be practical way to address those. but the less they are the better. if there is an obvious rule bug we should catch it."

After the five-axis re-score (learners-r6-results.md, "After the post-contrast ruling"): on dev-1, pack 0.12 leaves 166 of 900 stacks wrong or empty, System 1 adds little on the five axes, and pack 0.13 is estimated at 102.
- **Rules first.** Every obvious rule bug that the development references show is fixed in the pack, round after round. Disputes with the reference go to Nima.
- **Then abstention.** The certificate certifies the stacks rules plus System 1 decide at 1 %, and the rest go to manual review as "not sure", as body part does. The fewer, the better. Coverage at 1 % is reported beside every checkpoint and is what the rounds improve.

## Pack 0.13's disputes and the "decided none" scoring (Nima, 2026-09-29: "Yes on both")

These are rulings on the 72 dev-1 disputes and the 42 scoring cells listed in `studies/2026-09-28-dev-reference/pack-0130-errors.md`, all as recommended.

**Scoring:** a base the rules decide as nothing, on a map or phase image, is scored as the answer `none`, not as empty.

**Changes to pack 0.13:**
- **Group 2:** a single-echo SWI is SP-GRE when the header's sequence variant says SP, and GRE otherwise.
- **Group 6:** a GE calibration scan (ASSET or 3D calibration) is not a localizer. It is an acquired image (RawRecon), a fast spoiled GRE.
- **Group 9:** one value for the Philips isotropic image, Trace.

**Where the reference is corrected instead**, by a correction record beside dev-1 (the evidence run is not rewritten):
- **Groups 1, 3, 5 and 8:** the earlier rulings stand. These are the SWI magnitude, the GE DTI maps from a plain DWI, MDME named SyMRI, and BOLD MoCo written ORIGINAL.
- **Group 4:** SyMRI screenshots stay excluded.
- **Groups 7, 10, 11 and 12:** the pack's reading stands. These are Philips mFFE as combined multi-echo, long-echo MDME with base none, the header's pulse name deciding FRFSE, and the seven one-offs.

**dev-2:** a fresh development reference, read the way dev-1 was, grades pack 0.13 before the next sealed checkpoint.

## Rulings of 2026-09-29, evening (Nima: "ok on all")

- **Body part in production:** `bodypart-fusion@r7` (fine) and `bodypart-fusion-coarse@r7` (coarse) are promoted, and `bodypart-infer-fusion` serves every unsealed MR stack. Sealed stacks are left out so the sealed samples stay untouched.
- **Post-contrast pair read r1** is open with its 300 pairs. The 100 singles wait for a desk switch that hides the header text in single mode.
- **Pack 0.14:** fix every rule bug dev-2 shows, then grade on a fresh dev-3 honestly. Fixes found on dev-2 are never graded on dev-2.
- **The sealed checkpoint** is run after pack 0.14.
- **System 1's training guard** excludes by session, not by subject. A subject with a sealed or development stack keeps its other sessions for training, so the small cohorts keep most of their stacks. The certificate's own disjointness (G3) still applies to the sealed samples.
- **FreeSurfer licence:** the research account may read it, for the post-contrast region candidate FS-SC.

## Post-contrast is read against anchors (Nima, 2026-09-29, night)

> Nima: "if both are post or both are pre it is as if we have one image and there is no difference so it is virtually impossible to know if the brightness is simply the way image is or it is CE. I rather to have a sample of true contrast and true pre from same session or closest stack with same setting so i can tell. otherwise i don't think anyone can tell even the most trained neurologist."

- **Round 1 of the pair read is closed** (campaign `postcontrast-pair-r1`, 30 items answered). Its answers are exploratory and never gold.
- **Anchored reading.** A scan is judged beside a known-pre and a known-post anchor: from the same session, else the closest scan of the same subject with the same settings. All panels share one window normalised to a reference tissue, with a difference view and jumps to the enhancing structures. The answer is like pre / like post / can't tell.
- **Anchors come from objective evidence only:** a DSC bolus with a trustworthy time, a contrast tag at sites audited as filling it per series, or an unmistakable registered post/pre regional ratio that Nima confirms.
- **A scan with no usable anchor is not read by a person.** It is left to the model and its abstention and reported as such. The post-contrast certificate covers what anchors or the model decide at the bar, and the rest is openly "not sure".

## Rulings on pack 0.14's disputes (Nima, 2026-09-30: "Agreed on all")

This rules on dev-2's disputes-0140.md (N1 to N7, R1, R2) and dev-3's N8 to N11, all as recommended except N6 and N11, which wait for a sourced look-up (web research) before they are fixed.
- **Pack 0.15 changes:**
  - N2: a GE spin echo not split by echo, with a stated train of 2 or more, is TSE.
  - N3: a new technique value DWI-TSE (diffusion with a turbo spin echo readout, base DWI, family SE).
  - N4: a Philips diffusion image with a positive b value written PROJECTION IMAGE is Trace.
  - R1: a GE research-mode SWI with a train of 32 or more is EPI.
- **The reference is corrected** (a supplement correction record):
  - N1: EchoCombined stays on MEAN SWI outputs.
  - N5: a synthetic PD map has no base.
  - N7: no Magnitude on a plain acquired image.
  - N9: the ASL perfusion-weighted image is ASLRecon and DeltaM.
  - N10: a GE single b=1000 volume has no construct.
  - R2: no EchoCombined on a combined multi-echo FFE.
  - R1: the five settled cells become EPI.
- **Scoring (N8):** a stack settled as Localizer is scored on technique by family.
- **Pending the look-up:** N6 (bSSFP base, recommended T2w) and N11 (GE "MC" in diffusion output names, recommended no modifier).
- **Next:** pack 0.15, then a fresh dev-4 for the honest grade.

## N6 and N11 after the look-up (Nima, 2026-09-30: "Yes. Agreed.")

- **N6, the base of balanced steady-state scans** (FIESTA, FIESTA-C, TrueFISP, CISS, balanced FFE): T2w, as a convention. Their steady-state contrast is T2/T1 (Scheffler and Lehnhardt 2003, Eur Radiol 13:2409: "Balanced-SSFP is certainly not a T2-weighted sequence"; Bieri and Scheffler 2013, JMRI 38:2). BIDS and DICOM have no T2/T1 term, and clinical use calls these scans (heavily) T2-weighted. The technique axis carries the mechanism, and the rule's provenance says T2w stands for T2/T1. Never T1w. Two exceptions follow their own rules: a 2D balanced scan at a very low flip angle (proton-density-like, Huang et al. 2002, MRM 48:684) and a magnetisation-prepared one, whose preparation decides.
- **N11, "MC" in GE diffusion map names:** no modifier. No public GE source defines the token; GE's workstation DTI processing corrects motion and eddy currents before the maps are computed. On the archive, 556 GE maps carry "-MC-", none from a source series named MC, and nearly every GE map has it (isoADC 271 of 272, ExpAtt 270 of 271, FA 268 of 269). A map computed from corrected inputs is no motion-corrected copy of an acquisition (the MoCo ruling), and a token on almost every map distinguishes nothing. The reference is corrected to none.

## Rulings after dev-4 (Nima, 2026-09-30: "yes on all")

- **N12:** a Siemens BLADE with restore takes the pack's value, VFA-TSE.
- **Future development draws exclude past reads by session, not by subject,** as System 1's guard does. Sealed samples keep their full disjointness (G3).
- **C1 and C5 are confirmed** as corrections for the primary line:
  - C1: an acquired BOLD or ASL time series is RawRecon;
  - C5: FlowComp is taken from ScanOptions only.
- **Post-contrast:**
  - the 28-pair anchor check opens after production takes alpha.62;
  - contrast-tag anchors stay on the strict rule (4 scanners);
  - FLAIR, and T1-type stacks without anchors, are certified only where the model is sure, and are "not sure" otherwise.
- **Next:** production takes engine alpha.62 and desk alpha.59 (Nima), then reclassify on pack 0.15, System 1 round 7 and the audited sealed checkpoint; pack 0.16 from dev-4's 7 rule bugs and N12.

## Post-contrast after anchored round 2 (Nima, 2026-09-30)

- **The test's source:** "you can use anything and any cohort you need". The test frame may take any cohort and any subject outside the sealed samples, including the subjects of the development header reads (their post-contrast answers were header-read and are no reference here). Whole scanners are held out now by a registered seed; a new cohort with pre and post sessions is added as the final test when one comes. Sealed samples stay out (G3).
- **The 1 % bar:** "agreed". The model is held to at most 1 % error (one-sided 95 % upper bound) on all stacks it answers, per family where a test read exists, with its coverage reported beside it. The stratum where the rules are empty is reported apart with its own bound, and is not a pass condition until a test frame is large enough to show it.
- **Next:** the post-contrast model as proposed in the study's report, section 12: registered before any training, frozen with its sha256 committed before any test is drawn, then one anchored test read by Nima.
- **A long shot, not a goal** (Nima, 2026-09-30): once there are enough labelled stacks, try post-contrast as its own task from a single stack and the image alone, for T1 and for T2. No published work treats it that way on brain MRI (it is one class inside sequence classifiers, and T1 against post-contrast T1 is their hardest pair). It is reported only if it works and its performance can be stated honestly, for T1 and T2 apart. The first model registers the image-only single-stack reading as secondary and descriptive, and counts T2 stacks and anchors.
- **Nima's observation for the single-stack task** (2026-09-30): reading round 2 against anchors was easy, and he noticed that the veins and a few special structures could decide on their own, without a reference scan. The image-only arm therefore looks at those structures directly: the dural venous sinuses, cortical and deep veins, choroid plexus, pituitary and stalk, cavernous sinus and nasal mucosa, located by a template or venous atlas; contrasts within the stack against a reference tissue (sinus against white matter, choroid plexus against ventricular CSF, pituitary against pons); vessel conspicuity; and optionally a small network trained only on crops of those regions, so that it has to learn from them. It trains on the anchors, is picked on S and is reported as secondary.
- **Every mistake gets a reason and a mitigation** (Nima, 2026-09-30: "since we know all axes we can tell for mistakes what could be the reason and how we mitigate it"). Each wrong or abstained stack is joined with all its known axes (technique, base, 2D/3D, orientation, coverage, field strength, make and scanner, cohort, body part, anchor kind and distance, header evidence). The mistakes are grouped by those axes, and each group gets a likely cause with its evidence and a proposed mitigation. Rates are reported per stratum with intervals. Mitigations found on S may be applied before the freeze; what the test shows is only a proposal for the next round.

## Rulings after the re-adjudication of dev-2 to dev-4 (Nima, 2026-09-30: "agreed on your recommendations")

The re-adjudication with the raters' reasons visible (`studies/2026-09-30-readjudication`) raised four questions:
- **The base of a SWI EPI reformat with no SWI, M or P token:** can't tell. The header does not hold it.
- **The provenance of a MEAN SWI output:** SWIRecon, as the other 8 such outputs are settled.
- **A GE gradient echo with SequenceVariant SS\SK and FAST_GEMS, not spoiled:** the reference's SS-GRE stands. The packs' spoiled reading is a rule bug of the same kind as dev-3's GE steady-state echoes, fixed in the next pack.
- **A GE PROPELLER with FR_GEMS:** the pack's TSE stands, by D11: VFA-TSE (labelled RESTORE, the 90° flip-back or driven-equilibrium TSE) is decided from the pulse name FRFSE, not from the FR option alone. (Corrected 2026-09-30: this line first said "fast recovery does not mean variable flip angles", which misread the id VFA-TSE; the ruling itself is unchanged.)

## After the sealed checkpoint (Nima, 2026-09-30: "good on all 4")

The checkpoint graded pack 0.15 at 121 of 1,104 sealed-1 non-localizers wrong or empty (11.0 %), against dev-4's 9 of 900. The development draws had narrowed to what was left unread (dev-4: 77 % clean Siemens, 4 Philips stacks), so their grade described that slice, not the archive. So:
1. **Pack 0.16 is graded on sealed-1 once, now,** by replaying its rules over sealed-1's header packets offline, registered before it is scored. It is fair because 0.16 was finished before any sealed-1 answer was read. After this, no pack built from sealed-1's errors is graded on sealed-1.
2. **dev-5 is drawn to look like the archive,** stratified by make, stratum and cohort as the sealed samples were, with past reads excluded by session. It grades the packs after 0.15.
3. **The real rule bugs the checkpoint shows** are fixed in the next pack and graded on dev-5, never on sealed-1.
4. **Pack 0.16 is not released** until step 1 shows it no worse than 0.15 on sealed-1.

## After dev-5's reading (Nima, 2026-09-30: "ok on llm-2. ok on the rest too")

- **The dissent voter goes back to llm-2** (the local 27B) for future reads. On dev-5, llm-3's objections held on 5 of 233 routed cells (2.1 %), against llm-2's 12 of 170 on dev-4, as the dissent-voter study predicted.
- **dev-5 is graded once, after pack 0.18 is frozen:** packs 0.15 (stored), 0.17 and 0.18 together, with both registered lines (unweighted and design-weighted). Pack 0.18 takes its fixes only from sealed-1's error analysis, the deep research on vendor conventions, the empirical counts and Nima's rulings, never from dev-5.
- **The next fresh read uses the power-0.75 allocation within each make**, between sealed-1's square-root allocation and a proportional one, and the final certificate follows the deep research's two-phase design (a proportional main sample and a rare-cell supplement reported apart), to be registered with Nima before its draw.
- **Conventions follow the deep research** (Nima, 2026-09-30: "decision table should follow what we learned from research. why do you need me?"). Where the research is confident, its recommendation is the ruling. Where it says unknown or low confidence, the empirical counts from our own data decide. Nima is not asked case by case; he gets the table of what was decided and why. Only a new vocabulary value, a change to the certificate's design, or a case where the research and our data disagree comes to him first.
- **GE Cube is 3D-TSE** (Nima, 2026-09-30: "Cube is the 3D-TSE, literally Siemens SPACE"), as are SPACE and VISTA. VFA-TSE is the RESTORE flip-back TSE only. The deep research's "Cube → VFA-TSE" read our id as variable flip angle and is not applied.
- **The certificate's frame covers study B** (Nima, 2026-09-30: agreed). Its 41,997 production stacks sit in no stratum today; they are sorted into clean and kept-uncertain by the same rule as the rest before the final certificate's draw.
- **Missing vocabulary values are added, not forced** (Nima, 2026-09-30: "if we need to add something just do it. we cannot force things to category they don't belong to"). The values the research and the counts show are missing (CE-MRA, MRS, silent zero-TE MRA, NeuroMix, PBP, K2, Velocity, a standard-deviation projection) go into pack 0.18 before its freeze, each defined, with a correction record by rule for references settled before they existed. From now on a missing value is added when the evidence shows it, without asking first.

## The final certificate's design (Nima, 2026-09-30: "agreed on certificate")

As recommended in `studies/2026-09-30-final-certificate/plan-draft.md`:
1. The frame excludes only sessions already header-read (and every sealed stack).
2. The old void is taken by today's disposition. Its reformats and scouts form their own kept-uncertain sub-stratum.
3. The clean stratum is held to 1 %, and each make with 299 or more reads also to 3 %.
4. The main sample is 2,500 proportional, self-weighting clean reads, beside a rare-cell supplement reported apart and kept-uncertain with its own bound.
5. It waits for pack 0.19 and a fresh development read showing the clean rate near 0.5 %.
6. It grades System 1 round 9, trained after the draw. Round 8 is development.
7. 100 localizers are read and reported apart.
8. The claim covers the frame only.

The estimand is Hajek; the primary bound is Korn-Graubard with the variance-based effective n; the per-cell Jeffreys aggregate is a sensitivity.

## The nine dev-5 disputes, by the deep research (2026-09-30, conventions follow the research; `studies/2026-09-30-dev5-reference/deep-research-disputes.md`)

1. **A Philips FFE without SP:** SS-GRE when SequenceVariant has SS or the scan technique is FFE; GRE with only OSP or NONE; SP-GRE only with SP, T1FFE or Spoiling (0018,9016) RF or GRADIENT. The order is Spoiling, then SequenceVariant, then Philips (2001,1020), then the name. "T1W" in a name never makes a spoiled GRE.
2. **A Philips 4-echo SWIp:** comb-ME-GRE; the SWIp image is SWIRecon, its magnitude and phase RawRecon.
3. **A Siemens IR with ImageType R:** modifier PSIR, construct Real. The magnitude image keeps IR or FLAIR by TI.
4. **The ADC or eADC of a named DTI** (six or more directions, mddw, DTI, or an FA sibling): DTIRecon. Its trace-weighted image stays RawRecon with Trace.
5. **A Siemens MEAN phase of a multi-echo SWI:** RawRecon, Phase, comb-ME-GRE. SWIRecon's MEAN clause covers only the MEAN of SWI-processed images.
6. **A few-slice planning series:** Localizer when at most 5 slices per plane AND a planning word or LOCALIZER/SCOUT. Few slices without a planning word is can't tell.
7. **A reversed-phase b=0 EPI of a few volumes beside opposite-direction DWI:** FieldmapRef.
8. **GE "FL:":** unknown; our counts decide. Provisionally Filtered only when an unfiltered same-acquisition sibling exists, else none.
9. **A DERIVED motion-corrected DSC series:** RawRecon, MoCo, Perfusion-EPI. PerfusionRecon's "derived MoCo DSC" clause is withdrawn; only the maps are PerfusionRecon.

Cases 2, 3, 5 and 6 are confirmed against our counts before the pack relies on their header tokens.

### The nine, after our counts (`empirical-disputes.md`, 2872773)

The counts confirm cases 1, 2, 5, 6, 7 and 9 as ruled (for 7, the Siemens CSA sign decides where it exists, and the name only where it does not; 18 runs whose name says reversed but whose sign matches the partner are not FieldmapRef). They refine or decide three:
- **Case 3:** every Siemens ORIGINAL IR whose ImageType is R is PSIR. The 19 PSIR-named stacks and the 554 unnamed ones have the same fields, and only the name differed.
- **Case 4:** for Siemens and Philips, a multi-direction DTI's ADC is DTIRecon by the acquisition (six or more directions, a DTI or mddw sequence, or an FA sibling). Their headers write no separate MD. GE writes its tensor mean diffusivity as a separate AvDC output, so GE's ADC keeps the earlier ruling (RawRecon), and its AvDC is DTIRecon with MD.
- **Case 8, decided by our data:** GE "FL:" images are DERIVED copies (127 of 144) of a series whose original, where exported, is ORIGINAL. An "FL:" image written DERIVED is Filtered; one written ORIGINAL has no construct.

Found beside the nine: pack 0.18 gives comb-ME-GRE to single-echo images of Philips multi-echo FFE (each echo its own stack, with its own TE, about 1,000 stacks). These are ME-GRE; the combined-echo ruling applies only to the combined image. It is fixed in the next pack.
- **Vendor sequence names follow one detail rule** (Nima, 2026-10-01: "Agree"): GE's pulse sequence name, and any vendor sequence name among the allowlisted private elements, is hidden below detail "quasi" as Siemens' sequence name already is.
- **Sequence names are shown everywhere, at every detail level** (Nima, 2026-10-01: "we need to show all every time everywhere. this is not acceptable to be hidden. custom names would not include PHI in the seq name."). This replaces the ruling of the same morning: no vendor sequence name is held back by detail level, Siemens' SequenceName and PulseSequenceName, GE's pulse sequence names and Philips' scanning technique included, in the reader, the header door, packets, campaigns and the ask doors. They stay outside the identification blocks and keep their technical kind.
- **Philips MobiView provenance** (by the deep research's case 28, already applied for construct): a stitched MobiView composite is RawRecon with construct Composed, never ProjectionDerived; it is a Localizer only when a survey word names it. dev-6's 25 inconsistently settled MobiView cells are corrected by this rule in a correction record made before any rule answer is read, and the provenance audit is scored against it.

## Conventions, round 3 (2026-10-01; `studies/2026-10-01-conventions-r3/deep-research-r3.md` with `empirical.md`)

Applied by the standing rule (research where confident, our counts where it is not):
- **Localizers:** a vendor flag outranks the slice-count rule: GE SeriesPlane (0019,1017) = 256, ImageType LOCALIZER/SCOUT/POSDISP, and Siemens AutoAlign scouts (`AA…Scout`, with their MPRs as Localizer + MPR) are Localizers whatever their slice count. The ≤ 5-slices-plus-a-planning-word rule applies only when no vendor flag exists. (Our counts: pack 0.20 already stores these 24,000 stacks as Localizer.)
- **Philips MAXIMUM:** construct none unless geometry shows a projection (≤ 3 slices or a thick slab); our counts show all 157 are whole 3D volumes, so none.
- **Siemens 3D-card outputs:** each image by its own ImageType (CSA MPR → MPR, CSA MIP → MIP), whatever the series name.
- **Siemens STDDEV_SAG/COR/TRA:** ProjectionDerived with StdDevProj; STDDEV_TIME is a temporal map (new construct StdDevTime).
- **Phase contrast:** Philips VELOCITY MAP\P\PCA is Velocity, M_PCA Magnitude, the PCA angiogram a new construct Speed, PROJECTION a MIP; GE 2D COLLAPSE is RawRecon with no construct, a 3D COLLAPSE a MIP; base none throughout.
- **Calibration scans:** base none, provenance a new value Calibration (coil-sensitivity, receive-field and prescan references not meant for reading).
- **Gradient echo:** Siemens `fl` stems are SP-GRE whatever SequenceVariant says; GE `fgre` without SP is SS-GRE, `fspgr` FSP-GRE, `efgre3d` by SP else GRE; EDR_GEMS and FAST_GEMS say nothing about spoiling.
- **Dixon:** technique from Philips (2001,1020) (TSE or FFE), never from ScanningSequence GR on an mDixon TSE; the Dixon part from Philips' per-frame W/F/IP/OP tokens (2005,140F); Siemens DIXON + IN_PHASE/OPP_PHASE/WATER/FAT; modifier Dixon.
- **Philips echoes:** echo count from (2001,1014); the ImageType token T1/T2 on FFE says nothing about base; FEEPI is GRE-EPI.
- **Siemens multi-echo SWI magnitude** (decided by our counts): each echo is its own series, so ME-GRE, not comb-ME-GRE.
- **Philips 3D T1 TFE with InversionRecovery NO:** FSP-GRE, not MPRAGE; YES or TI > 0 is MPRAGE.
- **GE Silent MRA:** ZTE, Radial, no construct, base none; its MIPs ProjectionDerived + MIP.
- **GE PROPELLER private type 2** (our counts: it marks the newer software): no Real construct; PROP tokens are Radial.
- **Real or imaginary spin-echo components without PSIR evidence:** base none. **A T2-named gradient echo:** T2starw.
- **Diffusion:** a Siemens SequenceName ending `t` after the b value is Trace; a Philips "Reg -" copy is RawRecon with MoCo; a bare Philips b > 0 stack with no direction marker is can't tell (or session-inferred).
- **MS-EPI** includes GE MUSE and Philips IRIS as well as RESOLVE.
- **EPIMix and NeuroMix blocks:** EPIMix T2 (the b=0) and T2-FLAIR SE-EPI, T1-FLAIR EPI, T2* GRE-EPI, DWI Trace, ADC ADC; NeuroMix T2w and T2-FLAIR SS-TSE, T2w FSE TSE.
- **GE prefixes:** ImageType first; PU: no construct, ORIG: ORIG, PJN: MIP, SC: secondary capture. FL: stays as our counts decided.
- **Modifiers:** MT_GEMS is MT; PROMO is not MoCo; black blood only from a name, flagged.
- **Unchanged and confirmed:** EXP is eADC; SBRef base DWI; Siemens real IR PSIR; epir SE-EPI; integer ScanOptions, MRF_GEMS and Elscint ENHANCED are ignored; session decisions are flagged and certified apart (record 53).

Research and our earlier rulings disagree on four points, put to Nima before they are applied: MobiView provenance, GE FR_GEMS, GE T1FLAIR_GEMS, and the single-volume DSC's base.

### The four round-3 conflicts (Nima, 2026-10-01: "agreed")
1. **Composites** (Philips MobiView, Siemens COMPOSED/COMP_AD): provenance **ProjectionDerived** with construct **Composed**, the Siemens ORIGINAL flag notwithstanding; a composite of survey stations is a Localizer. This replaces the morning's RawRecon wording and V28.
2. **GE fast recovery:** the FR_GEMS option decides VFA-TSE (also on PROPELLER, with Radial); an inversion (IR_GEMS or TI > 0) takes precedence as IR-TSE with its modifier; an FRFSE pulse name without FR_GEMS prefers TSE. This replaces D11 and the re-adjudication's FR ruling.
3. **GE T1FLAIR_GEMS:** the option overrides the CSF-null rule and gives FLAIR; the rule still decides where the option is absent.
4. **A single-volume DSC:** base stays T2starw.

**Applied in MRI pack 0.21.0** (kineuro/nils#337; `studies/2026-10-01-pack-0210/README.md`), with dev-6's rule bugs A1 to A10, and with four readings the round left open, confirmed on 2026-10-01 under this record's rule (the research's "vendor flags win" and our counts):
- **A series split into planes and named for planning is a vendor localizer flag.** It is the three-plane localizer GE's SeriesPlane 256 states. Without it, 19,698 product localizers of more than 5 slices a plane would leave Localizer while SeriesPlane is unread.
- **Philips SmartExam (`SmartBrain`) and GE READYBrain count with Siemens AutoAlign.** They are the vendors' automatic planning scouts under the vendors' own protocol names.
- **Calibration applies to any vendor,** by its name, never DERIVED, as the final record's R3-CAL reads it. All 899 archive calibrations are GE's.
- **A merged Philips mFFE stays combined.** (2001,1014) counts the echo images a series holds, and a merged mFFE writes 1 there beside a train of 5 (156 stacks). The single-echo test therefore still reads the train.

## The final certificate's five open choices (Nima, 2026-10-01: "Let's go with your recommendations")

The final read's preparation (`studies/2026-10-01-final-certificate-read/plan.md`, a draft, not registered) left five choices, found on its dry runs:
1. **The picture-read stacks leave the frame, by stack.** Every production campaign's items and pair sides, post-contrast read r1's singles, anchored read r2's panels and the production host's campaign items are out; their sessions stay. Sealing a drawn gold item would mark its label set sealed, and the dry draws drew 18 to 28 of them, in five or six of the six body-part gold rounds. **This amends "The final certificate's design" 1,** under which the picture reads stayed in the frame.
2. **System 1 round 9's guard drops the final's sampled sessions, not their subjects.** G3 by subject stays for sealed-1. Round 9's grade gives every line on the stacks whose subject never trained as well. By subject, the seal would have put 68 % of the archive on sealed subjects; by session, round 9 loses about 61,000 stacks.
3. **The final certificate's samples do not count in record 50's body-part G3 rule.** This certificate does not grade body part.
4. **llm-2 reads 6 at a time** in the final read, dropping to 2 if the model server errors.
5. **The pack certified is the one dev-7 passes, as stored, with no further change** (0.21.0, job `pack-0210`, if dev-7 passes it).

With choice 1, the frame (dry run after dev-7's draw and the NILS server's reclassify with pack 0.21.0) holds 238,875 clean non-localizers, 7,502 kept-uncertain and 58,669 of the old void's kept reformats and scouts. The draw still waits for dev-7's grade (design 5).

## After dev-7's grade (2026-10-01; settled under this record's rule, research where confident and our counts where it is not)

dev-7 graded pack 0.21.0 at 14 of 900 (clean W 1.91 %), so the final certificate's draw still waits (design 5). Its two disputes, both on Siemens MEAN outputs of a multi-echo SWI, are settled without a new ruling from Nima, since neither conflicts with an earlier ruling:
- **Every Siemens MEAN output of a multi-echo SWI is comb-ME-GRE,** the SWI image, its minIP and its MIP included (dev-6's C1). No ruling named its technique (RJ2 rules the provenance, R3-SWIME the per-echo magnitudes, DR5 the MEAN phase and magnitude), and round 3 left Siemens' combination marker to our counts. The counts: Siemens writes each echo's SWI outputs as their own series (echo numbers 1 to 4) and one MEAN of each beside them (echo number 1; SWI 480, minIP 475); MEAN is on no single-echo SWI. With DR5's "a mean over echoes is an echo combination" and DR2, the MEAN is the echoes combined. Earlier references settled 19 such cells ME-GRE (dev-1 to dev-5); those are settled cells, not a ruling, and the final record corrects them.
- **A Siemens MEAN magnitude of a multi-echo SWI is Magnitude with EchoCombined,** by the second round's header rule for case 5 (M with MEAN: RawRecon, Magnitude, comb-ME-GRE), round 3's case 13 (the SWI route's magnitude keeps Magnitude) and N1.

For the next read (`studies/2026-10-01-pack-0220/README.md`):
- **The reading guide is v3.2.** Its N7 sentence, which led dev-7's raters to give four Siemens SWI magnitudes no construct, now says that the SWI route's own magnitude keeps Magnitude and that N7's exception names Philips' per-echo `M_FFE` and GE's unprefixed images only. The two settlements above are in it. Prompts v3.2 are composed from the registered v3.1 files (`compose_prompts_v32.py`), with their sha256 in the study.
- **The final record gains X7-MEAN, X7-MEANMAG, X7-SWIMAG and X7-BAREB** (a bare Philips b>0 stack with no direction marker settled Trace is can't tell, round 3's case 26), and SmartExam's planning reformat among the vendor localizer flags. They are for reads after dev-7 only and are never applied to grade dev-7 again.
- **Pack 0.22.0** carries dev-7's six rule bugs (a re-exported Philips 3D TFE read by the module's PulseSequenceName for case 17, no Trace on a raw Philips tensor, no diffusion technique from a name on an image typed LOCALIZER, SmartExam's planning reformat as a planning scout, a STAGE echo's named weighting) and the two settlements. It is graded on the next fresh read, never on dev-7.

**Applied in MRI pack 0.22.0** (kineuro/nils#341, merged 2026-10-01 as `c41c074`; engine 1.0.0-alpha.70, #342; `studies/2026-10-01-pack-0220/README.md`). Replayed over dev-1 to dev-6 against the final record with the rulings above: 20 cells change, 20 fixed, 0 broken; on the archive 3,659 stacks move. The NILS server needs a reclassify only (no re-read), which Nima runs with the update.

## After dev-8's grade (2026-10-02; settled under this record's rule, research where confident and our counts where it is not)

dev-8 graded pack 0.22 at 7 of 900 (clean W 0.66 %, Korn-Graubard upper 1.55 %): the clean rate is near 0.5 %, its bound is not, so the go condition of design 5 is not met in full, and whether the rate alone is enough is Nima's call. Its one dispute is settled without a new ruling from Nima. It needs no new value and conflicts with no ruling, and the research is silent where our counts decide:
- **A Siemens BLADE written `*tseBNR` is VFA-TSE with Radial, as N12's `*tseBR`.** The research names a BLADE with restore VFA-TSE and does not read the letters after `tse`. Our counts (the research container, every archive stack): Siemens writes the N with the R (`tseBNR`, 33, MAGNETOM Terra.X) and without it (`tseBN`, 5), the R with or without the N (`tseBR`, 260), and an inversion as `IR` (`tseBNIR`, 17, every one with a TI). So the lone R is the restore. `tseBN` stays TSE and `tseBNIR` IR-TSE. dev-4's one `tseBNR`, settled TSE before N12, is a settled cell, not a ruling; the final record corrects it (X8-BNR).

For the next read (`studies/2026-10-02-pack-0230/README.md`):
- **The final record gains X8-BNR,** for reads after dev-8 only, never applied to grade dev-8 again.
- **The reading guide's VFA-TSE line should name `*tseBNR…` beside `*tseBR…`** in the next read's registration, with its prompts composed and their sha256 recorded.
- **Pack 0.23.0** carries dev-8's three rule-bug groups, each widened to its rule:
  - **DR4 by the session:** a second session pass makes the ADC or eADC map of a Siemens or Philips tensor DTIRecon where only the source series beside it shows the tensor, or an FA was made from it. A plain DWI beside it as well keeps ruling 7's RawRecon.
  - **The physics vote's target** leaves out every stack whose base the rules decide as nothing.
  - **A Siemens 3D-card range** (DERIVED with PARALLEL or RADIAL) is a reformat whatever it is named.
  - **The settlement above.**

  It is graded on the next fresh read, never on dev-8.
- **The final certificate's frame still meets its registered draw** with dev-8 out by session: 6,932 kept-uncertain non-localizers (GE 4,238, Siemens 2,275, Philips 417) against the 600 drawn, and 230,208 clean against 2,500. The shortage dev-8 met was its own frame's, which also leaves out sealed subjects.

**Applied in MRI pack 0.23.0** (kineuro/nils#344, merged 2026-10-02 as `f9b7daa`; engine 1.0.0-alpha.71, #345; `studies/2026-10-02-pack-0230/README.md`). Replayed over dev-1 to dev-7 against the final record with X8-BNR: 1 cell changes, 1 fixed, 0 broken. On the archive, 104 stacks move by the rules and 282 maps by the new session pass, and 2,578 stacks leave the vote's target. The NILS server needs a reclassify only (no re-read), which Nima runs with the update.

## The final certificate goes on pack 0.23.0 (Nima, 2026-10-02: "Certificate next (Recommended)")

**dev-8's grade** (`studies/2026-10-02-dev8-reference/grade.md`, part 2): pack 0.22 (stored, job `pack-0220`) got 7 of dev-8's 900 non-localizers wrong or empty (U 0.78 %), all 7 clean. The clean W rate is 0.66 %, with a Korn-Graubard upper bound of 1.55 %; without the one open dispute it is 0.55 % (bound 1.39 %). Kept-uncertain was 0 of 332 and the localizers 0 of 100. So the clean rate is near 0.5 % and its bound is not under 1 %: design 5's go condition was met on the rate, not in full.

**Pack 0.23.0** (`studies/2026-10-02-pack-0230/README.md`; kineuro/nils#344, engine 1.0.0-alpha.71) fixes dev-8's 6 rule-bug stacks, in three groups, and settles its dispute (a Siemens BLADE `*tseBNR` is VFA-TSE, the final record's X8-BNR; "After dev-8's grade" above). Modelled on dev-8 for information only, it is at 0 of 900. Replayed over dev-1 to dev-7, 1 cell changes, 1 fixed and 0 broken.

**The question, as Nima saw it:** "0.22 is already at 0.66 % (0.55 % without the dispute) and 0.23 removes 6 of its 7 errors; about 78 to 87 % chance to pass. A dev-9 would use up kept-uncertain stacks the certificate needs."

**The decision:** pack 0.23.0 goes straight to the final certificate, with no dev-9. Production is reclassified with it as job `pack-0230` (engine alpha.71), and the final certificate certifies those stored answers. It is pack 0.23.0's first grade on a read it took no fix from.

**This amends two rulings, for that reason:**
1. **"The final certificate's five open choices" 5:** the pack certified is MRI pack 0.23.0 as stored, from job `pack-0230`, and no longer the pack a development read passed.
2. **"The final certificate's design" 5, the go condition:** a clean rate near 0.5 % on the last development read is enough, without its bound. dev-8's 0.66 % (0.55 % without the dispute) meets it.

**What changes in the final read's plan** (`studies/2026-10-01-final-certificate-read/plan.md`, "The amendments of 2026-10-02"):
- dev-8 joins the header reads that leave the frame by session (dev-1 to dev-8).
- The prompts are v3.3: v3.2 with the reading guide's VFA-TSE line naming `*tseBNR…` beside `*tseBR…`, as the pack 0.23 study asked at the next read's registration.
- The correction record is `rulings.py` as it stands, with X8-BNR.
- The packets are unchanged, since pack 0.23.0 shows a reader no private element that pack 0.21.0 did not.

## Position displays follow ruling 4 (Nima, 2026-10-02)

**The question** came from the 7 archive stacks that break `t2star-not-spin-echo` under packs 0.21.0 to 0.23.0 (`studies/2026-10-02-t2star-se-broken/README.md`). Pack 0.18.1 left a Siemens position display's technique (`POSDISP`, the FLASH picture of another series' plan, carrying that series' name) to the words of the series it plans: "a case the header cannot decide" (dev-1's H1). Ruling 4 of 2026-09-27 reads a localizer's technique by family, "GRE for `*fl2d1` or a gradient-echo scout". Pack 0.22.0 already read an image typed `LOCALIZER` or `SCOUT` on a gradient echo by its readout. 629 archive position displays were left with a spin-echo technique from their words. The development references split on the three they hold: GRE, family GRE, and TSE on one derived from a spin-echo image.

**The ruling:** a Siemens position display follows ruling 4. It is a gradient-echo scout, read GRE by the readout it actually used, never by the words of the series it plans. **This reverses pack 0.18.1's choice.** A position display derived from a spin-echo image (sequence name `*tse`, `*se`, `*h`, ScanningSequence SE) used a spin echo and keeps it.

**Applied in MRI pack 0.24.0** (draft kineuro/nils#349, with the fixes for the 7). It goes out after the final certificate's grade, so nothing the certificate grades changes. On the archive's replay, 631 position displays move to the GRE family: from TSE 499, SE 84, IR-TSE 43, 3D-TSE 3 and DWI-EPI 2, to FLASH 508, ME-GRE 114, MPRAGE 8 and bSSFP 1. On dev-1 to dev-8 against the final record, 1 scored position display moves (dev-1, settled GRE, fixed) and none breaks. No reference settled a gradient-echo position display TSE, so the final record needs no new rule.

## After the final certificate's grade (2026-10-02; settled under this record's rule, research where confident and our counts where it is not)

The final certificate failed pack 0.23.0 (`studies/2026-10-01-final-certificate-read/grade.md`, part 2: main 32 of 2,500, Korn-Graubard 1.72 %). **Its sample is development evidence from now on, seen, and never certifies again.** A further certificate needs a new sealed draw.

**The correction record, for reads after the certificate only** (`studies/2026-09-28-certificate/tools/rulings.py`; never applied to grade the certificate again):
- **Four rules narrowed in place** (the grade's kind b):
  - D3 keeps a GE MUSE map's MS-EPI (round 3, case 27);
  - V12 leaves EPIMix's T1-FLAIR block EPI (case 28);
  - DR9 leaves Siemens' relative perfusion maps PerfusionRecon (its own words);
  - DR6 drops a technique settled by family along with the Localizer it takes away.
- **X9-BAREB2:** a bare Philips diffusion stack with no b value above 0 and more images than slices is can't tell (C3, case 26).
- **X9-IRREAL:** a Siemens MPR of an inversion recovery whose source beside it is written real is PSIR with construct Real. This is DR3 for a reformat, the ruling the grade asked for: no new value, no conflict, and the certificate's adjudicators settled 5 of 6 so.
- **X9-MEANMAG:** a Siemens MEAN magnitude of a multi-echo gradient echo is Magnitude with EchoCombined, as the guide already words it.
- **X9-DANTE:** a Siemens reformat of a DANTE-named source beside it takes BlackBlood (case 37); the references split 4 to 3 on it.

**The disputes** (`studies/2026-10-02-pack-0240/disputes.md`), settled where the research or our counts decide and no ruling conflicts:
- C2: a GE `ksepi` 3D EPI at short TR and TE is GRE-EPI.
- C3: a Philips diffusion set with its directional images takes no Trace.
- C4: a Siemens `Image Filter` copy written DERIVED is Filtered.
- C5: a Siemens QSM-named acquired echo has no Magnitude.
- **C1, a STIR named T2 under TE 40 ms:** settled with the base question by round 4 (below), R2: it keeps T2w.

**Open, for round 5's research** (`studies/2026-10-02-pack-0240/README.md`, "For Nima"; they stay as pack 0.23.0 has them):
- a TWIST-VIBE dynamic run, DCE or CE-MRA;
- a GE SWAN's combined image, ME-GRE or comb-ME-GRE;
- MT from `MTC` in a name;
- Siemens ScanOptions `SFS`;
- the construct of a subtraction of a composite;
- the base of a quantitative T1 map.

**Applied in MRI pack 0.24.0** (kineuro/nils#349, with the t2star-se fix and round 4; merged 2026-10-02 as `38285ed` after its diff was read and all 12 checks passed, the engine check on one rerun of a timing flake; not yet released): the grade's rule gaps, each widened to its rule, and C2 to C5. The base stated in the name against the physics, which A6 and the 419 T1-type physics failures raised, is round 4 (below).
- **On the archive:** 2,311 stacks move by the rules against 0.23.0, and 1,690 more by two new session passes (1,000 inversion-recovery reformats to PSIR with Real, 690 DANTE reformats to BlackBlood).
- **On dev-1 to dev-8, against the record after the grade:** 7 cells change, all fixed.
- **On the certificate's sample (seen):** 76 cells fixed and none broken. Modelled for information only, main is 11 of 2,500 against the record as graded and 6 against the record after it.
- **The reading guide** should carry the narrowings, the settlements and the X9 readings at the next read's registration, with its prompts composed and their sha256 recorded.

## Conventions, round 5 (2026-10-03; `studies/2026-10-02-pack-0240/deep-research-r5.md` with the counts in `studies/2026-10-03-pack-0250/README.md`)

The deep research on the six questions the final certificate's grade left, checked against our archive's counts, and Nima's answers of 2026-10-03:
- **Quantitative maps have no base.** A T1 or R1 map is a quantity, not a weighting (BIDS T1map, DICOM QUANTITY); a synthetic T1-weighted image and MP2RAGE's UNI stay T1w. Old Philips acquisitions typed `T1 MAP` are weighted images, not maps (our counts). **T2 and R2 maps take base none too** (Nima, 2026-10-03, for consistency).
- **GE SWAN:** its outputs written one image per slice combine the echoes as a weighted average (GE's words): comb-ME-GRE. **Nima: "Keep SWI for SWAN".** The combined image keeps base SWI under the 2026-09-28 SWI convention, and its minIP keeps SWI; only the technique changes.
- **A Siemens TWIST-VIBE dynamic run that names no angiography is DCE.** On our archive none carries AngioFlag Y or a spatial MIP or subtraction child; its children are Siemens' dynamic evaluation maps.
- **MTC in a name is MT,** below the header's tokens and never where the words switch it off, as the pack has read it since 0.12.0. A bare `MT` is not read (our counts: Philips spine surveys).
- **Siemens `SFS` keeps FatSat: kept by our counts; the research was unsure.** Nima, 2026-10-03, asked why a call was needed at all, since the pack already reads it so and our data supports it: 53 of 61 `SFS` stacks are named for fat saturation, and no corroborating field exists in our registry.
- **A subtraction of a composite** is SubtractionDerived (our vocabulary's subtraction) with construct Composed.
- **Open, for the next engine wave** (Nima, 2026-10-03: yes): the fingerprint gains AngioFlag (0018,0025), the temporal resolution and Acquisition Contrast (0008,9209), so the research's header rules for TWIST and round 4's R9 can read them. Nothing changes before.

## After the protocol sweep's grade (2026-10-03; settled under this record's rule)

The protocol sweep (`studies/2026-10-02-protocol-sweep/grade.md`, part 2) found 177 stacks outside p-go that pack 0.24.0 gets wrong. p-go's own error list stays sealed in `protocol-sweep-go`; certificate 2's go grades the next pack once on p-go.
- **Kind (a)** is fixed by rule in pack 0.25.0.
- **Kind (c), the disputes** (`studies/2026-10-03-pack-0250/sweep-disputes.md`), settled where no ruling conflicts and the research and our counts agree:
  - a Philips real IR (`R_IR`) is PSIR as a Siemens one is (DR3);
  - a Siemens composed copy named `_FILT` is Filtered;
  - a trace-named Siemens mixed diffusion stack is Trace;
  - a GE b 0/10 reference named PA is FieldmapRef (DR7);
  - a thin Philips projection named mip is MPR (R3-MAX);
  - a 3D HASTE is SS-TSE;
  - a sequence named `scout` alone makes no localizer of many slices;
  - GE `IFLOW_GEMS` is a phase contrast;
  - SyMRI's EPW and its kin are Qmap;
  - the short-TE STIR's base follows round 4.
- **The correction record, for reads after the sweep only:**
  - D10, V28 and R3-FEEPI narrowed;
  - X10-TWISTDCE, X10-SWAN, X10-MTC, X10-T1MAP and X10-T2MAP (round 5);
  - X10-R4STIR (round 4's R1 and R2, as Nima asked);
  - X10-FILCOPY and X10-TRACE (the disputes);
  - X10-GRECOMP (round 3's case 24 on gradient-echo real and imaginary components);
  - X10-COMBINED (a Siemens SWI output named Combined).
- **Open for Nima** (`studies/2026-10-03-pack-0250/README.md`, "For Nima"):
  - a classic Philips 3D T1TFE without `MP`;
  - GE private image type 2 on MAGiC;
  - GE BRAVO-header reformats named Cube T2 FLAIR;
  - a Siemens 3D SPACE real IR at a long written TE;
  - Siemens `_Fs` without FS;
  - maps computed from a Philips registered diffusion (N11);
  - a Philips Reg DWI set with direction I (C3);
  - a two-echo `fl3d2r` SWI image;
  - GE's QSM R2\* map;
  - a GE `efgre3d` at a high flip read bSSFP by our counts;
  - DiffusionDirectionality for the fingerprint.
  
  A brief covers four (`deep-research-brief-r6.md`).

**Applied in MRI pack 0.25.0** (draft kineuro/nils#352; not merged, not released).
- **The constraints:** the NILS server's reclassify with 0.24.0 (alpha.73) reported 63 stacks breaking `implied:base/technique:comb-ME-GRE`: a GE multi-echo QSM's outputs, base SWI by the route. The implication now reads only an acquired echo combination, so a combined SWI image can be comb-ME-GRE with base SWI (Nima's SWAN answer, the 2026-09-28 convention). A replay of the archive, which now names each answer's broken constraints, finds 63 for 0.24.0 and 0 for 0.25.0.
- On the archive, 2,770 stacks move against 0.24.0 by the rules.
- Against the final record after the sweep:
  - dev-1 to dev-8: 6 cells change, all fixed;
  - the certificate's sample (seen): 4 fixed;
  - the sweep outside p-go: 180 to 36 stacks wrong or empty by the rules alone, 195 cells fixed and none broken.

## Post-contrast: model r1's test, model r2, and T2-type (2026-10-01 and 02)

The image track's history from the first model on. The study is `studies/2026-09-28-post-contrast-image/`, and its running report is `report.md` sections 13 to 16.

1. **Model r1 passed its registered test** (`model-r1.md` part 7). N read 718 anchored items: 0 errors in 650 answered T1-type brain stacks at the held-out scanners (upper bound 0.46 %), all 40 hidden controls agreed, and the rules-empty stratum went 0 of 329. The readers were corrected the same day: R read round 1, D the anchor check and round 2, and N only the test. So the test is independent of the gold that trained the model.
2. **The anchor look** (`model-r1.md` 8.6). N and D read the same 23 items and agreed on all 20 inter-reader items. Both read the two GE tag anchors as pre, so the tag audit tightens. The model's one confident miss was a thin coronal 3D MPRAGE (about 54 mm through the plane) where inflow looked like contrast.
3. **Model r2 was registered before training** (`model-r2.md` part 1). It takes model r1's proposals:
   - the tightened audit;
   - the through-plane extent measured on the image;
   - calibration by make where a site has none;
   - abstention where the tag and a confident image disagree;
   - a test drawn for a bar per make and field strength, held out by subject;
   - N reading the test, with a 300-item share read by D.
   The pool was counted first. Per-make bars need references at relaxed levels, and Philips is only just reachable.
4. **Amendment A1** (Nima, 2026-10-02: "option (b)"). Stacks under 100 mm through the plane answer "not sure", registered before a refreeze. N's earlier reads helped make that choice, so they became a picking set and the regression check retired.
   - The refrozen model answers 2,306 of the 2,721 test-frame stacks: GE 624, Philips 342, Siemens 1,340, 1.5 T 1,329, 3 T 898.
   - The test is drawn: 1,442 items for N, a 300-item share for D (`model-r2.md` part 5).
   - The campaigns wait for Nima's go.
5. **T2-type feasibility** (`model-r2.md` T.5; Nima, 2026-09-30: "only if T1 works, and report honestly"):
   - **Objective anchors exist:** the audited tag and a new time bracket by the session's own T1-type anchors, 2,118 post and 4,065 pre, agreeing on all 2,351 stacks they share.
   - **The regional ratio does not separate:** only 11 same-setting pairs span the injection, and it calls none of the 9 it registers.
   - **The structures-and-vessels arm does not see contrast:** a cross-fitted AUC of 0.73, 0.58 on the clock-based anchors.
   - T2-type stays "not sure".
6. **The T2-type anchored learning read** (Nima, 2026-10-02: "we should start the t2 anchor too"; `model-t2.md`). It asks whether readers can tell pre from post on T2-type brain stacks beside T2-type references, and how well. It is never a test.
   - **N reads 408 items:** 304 labelled anchors (GE and Siemens 150 each), 4 same-setting pairs, and 100 unresolved stacks.
   - **D reads a 120-item shared share.**
   - The verdict rule is fixed before the read: readers "can tell" a make if the Wilson lower limit of decided accuracy is at least 0.90 and can't tell is at most 25 %.
   - **Philips cannot be measured** (no readable T2-type post anchor). Every T2-type reference is at a relaxed level.
   - The campaigns go on the NILS server after the reader update and Nima's go.

## Round 4: the name against the physics (Nima, 2026-10-02: adopted)

**Nima gave his go on 2026-10-02** for the rules below and the two axes. They are applied in MRI pack 0.24.0 (kineuro/nils#349). The convention and its impact are in `studies/2026-10-02-name-vs-physics/round4-conventions-draft.md`, from the deep research in the same folder. It settles C1 of the final certificate's grade by R2: a STIR named T2 keeps T2w, and the final record's X9-STIRT2 corrects the 3 cells the certificate's adjudicators settled PDw, for reads after the certificate only.

**Nima's direction (2026-10-02):** "physics should come to help on extreme cases and edge cases or when we have no keyword ... this need a cautious intervention not to break what we have built in pack 23". On the two axes: "i like the idea of adding the two new fields, proceed with them. seems like a more robust way to do this".

**As ruled:**
- **The name stays the default.** The physics overrides a stated weighting only where it contradicts it outright:
  - R1: a STIR-window TI named T1 is T2w;
  - R3: a FLAIR-window TI named T1 at TE 80 ms or more, or on a 3D TSE, is T2w;
  - R5: a 2D spin echo with no inversion at TR 2,500 ms or more and TE 80 ms or more, named T1, is T2w;
  - R7: a spin echo at TR 1,000 ms or less and TE 30 ms or less, named T2, is T1w;
  - R8's exception: a spoiled T1 gradient echo named T2 is T1w.
  
  The windows are the research's, by field.
- **The name stands** for:
  - a STIR named T2 or PD (R2);
  - a T1-IR window named T1 at any echo time (R4: `t1_space_ir`, the T1-FLAIR BLADE and PROPELLER, `T1FLAIR_GEMS`);
  - the PDw rule (R6);
  - a gradient echo named T2 or T2\* (R8);
  - everything else (R12).
- **DIR is out of the TI rules,** and a 3D TSE's written echo time is never its contrast.
- **Two axes, described and not graded:**
  - `contrast_mix` (`intermediate_PD_T2`, `T2like_T1`, `T1like_IR_longTE`, `PD_T2star`; no value means none);
  - `base_basis` (`name`, `physics`, `name+physics`, `acq_contrast`).
  
  The roles take no stack with a `contrast_mix` value.
- **Not built:** R9 (Acquisition Contrast is not in the fingerprint), R10 and R11 (the research's physics fill and can't tell, which differ from the pack's windows on about 14,900 archive stacks), and R5's 3D branch (counted for review).
- **On the archive:**
  - 209 stacks move T1w to T2w (R1 31, R3 178);
  - 986 take a `contrast_mix` value;
  - 692 lose the t2w role and 36 the t1w role.
- **On the references:**
  - dev-1 to dev-8 have 0 changes;
  - the final certificate's sample (seen) has 2 fixed and 0 broken;
  - sealed-1 has 0 changes.

