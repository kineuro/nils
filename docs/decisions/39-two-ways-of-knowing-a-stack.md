# 39 · Two ways of knowing a stack

Opened 2026-09-23. **Status: accepted 2026-09-23** through [40](40-the-road-through-wave-6.md), which places Phases 0 and 1 inside Wave 6 as waves 41 to 46 and Phase 0 as a study beside them; Phases 2 to 4 run past the Wave 6 gate. In the master plan this is **Wave 6** ([11](11-order.md), [17](17-wave4-reframed.md)): Wave 5 is the desk ([20](20-wave5-the-whole-desk.md)), and Wave 6 already holds the model registry and decisions as labelled datasets (D15, from C7), which is the ground this record builds on. It is carried out as several waves of record 31's kind. Follows [36](36-how-sure-a-rule-is.md), whose method it offers to replace, and 37 and 38, whose facts it reads.

## The ask

Record 36 found that a rule's confidence is a constant set by its tier, so the review thresholds never fire, and planned to fix it by setting 254 numbers by hand. Nima proposed a better way, and a larger aim:

> Nima, 2026-09-23: "I have a better idea that takes nils classification to the next level. It might involve some mathematics and building a model but I think it worth it if we want to publish nils in a high rank journal ... What I was thinking is a **System 1 model**. This can go hand in hand with our deterministic algorithm in nils."

The example he gave is TypeSafe's Jev: typed decisions (a choice, a score, a yes or no) with calibrated probabilities, in one pass, unable to answer outside its schema. Jev is a hosted service and our data may not reach it, so the idea is taken and the model is ours, trained and run on the group's own machines, as the standing rule for the assistant already says of models.

Russell Ouellette ranked ten journals for the work. His warning for the top of the list is the design constraint here: the methodological advance has to remain interesting "even if the words multiple sclerosis and Karolinska are removed".

## What I found before planning

1. **There is almost no ground truth to learn from.** The production host's v0 metadata database classifies 518,353 MR stacks, but every label was written by v0's rules. Its review flag is set on 84 % of stacks and means nothing. Its body-part model learned from 284 hand-labelled slices. There are five classification overrides. So a model cannot simply be trained on "the answers": there are none worth the name.
2. **The archive is diverse enough to matter.** 518,353 MR stacks, 35,161 sessions and 5,319 people, 2001 to 2026, 11 vendors, 84 scanner models, 0.5 T to 7 T. Headers were anonymised before v0 stored them, so every study carries one institution name and a site has to mean a scanner. Image embeddings (BiomedCLIP, SigLIP2) exist for about 11,000 stacks.
3. **The pack is already a typed schema.** 12 axes and 156 values: eight single-valued (technique alone has 38 values), four multi-valued (construct has 53), with exclusion groups and implications between them. About 250 rule-and-axis pairs fired on record 34's corpus.
4. **The engine already has the slot.** A pass is an algorithm the pack configures, and there is one kind today, `nearest_neighbour_vote`. Its rule carries straight over: a pass learns from what a rule or a person decided and never from a pass's output, because the measurement showed that a vote reading its own answers agrees with itself on 14 of 9,014 answers and one that may not on 31,880 of 31,880.
5. **The literature classifies well and says little about how sure it is.**

| Work | What it does | What it leaves out |
|---|---|---|
| DeepDicomSort, Neuroinformatics 2021 | pixel CNN, 8 brain scan types, 98.7 % | metadata, calibration, anything outside its 8 types |
| Gauriau 2020, Liang 2021 | machine learning on DICOM metadata | hand labels; one axis |
| Reid et al., Alzheimer's & Dementia 2025 (conference abstract) | metadata classifier, 4,060 series, 1.5 to 7 T, 99.4 % | semi-automatic labels; "likely unsatisfactory for body MRI" |
| Kujawa et al., Eur Radiol Exp 2026 | five-module hierarchy, 18,181 series | end to end 85.3 %; no calibration or abstention |
| Truong et al. (Bayer), MICCAI 2026 | image and metadata by cross-attention, 82,134 series | drops to 65 to 74 % on some classes at another site; no calibration |
| LLMs on series descriptions, JIIM 2025 | GPT-4o reads descriptions, AUC 0.983 | one text field; hosted models our data cannot reach |
| ezBIDS, Scientific Data 2024 | rule heuristics, propose and revise | no learning, no probabilities |
| Fries et al., Nat Commun 2019 | expert heuristics as weak supervision | cardiac images, not series or metadata |
| Na et al., Diagnostics 2024 | one keyword table labels brain series; a random forest on acquisition parameters learns from it, 98.5 % where the rules were silent | one rule source, no label model, no rule accuracies, no correction where rules fire wrongly |

What I did not find, and two research rounds confirmed with one correction (Na et al. above trained on one rule table): many rules combined by a label model that estimates each rule's accuracy without labels; a certified error rate for what is decided automatically; many axes at once under a declared schema; a loop in which a person's answer improves both the model and the rules; and all of it at the scale of decades and dozens of scanners with a prospective test.

## The idea

Kahneman's two systems, mapped onto NILS:

- **The pack's rules are System 2's written knowledge.** Explicit, deterministic, auditable, and blind outside the cases they were written for.
- **System 1 is a learned model.** It reads every fact of a stack at once, generalises to what no rule names, and says how sure it is.
- **Between them sits the part that makes this publishable: knowing when not to trust either.** Where they agree and the evidence is strong, the stack is decided, with an error rate that is proved rather than hoped. Where they disagree or both are unsure, a person is asked, with a short list of candidates.
- **A person's answer teaches both**: the model retrains on it, and a pattern the model keeps finding against a rule becomes a proposed edit to the pack.

```
   a stack's facts ──┬──> rules (the pack) ──> each rule's vote ──> label model ──┐
   (header, text,    │                        (the witnesses)     (how good is   │
    pixels later)    │                                             each witness?) │
                     └──> System 1 ──> calibrated probabilities ─────────────────┤
                                                                                  v
                                                     arbiter with a certificate
                                                     (error <= 1 % among what it decides)
                                                         │                 │
                                                  decided alone      asked, with a short
                                                                     candidate list
                                                                            │
                                            a person's answer ──> gold ──> retrain System 1
                                                                      └──> propose a rule edit
```

## Design

### 1. The schema is the pack

Each axis becomes an output head of the kind Jev calls it:

| Pack axis kind | Head | Example |
|---|---|---|
| single-valued | a choice: a softmax over the axis's values | technique, 38 values |
| multi-valued | a yes or no per value: independent sigmoids | construct, 53 values |
| two values | a yes or no | post_contrast |
| ordered grade (none yet) | a score | a future motion grade |

The pack's exclusion groups and implications form a constraint set $C$. The answer is the most probable joint assignment that satisfies $C$, so the model cannot return a value the pack does not declare or a combination the pack forbids. The heads are built from a pack version, and a model artifact names the version it was built for; a pack it was not built for is refused, as a contract is.

### 2. The rules become measured witnesses

This is record 36 answered by measurement instead of by hand.

For a stack $x$, rule $j$ on axis $a$ either votes a value or abstains: $\lambda_j(x) \in V_a \cup \{\varnothing\}$. Over $n$ stacks and $m$ rules that is a vote matrix $\Lambda$. A label model, in its simplest form Dawid and Skene's (1979), gives each rule a confusion matrix $\pi_j[u,v] = P(\lambda_j = v \mid y = u)$ and fits them all from the agreements and disagreements between rules, with no labels:

$$P(y = u \mid \Lambda(x)) \propto P(y = u) \prod_{j : \lambda_j(x) \neq \varnothing} \pi_j[u, \lambda_j(x)]$$

Snorkel's multi-task label model and FlyingSquid estimate the same kind of model faster and allow for rules that depend on each other. Two outputs matter:

1. **A probability of each value for each stack**: the training signal for System 1, with no hand labels.
2. **Each rule's estimated accuracy, with an interval**: the table record 36 wanted, filled from the archive. Nima then reviews the outliers, not 254 numbers.

Two honest limits. The model needs roughly independent witnesses: NILS has rules that read text, rules that read physics and flags the parser sets, which are different views, but keyword rules reading the same field are not independent and have to be grouped. And an axis only one rule covers cannot have that rule's accuracy estimated without gold; the gold standard covers it.

The engine needs one change for this: `nils classify` records every rule's vote, not only the first rule of an ordered set that fired.

### 3. System 1: one model, several views, typed heads

What it reads, all per stack:

- **Header facts** from the fingerprint: echo, repetition and inversion times, flip angle, echo train, scanning sequence, variant and options, the `ImageType` tokens, geometry and coverage (record 37), coil, vendor, model and field strength. A missing field is a fact, not a zero.
- **Text**: series description, protocol name and sequence name, as character pieces, so misspellings, Swedish and abbreviations are handled without a dictionary.
- **Pixels, later**: an embedding computed by a pipeline and registered as a derivative, since the binary never decodes pixels (ruling 3). Porting v0's body-part detector is the start of this view.

How it learns: a noise-aware loss against the label model's probabilities, plus the gold labels at a higher weight. Never from a pass's output, including its own.

Which learner is an experiment, not a belief. Phase 0 compares gradient-boosted trees (strong on tabular facts, fast, deterministic to evaluate), a small multi-task network with the typed heads, and TabPFN-2.5 on the gold alone as the strong small-data baseline.

### 4. A certified automation rate

This is the part a reviewer cannot get elsewhere.

