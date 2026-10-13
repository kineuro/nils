# 40 · The road through Wave 6

Opened 2026-09-23. **Status: in force.** Everything from the release of 1.0.0-alpha.37 to the end of master-plan Wave 6, in the order I do it.

## The ask

> Nima, 2026-09-23: "We need to first see where we are and go step by step. See the hanging jobs and wave and make a plan to do all till the end of wave 6. If needed write them down. Then we go. You have all permission and I grant you everything."

I took stock the same evening: every record from 16 to 39 for what it leaves open, and the live state (repositories, branches, worktrees, installs, issues, memory, the other sessions). The two inventories are in `studies/2026-09-23-stock/`. This record is what they add up to.

## Where we are

- **Released:** engine and desk at 1.0.0-alpha.37 (records 35, 37, 38), assistant at alpha.25, Kvasir at alpha.7. Main CI is green everywhere, and no pull request is open in any app repository. The install matrix passed 13 of 13 against wave 38.
- **Installs:** the development laptop runs alpha.37 on user units. The NILS server still runs alpha.36.
- **Open design:** record 36 (how sure a rule is) was never closed. Record 39 (System 1) is proposed. The master plan (11) was last edited on 2026-09-07 and knows nothing of records 20 to 39.
- **What the protocols said** (2026-09-23): of 17,264 v0-clean stacks in protocol sessions, 3,063 have a confirmed wrong label, 2,916 of them from one rule that calls single-shot EPI "RESOLVE". Across the archive, 21,619 clean stacks carry that error, and v1's pack keeps the same rule. "Clean" is not 100 %: it needs its certificate.
- **Hanging, small:**
  - five engine issues (#94, #98 to #101) and Kvasir #8, untouched since 2026-09-09;
  - the serde_json `float_roundtrip` fix;
  - the reboot proof of S5;
  - a stale worktree, two detached development worktrees, and a dev-channel server still running;
  - nils-design PR #1, open since 2026-09-17;
  - records 21, 26, 32, 33 and 34 without a closing line;
  - memory saying the installer gaps are open (they are closed).
- **Another session** is building Kvasir as a standalone gateway on the model server and waits for Nima's choice of models for card 0.

## What Wave 6 is

The records disagree on this, so I settle it here.

1. **Wave 6 is both halves.**
   - The master plan's half (11, widened 2026-09-04; 17 §7):
     - nils-segment on the contracts;
     - one campaign mechanism for annotation and curation;
     - v0's five QC products;
     - the pixel door and the visual review kinds;
     - the model registry;
     - decisions exported as labelled datasets (D15).
   - Record 39's half: System 1 built into the engine with its certificate.

   They share one foundation: a model's answer is a decision with a registered author, and a person's review is a campaign. So they belong in one wave.
2. **Two kinds of wave, two names.** A master-plan wave keeps its number ("Wave 6"). A wave of record 31's kind is named by its record ("wave 41"). Wave 6 is carried by waves 41 to 46.
3. **Wave 5 is the desk** (20). The query MVP that 11 once called Wave 5 was absorbed into 4b (17). I amend 11 to say so.
4. **A minimal pipeline runner moves forward from Wave 7.** Wave 6's gate needs "prep via seeded pipelines", and the runner was planned for Wave 7. Wave 6 gets the smallest runner that passes: one registered container run on a selection, with its outputs registered as derivatives. Hardening, GPU leasing, the starter catalog and SLURM stay in Wave 7.
5. **Pixels.** Wave 5 built tile, slab and render doors that decode pixels inside the engine, against Wave 6's ruling that the binary never decodes pixels. I read the ruling as being about **judging**. A person looking at an image through a door is viewing, and it stays. Anything that decides from pixels (the body-part detector, System 1's image view) is a pipeline whose output is a registered derivative.
6. **The paper runs past the gate.** Record 39's Phases 0 and 1 are inside Wave 6. Phase 2 (the full reference standard), Phase 3 (external data, a partner site, the effort study) and Phase 4 (writing) are paced by experts and partners, not code, so they continue beside Wave 7.

## Rulings I take now

Nima granted everything. These are the defaults I work on; each is reversible and listed so he can overturn it.

