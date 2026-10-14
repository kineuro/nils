# The final header certificate: the reference is ready

2026-10-02. Run `final-cert-read` (`plan.md`). This file holds counts only. Stack ids, per-cell values and every reason stay in the evidence archive. **No rule answer on a drawn stack has been read** when this is written; the grade is registered in `grade.md`, part 1, before any is fetched.

**In short:**
- **The draw and the seal are the registered ones:** 3,750 stacks (main 2,500, supplement 250, kept-uncertain 600, void-kept 300, localizers 100), sealed by Nima as `selection:final-cert@1` (all 3,750 sealed, checked read only). Every packet was built, with every identifier check passed.
- **Every audited axis passes the 2 % joint-error bar:** 0 joint errors in 500 to 537 audited cells per axis (upper bounds 0.56 % to 0.60 %); per stack, 0 of 560 audited stacks (upper bound 0.53 %).
- **The raters agree closely.** Fleiss' kappa runs from 0.947 (modifier) to 0.993 (base) on the five certificate axes. The second adjudicator agreed with the first on 129 of 133 cells.
- **llm-2's dissent added little:** of 601 cells it alone routed, 1 was settled to its value and 28 to can't tell; 572 went to the raters' value.
- **The correction record** (`final-cert-read-corrections`): 122 cells on 95 stacks, 113 of them protocol-truth cells; no audited cell is corrected, so the audit stands against the corrected reference. The localizers' record corrects 0 cells.
- **Two deviations from the registered run,** written in `plan.md` section 13: the account's usage limit stopped the reading for 75 minutes, and four batches were answered once more by a fresh agent; and the session's permission classifier refused one agent's read and another's writes, so their batches were answered once more too. The design is unchanged.

## The reading

- **The three llm-1 raters answered 3,750 of 3,750 each,** on prompts v3.3, one rater at a time (`claude-tags`, then `claude-family`, then `claude-adversarial`), at most 20 agents at once. 189 batches, plus the reruns below; every launched prompt was compared with the task text from the agents' own transcripts: 192 of 192 identical. `validate.py`: no schema failure; one legality failure (one `claude-adversarial` answer gave comb-ME-GRE a base other than T2starw), answered once more by a fresh agent under the plan's rule, after which there were none.
- **llm-2 (`27b-v33`, thinking off) answered 3,750 of 3,750** at concurrency 6 throughout (no drop to 2), with the 120 s client timeout, in 12,761 s, with no answer illegal after its retry. The default model was swapped back. Its identity is in `models/27b-v33.json`.
- **Routing** sent 557 stacks with 656 cells to adjudicator-1: 650 disagreements (601 of them by llm-2's dissent alone) and 6 truth checks. 73 cells where every rater said can't tell are out. The audit drew 2,625 cells (15 % of each axis's agreements, 500 to 537 per axis), and 135 cells went to the second opinion.
- **The adjudicators** (41 fresh agents, 19 and 22, each in its own folder, every prompt identical to the task text) answered 557 of 557 and 656 of 656 items. `check_adjudication.py` found 0 problems. Adjudicator-2 confirmed all 2,625 audit cells.

### The interruptions and reruns (deviations, plan.md section 13)

- **The usage limit,** 2026-10-02 10:15 to 11:30 UTC: it stopped every running agent. `claude-adversarial` batches 029 and 031 were part-way (18 of 60 each), and 033 to 063 had not started. The partial outputs are kept unread in `raters/_first/`, and each batch was answered once more by a fresh agent with the same task text in its own folder. llm-2 ran on through it.
- **The permission classifier:** `claude-tags` 041's agent had a read of its own packets refused ("PII Data Handling") after the harness saved a long output outside its folder, and left 6 stacks unanswered; `claude-family` 021's agent had two of its own output writes refused ("Session Transcript Tampering") and left 2 unwritten. Nothing was bypassed and nothing an agent said in its reply was used as an answer. A first fix answered the missing stacks in separate folders; after the usage-limit stop, both batches were instead answered once more in full by a fresh agent, so that no batch mixes two agents' answers. Every first output and the two partial fixes are kept unread in `raters/_first/`.
- **The order of the waves:** the reruns of `claude-tags` 041 and `claude-family` 021 ran during the `claude-adversarial` wave, after the stop.

## The reference

Non-localizers and localizers together; localizers are asked provenance and technique only.

| axis | truth | consensus | audited | audit-corrected | adjudicated | adjudicated can't tell | all can't tell |
|---|--:|--:|--:|--:|--:|--:|--:|
| provenance | 124 | 3,036 | 544 | 0 | 36 | 9 | 1 |
| technique | 106 | 2,939 | 519 | 0 | 181 | 5 | 0 |
| modifier | 32 | 2,828 | 505 | 0 | 252 | 9 | 24 |
| construct | 29 | 2,967 | 526 | 0 | 91 | 18 | 19 |
| base | 58 | 2,983 | 531 | 0 | 36 | 13 | 29 |

- **The joint error (B1):** 0 in every audited axis (provenance 0 of 537, technique 0 of 519, modifier 0 of 500, construct 0 of 524, base 0 of 528); all five pass at 2 %.
- **Rater agreement,** Fleiss' kappa: provenance 0.990, technique 0.973, modifier 0.947, construct 0.982, base 0.993.
- **Why cells were routed:** 638 settled cells by cause: 545 rater error (almost all llm-2's dissent), 61 convention gap, 32 header ambiguity, 0 rule bug, 0 reader slip.
- **Protocol truth** decides 349 cells. Where it disagrees with every rater it is the truth table that is stale, and the correction record fixes those cells (C1 and D8 for acquired BOLD time series, D1, D2, D5 and others).
- **Misbehaviour:** 1,058 flags, of which 787 match the checker's known false-positive patterns. The other 271 were read one by one (`scoring/flags-read.md`): all 271 are checker artefacts (a reason naming the family or a rejected candidate, a fact held in the session list, a b value in a name, a shortened quote, a guide threshold and the like), and no settled value rests on a slip.

## The correction record (`final-cert-read-corrections`)

Built by `rulings.py build --record final` (unchanged since `82c924b`, the state registered with the plan) from the settled reference and the packets alone, and sealed and mirrored before any rule answer on a drawn stack was read.
- **122 cells on 95 stacks:** D1 26, C1 20, D8 18, D2 17, D5 17, C5 6, ND5 6, D3 2, DR3 2, DR9 2, E1 2, DR6 1, S3 1, V12 1, W-CEMRA 1. By settle status: 113 truth, 8 consensus, 1 adjudicated. X8-BNR fired on none.
- **The audit against the corrected reference:** no audited cell is corrected, so 0 joint errors still.
- **The localizers' record:** 0 cells.

## Evidence

| run | what | files | manifest sha256 |
|---|---|--:|---|
| `final-cert-read` | the draw, the seal kit, packets, raters, routing, adjudication, the reference | 21,504 | `8d97073da08f720e32d5547e6dc5597b2b85dcd46eff952c97ec43ac74fd1425` |
| `final-cert-read-corrections` | the correction record, the localizers' record, the audit re-scored | 7 | `8e408c1e21e8b882c1b86b529e4e5720538747e5da32c27a3f712b69d051bd4a` |