1. **Calibrate each head** on a held-out slice of the gold standard (temperature or vector scaling), so that 0.8 means right about 80 % of the time.
2. **Combine the two systems** into one score $s(x)$ per stack, with an arbiter fitted on gold: the rule posterior, the System 1 posterior and whether they agree.
3. **Choose the automation threshold with a guarantee.** Learn then Test (Angelopoulos, Bates, Candès, Jordan and Lei): for thresholds $\lambda_1 > \lambda_2 > \dots$, test "the error among stacks decided at $\lambda_k$ exceeds $\varepsilon$" with an exact binomial p-value on the calibration stacks decided at $\lambda_k$, in that fixed order, stopping at the first failure and deploying the last threshold that passed. Then, with probability at least $1-\delta$, **the error rate among automatically decided stacks is at most $\varepsilon$**, for example 1 % with 95 % confidence.
4. **Ask with a short list.** Everything below the threshold becomes a review item whose candidates are a conformal prediction set, typically two or three values instead of 38.

The certificate is for the whole stack (any axis wrong counts as wrong), because that is what a person releasing data needs; per-axis rates are reported beside it.

> **Warning: the guarantee assumes the stacks it is used on are like the ones it was calibrated on.** A new scanner or a new site breaks that. So every pre-registered group must pass at one shared threshold (an intersection-union test, which needs no split of the confidence budget), a drift test watches for a scanner the calibration never saw, and a new site lowers its own automation until its own gold arrives.

Sample size, so the reference standard has a target: certifying 1 % needs 299 automatically decided calibration stacks with no error among them, 473 with one and 628 with two, per stratum and per certified group, drawn as a random sample of stacks. Plan about 800, which passes 80 % of the time at a true error of 0.25 %.

What this does to record 36: the threshold "below 0.70" becomes a risk the site chooses, "at most 1 % wrong", and the tier constants retire.

### 5. The loop

1. A person's answer to a review item is stored with `author_kind = person` and joins the gold used for **training**. It never joins the **certification** set, because review items are chosen by the system and are therefore not a random sample; that set is drawn at random and kept apart.
2. Review items are already grouped, so one answer labels a group. Groups are asked in order of expected reduction in error: size times uncertainty.
3. Where System 1 is confident and a rule keeps firing against it, the pack gets a question: this rule looks wrong here. The assistant's keyword station could draft the edit; a person accepts it; the pack's version moves. That is how System 1 teaches System 2, and the rules stay the auditable backbone.
4. Every model answer carries `author_kind = model`, its version and its artifact's hash, which is the shape Wave 3 §10.1 reserved and v0's 4,692 unmarked body parts showed was needed.

### Where it lives in NILS

| Part | Where | Note |
|---|---|---|
| rule votes | engine, `nils classify` | every rule's vote, not only the winner's |
| System 1 at run time | engine, a pass kind `model` beside `nearest_neighbour_vote` | evaluated in Rust, deterministic, pinned by hash; writes one evidence row per axis with its probabilities |
| the certificate | engine, the classify report | the risk the site chose and the automation it bought |
| training | a separate tool, run at the site | like `tools/v0-compare`; outputs an artifact and a model card; data never leaves the site |
| pixels | a pipeline | registered derivative; the binary never decodes pixels |
| review | desk | candidates with probabilities and both systems' evidence |

> **Warning: a model trained on free text can remember rare strings, and a series description can hold a name.** Its vocabulary keeps only pieces seen in at least k people and on at least two scanners, and no weights leave the site before a membership-inference test says they are safe.

## The study

**The claim, stated so that it survives removing MS and Karolinska:** a heterogeneous clinical imaging archive can be curated without hand-labelled training data. Deterministic domain rules and a learned model, reconciled statistically, decide most stacks automatically with a certified error rate, and route the rest to people with short candidate lists.

Questions:

1. **Accuracy**, per axis, against the rules alone and the baselines, and separately by vendor, field strength, era and body region, as Russell asks.
2. **Calibration**: expected calibration error and Brier score.
3. **Automation**: the share of stacks decided at a certified 0.5, 1 and 2 %, the error actually realised against the error certified, and people's decisions needed per 1,000 stacks.
4. **Robustness**: trained up to 2018 and tested from 2021; one scanner model left out at a time; external open datasets; a partner site, prospectively.
5. **What each part is worth**: header, text, pixels, the label model, calibration, the certificate against a naive threshold.
6. **What it changes downstream**: BIDS names right (datatype, suffix, entities, `run-`), and cohort questions answered right. Prediction-powered inference (Angelopoulos et al., Science 2023) gives valid archive-wide counts from the model plus the small gold.
7. **People's effort**: time per decision with and without candidate lists.

Data:

| Set | What | Role |
|---|---|---|
| internal | The production host's archive, 518,353 MR stacks | training, internal test with temporal and scanner splits |
| record 34's corpus | 1,003 stacks on the development laptop | development, the awkward cases |
| external, open | candidates: OpenNeuro (the BIDS suffix is a label and the sidecars carry the headers), TCIA collections with DICOM intact, ADNI and others | the external test Russell asked for; to be confirmed by research |
| prospective | partner sites through Bifrost | the independent deployment NBE wants |

**The gold standard**, the one part that cannot be skipped:

- A random sample stratified by scanner group, era and predicted class, rare classes oversampled with known weights so estimates stay unbiased (Horvitz and Thompson).
- About 3,800 internal stacks: 630 to certify the clean stratum, about 900 for the uncertain one, 1,300 for evaluation, 500 to 1,500 for rare values and 150 to audit the cohorts; external stacks on top.
- Two raters, a neuroradiologist and an MR physicist or senior technologist, blind to the system; every certification stack read by both and adjudicated, because the reference's own errors decide whether a certificate can pass; 30 % of the rest read by both.
- Kept apart from the review queue's answers.
- About 6,200 reads: 100 to 180 expert hours at 45 to 90 seconds a read, measured in the pilot.

Baselines: the v1 pack alone; v0's classifier; ezBIDS's heuristics; metadata machine learning trained on the gold alone (in the style of Gauriau and Liang, plus TabPFN-3, since TabPFN-2.5 handles at most 10 classes); an image CNN (DeepDicomSort); and the site's own local LLM reading header and text zero-shot.

## Journal

I recommend one flagship paper to Nature Biomedical Engineering, Russell's first choice, with System 1 and the certificate as the methodological core and NILS as the deployable infrastructure that carries it. Medical Image Analysis is second, leading with the method; Radiology: AI third, if the time-saved study leads. NBE comes after a presubmission enquiry, and only once there is a partner site, a certificate that holds under shift and the effort study. Decided with Russell once Phase 2's numbers exist.

## Phases and gates

| Phase | What | Gate to continue | Rough time |
|---|---|---|---|
| 0 feasibility | v1 digest and classify over the production host's archive, read only; the vote matrix; the label model; a first System 1; 500 gold stacks from one person | at 1 %, rules with System 1 decide at least a fifth more stacks than the rules alone at their own measured error, and the certificate holds on held-out data. If not, I write up what we learned and stop | 3 weeks |
| 1 build | one or two waves: votes, the `model` pass, Rust evaluation, the training tool, calibration and certificate, candidate lists in the desk; tier confidences retire | the gates and the corpus as for every wave | 6 weeks |
| 2 evaluate | the full gold standard, internal splits, the pixel view, ablations | numbers worth a flagship | 6 weeks |
| 3 outside | external datasets, baselines, a partner site, the effort study | the result holds outside | 6 weeks |
| 4 write | with Russell; code, model card, data statement | submission | 4 weeks |

About six months to submission. The gold standard and data access set the pace, not the code.

## Risks

- **Rules wrong in the same way fool the label model.** The normalizer that deleted "brain" made every body-part rule silent at once. Views the rules do not read (physics, pixels) are the defence, and the gold standard measures whether it worked.
- **The guarantee breaks under shift.** Scanner groups, the drift test and per-site gold, as above.
- **Gold is expensive**, and it is the one thing that cannot be replaced by cleverness.
- **Privacy of the weights**, as above.
- **A model changes answers between versions.** The pass is versioned and hash-pinned, and a new version is run on purpose, never silently.
- **Novelty.** If the search finds weak supervision already applied to series classification, the contribution moves to the certificate and the loop, which I expect to stand either way.
- **Scope.** The CT pack, federation and an LLM as a slower third reader of hard cases are later.

## What the research changed

Nine reports came back on 2026-09-23; they and my reading of them are in `studies/nils-system-1/`. Ten things change or firm up.

1. **The novelty holds, in narrower form (round 2).** Na et al. (*Diagnostics* 2024) trained a series classifier on one keyword table's labels. New: many rules combined by a label model that estimates each rule's accuracy; a high-probability bound on the error among automatically curated series; a loop that improves model and rules together. The certificate's method has neighbours the paper cites: Gamble et al. (*Radiology: AI* 2025), Mulford et al. (*J Arthroplasty* 2025), per-field selective risk for document extraction (arXiv 2026), Trust or Escalate (ICLR 2025) and PAC labels (2025).
2. **Every rule's vote is recorded.** First-match lists hide the overlaps a label model needs; without them no rule's accuracy can be estimated. This was already S1 of Phase 1; it is now the first engine change.
3. **NILS rules are simpler witnesses than the literature assumes, and harder in one way.** Each votes one value, which cuts the parameters to a firing propensity per value; but some read axes decided earlier, which no standard label model handles. That is question 10 below.
4. **One loss, one calibration.** "Any axis wrong" is a single loss, calibrated once by Learn then Test with exact binomial tests; no correction across the 12 axes is needed. 299 error-free decided calibration stacks certify 1 % in a group.
5. **Sample stacks, never whole sessions (round 2).** Certificates fail silently when correlated items are tested as independent: labelling whole sessions and testing per stack certified a true 1.2 % error 19 % of the time in simulation. A random sample of stacks makes clustering irrelevant for the archive claim; a claim about future patients needs the patient as the unit.
6. **Groups share one threshold (round 2).** The group-conditional conformal methods give coverage, not the error among accepted stacks, so they serve the candidate lists. Groups are certified by an intersection-union test at one shared threshold: each pre-registered group needs its own random sample of about 300 to 800 accepted stacks, so only large groups are certified.
7. **The constraint layer is exact.** Enumerate the pack's valid joint assignments per constraint component, renormalise the model over them, train with that likelihood, calibrate after it, and draw conformal candidate sets from valid assignments only. The pack loader also learns to report any value no valid assignment can reach.
8. **Simple learners first.** Gradient-boosted trees per head against a small multi-task network, early stopping on the reference set, and confident learning to surface rules the model keeps contradicting. The heavy stack one report proposed (a pretrained biomedical language model among it) is rejected: wrong language and a privacy liability.
9. **The weights may be personal data.** Under EDPB Opinion 28/2024 a model trained on free text can be. Before any weights leave: a vocabulary threshold, a canary exposure test, a membership-inference test at a very low false-positive rate, and sign-off from the data protection officer and the ethics board. The plan for the paper is open code, a model trained on open data only, and the Karolinska model released only if it passes.
10. **What reviewers will ask for is now a list.** Results by vendor, field strength and era; a temporal split; reliability diagrams; a measured deferral rate and measured time savings; rules against model against both; CLAIM 2024 and DECIDE-AI; "reference standard", not "ground truth", in the paper. Nature BME first, Medical Image Analysis second, Radiology: AI third.