| | Ruling | Why |
|---|---|---|
| R1 | Record 36 retires into 39. The label model measures each rule's accuracy; Nima reviews only the outliers it reports. The worksheet stays as the prior | A sitting over 254 rules gives opinions; the protocols just showed that a measurement finds what opinion missed |
| R2 | The clean stratum is certified, never assumed. Record 39's premise sentence is corrected | 17.7 % of comparable clean protocol stacks were wrong |
| R3 | The protocol stacks are development data from now on. The proof comes from random samples drawn and sealed before anyone looks | We already used them to find bugs |
| R4 | The five protocol printouts go into this private record as `studies/protocols-docs/` | Record 39 and wave 41 cite them, and they hold no patient data |
| R5 | The v1 classifier runs over the production host's archive headers, read only, in a scratch folder at low priority | The model must learn from v1's labels, not v0's |
| R6 | Beta is cut when wave 44 lands and the corpus survives it | Record 34's bar was "beta, if it survives"; wave 44 replaces the hand-set confidences it waited for |

## The road

Each step is one record of record 31's kind unless it says otherwise: slices in worktrees, one pull request, tried on the development laptop through the development channel, the corpus re-run, no tag until a release closes it.

### Step 0. Clear the decks (no record)

1. Stop the dev-channel server. Remove the merged `w35-s4` worktree and branch, and reset the two detached `dev-build` worktrees. Pull nils-desk.
2. Commit the protocol printouts (R4) and the stock inventories.
3. Review nils-design PR #1. Merge it if it still reads true, or extend it to record 39 first.
4. Record hygiene:
   - closing lines on 21, 26, 32, 33 and 34;
   - the README index and register to record 40;
   - 11 amended as settled above;
   - 36 marked retired into 39 (R1);
   - 39 corrected (R2, R3) and its status set to accepted.
5. The operators' notes: the installer gaps are closed, the NILS server's version, and Kvasir's CUDA archive.
6. Update the NILS server to alpha.37 and check it. (Its rollback steps are kept in the private record.)
7. Read a data copy's verification that another session left unreported, and record the result.
8. Triage engine issues #94 and #98 to #101 and Kvasir #8: into wave 41, or closed with a reason.
9. The reboot proof of S5 waits until the Kvasir gateway session has its answer and finishes, because a reboot restarts every session.

**Done 2026-09-23, late evening:**
- Items 1, 2, 4, 5, 7 and 8 are done.
- Item 3: I reviewed PR #1 and it reads true, but the merge is Nima's to run, because the permission classifier refuses a session's merge.
- Item 6 was already done: Nima had updated the NILS server to alpha.37 that morning, and it answered its checks.
- Item 7: the copy verified 18,536 of 18,536 files.
- Item 8 went as follows:
  - #98, #99, #100 and #101 (the ask door's four wrong refusals) and #94 (a sample of the text an axis matched) go into wave 41 S4.
  - Kvasir #8 goes to the Kvasir gateway record of the other session.

### Step 1. Wave 41: what the protocols said

- **S1 The rules the protocols proved wrong:**
  - RESOLVE only from a readout-segmented sequence name (`re_b`), never from SK plus EPI;
  - FlowComp only from the header, not from words in a name;
  - MEDIC's multi-echo GRE is not GRASE;
  - a series' own body-part evidence beats the exam's.
  - Each fix has a synthetic test. The protocol comparison is re-run with a target of no known error pattern left.
