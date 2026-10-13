# 56 · The rules decide; System 1 knows when not to trust them; the loop improves the rules

Opened 2026-10-09. **Status: accepted by Nima on 2026-10-09:** the principle, what goes to Review, System 1's place, how the error is known, and the loop. Nima: "this is good but document it heavily so we dont loose track of it. proceed with it."

Follows [39](39-two-ways-of-knowing-a-stack.md) and [55](55-ready-for-production-and-the-paper.md) (D1 to D7, H3), and amends them as section 9 says. The evidence behind it is in section 8; where everything lives is section 10.

## The ask, in Nima's words (2026-10-09)

> "we need to do review only and only if truly neccessary and not on rule we have decided on."

> "lets system 1 be in development and research but I think we should decide on rule pack then what goes to review and then use system 1 on them ... I think for the most part we can decide with rules."

> "we had a plan and goal and purpose for System 1. explore and see what exactly it was and how we can address that fitting it into the picture."

> "good solidify this in writing so we follow it. the loop should also be thought through. should it be automatic? should it be based on dataset and ingest? or both? or manual edit on rule?"

> "this is good but document it heavily so we dont loose track of it. proceed with it. I also think the change in rule should show how it affect the sorting (like how many stack and to what) and for most part maybe it is programable (adding keyword, changn order, adding new item to axes and so on)."

## In one paragraph

The rule pack decides every answer. A person is asked only when the rules leave an axis that matters empty, or, on a dataset whose headers are damaged, when System 1 does not confirm the rules. System 1 stays in research until it is confirmed. Its job is to know when the rules cannot be trusted, to give a second opinion inside Review, and to point at weak rules. Errors are fixed in the rules, not scan by scan. Evidence is gathered automatically at every ingest and grouped into proposed rule changes. Each proposal is written as a typed operation and comes with its effect on the sorting ("how many scans, from what to what"). A person decides, and the system applies.

## 1. The principle

**The rule pack decides every answer.** It is written, versioned, auditable knowledge, and every release names the pack version it was sorted with. Review is the exception: it holds only what the rules cannot decide or cannot be trusted on, and nothing the pack has decided.

## 2. What goes to Review

A scan becomes a Review question only when:
1. **An axis that matters is left empty by the rules.**
   - The axes that matter are those the release naming, the main-scan pick or a release reads.
   - The list comes from the code (`nils-pack/src/matters.rs`), and a test fails if it drifts from `nils-release/src/run.rs`.
   - For MRI, missing is asked only on single-valued axes with no default that no model owns. Today that is **base**.
2. **On a dataset flagged as having damaged or foreign headers: the scan's rules answer is not confirmed by System 1** (section 3). This waits until System 1 is served and confirmed (D2); until then it raises nothing.

**Never a question:**
- **A conflict between the pack's rules, whatever their ranks.** The pack's order decides (section 4.1), and the overridden rule is kept as evidence for "why".
- **A rule's low confidence.**
- **Body part.** It belongs to the body-part image model: its own operation, a step of a dataset and of a cohort, with its own review of what it is unsure of.
- **Post-contrast.** Nima, 2026-10-09: "post contrast is like body part. it should have its own operation. we will add it as a step to the dataset/cohort." It belongs to its own operation: F2's time rule as the label, then the post-contrast model (record 55, F2, F5, E3), with their own review of what they are unsure of. That operation is a step of a dataset and of a cohort, like the body-part model's, shown in "Where it is" and run from there. Sorting asks nothing about it; asking would have hit 738 of record 34's 991 scans, because the pack only ever states "given". The change goes in with its effect report (5.4).
- **Scouts' base,** proposed with its impact shown. A scout has no base contrast by definition; the pack's silencing covers excluded scans but not scouts.
- **The split note.** It is information on the series.
- **An equal-rank disagreement inside the pack.** That is a defect of the pack: it goes to the pack's quality report (`GET /api/packs/{name}/disagreements`), never to a person's queue.

**Not yet ruled:** the main-scan pick's borders (`pick.border`, 59 on study-big). They are counted and left as they are until Nima rules under this same principle.

## 3. System 1's place

System 1 keeps record 39's purpose, "knowing when not to trust either", with its roles made explicit.