Two things are measured rather than researched: how often OpenNeuro sidecars carry the fields we read, and what TCIA's MR collections keep in their series metadata. Spine Generic is the first external set.

## What the archive already knows

Nima, 2026-09-23: "we have some data already in place (like the study G cohort or study D) which we have full protocol details of scans and we can be 100 percent sure which stack is which. Some other classifications in v0 are also 100 % accurate as they have no low confidence, conflict or missing on any axes (including SWIs, EPIMix, STAGE, SyMRI and more simple ones like MP2RAGE and so on), where we do not need human confirmation."

Measured on the production host's v0 database the same day, counts only:

- **394,801 of 518,353 MR stacks (76 %) are clean on the core axes**: no low confidence, conflict or missing value on base, technique, orientation, modifier, construct or provenance, and not excluded. An axis that does not apply stays empty and is not a flag: SyMRI's raw MDME images have no base contrast, and only 77 of its 36,692 stacks carry any core flag. v0 flagged body part and post-contrast status almost everywhere, which is why only 2,332 stacks carry no flag at all; those two axes are not in the clean test.
- **By technique**: MP2RAGE 5,170 of 5,170 clean, EPIMix 437 of 437, MPRAGE 39,160 of 39,250, SPACE 40,330 of 40,412, FIESTA 9,253 of 9,259, MDME 36,504 of 36,592. What is not clean is mostly the 79,742 secondary reformats v0 excluded, and flagged TSE, GRE and HASTE.
- **The protocol-known cohorts**: study G spans 1,760 sessions and 32,425 stacks of its 143 people (26,615 clean), study D 203 sessions and 4,388 stacks (3,990 clean). Only their research-protocol sessions carry exact labels; which sessions those are is Nima's to say.

What this changes:

1. **Two strata, two certificates.** In the clean stratum the policy is "accept the rules", and a random sample of it certifies that policy directly: about 300 clean stacks with no error certify 1 %. System 1 and the referee then have to earn their own certificate only on the remaining quarter. The expert labelling shrinks accordingly.
2. **"100 % accurate" becomes a measured result.** In the protocol-known cohorts, v0's clean labels are compared with the protocol at scale, and a small random sample checks the clean stratum outside them.
3. **The protocol cohorts are exact but not random.** They anchor the label model, train System 1 and test it, but they cannot calibrate the certificate for the whole archive; at most they form a group of their own.
4. **System 1 still earns its place inside the clean stratum.** A clean stack it confidently disagrees with is the confidently-wrong case, like the phase-contrast study once called anatomical T1w 297 times without a flag. Those are the stacks a person should see first.

### What the protocols measured

Nima's five Siemens Prisma printouts (study B site B1, study C, study D, study E, and study E before 2020-12-28) are in `studies/protocols-docs/`; the check is in `studies/2026-09-23-protocol-check/`. Study G splits by its identifier's prefix into site B1 (67 people), study C (36) and study F (40, no printout). 23,053 stacks lie in sessions that match a printout.

> **Warning: "clean" is not 100 %.** Of 17,264 v0-clean protocol stacks that could be compared, 3,063 (17.7 %) carry a confirmed wrong label. 2,916 come from one rule: technique RESOLVE from segmented k-space plus EPI, but Siemens writes SK on every EPI, and only a `re_b` sequence is readout-segmented. Across the archive, 21,619 clean stacks are RESOLVE with a single-shot sequence name, about 5.5 % of the clean stratum, and v1's pack keeps the same rule. The rest: FlowComp read from "0 flow comp" in a name (87), MEDIC called GRASE (24), brain and spine taken from the exam (42).

So item 1 stands only with its certificate: the clean stratum is certified, never assumed, and the protocol stacks are development data from now on ([40](40-the-road-through-wave-6.md) R2, R3). Construct, provenance, localizer and directory type agreed everywhere, and base in 99.8 %. Human QC is smaller than believed: on the legacy storage server only 610 body parts were set by a person (study B), and no classification axis has a per-stack human decision there. Study B itself (7 sites, three vendors, 49,650 stacks) is the variety the leave-one-site-out test needs.

### A contrastive System One: CLM (2026-09-24)

Nima pointed to Contrastive-LM's CLM (github.com/Contrastive-LM/CLM, `Contrastive-LM/CLM-v0.1-8B`).
- **What it is:** a frozen Qwen3-8B encoder with a 20M-parameter projection head, Apache 2.0 for code and weights. It is trained with a bidirectional contrastive loss between states and actions.
- **What it answers:** the same typed questions as Jev (yes/no, choice, score) in one forward pass, through a `/v1/systemone` API. It takes about 28 ms per new state on a 4090.
- **Tuning:** its heads can be fine-tuned on one's own labels with the encoder frozen.
- **What it is not:** its probabilities are a softmax over similarities with a temperature, not calibrated. It is v0.1, text only, and needs an NVIDIA GPU. Its reported results are agent benchmarks, not classification of scans.

It is the concrete form of two ideas this record left as comparison arms: Nima's "a light language model without its generative part, tuned for our terms", and the Jev-style reader. Its trained part is a small head, a far smaller privacy surface than a fine-tuned language model.

**Its place.** A Phase 0 arm, run on the production host's own GPUs so that nothing leaves the production host:
1. **Zero-shot.** One choice question per axis, with the pack's values and their descriptions as the options, and the stack's header summary and allowlisted words as the state. Scored against the protocol truth and against the label model.
2. **Tuned heads.** The heads are trained on the label model's posteriors, never on a sealed sample, and compared with LightGBM per axis.
3. **Calibration.** Temperature per axis on the development split. It enters System 1 only through the same certificate as everything else.

If it earns a place, it runs as a pipeline whose scores are stored as derivatives pinned by digest. System 1's model pass reads them, and the engine's own path stays deterministic. Kvasir's batch door (record 47, R9) can serve it the same way.

## What the second round settled

Seventeen reports on the rewritten questions, read in full by seven reviews that checked citations at primary sources, recomputed the numbers and tested the engineering claims in code. The detail is in `studies/nils-system-1/README.md`; the decisions are these.

1. **The label model** is a restricted latent class model over the pack's valid assignments: two parameters per clause (it fires when its value is true; it fires when it is false); a clause whose field is empty, whose condition on another axis is false, or which a more specific clause shadows contributes nothing; 56 clauses that only restate another axis become schema implications; three families of clauses (free text, coded headers, physics), each with a per-stack state for errors the family shares; separate class priors for the archive and each cohort. Cohort stacks are clamped, expert labels enter by cross-fitting, certification stacks never. **The clean 76 % never enters the fit and never validates it.** MAP-EM over deduplicated vote patterns, our own code of about 500 lines. A value needs clauses from two families to be identifiable; with one, its precision is estimable and its recall is not.
2. **The certificate** is one fixed sequence of exact binomial tests on a random sample of stacks: the clean stratum's "accept the rules" first, then the uncertain stratum's thresholds from strict to loose, stopping at the first failure. Every pre-registered group must pass at one shared threshold. The protocol cohorts never calibrate the rest of the archive. The guarantee: with probability at least 0.95 over the choice of certification stacks, at most 1 % of automatically decided stacks in this archive have any of the 12 variables wrong, in each stratum and in every certified group.
3. **The model** is LightGBM, one per head, first; a small multi-task network second; TabPFN-3 as an offline baseline. Targets in precedence: a person's answer, a cohort label, v0's clean label, the label-model posterior. The random reference sample is never training data. Text comes from a word allowlist (at least 10 people, 2 scanner models) screened by two people for staff names, with digits kept and no hashing. **The check for confidently wrong clean stacks uses a model that never saw those labels**: the physics-only model, or one trained without that scanner group.
4. **The evaluation** labels about 3,800 internal stacks, every certification stack read twice, blind and adjudicated: about 100 to 180 expert hours, measured in the pilot. A time-saved study (8 curators, crossover, about 17 curator-hours) is itself new: nobody has timed series curation.
5. **The engine** evaluates the trees in its own code, bit-for-bit against LightGBM, with a pinned pure-Rust `exp` and `log`, a canonical artifact named by its digest, golden vectors, and a CI proof on all six targets. The site's model is never compiled into the public binary.
6. **Privacy**: training rests on GDPR Art. 6(1)(e) and 9(2)(j) with ethics approval as the safeguard; a DPIA is likely required. The paper releases the code and a model trained on open data; the Karolinska model is released only after an audit (allowlist, at least k people per leaf, canaries, person-level membership attacks) and sign-off by the DPO, the ethics board and KI's legal office.
7. **The journal**: Nature BME after a presubmission enquiry, once there is a partner site, a certificate that holds under shift and the effort study; Medical Image Analysis second; Radiology: AI third.