- **S2 Every rule's vote.** `nils classify` records every rule that fired and what it said, not only the winner. The label model reads this.
- **S3 The pack reports values no valid combination reaches**, and the 56 clauses that only restate another axis are marked as schema implications.
- **S4 Small fixes:**
  - serde_json `float_roundtrip` in the sidecar path;
  - the triaged issues;
  - an ask fixture with a ruled-out stack (record 35's leftover).
- **Proven by:** the protocol comparison, the corpus, and each fix's own test.

### Step 2. Phase 0 of System 1 (a study, beside waves 42 and 43)

In `studies/2026-09-24-phase-0/`, about three weeks:

1. Check the expected-label table by hand against the printouts, sequence by sequence.
2. Run v1 with wave 41 over the production host's archive headers (R5), and write the vote matrix.
3. **Draw and seal the certification samples** (about 800 for each stratum), stratified as record 39 says, before anyone reads a result.
4. Fit the label model, check its estimated rule accuracies against the protocol stacks' true ones, and train the first LightGBM heads.
5. Score everything on the protocol stacks, and on study B leaving one site out at a time.
6. Measure OpenNeuro's sidecar coverage and TCIA's MR metadata.
7. The 500 reference stacks from one rater. They need a named rater (see "From Nima").

**Gate (record 39):** at 1 %, rules with System 1 decide at least a fifth more stacks than the rules alone, and the certificate holds on held-out data. If not, I write up what we learned and Wave 6 continues without System 1's half.

### Step 3. Wave 42: labels and models have a home

- A model registry in the engine (D15). Each artifact is named by its digest, with its training set, version and card.
- A model's answer is a decision with `author_kind = model` and the model's id.
- Any decision kind is exportable as a labelled dataset with provenance (C7).
- The campaign mechanism: a selection, assignments, one decision per item, adjudication and export, with no table specific to QC. Whether a draft state is needed (16 §6, D14) is answered here, after reading v0's `classification_qc` rules engine, which no one has read.
- The model lifecycle Kvasir built for language models is compared first, so there is one idea of "a registered model", not two.
- **Proven by:** a campaign run twice, once as annotation and once as body-part curation, with no database-level integration.

### Step 4. Wave 43: pipelines make derivatives

- The minimal runner (settled above, item 4).
- Derivatives registered, with a named transport: a download door per derivative, or a declared shared volume.
- The embedding cache as derivative state keyed by encoder version.
- **The first real use:** v0's body-part detector ported as a pipeline, a registered model and proposals that land as review items. This is v0's largest QC product (6,672 lines) and the image view System 1 will read.
- **Proven by:** the body-part pipeline on the corpus, its proposals reviewed through a wave-42 campaign.

### Step 5. Wave 44: System 1 in the engine (after the Phase 0 gate)

- A `model` pass kind beside `nearest_neighbour_vote`, evaluated in Rust bit for bit against LightGBM:
  - `libm` pinned;
  - the canonical artifact named by its digest;
  - golden vectors;
  - CI on all six targets;
  - the site model never compiled into the public binary.
- The training tool, run at the site like `tools/v0-compare`.
- The legal-combinations layer and calibration after it.
- The certificate in the classify report: the risk the site chose and the automation it bought.
- Hand-set tier confidences retire (R1).
- **Proven by:** golden vectors on six targets, the certificate re-computed from the sealed sample, and the corpus. Beta is cut here (R6).

### Step 6. Wave 45: review on one mechanism

- The visual review kinds in the desk, so v0's interface is no longer needed for them.
- MPR in the browser from the slab door (20 §12).
- Per-axis review (v0's `axes`) and classification QC (v0's `classification_qc`) as campaigns.
- System 1's candidate lists: only legal combinations, each with its probability and both systems' evidence.
- `cohort_main` and `main_acquisition` checked as covered by Wave 3's picks and decisions.
- Custody entries for every new store, and the absence story for each new part.
- **Proven by:** a body-part curation run and a classification review done end to end in the desk on the corpus.

### Step 7. Wave 46: segment on the contracts

nils-segment in its own repository, built only on the contracts (07):
- subsets are selections;
- catalog and AST reads replace the `seg_readonly` role;
- prep runs through the wave-43 runner on a frozen selection;
- rater disagreement becomes review items;
- the AI assist stays an outbound client;
- authentication only verifies.

**Proven by master-plan gate bars 1 and 2:** a full annotation work (subset, prep, rating, adjudication, export), and one campaign mechanism serving both it and a body-part run, with no database-level integration and no QC-specific table.

### Step 8. The Wave 6 gate

- The two bars above, the absence test, and the custody page.
- A release closing waves 41 to 46.
- v0 keeps running on the production host until every bar is green. The visual review kinds leave v0's interface only when wave 45 serves them.

### Beside and after the gate: the paper

Record 39's Phases 2 to 4:
- **Phase 2:** the full reference standard of about 3,800 stacks, all certification stacks double-read, 100 to 180 expert hours.
- **Phase 3:** external datasets (Spine Generic first), a partner site through Bifrost, the effort study with 8 curators.
- **Phase 4:** writing with Russell, for Nature Biomedical Engineering after a presubmission enquiry.

About six months to submission. The reference standard and data access set the pace, not the code.

## From Nima

What only Nima, or people he reaches, can do, in the order it is needed:

| When | What |
|---|---|
| now | The Kvasir gateway session's question: which models go on the model server's card 0 |
| now | The three questions to KI's data protection officer and legal office (record 39): does the approval cover this work; is a model file a disclosure under OSL; may partners receive it. The answers take weeks |
| Phase 0, week 2 | A named rater for the first 500 reference stacks, and the ethics approval it runs under |
| Phase 0 | Research questions Q1 to Q3 (record 39 appendix), run like the earlier rounds |
| Phase 2 | The second rater (neuroradiologist, or MR physicist or senior technologist), and the time of both |
| Phase 3 | A partner site, and Russell on the paper's scope |
| any time | Printouts of the 2018–2020 and 2020–2022 study B protocols and of study F (about 9,000 more reference stacks) |
| any time | Where the human QC he remembers lives, if not in what I found (610 body parts in the legacy database, 284 in v0's) |
| one sitting | The rulings carried by records 25 to 28 and the NILS server's cleanup list (`studies/2026-09-23-stock/`). None blocks Wave 6 |

## Nima's answers, 2026-09-24

- **Raters:** Nima and Russell read the reference standard, both the first 500 stacks and the double reads of Phase 2. The first 500 are split between them, each stack read once (Nima, 2026-09-24: "Split is ok for now"). Each reads under a rater-only account on a reader that runs on the production host against Phase 0's registry, so the sealed stacks and their images stay on the production host.
- **Data protection:** Nima asked KI's data protection officer and legal office the three questions of record 39. His word: "i did. everything is ok."
- **The development laptop's GPU path:** Nima installed rootless podman and the NVIDIA container toolkit with CDI. A rootless container sees the laptop's card, so wave 43 runs on it.
- **Clean-ups:** approved; Nima runs the deletions himself. (The list is kept in the private record.)
- **The Kvasir gateway:** Nima is changing Kvasir himself and will say when.
- **The older study B and study F printouts:** Nima will try to get them.

## Progress, 2026-09-24

- Waves 41, 42 and 43 are merged (kineuro/nils#212, #213, #214; nils-desk#124) and run on the development laptop as `1.0.0-alpha.37.dev.3`. None is released.
- **Wave 45 runs before wave 44.** Wave 44 waits for the Phase 0 gate; wave 45 builds System 1's slot against a fixture, so it need not wait. Records 45 and 46 are written.
- **Phase 0:**
  - The archive digest is running.
  - The label model, sealing and a first LightGBM are built.
  - On the protocol stacks, the label model estimated each rule's precision at Pearson 0.935 against the truth, without labels.
  - The older study B versions and study F run the final protocol under other names, so about 8,500 more stacks inherit exact labels by physics.

## Progress, 2026-09-24 evening

- **Released as 1.0.0-alpha.38**, with Kvasir 1.0.0-alpha.8 and the assistant 1.0.0-alpha.26: waves 41, 42 and 43, record 47, and the fixes the body-part loop found. The install matrix passed 14 of 14 first.
- **The group's install:** the NILS server runs alpha.38, with its own key to the model server; the address exception is gone.
- **Wave 45** is merged, not released.
- **Left in Wave 6:**
  - wave 44, which waits for the Phase 0 gate;
  - wave 46;
  - the move of the model server from modelgate to Kvasir (record 47, M);
  - the Phase 0 archive run, whose digest is still going on the production host.

## Not in Wave 6

- Wave 7: the hardened runner, GPU leasing, the starter catalog seeded at boot, SLURM and Apptainer, the CT pack on photon-counting data, v0's decommission.
- Wave 8: federation.
- The Kvasir gateway record belongs to the other session.
- Naming look-alike acquisitions (434 stacks refused a shared name): wave 41 if it stays small, otherwise its own record after 46.
- An LLM as a third reader of hard cases, the fine-tuned small encoder and the zero-shot reader: comparison arms in Phase 0, and a later record if one of them wins.

## Risks

- **The Phase 0 gate fails.** Then waves 42, 43, 45 and 46 still carry Wave 6, and System 1 is written up as what we learned.
- **The rater is not found in time.** Everything else proceeds, but no certificate can pass without them.
- **The production host is shared.** The archive run is read only and niced, and it stops if anyone's work slows.
- **Scope.** Six waves is the largest stretch since Wave 4. Each wave closes before the next opens, except the Phase 0 study, which runs beside.

## Amended by record 55 (2026-10-08)

Wave 6 closed on 2026-10-07 with steps 5 and 7 unbuilt. [55](55-ready-for-production-and-the-paper.md) carries them: step 5 (System 1 compiled into the engine) is replaced by 55 D3, System 1 served as a registered model through the pipeline runner beside the rules' review rule; step 7 (segment on the contracts) by 55 H3 and H4, segmentation as job types of one review mechanism. Beta is no longer tied to step 5 (55 A2).
