# Certificate 2: the reference is ready

2026-10-04. Run `final-cert-2-read` (`plan.md`, section 11, steps 9 and 10). This file holds counts only. Stack ids, per-cell values and every reason stay in the evidence archive. **No rule answer on a certificate stack has been read** when this is written: `final-cert-2-rules` is sealed and unopened, and `scoring/in/rules.jsonl` is empty. The grade (step 11) has not started.

**In short:**
- **The read:** 6,245 stacks were drawn and sealed, and 6,244 have a packet. 1 was left out for an identifier shape and counts as wrong in the grade (deviation 8). That leaves 6,144 non-localizers read in full and 100 localizers.
- **The audit:** 4,426 cells were drawn (15 % of each axis's agreements, from 934 stacks), and adjudicator-2 confirmed every one. Against the corrected reference, the correction record turns 2 audited cells into joint errors: 1 modifier cell and 1 construct cell. Every axis still passes the 2 % joint-error bar, with upper bounds of 0.33 % to 0.54 %. Per stack, 2 of 934 audited stacks hold one (upper bound 0.67 %).
- **The raters agree closely.** Fleiss' kappa runs from 0.965 (modifier) to 0.992 (provenance) on the five certificate axes.
- **llm-2's dissent added little.** Of the 832 cells it alone routed, 808 were settled to the raters' value, 22 to can't tell and 2 to its own value.
- **The correction record** (`final-cert-2-read-corrections`, final-32) corrects 127 cells on 103 stacks: 109 protocol truth, 14 adjudicated, 2 audited and 2 consensus. The localizers' record corrects none.
- **No stack was refused.** Every rater answered every stack, so the refused-stack list is empty. One permission refusal happened: `claude-adversarial` 065 was refused a read of the harness's saved copy of its own packet printout. It read its packets again from its folder in smaller parts, and no answer was lost.
- **Isolation holds.** Every launch's text was the registered pointer, every task file was unchanged, and no agent named another agent's folder.
- **Four deviations** belong to the read, all in `plan.md`:
  - 16: the two-part seal;
  - 17: agents started with a pointer to their task file;
  - 18: the usage-limit stop;
  - 19: llm-2 at concurrency 8.

## The reading

- **The three llm-1 raters** worked on prompts v3.4, in 105 batches each (103 of 60 stacks, one of 24 and the localizers' 40), at most 19 agents at once.
  - Answers: `claude-tags` 6,244 of 6,244, `claude-family` 6,244 of 6,244, `claude-adversarial` 6,244 of 6,244.
  - `validate.py`: no schema failure and no legality failure for any rater.
  - **The launches** (`tools/checkagents.py`, deviation 17's form of the sweep's `checkprompts.py`): 334 launches of 315 agents. The 19 `claude-family` batches answered again count twice. Every first message was the registered pointer, byte for byte, and every agent's `TASK.md` still had the sha256 recorded before its launch.
- **llm-2** (`27b-v34`, thinking off, temperature 0) answered 6,244 of 6,244 with no schema or legality failure.
  - It started at concurrency 6, was restarted after the reboot (deviation 18) and ran at 8 from 2,474 answers on (deviation 19).
  - The last 3,770 answers took 11,795 s. The default model was swapped back afterwards. Its identity is in `models/27b-v34.json`.
- **Routing** (`tools/route.sh`):
  - **Adjudicator-1:** 773 stacks with 876 cells. These are 860 disagreements, 832 of them routed by llm-2's dissent alone, and 16 truth checks.
  - **Adjudicator-2:** 1,071 stacks with 4,633 cells. These are 4,451 audit cells and 182 second opinions. The audit's draw is 4,426 cells on the five header axes, plus 25 extra cells listed in the report.
  - The stack left out for an identifier shape is not in the read. `to_inputs.py` now skips a drawn stack without a packet, the sweep's line under deviation 8.
- **The adjudicators** were 62 fresh agents (26 and 36), started as the raters were.
  - Every launch's pointer was identical and every `TASK.md` unchanged (62 of 62). Each agent answered all of its items: 773 of 773 and 1,071 of 1,071.
  - Adjudicator-2 confirmed all 4,451 audit cells. Of the 182 second opinions, it agreed with adjudicator-1 on 181 (99.5 %).
- **`check_adjudication.py`:** 0 problems for either adjudicator.

### The interruption and what the agents did outside their task (`plan.md`, deviation 18; `logs/agent-notes.txt`)

- **The usage limit** stopped every running agent at about 12:30 UTC, part-way through `claude-family`. The development laptop was then rebooted and the session moved to another account.
  - 46 `claude-family` batches were complete and are kept.
  - 19 were part-way. Their outputs were moved unread to `raters/_first/claude-family/`, and each batch was answered once more in full by a fresh agent with the same task file.
  - 40 had not started.
- **One collect ran early:** `agents.py collect raters` ran while `claude-adversarial` 001 to 019 were still working. Its 512 copies of their answers were deleted at once, so the agents' folders kept the originals. `claude-adversarial` was collected whole after all 105 batches were done.
- **No agent read another agent's folder.**
  - Two agents named their own stray file beside their folder: `claude-tags` 032 an empty file, and `claude-adversarial` 032 a dump of its own packets, deleted unread.
  - 73 rater launches and 2 adjudicator launches read the harness's saved copy of their own command output. Each copy was the agent's own (`checkagents.py`: 153 such reads, none of another agent's).
  - Three agents kept a scratch file of their own packets in the session scratchpad and removed it (`claude-family` 068 and 093, `claude-adversarial` 076). Each file was named by that agent alone.
  - Two agents wrote a scratch file inside their own folder and deleted it (`claude-family` 058 on its rerun, `claude-adversarial` 062).

## The reference

Non-localizers and localizers together. Localizers are asked provenance and technique only. These are the settled values, before the correction record.

| axis | truth | consensus | audited | audit-corrected | adjudicated | adjudicated can't tell | all can't tell |
|---|--:|--:|--:|--:|--:|--:|--:|
| provenance | 171 | 5,124 | 905 | 0 | 32 | 12 | 0 |
| technique | 120 | 4,972 | 883 | 0 | 268 | 1 | 0 |
| modifier | 52 | 4,923 | 880 | 0 | 268 | 3 | 18 |
| construct | 71 | 4,986 | 883 | 0 | 181 | 12 | 11 |
| base | 112 | 5,029 | 900 | 0 | 80 | 3 | 20 |

- **The joint error (B1), against the corrected reference:** every axis passes at 2 %.

  | axis | joint errors / audited cells | upper bound |
  |---|---|--:|
  | provenance | 0 of 905 | 0.33 % |
  | technique | 0 of 879 | 0.34 % |
  | modifier | 1 of 871 | 0.54 % |
  | construct | 1 of 881 | 0.54 % |
  | base | 0 of 890 | 0.34 % |

  The two cells are ones the record corrects: one by X9-DANTE (modifier) and one by W-K2 (construct). Against the settled reference before the record, the audit found 0 joint errors.
- **Rater agreement,** Fleiss' kappa: provenance 0.992, technique 0.974, modifier 0.965, construct 0.976, base 0.991.
- **Why cells were routed.** Of 847 cells a person settled:
  - 785 rater error, almost all llm-2's dissent;
  - 42 convention gap;
  - 20 header ambiguity;
  - 0 rule bug and 0 reader slip.
- **Misbehaviour flags:** 1,549 in all.
  - 1,138 match the checker's known false-positive patterns.
  - The other 411 were each read by one of three agents against the stack's packet, the party's answer and the settled cell (`scoring/flags-read/verdicts-*.tsv`). 399 are checker artefacts.
  - 12 are real slips, all on construct and all the same one: the reason says a Siemens `*ep_b1000t` trace stack holds "b0 and trace" images, from its two images per slice, while the header lists b 1000 only. The cells are on 6 stacks, from `claude-tags` 4, `claude-family` 5 and `claude-adversarial` 3.
  - No settled value rests on a slip. Each of those stacks is settled ND+Trace from the sequence name, the series name and ImageType.

## The correction record

Built by `rulings.py build --record final` as registered: final-32, sha256 `83d220db92bf6cbd1936193e2dd5edf734a4ded18f111ce294b1dc47bc46be18` (`plan.md`, section 8; unchanged since the registration commit `c75f44a`). It was built from the settled reference and the packets alone. It was sealed and mirrored before `final-cert-2-rules` was opened, and that run has still not been opened.

- **127 cells on 103 stacks.** By rule:
  - D1 24, C1 17, C5 16, D5 16, D2 15, D8 13;
  - D12 9, ND5 7, E1 3;
  - W-K2 2, X9-DANTE 2;
  - D7 1, R3-SWIME 1, X10-TRACE 1.
- **By settle status:** 109 truth, 14 adjudicated, 2 audited and 2 consensus (`scoring/audit-rescored.txt`).
  - Most truth cells are provenance (57). The raters agreed with the protocol printout's provenance on only 111 of 168 stacks, as on the sweep (46 of 69).
  - The 2 audited cells are the joint errors above.
- **The localizers' record** corrects 0 cells.

## The sealed runs

| run | what | files | manifest sha256 |
|---|---|--:|---|
| `final-cert-2-read` | the draw, packets, prompts, the four raters, routing, the adjudicators, the settled reference, the flags, the logs and code | 35,436 | `4fa1b974976a4b7c5b76220a577e5252f1299fa1940deaad0dceb788edd8b780` |
| `final-cert-2-read-corrections` | final-32's record over the settled reference, the localizers' record, the audit re-scored | 9 | `da9169214291546c928e186c4b84b87a3ad7de5bb3af5b12a40059c8fc5dc50d` |
| `final-cert-2-rules` | the rules' stored answers at the draw, sealed unread (step 7) | 32 | `757e12b05deec381f273a065bebccbeda57a87a7ee8279475160ac1019c491c3` |

`final-cert-2-read` was sealed at this stage over its earlier seal, as the plan allows the working run. The first two runs are mirrored to the research container through Bifrost and checked there against their manifests. The manifests' copies are in `../phase0-evidence/runs/`.