Left open: three narrow research questions (Q1 to Q3 in the appendix), three questions for KI's data protection officer and legal office (does the approval cover this work; is a model file a disclosure under OSL; may partners receive it), and the measurements of Phase 0.

## What this record does not settle

Nima's decisions, listed in the reply that opened this record: the scope of the first paper, what wins when the two systems disagree, who labels the gold and under which ethics approval, where training runs and whether weights may leave the site, one paper or two, and whether record 36 retires into this.

## References

- Dawid and Skene, Maximum likelihood estimation of observer error-rates using the EM algorithm, JRSS C, 1979.
- Blum and Mitchell, Combining labeled and unlabeled data with co-training, COLT 1998.
- Ratner et al., Snorkel, VLDB 2018; Training complex models with multi-task weak supervision, AAAI 2019.
- Fu et al., Fast and three-rious: speeding up weak supervision with triplet methods (FlyingSquid), ICML 2020.
- Fries et al., Weakly supervised classification of aortic valve malformations using unlabeled cardiac MRI sequences, Nature Communications 2019.
- Guo et al., On calibration of modern neural networks, ICML 2017.
- Angelopoulos, Bates, Candès, Jordan and Lei, Learn then Test: calibrating predictive algorithms to achieve risk control, 2021.
- Angelopoulos, Bates, Fisch, Lei and Schuster, Conformal risk control, ICLR 2024.
- Angelopoulos et al., Prediction-powered inference, Science 2023.
- Tibshirani, Barber, Candès and Ramdas, Conformal prediction under covariate shift, NeurIPS 2019.
- Hollmann et al., Accurate predictions on small data with a tabular foundation model, Nature 2025.
- van der Voort, Smits and Klein, DeepDicomSort, Neuroinformatics 2021.
- Kujawa et al., Automated identification of MRI series using a hierarchical modular machine-learning pipeline, European Radiology Experimental 2026.
- Truong, Dohmen, Lorio and Lenga, Revisiting integration of image and metadata for DICOM series classification, arXiv 2602.23833, 2026.
- Reid et al., Automatic classification of head MR series, Alzheimer's & Dementia 2025.
- Standardizing heterogeneous MRI series description metadata using large language models, Journal of Imaging Informatics in Medicine 2025.
- Levitas et al., ezBIDS, Scientific Data 2024.

## Appendix: the research asked for

Nine questions went to deep research on 2026-09-23, each written to stand alone. The answers decide the open choices above; they are recorded here when they come back.

### 1. Prior art and novelty

I am preparing a research paper and need a rigorous prior-art search.

Setting: automated classification of clinical MRI (and later CT) image series from their DICOM metadata, and optionally their pixels, into categories such as contrast weighting (T1w, T2w, FLAIR, DWI, SWI), sequence technique (MPRAGE, TSE, GRE, EPI), body region, slice orientation, post-contrast status and derived products (ADC maps, MIPs, reformats). This is what is needed to organise hospital imaging archives or convert them to the BIDS standard. The metadata includes SeriesDescription, ProtocolName, ImageType, ScanningSequence, SequenceVariant, EchoTime, RepetitionTime, InversionTime, FlipAngle and geometry.

Our planned method has three parts:
(a) treat an existing set of about 250 hand-written deterministic rules as noisy labelling functions and combine them with a weak-supervision label model (Dawid-Skene, Snorkel data programming or similar), so that no hand labels are needed for training;
(b) train a learned classifier on the result;
(c) decide automatically only where the error rate is certified by conformal prediction, Learn then Test or conformal risk control, and route the rest to human review with small candidate sets.

Questions:
1. Has anyone applied weak supervision, data programming or label models to DICOM series or MRI sequence classification, or to BIDS conversion?
2. Has anyone given calibrated probabilities, abstention, or statistically certified error rates for automated imaging-archive curation or series classification, as opposed to diagnostic tasks?
3. What are the strongest works from 2019 to 2026 on MRI series or sequence classification (metadata, pixels, both, or LLM-based)? For each: dataset size, number of sites and vendors, label set, external validation and reported accuracy.
4. Is there work that combines deterministic rules with a learned model and a human-in-the-loop that improves both?

Output: a table of papers (full citation, venue, year, DOI or link, data size and sites, inputs used, labels, method, uncertainty handling, external validation, headline result). Then a short assessment of whether each of (a), (b) and (c) is novel, partly done or already done, naming the closest prior work for each. Prefer peer-reviewed work and mark preprints.

### 2. Label models for correlated, ordered rules

I need a technical review of weak-supervision label models for a specific setting.

We have about 250 deterministic rules that label MRI image stacks (about 500,000 unlabelled items). Each rule reads one view of an item (free-text fields, numeric physics parameters, or structured flags). It either votes a class for one of about 12 categorical target variables, or abstains. Some targets are single-label with up to 38 classes; others are multi-label with up to 53 binary tags.

The rules are not independent. Many keyword rules read the same text field. Many rule sets are ordered first-match lists, where the first rule that fires wins. Later we will have a few thousand expert-labelled items.

Questions:
1. Which label models suit this: Dawid-Skene with EM, Snorkel's matrix-completion and multi-task label model (MeTaL), FlyingSquid (triplet methods), and newer models from 2021 to 2026? Compare their assumptions and how each handles abstentions, class imbalance, multi-class and multi-label targets, dependencies, and scaling to 500,000 items.
2. Identifiability: under which conditions can each rule's accuracy or confusion matrix be estimated without labels? What should be done when a class is covered by only one or two rules?
3. How should we model dependencies between rules that read the same field, and first-match ordered rule lists? Should we record every rule's vote rather than only the winner's?
4. How can a small labelled set validate or anchor the estimates (semi-supervised label models)?
5. How should each rule's estimated accuracy be reported with a confidence interval?
6. Which open-source implementations are maintained and recommended?

Output: a comparison table, a recommended approach with reasons, pitfalls, and key references with links.

### 3. Certified selective automation

I need a method recommendation for statistically certified automation of a classification task.

A classifier assigns each item (an MRI image stack) values on about 12 categorical target variables. We want to decide items automatically only when we can guarantee, with finite-sample validity, that the error rate among the items decided automatically is at most epsilon (for example 1 %), with probability at least 1 - delta (for example 95 %). The rest go to human reviewers, each with a small set of candidate answers.

The data come from many scanners (84 models, 11 vendors) over 26 years, and new scanners and sites keep appearing. We will have a randomly sampled, expert-labelled calibration set of a few thousand items. It is kept separate from human review decisions, which the system selects and which are therefore not a random sample.

Questions:
1. Compare Learn then Test (Angelopoulos, Bates, Candes, Jordan, Lei), conformal risk control, selective conformal risk control and related methods for this selective-risk guarantee. Cover the exact statement of each guarantee, its assumptions, the choice of p-values (binomial, Hoeffding-Bentkus) and multiple testing over thresholds.
2. How can we guarantee (a) per-variable error, (b) whole-item error (any variable wrong), and (c) error per scanner group (Mondrian or class-conditional conformal)?
3. What should we do under distribution shift: covariate shift (weighted conformal), temporal drift (adaptive conformal inference), detecting violations of exchangeability (conformal test martingales), and new sites with no labels?
4. How many calibration samples are needed, per group, for epsilon between 0.5 % and 2 % at delta = 5 %?
5. How do we build small prediction sets for reviewers (APS, RAPS)?
6. Are there worked examples in medical imaging or data curation, with open-source code?

Output: a recommended method as a step-by-step procedure, the guarantee stated in one sentence, a table of sample sizes, pitfalls, and references with links.

### 4. Training on probabilistic labels across several views

I need best-practice advice for training and calibrating a classifier from weak labels.

We will train a multi-task classifier for MRI image stacks with about 12 categorical outputs: single-label softmax heads with up to 38 classes, and multi-label sigmoid heads with up to 53 tags. It reads three views:
- structured DICOM header fields: numeric physics parameters and categorical codes, often missing;
- short noisy free text: series description and protocol name, multilingual, with abbreviations and misspellings;
- optionally, an image embedding.

The training labels are the probabilistic outputs of a weak-supervision label model built from about 250 deterministic rules (not hand labels), plus a few thousand expert labels. There are about 500,000 items, heavy class imbalance and rare classes.

Questions:
1. What is best practice for training on probabilistic (soft) labels? Cover noise-aware losses, soft cross-entropy, how to weight gold labels against weak ones, and co-training or multi-view theory when the rules and the model read different views.
2. Can the final model generalise beyond the rules' coverage and correct errors that several rules make together? What is the evidence, and under which conditions?
3. How should a model trained on weak labels be calibrated: temperature, vector or Dirichlet scaling, per head, and per subgroup? How many gold labels are needed to measure expected calibration error reliably?
4. Which architecture fits mixed tabular data, short text and an optional embedding with missing fields? Compare gradient-boosted trees (LightGBM, CatBoost) with text n-grams, multi-task neural networks with missingness-aware encoders, and tabular foundation models (TabPFN-2.5, TabICL) on accuracy, calibration, speed and determinism.
5. For active learning, which items should go to experts to reduce error fastest? Include batch or cluster labelling, where one decision labels many identical items.

Output: recommendations with justification, likely pitfalls, key references with links, and a suggested experimental comparison.

### 5. Structured outputs under logical constraints