1. **The gate is chosen per dataset, not per scan.**
   - **A dataset with normal headers:** the rules decide, and Review holds only empty axes that matter. Its error is known from the rules' certificate and the audit. It improves through the loop (section 5).
   - **A dataset flagged at ingest as having damaged or foreign headers** (another site's anonymised export, stripped names, another language): the full registered gate applies. System 1 must agree with the rules on each scan, or the scan goes to Review with both suggestions.
     - Measured: the gate keeps the accepted error at 0.3 to 1.6 % where the rules alone lose 8 to 33 %, and sends 17 to 52 % of such a dataset to Review, depending on the damage.
     - This waits for System 1 to be served and confirmed (D2).
   - **The per-scan "reliability first" form was measured and is not adopted** (section 8). It sent only 1 % of clean scans to Review, but accepted 8 of the 10 rule errors unexamined, and up to 6.6 % error under damage.
2. **A second opinion inside Review.** Beside every question sit the rules' answer, with the clauses and fields that decided it, and System 1's answer, with its confidence and the fields that mattered (D3, D6). It speeds the person's choice. It never decides what is asked.
3. **A source of evidence for the loop.** System 1's confident disagreements with the rules point at weak rules; where it contests the base, it is nearly always right.
4. **Research continues at research pace.** The next steps are a stronger model (the encoder with JEPA pretraining, D5, and a sharper reliability model), then one confirmatory grade on D2's sealed sample with the design frozen and registered first. Neither gate makes 1 % on the development set yet. **Nothing in production waits on it.**

**The damaged-header flag** sits on a dataset's Data card ("headers stripped: sorting less reliable"). It is set at ingest from simple signs (missing series descriptions, unknown words, an unseen vendor) until System 1's reliability model serves.

## 4. How the rules decide, and how the error is known

### 4.1 How rank is decided in the code (measured 2026-10-09)

- **Rule sets run in the pack's `order:` list** (`nils-pack/src/pack.rs:736`, evaluated at `eval.rs:514`).
  - A set that decides closes the axis (`eval.rs:646`).
  - Later sets cannot change a closed axis (`eval.rs:553`, `eval.rs:603`), except a set that declares `redecides` (`eval.rs:548`).
- **Inside a deciding set, the first rule that fires wins** (`eval.rs:744`).
  - Value-major axes try values in vocabulary order (`pack.rs:2229`).
  - Values are collected only on multi-valued axes (`pack.rs:2465`).
- **Inside an exclusion group, the lower priority wins.** A tie keeps the first value collected (`eval.rs:782`).
- **A default applies only when no rule fired** (`eval.rs:856`).
- **Decisions come before the rules, and passes come after.** A person's or model's decision overrides the rules (`classify.rs:1241`), and passes fill gaps afterwards.
- **Tier and confidence never decide who wins.** They are only what the winner writes.

**Equal rank can still happen in three ways**, and each is a pack defect:
- one rule lists two values for a single-valued axis and both hold (`rules/base.yml:335`, `stage:named`);
- a collecting set writes two values on a single-valued axis;
- two values of one exclusion group share a priority.

None occurred on record 34.

### 4.2 How the error is known

- **On normal headers:** the rules' certificate (certificate 2, and its successor for each rules version).
- **On damaged headers:** System 1's robustness certificate (D2).
- **In production:** a blind audit of a random share of accepted scans (D7). Agent raters read them and a person settles their disagreements; its reads never train a model. That is the honest number, and it replaces a growing queue as the safeguard.

## 5. The loop

The loop turns what people and models find into better rules. One scan's answer fixes one scan; a rule fix fixes every scan it touches, past and future.

### 5.1 Where the evidence comes from

| Source | What it says | When |
|---|---|---|
| Review answers | the right answer for a scan the rules left empty or could not be trusted on | as people answer |
| The audit (D7) | errors among accepted scans | each audit round |
| System 1's confident disagreements | a rule may be wrong (on base nearly always) | after each sort, in shadow |
| The label model | each rule's real precision, measured without labels (record 39) | over the archive, periodically |
| Equal-rank disagreements | the pack contradicts itself | after each sort |
| The overrides kept as evidence | which rules lose to which, and how often | after each sort |

### 5.2 Three levels of fix

1. **A decision on one scan** (or its alike group): a person's answer in Review. It applies at once and re-sorts that scan. It is never a rule.
2. **A scoped overlay:** a rule change for one site's or one dataset's conventions, such as a partner site's "mdc" for contrast, or one scanner's private series names.
   - The engine already has overlays (Wave 4c §6.6, `nils overlay`): proposed with a rehearsal, adopted by an operator, applied by re-sorting their scope.
   - This is the right level when a dataset from another site brings its own words.
3. **A pack edit:** a change to the rules everyone uses. It ships as a rules release (B5: 1.0.2 and on), and every scan it touches is re-sorted. Every earlier version stays available.

### 5.3 What is automatic and what is a person's

| Step | Who | Why |
|---|---|---|
| Collecting the evidence, per dataset at each ingest and over the archive | automatic | it is measurement |
| Grouping it into proposals: one rule and one pattern across many scans is one proposal | automatic | the same pattern on 47 scans is one proposal, not 47 questions |
| Writing each proposal as typed operations (5.5) | automatic, or a person or the assistant drafts it | the operations are checked against the pack's schema before anything runs |
| Its effect on the sorting (5.4) | automatic | Nima's rule: the effect is shown before any change; the replay takes seconds |
| Deciding: adopt, edit or decline | **a person** | the pack is the auditable source of truth and a release names its version; a rule never changes by itself |
| Applying an adopted change: the overlay or rules release, the re-sort, the before and after counts, a point on the development curve | automatic | once decided, applying it is mechanics |
| Training the next System 1 on new blind answers | automatic, on research machines | training is mechanics |
| Promoting a new System 1 | **a person**, after its registered test passes | D4: models are versions with cards, and the certified one is the default |

**Dataset and ingest, or both: both, at different levels.**
- The evidence is gathered per dataset at each ingest, and a dataset's detail shows what its read found ("3 proposed rule fixes from this read").
- A fix lands where it belongs: a general rule error is a pack edit for everyone; a local naming habit is an overlay scoped to that site or dataset.

**Manual edits on the rules:** yes, for whoever tunes the pack. Most go through the typed operations; a free-form edit of the pack's files is still possible for its maintainer, and it gets the same effect report before it is released.

### 5.4 A rule change shows its effect on the sorting

Every proposed change, typed or free-form, scoped or global, is replayed over the whole registry before anyone decides. The effect report shows:

1. **How many scans change, and from what to what, per axis.** A transition table, for example:
   - `technique: SE → MDME, 51 scans`;
   - `base: T2w → FLAIR, 47 scans`;
   - `modifier: FLAIR → FLAIR+FatSat, 12 scans`.

   Each row has its datasets, sites and makes, and example scans with their pictures.
2. **What it does to the names.** The release names and BIDS names that change, as before and after for the same examples.
3. **What it does to the main scans:** picks that change, by role. Each side is scored by its own pack's pick (2026-10-10), so a change to the pick file shows its effect: per role, the picks kept, changed, removed and added, and the occasions whose borders are raised, settled or still there.
4. **What it does to Review:** questions that appear and disappear.
5. **What it does to the answers we know.** On the settled answer keys (the development reference R, audit answers that may be used, Review answers), it counts:
   - **right → wrong**, the changes that break something;
   - **wrong → right**, the fixes;
   - **unchanged.**

   The net is the change's measured worth. A change that fixes 47 and breaks 3 says so. Sealed samples are never read for this.
6. **Its reach:** the rules version it would ship in, or the scope of the overlay.

The engine's pack archive replay (`nils pack replay`, flat packets in 32 chunks, seconds over 556,000 stacks) and the overlay rehearsal already compute most of this. The transition table, the effect on names and picks, and the right/wrong counts are what is added.

### 5.5 Rule edits are typed operations

Most rule changes are a handful of kinds. Each is an operation the engine checks against the pack's schema and applies deterministically, so a proposal is a reviewable diff of the pack, reproducible, and writable by a form, a person or the assistant.

| Operation | What it does | Example |
|---|---|---|
| `add_words` / `remove_words` | adds or removes words a value's rule reads, on a field (series description, protocol name, sequence name) | `add_words axis=post_contrast value=given words=[mdc] fields=[series_description] scope=site:IT2` |
| `move_rule` | moves a rule before or after another, inside its set | `move_rule rule=technique/MS-EPI before=technique/implied` |
| `move_set` | moves a rule set in the pack's order | `move_set set=symri before=technique` |
| `set_priority` | changes a value's priority in an exclusion group | `set_priority axis=modifier value=FLAIR priority=2` |
| `add_value` | adds a new value to an axis's vocabulary, with its label, its family, its BIDS mapping and its place in the order | `add_value axis=technique value=MP2RAGE family=GRE bids=acq` |
| `add_rule` | adds a rule from a template: words in a field, a tag equal to a value, a physics range (TE, TR, TI, flip angle), or their combination | `add_rule axis=base value=FLAIR when=[ti in 1800..2800, te >= 80]` |
| `silence` | an axis says nothing, and asks nothing, when a condition holds | `silence axis=base when=directory_type=localizer` |
| `by_model` | hands an axis to a model's run and its own review | `by_model axis=post_contrast` |
| `map_name` | changes how a value appears in the descriptive and BIDS names | `map_name axis=construct value=Uniform bids=UNIT1` |
| `set_candidates` | which stacks holding a role compete for it in the main-scan pick (a pack edit; pack contract 9) | `set_candidates role=t1w unless=[body_part is spine, neck, chest or other]` |
| `remove_border` | a border kind the pick raises no more | `remove_border border=rare` |
| `set_near_tie` | the order a near tie is decided by, instead of a question (pack contract 9) | `set_near_tie order=[3D first, then axial, coronal, sagittal, then the lower series number]` |
| `set_runner_up_within` | the margin of a near tie | `set_runner_up_within within=0.02` |

- **Every operation carries:** its scope (the whole pack, or an overlay for a site, a dataset or a scanner), its reason, and its evidence (the proposal it came from).
- **The pick operations** (added 2026-10-10 from `studies/2026-10-10-pick-borders/`) are pack edits only: a pick's file is one for every stack and its population the whole registry's. `add_rule` may also set nothing on an axis that holds several values, such as `role`: the stack holds none of them where the conditions hold, the rule placed first in the first set that decides the axis.
- **Applying operations** to a pack version gives a new version. The pack's own loader validates it, and the effect report (5.4) is computed on it.
- **The assistant's tune-sorting-words skill writes these operations** instead of raw overlay documents. The overnight station bench of 2026-10-08 found the 27B writing the overlay scope wrong 36 times; a typed, validated operation removes that failure.
- **Free-form edits** of the pack's files remain for its maintainer, and go through the same effect report.

### 5.6 Guards

- **Sealed samples never feed the loop.** No proposal, overlay or edit is made from a certificate's or an audit's sealed answers; their errors are counted, not fixed from.
- **Only blind answers train System 1** (D3's one in five).
- **Every rules version stays available,** so past releases can be reproduced; a release names the version.
- **An overlay never leaves its scope.**
- **A change whose effect report shows any right → wrong** says so on its approval card, with the scans.

## 6. What gets built, in order

1. **The engine's question logic.** Branch `wave7a-review-logic`, nils #386, done and waiting for the merge. Conflicts, low confidence, body part and split notes raise nothing; missing on an axis that matters does; equal-rank disagreements go to the pack's door. Next on it: post-contrast and scouts (section 2), with their effect report shown first.
2. **The effect report (5.4) and the typed operations (5.5)** in the engine. **Built 2026-10-09** (nils #389, contract `patch/v1`): ten operations with scopes, `nils pack rehearse` / `POST /api/packs/{name}/rehearse` and `nils pack apply`; on record 34 a report takes about 0.5 to 1.8 s and matched a real re-sort exactly. It found the physics vote filling MRI answers on CT stacks, which is fixed. Adopting a scoped overlay still needs a scoped re-sort (step 5). It covers:
   - the operations as a pack patch format with validation;
   - a rehearse door that applies operations to a copy of the pack and replays the registry;
   - the transition table, names, picks, Review and right/wrong counts.
   - **Extended 2026-10-10** (branch `wave7a-pick-rules`): the four pick operations of 5.5 and pack contract 9's `candidates` and `near_tie`, each side's picks scored by its own pack's pick, so every ruling of `studies/2026-10-10-pick-borders/` can be rehearsed before it is applied.
3. **Review round 1's interface** (`studies/2026-10-09-review-round1/`) on the few questions that remain, with the rules' evidence and the overrides beside each.
4. **The Rules page:**
   - the pack-quality report (equal-rank disagreements, the overrides, later System 1's disagreements);
   - proposals with their effect report and an approval card;
   - a form for each typed operation.

   Also on a dataset's detail, and in the one chat.
5. **Adopting a proposal:** an overlay, or a rules release (B5) with its version, then the re-sort and its counts on the development curve.
6. **The damaged-header flag** on a dataset's card.
7. **System 1:** research continues (a stronger model, then D2's single confirmatory grade). Then it is served through the pipeline runner as a second opinion, and as the gate for flagged datasets.
8. **The audit stream (D7).**

## 7. What changes for a person, in practice

- **Sorting a dataset from our own site raises almost nothing.** On study-big, the 500 "need a look" become a handful once post-contrast and scouts are settled.
- **The few questions left** say plainly why: the rules found no base for this scan.
- **Errors found by a person or the audit become proposed rule fixes**, each with its effect ("47 scans from T2w to FLAIR, fixes 47, breaks 0"). A person adopts one with a click, and everything it touches is re-sorted.
- **A dataset from another site with stripped headers** is flagged at ingest. Once System 1 serves, its scans go through the full gate.

## 8. The evidence (2026-10-09)

**Today's Review was noise.**
- On study-big (record 34's corpus, 1,003 scans), 500 scans were marked "need a look".
- 297 were split notes. Nearly all the rest were overrides the pack ranks on purpose: body part "spine over neck" 64, "brain-neck over neck/brain" 36 + 36, technique "MDME over SE/TSE" 51 + 39, base basis "physics over name" 47.

**The question logic, before and after, on record 34** (nils #386):

| | before | after |
|---|---|---|
| asked about rule conflicts | 198 scans | 0 (940 overrides kept as evidence) |
| asked about low confidence | 2 | 0 |
| split note | 297 | 0 (kept as information) |
| body part | 100 | 0 |
| equal-rank disagreements | | 0 |
| base missing | 0 | 7 (6 of them scouts) |
| post-contrast missing | 0 | 738, then 0: post-contrast is its own operation (section 2) |

**The rules against System 1 on the graded development set** (R-dev, 1,044 stacks):

| | rules alone | System 1 alone |
|---|---|---|
| clean headers | wrong on 10 | wrong on 61 |
| damaged headers | wrong on 8 to 33 % | |

The registered gate keeps the accepted error at 0.3 to 1.6 % under damage. Certificate 2 already proves the rules on clean data: 97.1 % decided with 2 of 4,856 wrong. The study's report is `studies/2026-10-08-system1-damage-and-cheap-model/report.md`. Step 1 failed narrowly against a 1 % bound: clean 1.23 %, mixed 1.72 %.

**System 1 as the trigger for Review** (exploratory, "H3 in shadow"):
- On clean headers it would send 56 of 1,044 to Review, 54 of them where the rules were right, and it misses 4 of the 6 rule errors.
- Under damage it catches most rule errors.

**Variant R, the reliability-first gate** (exploratory):

| set | Variant R: Review | Variant R: accepted error bound | registered gate: Review | registered gate: bound |
|---|---|---|---|---|
| clean | 1.0 % | 1.51 % | 8.1 % | 1.23 % |
| damage | 1.7 to 29.8 % | 1.65 to 6.59 % | 10 to 52 % | 0.80 to 2.86 % |

On the archive's unseen sessions, Variant R would send 9.3 per 1,000 to Review, and the registered gate 54.6.

> **Warning: the R-dev numbers are exploratory.** R-dev has been read more than once now. Any design chosen from these numbers is confirmed once on D2's sealed sample before it makes a claim.

## 9. Amendments

- **Record 39:** its arbiter decides only on datasets flagged as damaged, and its loop is section 5 here.
- **Record 55:**
  - **D3's review rule** (agree above thresholds, else review) now applies only to datasets flagged as damaged. Elsewhere the rules decide (sections 2 and 3).
  - **The H3 ruling** of 2026-10-09 and the D3 entry "System 1's place" stand, and this record carries them.
  - **D5 and D2 stand,** at research pace.
  - **D7 stands.**
  - **B5's rules releases** are how a pack edit ships.
- **Record 36:** its hand-set rule confidences are not needed for Review any more; a rule's confidence no longer raises a question.

## 10. Where everything lives

| What | Where |
|---|---|
| This record | `decisions/56-the-rules-decide.md` |
| The wave record, rulings H3 and D3 | `decisions/55-ready-for-production-and-the-paper.md` |
| System 1 step 1: plan, freeze, grade, rerun, H3 shadow, Variant R | `studies/2026-10-08-system1-damage-and-cheap-model/plan.md` (section 16), `report.md` |
| Evidence runs (sha256 manifests, mirrored to the research machine) | `studies/phase0-evidence/runs/`: s1-7a-check, s1-7a-guard, s1-7a-real, s1-7a-x2, s1-7a-h3, s1-7a-varr |
| The frozen System 1 | `FROZEN.sha256` 456b1907…c4bf, in the research container's private place |
| The question logic | nils #386, branch `wave7a-review-logic`; `nils-pack/src/matters.rs`, `nils/src/disagreements.rs`; record 34's measurement on the development laptop |
| Review round 1's design | `studies/2026-10-09-review-round1/report.md` |
| The desk's design | `studies/2026-10-09-desk-design/` |
| The pack archive replay | our operating notes |