I need a review of methods for keeping a multi-output classifier's predictions logically consistent.

Our classifier predicts about 12 categorical variables per item: 8 single-label and 4 multi-label, with 156 values in total. A declared schema imposes constraints between them:
- mutually exclusive groups inside the multi-label variables;
- implications (for example, the technique MPRAGE implies T1-weighted contrast and a 3D acquisition);
- forbidden combinations.

The outputs must never violate the schema, and the probabilities must stay calibrated.

Questions:
1. Which methods enforce such constraints during training, inference or both? Cover semantic loss, probabilistic circuits, constrained MAP decoding, CRF-like structured prediction and post-hoc projection.
2. How does applying the constraints affect calibration and conformal prediction sets?
3. What are the practical, scalable approaches for a joint space this small (12 variables, 156 values) with fast CPU inference?
4. Which open-source tools exist?

Output: a comparison, a recommendation, pitfalls, and references with links.

### 6. Public datasets for external validation

I need a survey of public imaging datasets suitable for externally validating a classifier.

The classifier labels MRI (and later CT) series by:
- contrast weighting and sequence technique;
- body region and slice orientation;
- post-contrast status and derived products.

It reads DICOM header metadata, and optionally the pixels. The headers used include SeriesDescription, ProtocolName, ImageType, ScanningSequence, SequenceVariant, EchoTime, RepetitionTime, InversionTime, FlipAngle, Manufacturer, ManufacturerModelName, MagneticFieldStrength and geometry.

The ideal data would be:
- multi-vendor, multi-site and multi-year;
- brain and spine (relevant to multiple sclerosis), plus some body MRI;
- either original DICOM with headers intact, or BIDS datasets whose JSON sidecars (written by dcm2niix) keep those fields and whose BIDS suffix provides a label.

Questions:
1. Which public datasets fit? Candidates include:
   - OpenNeuro: how often do its sidecars contain SeriesDescription, ProtocolName, ImageType, ScanningSequence and the scanner model?
   - TCIA collections with intact DICOM headers: which collections, modalities, vendors and sizes?
   - ADNI, PPMI, OASIS-3, UK Biobank, the DICOM parts of fastMRI, and ABIDE.
   - Multiple sclerosis datasets such as MSSEG, spine datasets, and photon-counting CT data.
2. For each dataset give:
   - size (series and subjects), and vendors, sites and years;
   - what labels exist and how trustworthy they are, and how complete the headers are;
   - the access procedure and typical approval time;
   - the licence, and whether results and derived models may be published.
3. What are the known pitfalls? For example, de-identification that removes descriptions, and curated subsets biased towards clean data.

Output: a ranked table with these columns and links, plus a recommended set of three to five datasets for an external test.

### 7. Privacy of models trained on free-text DICOM fields

I need a privacy risk assessment for releasing a small classifier trained on hospital data.

We will train a classifier on hospital MRI metadata. It includes free-text DICOM fields (SeriesDescription, ProtocolName, sometimes comments) that occasionally contain personal names, clinical notes or other identifiers. The training data never leave the hospital. We may want to publish the model weights and code with a paper, and share the model with partner hospitals.

Questions:
1. What are the memorisation and membership-inference risks for small classifiers trained on such text? This covers gradient-boosted trees, and small neural networks with character n-gram text features. What is the evidence, and how should the risk be measured (canaries, membership-inference attacks)?
2. Which mitigations work, and what accuracy do they cost? Candidates include:
   - vocabulary frequency thresholds, for example keeping only tokens seen in at least k patients and on at least two scanners;
   - hashing;
   - differential privacy (DP-SGD, DP for tree ensembles).
3. What do GDPR (EU and Sweden) and ethics boards typically require before a model trained on patient-derived data is released? Are there precedents in medical imaging?
4. What do top journals (the Nature family, Radiology: AI, Medical Image Analysis) require for code, weights and data availability in such work?

Output: a risk assessment, recommended safeguards with references and links, and a checklist to pass before weights may leave the hospital.

### 8. What top journals required

I need a benchmark of what top journals expected from comparable papers.

We are planning a paper on a system that curates heterogeneous clinical MRI archives (hundreds of thousands of image series, 26 years, 84 scanner models) into standardised datasets ready for analysis. Its core is a new classification method: deterministic rules plus a learned, calibrated model. Automated decisions carry a certified error rate, and the rest go to human review.

Candidate journals: Nature Biomedical Engineering, Nature Methods, Radiology: Artificial Intelligence, npj Digital Medicine, Medical Image Analysis, IEEE Transactions on Medical Imaging, Patterns and JAMIA.

Questions:
1. Find papers from 2022 to 2026 in these journals on imaging data curation, DICOM or BIDS standardisation, series or sequence classification, or large clinical-imaging infrastructure. For each, what validation did they report? Look for:
   - external sites;
   - prospective deployment;
   - reader studies;
   - time savings;
   - subgroup analyses by vendor or field strength;
   - calibration.
2. What do reviewers and editors of these journals commonly demand for such papers? Use author guidelines, editorials, and published peer-review files where they exist (for example, Nature's transparent peer review).
3. Which reporting guidelines are likely to apply: CLAIM 2024, TRIPOD+AI, STARD-AI, DECIDE-AI?
4. How do Nature Biomedical Engineering and Nature Methods differ in what they expect from tools and resources?

Output: a table of comparable papers with links and their validation elements, a list of must-have evaluation elements, and a recommended order of journals with reasons.

### 9. Deterministic machine-learning inference in Rust

I need an engineering recommendation for embedding a trained classifier in a Rust application with bit-exact outputs.

We ship a Rust application for Linux, macOS and Windows, on x86-64 and arm64. It must give bit-for-bit identical outputs for the same input on every platform, because outputs are hashed and compared in release gates.

We want to embed a small classifier, trained in Python, and run it on the CPU inside this binary. It would be either a gradient-boosted tree ensemble (LightGBM, XGBoost or CatBoost) or a small neural network (a few MLP layers, perhaps a character-level text encoder). It has about 12 output heads and runs on up to millions of items.

Questions:
1. What are the options, and how do they compare on maturity, binary size, dependencies and speed? Candidates include pure-Rust tree-ensemble evaluation (trees exported to JSON), ONNX Runtime (the ort crate), candle, tract and burn.
2. How does floating-point determinism break across CPUs and operating systems (fused multiply-add, SIMD, threading, summation order, libm differences)? How can identical results be guaranteed: a fixed evaluation order, integer or fixed-point quantisation, disabling FMA, deterministic reductions, or rounding outputs before hashing?
3. How do tree ensembles and quantised MLPs compare on determinism?
4. How should model artifacts be versioned and pinned by hash, and verified when loaded?
5. Which projects have achieved deterministic machine-learning inference across platforms?

Output: a recommendation with reasons, a list of determinism pitfalls with countermeasures, and links.

### Asked after the first round

Three questions arose from reading the first nine, sent on 2026-09-23.

### 10. Weak supervision over a structured label space

I need a technical review of weak supervision (programmatic labelling with label models) for a structured, multi-output label space.

Setting: we label about 500,000 MRI image stacks on 12 categorical target variables: 8 single-label (up to 38 classes) and 4 multi-label (up to 53 tags), 156 values in total. A declared schema constrains the joint output with mutually exclusive groups, implications (for example, the technique MPRAGE implies T1-weighted contrast and a 3D acquisition) and forbidden combinations. We can enumerate the valid joint assignments within each connected component of the constraint graph.

About 250 deterministic rules act as labelling functions. They have three properties that standard label models do not assume:
1. Most rules are unipolar: each can only vote one specific value of one variable, or abstain.
2. Some rules are conditioned on other variables: they fire only when another variable already holds a given value (for example, a rule for "folder type = anatomical" that requires "contrast = T1-weighted"), so the rules form a cascade across variables.
3. Many rules read the same input field (keyword rules on one free-text description), so their errors are correlated; others read different views (numeric physics parameters, structured DICOM flags).

We will record every rule's vote on every item, with no first-match censoring. Later we will have a few thousand expert-labelled items.

Questions:
1. Which label models support structured or multi-task label spaces with logical constraints? Cover, for example, Snorkel MeTaL (multi-task, hierarchical), Universalizing Weak Supervision (Shin et al., ICLR 2022), label models over label-powerset or factor-graph spaces, and newer work from 2021 to 2026. How is the latent variable defined when outputs are joint assignments restricted to a valid set?
2. How should unipolar labelling functions be parameterised (for example, per-rule class-conditional firing propensities rather than full confusion matrices), and what are the identifiability conditions in that case?
3. How should rules that condition on other target variables be modelled, so that cascaded dependencies are neither double-counted nor treated as independent evidence?
4. How should correlated rules that read the same field be grouped or modelled (dependency-structure learning, meta-rules), and how can we test whether conditional independence is violated?
5. How can a few thousand labelled items anchor the model, including rare values covered by only one or two rules, and how should each rule's accuracy be reported with an interval?
6. Practically: which implementations can be adapted, or is a custom EM over the valid assignment space advisable, and what does it cost at 500,000 items?

Output: a recommended model with its likelihood written out, identifiability conditions, an estimation algorithm, pitfalls, and references with links. Prefer peer-reviewed work and mark preprints.

### 11. Certificates for overlapping groups with clustered data

I need a method recommendation for statistically certified selective automation when the guarantee must hold for several overlapping subgroups and the data are clustered.

Setting: a classifier labels MRI image stacks. A stack is decided automatically only if its score exceeds a threshold; otherwise a person reviews it. We want the following. With probability at least 1 - delta (for example 95 %), the error rate among automatically decided stacks is at most epsilon (for example 1 %). An error means any of the stack's 12 output variables is wrong: one whole-stack 0-1 loss.

The archive spans 11 vendors, 84 scanner models, field strengths from 0.5 to 7 T and 26 years. We want the guarantee overall and also for each of several overlapping groups (vendor, field strength, era, body region). We do not want to partition into every combination, because expert labels cost time.

The data are clustered: several stacks come from one session, and several sessions from one patient, so labelled stacks are not independent. The expert-labelled calibration set will be a sample from the archive. For other purposes we may oversample rare classes, with known inclusion probabilities.

Questions:
1. How can selective risk (the error among accepted items) be controlled simultaneously over overlapping groups? Can these be used or adapted for selective risk rather than coverage, and what exact guarantee results?
   - Conformal Prediction with Conditional Guarantees (Gibbs, Cherian, Candes, JRSS-B 2025)
   - Batch Multivalid Conformal Prediction (Jung, Noarov, Ramalingam, Roth, ICLR 2023)
   - multicalibration
   - Learn then Test with group-wise hypotheses
2. How should we correct for clustering of stacks within sessions and patients? Options include cluster-level sampling, design-effect corrections, the cluster bootstrap, or document-iid certificates as in "Valid Per-Field Selective Risk Control for Document Extraction" (Gurram, arXiv 2608.14639, 2026).
3. How can a calibration sample drawn with unequal but known inclusion probabilities (stratified or oversampled) still give a valid finite-sample guarantee, for example by weighted Learn then Test or importance-weighted conformal methods?
4. Sample-size planning: how many labelled stacks, and how many sessions, are needed per group for epsilon of 1 % and 2 % at delta = 5 %? Compare overlapping groups with clustering against partitioning into disjoint groups.
5. What should happen to a group with too few labels (defer everything, borrow strength hierarchically, or use a conservative pooled bound), and how should a new scanner model be added over time?

Output: a recommended procedure step by step, the exact guarantee in one sentence, a sample-size table, pitfalls, and references with links. Prefer peer-reviewed work and mark preprints.

### 12. Evaluation design and measuring time saved

I need a study design for evaluating an automated curation system for a large clinical MRI archive with as few expert labels as possible, and for measuring the time it saves people.

Setting: a system assigns each MRI image stack values on 12 categorical variables (for example contrast weighting, sequence technique, body region, orientation and post-contrast status). It either decides a stack automatically or sends it to a person with a short list of candidate answers.

The archive holds about 518,000 stacks from 35,000 sessions and 5,300 patients, over 26 years, 84 scanner models and 11 vendors. There is no existing reference standard. Experts (a neuroradiologist, and an MR physicist or senior technologist) will label a sample.

We must report, with valid confidence intervals:
- accuracy per variable and per value, including rare values, by vendor, field strength and era;
- calibration;
- the automation rate;
- the time saved compared with manual curation.

The paper will follow CLAIM 2024 and DECIDE-AI.

Questions:
1. Sampling design for the reference standard. Cover stratified sampling with known inclusion probabilities and Horvitz-Thompson estimators, active testing (Kossen et al., ICML 2021), prediction-powered inference (Angelopoulos et al., Science 2023) and related methods. How can accuracy per value and per subgroup, including rare values, be estimated without bias while minimising labels? Can one sample also serve as the calibration set for a conformal certificate, or should they be separate?
2. Sample sizes. How many labelled stacks give a chosen confidence-interval width on accuracy and macro-F1, per variable, per subgroup and for rare values? How should clustering of stacks within sessions and patients be accounted for?
3. Labelling protocol. Cover double reading and adjudication; agreement statistics for multi-variable categorical labels (Cohen's or Fleiss' kappa, Krippendorff's alpha); blinding readers to the system's output; and what CLAIM 2024 asks about the reference standard.
4. Measuring time saved. Which designs suit a human-effort study (within-subject crossover, randomised order, washout, learning effects)? What should be measured: time per decision, accuracy with and without candidate lists, and automation bias? What sample sizes are needed? Give examples of such studies in radiology informatics or data curation, and state what DECIDE-AI requires.
5. How can a decision-curve or cost-benefit analysis choose the automation threshold?

Output: a recommended design with formulas or procedures, sample-size tables, a checklist mapped to CLAIM 2024 and DECIDE-AI items, and references with links.

## Appendix: the redo

Nima is running the research again, except question 5, whose report was sound. These nine replace questions 1 to 4 and 6 to 12: they merge where the first round overlapped, carry the corrections above and the two strata, and require a primary source for every claim.

### R1. Prior art and novelty, verified

I am preparing a research paper and need a rigorous, verified prior-art search.

Topic: automated classification of clinical MRI (and later CT) image series or stacks from DICOM metadata, and optionally pixels, into sequence categories. The categories cover:
- contrast weighting (T1w, T2w, FLAIR, DWI, SWI);
- technique (MPRAGE, TSE, GRE, EPI);
- body region and orientation;
- post-contrast status;
- derived products (ADC maps, MIPs, reformats, synthetic MRI).
This is what is needed to organise hospital archives or convert them to BIDS.

Our method has four parts:
(a) about 250 hand-written deterministic rules are treated as noisy labelling functions and combined by a weak-supervision label model, so no hand labels are needed to train;
(b) a learned classifier is trained on the result plus exact labels from research cohorts with documented protocols;
(c) a stack is decided automatically only where a finite-sample statistical certificate (Learn then Test, conformal risk control) bounds the error rate among decided items; the rest go to people with small candidate sets;
(d) a loop in which people's answers retrain the model and flag rules for correction.

Questions:
1. Has anyone applied weak supervision, data programming or label models to DICOM series or sequence classification, to BIDS conversion, or to imaging-archive curation? Search specifically, and report negative results as such.
2. Has anyone given calibrated probabilities, abstention or certified error rates for automated curation of imaging data (not diagnosis)? Include conformal pipelines for registries or curation, for example:
   - an uncertainty-aware knee-radiograph registry pipeline (Journal of Arthroplasty, 2024);
   - per-field selective risk control for document extraction (arXiv 2608.14639, 2026).
3. Verify and complete this list of MRI series and sequence classification works. For each give: citation, venue, year, DOI, data size, sites and vendors, inputs, label set, external validation, uncertainty handling and headline result.
   - DeepDicomSort (Neuroinformatics 2021); Pizarro et al. (Neuroinformatics 2019); Gauriau et al. (Journal of Digital Imaging 2020); Liang et al. (Frontiers in Neuroinformatics 2021).
   - HD-SEQ-ID (Radiology: AI 2024); Mahmutoglu et al. (European Radiology 2025); Kujawa et al. (European Radiology Experimental 2026); Truong et al. (arXiv 2602.23833, 2026); Reid et al. (Alzheimer's & Dementia 2025).
   - Body MRI series classifiers (2024 to 2025); Cluceru et al.; Kasmanoff et al.
   - The LLM series-description study (Journal of Imaging Informatics in Medicine 2025); NeuroPilot (arXiv 2608.07541).
   - BIDScoin, HeuDiConv and ezBIDS.
   Add other work from 2018 to 2026 of comparable weight.
4. Is there prior work in any field of scientific or clinical data curation that combines deterministic rules, a learned model and a human loop that improves both?

Output: the verified table; a list of claims you could not verify; and, for each of (a) to (d), whether it is novel, partly done or done, naming the closest verified prior work.

Every claim about a paper, dataset, law or tool must cite a primary source (DOI, publisher, arXiv, official or project page) that you actually read. Say explicitly when something could not be verified, and do not fill gaps by inference.

### R2. Label models for structured outputs with partial exact labels

I need a technical review of weak-supervision label models for a structured multi-output problem that also has partial exact labels.

Setting: about 518,000 MRI image stacks are labelled on 12 categorical variables: 8 single-label (up to 38 classes) and 4 multi-label (up to 53 tags), 156 values in total. A declared schema constrains the joint output with mutually exclusive groups, implications and forbidden combinations. The valid joint assignments can be enumerated for each connected component of the constraint graph.

About 250 deterministic rules act as labelling functions, and every rule's vote is recorded on every item. Their properties:
- Most rules are unipolar: each can vote one value of one variable, or abstain.
- Some rules fire only when another variable holds a given value, so the rules form a cascade across variables.
- Many rules read the same free-text field, so their errors correlate. Others read numeric physics parameters or structured DICOM flags.

Label sources beyond the rules:
- Exact labels for tens of thousands of stacks, from research cohorts with documented acquisition protocols. These cohorts are not representative of the whole archive.
- A legacy rule system whose output was complete and conflict-free on the core variables for about 76 % of stacks. It is believed to be close to 100 % correct but has not been verified at scale, and this subset is easier than the rest.
- A few thousand expert labels to come, from a random sample.

Questions:
1. Which label models suit structured or multi-task outputs with constraints? Cover Snorkel MeTaL, Universalizing Weak Supervision (Shin et al., ICLR 2022), factor-graph or label-powerset models, and work from 2021 to 2026. How is the latent variable defined over the valid joint assignments?
2. How should unipolar labelling functions, and rules conditioned on other variables, be parameterised? What are the identifiability conditions?
3. How should correlated rules that read the same field be modelled or grouped, and how is a violation of conditional independence tested?
4. How should partial exact labels from non-representative cohorts, and a large "easy" high-precision subset, be used to anchor or validate the label model without biasing it (selection bias, and covariate shift between the anchored subsets and the rest)?
5. How should each rule's accuracy be reported with an interval, and how can rules that are confidently and consistently wrong in the same way be detected?
6. Implementation: which maintained libraries fit, or is a custom EM over the valid assignments advisable, and what does it cost at 500,000 items?

Output: a recommended model with its likelihood written out, identifiability conditions, an estimation algorithm, pitfalls, and references with links.

Every claim about a paper, dataset, law or tool must cite a primary source (DOI, publisher, arXiv, official or project page) that you actually read. Say explicitly when something could not be verified, and do not fill gaps by inference.

### R3. A stratified, clustered, grouped certificate for automation

I need a method recommendation for statistically certified selective automation in a stratified, clustered and grouped setting.

Setting: a system labels MRI image stacks on 12 categorical variables. Each stack is either decided automatically or sent to a person with a short candidate list. The target guarantee: with probability at least 1 - delta (for example 95 %), the error rate among automatically decided stacks is at most epsilon (for example 1 %). An error means any of the 12 variables is wrong; this is one whole-stack 0-1 loss, calibrated once.

The archive holds about 518,000 stacks from 35,000 sessions and 5,300 patients, over 26 years, 84 scanner models, 11 vendors and field strengths from 0.5 to 7 T. Three facts shape the design:
1. Strata. About 76 % of stacks are "rule-clean": a legacy rule system labelled them completely and without conflict, and we believe they are nearly all right. The rest are "rule-uncertain", and a learned model with a referee decides among them.
2. Groups. We want the guarantee overall and for overlapping groups (vendor, field strength, era, body region), without labelling every combination.
3. Clustering. Stacks cluster within sessions, and sessions within patients.

We also have exact labels for tens of thousands of stacks from research cohorts with documented protocols, but those cohorts are not a random sample of the archive.

Questions:
1. Is a stratified design sound? That is: certify the rule-clean stratum with its own random sample (the policy there is "accept the rules"), certify the rule-uncertain stratum with another, and combine them. What exact overall guarantee results, and how should delta be split?
2. Compare these for selective risk over overlapping groups, and state the guarantee each gives:
   - Learn then Test with exact binomial p-values and fixed-sequence testing;
   - conformal risk control and selective conformal risk control;
   - group-conditional methods: Conformal Prediction with Conditional Guarantees (Gibbs, Cherian, Candes, JRSS-B 2025) and Batch Multivalid Conformal Prediction (Jung et al., ICLR 2023).
3. How should clustering be corrected: by sampling whole sessions, design-effect corrections, the cluster bootstrap, or document-iid certificates as in Valid Per-Field Selective Risk Control for Document Extraction (arXiv 2608.14639)?
4. Can non-random, exactly-labelled cohorts contribute to calibration at all (for example weighted conformal under covariate shift), or only as a separate group? How is a sample with known unequal inclusion probabilities used validly?
5. What sample sizes are needed per stratum and group for epsilon of 0.5, 1 and 2 % at delta = 5 %, with and without clustering and overlapping groups?
6. For drift and new scanners, how should conformal test martingales and adaptive conformal inference be used, and when should the system fall back to full review?

Output: a step-by-step procedure, the guarantee in one sentence, a sample-size table, pitfalls, and references with links.

Every claim about a paper, dataset, law or tool must cite a primary source (DOI, publisher, arXiv, official or project page) that you actually read. Say explicitly when something could not be verified, and do not fill gaps by inference.

### R4. Training and calibrating from labels of mixed quality

I need best-practice advice for training and calibrating a classifier from labels of mixed quality.

Setting: a multi-task classifier for about 518,000 MRI image stacks, with 12 categorical outputs: single-label heads with up to 38 classes and multi-label heads with up to 53 tags.

Inputs:
- structured DICOM header fields: numeric physics parameters and categorical codes, often missing;
- short noisy free text: series description and protocol name, in Swedish and English, with abbreviations and misspellings, sometimes containing personal names, so privacy matters;
- optionally, an image embedding from a separate pipeline.

Inference must run on CPU inside a Rust application with bit-exact results across platforms, so the model should be small and simple to evaluate deterministically.

Labels come in three qualities:
- exact labels for tens of thousands of stacks from research cohorts with documented protocols (not representative);
- high-precision labels from a legacy rule system on about 76 % of stacks (complete and conflict-free, believed to be near 100 % correct, but easier than the rest);
- probabilistic labels for everything from a weak-supervision label model over about 250 rules, plus a few thousand expert labels from a random sample.

Questions:
1. How should the three label qualities be combined in training (soft labels, weighting, noise-aware losses) without producing a model that only learns the easy cases?
2. Under which conditions can the model generalise beyond the rules and correct errors that several rules make together (weak-to-strong generalisation theory, co-training for multi-view data)? What evidence exists?
3. Which model fits these constraints? Compare gradient-boosted trees per head with character n-gram or hashed text features, a small multi-task neural network, and tabular foundation models, on:
   - accuracy and calibration;
   - handling of missing values;
   - deterministic CPU inference.
   Is there evidence that pretrained biomedical language models help on short scanner descriptions, given their privacy cost?
4. How should each head and each subgroup be calibrated (temperature, vector or Dirichlet scaling), and how many labels are needed to measure calibration?
5. How should active learning work when one expert decision can label a whole group of identical stacks?

Output: recommendations with justification, pitfalls, a design for an experimental comparison, and references with links.

Every claim about a paper, dataset, law or tool must cite a primary source (DOI, publisher, arXiv, official or project page) that you actually read. Say explicitly when something could not be verified, and do not fill gaps by inference.

### R5. Public datasets for external validation, verified

I need a verified survey of public MRI (and some CT) datasets for externally validating a classifier of image series. Verify every fact from the dataset's own page or data descriptor, link it, and say when you could not.

The classifier labels series by:
- contrast weighting and sequence technique;
- body region and orientation;
- post-contrast status and derived products.

It reads these DICOM header fields, and optionally the pixels: SeriesDescription, ProtocolName, ImageType, ScanningSequence, SequenceVariant, EchoTime, RepetitionTime, InversionTime, FlipAngle, Manufacturer, ManufacturerModelName, MagneticFieldStrength and geometry.

The best labels come from studies with documented acquisition protocols, where each series' identity is known exactly, or from BIDS datasets whose suffix is a curated label.

Questions:
1. For each candidate dataset, verify:
   - the distribution format: original DICOM, NIfTI with dcm2niix JSON sidecars, or NIfTI only;
   - which header fields survive, in particular SeriesDescription and ProtocolName;
   - vendors and scanner models (with counts, if published), sites and years;
   - the number of subjects and series;
   - whether an acquisition protocol document is published, and what labels are available;
   - the access procedure and how long it takes;
   - the licence, and whether derived results and models may be published.

   Candidates: Spine Generic; OASIS-3; PPMI; ADNI; ABIDE I and II; UK Biobank; the DICOM parts of fastMRI; multiple sclerosis datasets; OpenNeuro; and TCIA collections with original clinical MR DICOM, for example TCGA-GBM, TCGA-LGG, CPTAC-GBM, IvyGAP, REMBRANDT, ACRIN-DSC-MR-Brain, UPENN-GBM, UCSF-PDGM, Duke-Breast-Cancer-MRI, PROSTATEx and Prostate-MRI-US-Biopsy.
2. For OpenNeuro: is there published or computable evidence on how often sidecars contain SeriesDescription, ProtocolName, ImageType, ScanningSequence and ManufacturersModelName? How can the sidecars be retrieved in bulk without downloading images?
3. What are the known pitfalls? For example, de-identification that removes descriptions, curated subsets biased towards clean data, and conversion survivorship.

Output: a ranked, verified table with links; a list of claims that could not be verified; and a recommended external test set of three to five datasets, favouring multi-vendor original DICOM and documented protocols.

Every claim about a paper, dataset, law or tool must cite a primary source (DOI, publisher, arXiv, official or project page) that you actually read. Say explicitly when something could not be verified, and do not fill gaps by inference.

### R6. Privacy and law for releasing a model trained on hospital metadata

I need a privacy and legal risk assessment for releasing a small classifier trained on hospital MRI metadata.

The classifier is trained inside a Swedish university hospital on DICOM metadata, including free-text fields (SeriesDescription, ProtocolName) that occasionally contain personal names or clinical notes. The training data never leave the hospital. We want to publish the code, and ideally the weights, with a paper, and to share the model with partner hospitals in Europe.

The candidate models are gradient-boosted trees with hashed character n-gram features, or a small neural network. The vocabulary could be limited to tokens seen in at least k patients and on at least two scanners.

Questions:
1. What are the memorisation and membership-inference risks for such small models? How should they be measured (canary exposure tests, LiRA at low false-positive rates)? What published results exist for tree ensembles and small networks on text features?
2. Which mitigations work, and what accuracy do they cost?
   - vocabulary frequency thresholds;
   - hashing;
   - differential privacy (DP-SGD, DP tree ensembles);
   - training the released model on public data only.
3. What do the law and governance require?
   - what EDPB Opinion 28/2024 requires for a model to count as anonymous;
   - GDPR Articles 9 and 89;
   - Swedish specifics: the Ethical Review Act and Etikprovningsmyndigheten, the Swedish Authority for Privacy Protection (IMY), and the Public Access to Information and Secrecy Act;
   - what an ethics application should state for method development and for releasing a model;
   - precedents of European hospitals releasing models trained on patient-derived data.
4. What do journals require for code, weights and data availability when weights cannot be released (the Nature portfolio, Radiology: AI, Medical Image Analysis)?

Output: a risk assessment, safeguards, a pre-release checklist, and references with links.

Every claim about a paper, dataset, law or tool must cite a primary source (DOI, publisher, arXiv, official or project page) that you actually read. Say explicitly when something could not be verified, and do not fill gaps by inference.

### R7. What top journals expected, verified

I need a verified benchmark of what top journals expected from papers comparable to ours. Verify every example paper from the publisher's page, give its DOI, and say when you could not.

Our paper: a system that curates heterogeneous clinical MRI archives into standardised datasets ready for analysis. The archive holds about 518,000 image stacks, spanning 26 years, 84 scanner models and 11 vendors. The core is a new classification method:
- deterministic expert rules used as weak supervision for a learned, calibrated model;
- a statistical certificate bounding the error among automatically decided stacks;
- human review with short candidate lists for the rest.
The software is open source.

Candidate journals: Nature Biomedical Engineering, Nature Methods, Radiology: Artificial Intelligence, npj Digital Medicine, Medical Image Analysis, IEEE Transactions on Medical Imaging, Patterns and JAMIA.

Questions:
1. Find verified papers from 2022 to 2026 in these journals on imaging data curation, DICOM or BIDS standardisation, series or sequence classification, health-system-scale clinical imaging, or research data infrastructure. One example is Prima, "Learning neuroimaging models from health system-scale data" (Nature Biomedical Engineering 2026). For each, list the validation reported:
   - external sites;
   - prospective use;
   - reader or user studies;
   - measured time savings;
   - subgroup analyses;
   - calibration and uncertainty.
2. What did reviewers and editors demand? Use published peer-review files where they exist (Nature's transparent peer review) and editorials.
3. Which reporting guidelines apply (CLAIM 2024, DECIDE-AI, TRIPOD+AI), and which of their items matter most here?
4. From the current author guidelines of Nature Biomedical Engineering, Medical Image Analysis and Radiology: AI, what are the article types, word and display limits, and code and data policies?
5. How do Nature Biomedical Engineering and Nature Methods differ for a tool of this kind?

Output: a verified table with DOIs, a list of must-have evaluation elements, a recommended order of journals with reasons, and a list of claims that could not be verified.

Every claim about a paper, dataset, law or tool must cite a primary source (DOI, publisher, arXiv, official or project page) that you actually read. Say explicitly when something could not be verified, and do not fill gaps by inference.

### R8. Deterministic machine-learning inference in Rust

I need an engineering recommendation for embedding a small trained classifier in a Rust application that must give bit-for-bit identical outputs on every platform.

The application ships for Linux, macOS and Windows, on x86-64 and arm64. Its outputs are hashed and compared in release gates.

The model is trained in Python and runs on CPU over up to millions of items. It is either a gradient-boosted tree ensemble (LightGBM, XGBoost or CatBoost) or a small MLP, with about 12 output heads. After the heads, a constrained layer renormalises the outputs over an enumerated set of valid joint assignments: a sparse matrix-vector product, then logsumexp and softmax. So exp and log must be deterministic too.

Questions:
1. What are the options, and how do they compare on maturity, binary size, dependencies and speed? Candidates: a pure-Rust tree evaluator (trees exported to JSON), tract, candle, burn, and ONNX Runtime through the ort crate.
2. Where do floating-point results diverge across CPUs and operating systems (fused multiply-add, SIMD, threading, summation order, libm)? How can identical results be guaranteed:
   - a fixed evaluation order;
   - avoiding fused multiply-add;
   - a pure-Rust libm (for example the libm crate);
   - integer or fixed-point arithmetic?
   Is rounding before hashing safe?
3. How do tree ensembles and quantised MLPs compare on determinism, including the final summation and the link functions?
4. How should model artifacts be versioned, hashed and verified when loaded?
5. Which projects are known to achieve deterministic inference across platforms? Give links.

Output: a recommendation with reasons, pitfalls with countermeasures, and verified links.

Every claim about a paper, dataset, law or tool must cite a primary source (DOI, publisher, arXiv, official or project page) that you actually read. Say explicitly when something could not be verified, and do not fill gaps by inference.

### R9. Evaluation design and measuring time saved

I need a study design for evaluating an automated curation system for a large clinical MRI archive with as few expert labels as possible, and for measuring the time it saves people.

The system assigns each MRI image stack values on 12 categorical variables, and either decides the stack automatically or sends it to a person with a short candidate list. The archive holds about 518,000 stacks from 35,000 sessions and 5,300 patients, over 26 years, 84 scanner models and 11 vendors. The paper will follow CLAIM 2024 and DECIDE-AI.

Labels available or planned:
- exact labels for tens of thousands of stacks from research cohorts with documented protocols, which are not representative of the archive;
- a "rule-clean" stratum of about 76 % of stacks, labelled completely and without conflict by legacy rules, and believed nearly all correct; the remaining "rule-uncertain" stratum is harder;
- expert labels on samples yet to be designed, from a neuroradiologist and an MR physicist or senior technologist.

We must report, with valid confidence intervals:
- accuracy per variable and per value, including rare values, by vendor, field strength and era;
- calibration;
- the automation rate and the certified error;
- the time saved compared with manual curation.

Questions:
1. Sampling design. Cover stratified sampling by stratum, group and predicted class with known inclusion probabilities (Horvitz-Thompson), two-phase sampling, active testing (Kossen et al., ICML 2021) and prediction-powered inference (Angelopoulos et al., Science 2023). How can the exactly-labelled, non-random cohorts be combined with random samples without bias? Can one sample serve both estimation and the conformal certificate?
2. Sample sizes. How many labels give a chosen interval width on accuracy and macro-F1, per variable, per subgroup and per rare value, with stacks clustered within sessions and patients?
3. Labelling protocol. Cover double reading, adjudication, agreement statistics for multi-variable categorical labels, blinding readers to the system's output, and what CLAIM 2024 requires of the reference standard.
4. Measuring time saved. Cover crossover or randomised designs; what to measure (time per decision, accuracy with and without candidate lists, automation bias); sample sizes; examples from radiology informatics or data curation; and what DECIDE-AI requires.
5. How can a decision-curve or cost-benefit analysis choose the automation threshold?

Output: a recommended design with formulas, sample-size tables, a checklist mapped to CLAIM 2024 and DECIDE-AI, and references with links.

Every claim about a paper, dataset, law or tool must cite a primary source (DOI, publisher, arXiv, official or project page) that you actually read. Say explicitly when something could not be verified, and do not fill gaps by inference.

## Appendix: after the second round

Three narrow questions remain after two rounds. Each is written to stand alone.

### Q1. Prior art in workshops, abstracts and other fields

I am writing a paper on automated curation of clinical MRI archives and need a verified search for prior work anticipating three claims. Search 2016 to September 2026: arXiv, medRxiv, PubMed, IEEE Xplore, ACM DL, OpenReview (including MICCAI workshops and MIDL), SPIE Medical Imaging, and SIIM, RSNA, ISMRM and AAIC abstracts. List every query, and report negative results as such.

1. Several hand-written rules over DICOM metadata, combined by a label model that estimates each rule's accuracy without labels (Dawid-Skene, Snorkel or similar). This covers MRI or CT series, sequence, body-part or contrast-phase classification, and BIDS conversion. Already known, each with one rule source and no label model: Na et al., Diagnostics 2024; Zhou et al., MIL3ID 2019; US patent 11,727,087.
2. A finite-sample, high-probability bound on the error among items decided automatically when curating medical images or their metadata, as opposed to diagnosis. Already known: Mulford et al., J Arthroplasty 2025; Han et al., DEMI 2026; Gurram, arXiv 2608.14639; Angelopoulos et al., medRxiv 2024.
3. A loop in which reviewers' answers on deferred items both retrain a model and repair the rules that erred, in any kind of scientific or clinical data curation (EHR phenotyping, biocuration, genomics metadata, clinical NLP, imaging). Already known: IWS, REGAL, RuleCleaner, ezBIDS, NeuroPilot.

For each hit, give the full citation, the link you opened, what it does and how it differs from the claim. Cite only sources you opened.

### Q2. The AI Act and the Medical Device Regulation

I need a regulatory classification, citing only primary sources I can check.

The software is open source, from a Swedish university. It classifies MRI series from DICOM header metadata, for research curation and conversion to BIDS; it does not diagnose. It may ship a small trained classifier.

Under Regulation (EU) 2024/1689 (the AI Act, as amended in 2026; Article 2(6) and Annex III) and Regulation (EU) 2017/745 (the Medical Device Regulation) with MDCG 2019-11, is this software, or a model shipped with it, any of the following?
- an AI system with obligations;
- a high-risk AI system;
- a medical device.

Answer separately for three uses:
(a) research use;
(b) partner hospitals running it on their own archives;
(c) routine management of hospital PACS data.

For each use, say which provisions decide it, and what would change the answer. Cite only primary sources you actually read (the regulations' text, MDCG guidance, Commission guidelines); mark anything unverified.

### Q3. A certificate when the reference labels contain errors

How can a finite-sample upper confidence bound on the error rate of automatically accepted items stay valid when the reference labels themselves contain errors?

The certificate uses Learn then Test with exact binomial tests: 1 % at 95 % confidence, with one 0-1 loss over 12 categorical outputs. Two experts label every item independently, and a blinded third resolves their disagreements. The remaining reference error may be 0.1 to 0.5 % of items, and it may be correlated with the system's errors.

Cover:
1. bounds that use the readers' disagreement rate to bound the reference error;
2. the direction of the bias when reference errors are independent of the system's errors, and when they are correlated with them;
3. whether re-reviewing only the items where system and reference disagree is ever valid, and how a random re-review of agreements corrects it;
4. precedents from diagnostic-accuracy studies with imperfect reference standards, such as latent-class and composite references.

Give formulas, sample sizes and primary references with links; mark preprints.

## Amended by record 55 (2026-10-08)

Phase 2's expert reference was replaced by agent reads (record 48). The rest of Phases 2 to 4 is carried by [55](55-ready-for-production-and-the-paper.md): I3 (a 300-item expert check of the agents' answer key, and a small time-saved study), D2 (System 1's robustness certificate, with public headers from TCIA and OpenNeuro as external data), and I1 (the writing, with Russell).
