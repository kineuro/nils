# The final header certificate's read: the plan

**REGISTERED** 2026-10-02 by the commit that fills every blank below, after the NILS server's reclassify with pack 0.23.0 (job `pack-0230`, id 367) was seen finished with rc 0 (read only) and before the frame and the draw ran. Written 2026-10-01, brought up to date 2026-10-02 for pack 0.23.0 ("The amendments of 2026-10-02"). Nothing is drawn, sealed or read. This is the final certificate of record 48 ("The final certificate's design", Nima 2026-09-30: "agreed on certificate"), written so that its draw can start with no tool left to write. It follows the approved draft (`../2026-09-30-final-certificate/plan-draft.md`) and reuses dev-7's and dev-8's tools (`../2026-10-01-dev7-reference/`, `../2026-10-02-dev8-reference/`) at the larger scale.

> **Warning: this plan is registered.** The registration is the commit that filled every blank below and changed the header to REGISTERED, before the frame and the draw run. Any later change is a deviation and is reported as one.

> **Warning: the go condition and the pack are amended (Nima, 2026-10-02).** dev-7 did not show the go condition; dev-8 showed the clean rate near 0.5 % but not its bound. Nima ruled that the rate is enough and that pack 0.23.0, which fixes 6 of dev-8's 7 errors, goes straight to this certificate as stored from job `pack-0230`, with no dev-9 ("The amendments of 2026-10-02"; record 48, "The final certificate goes on pack 0.23.0"). The NILS server's reclassify with it finished on 2026-10-02 (job `pack-0230`, id 367, engine 1.0.0-alpha.71, rc 0).

> **Warning: graded 2026-10-02, and the certificate does not pass** (`grade.md`, part 2). The main line has 32 of 2,500 wrong (Hajek 1.28 %, Korn-Graubard upper bound 1.72 %) against the 1 % bar. Every make passes its 3 % bar. 24 of the 32 are the pack's own rule bugs or gaps.

## In plain words, for Nima

**What it will say, if it passes:** "Of the clean stacks NILS keeps, outside the sessions any header read has touched, at most 1 % have any of the ten header axes wrong, at 95 % confidence; and at most 3 % for each of Siemens, Philips and GE." Kept-uncertain and the old void's kept reformats and scouts get their own bounds. The rare cells, the localizers, body part and post-contrast are reported apart, as ruled.

**What is ready:** the frame, the draw, the weights, the seal kit for the NILS server, the packets, the batches and agent folders, the llm-2 voter, routing, adjudication and the lines with the per-make bars. Each was run on a dry run in the research container or on the development laptop with throwaway seeds, and the seal kit on a scratch Postgres registry (section 12).

**Nima's decisions, taken 2026-10-01** ("Let's go with your recommendations"; record 48, "The final certificate's five open choices"; section "Nima's decisions"), with decision 5 amended on 2026-10-02:
1. **The picture-read stacks leave the frame, by stack,** so the seal never touches a campaign's label set (the body-part gold sets above all). This amends record 48's ruling that they stay in.
2. **Round 9's guard drops the final's sampled sessions, not their subjects,** and round 9's grade gives every line on the stacks whose subject never trained as well.
3. **This certificate's samples do not join record 50's G3 union** for the body-part models.
4. **llm-2 reads 6 at a time,** dropping to 2 if the model server errors.
5. ~~The pack certified is the pack dev-7 passes, as stored.~~ **Amended 2026-10-02: the pack certified is MRI pack 0.23.0 as stored, from job `pack-0230`** (engine 1.0.0-alpha.71), with no further change and no development read of its own.

**Nima's amendments, 2026-10-02** ("Certificate next (Recommended)"; section "The amendments of 2026-10-02"): the go condition is a near-0.5 % clean rate on the last development read, without its bound (dev-8: 0.66 %), and the pack is 0.23.0 (decision 5 above).

**The frame as it will be drawn** (the third dry run, 2026-10-02, after dev-8's draw and the NILS server's reclassify with pack 0.23.0): **230,208 clean non-localizers** and 6,932 kept-uncertain. Every production stack outside the sealed ones holds pack 0.23.0 from job `pack-0230` (548,714 of 548,714). (The second dry run, 2026-10-01, with pack 0.21.0 and dev-1 to dev-7 out: 238,875.)

**Cost and time** (the third dry run's draw of 3,744 stacks): about 236 to 252 Claude agent runs (189 raters, about 47 to 63 adjudicators), about 3.5 to 3.7 times a development read. dev-6's read hit the account's usage limit once, so the raters go one rater at a time. Wall time from "go" to a settled reference: about 5 to 6 hours with llm-2 at 6 (about 17 to 18 hours if it has to drop to 2). Your part is two commands on the compute server and the NILS server (section 4).

## What waits, and what is fixed

| blank | what fills it | when |
|---|---|---|
| ~~the pack~~: version **0.23.0**, job **`pack-0230`** (id 367, finished 2026-10-02 08:51:31+02), engine **1.0.0-alpha.71** | MRI pack 0.23.0 as stored (decision 5 as amended on 2026-10-02; `config.json` `frozen`); the third dry frame showed every stored row `pack-0230`, and the NILS server's `nils --version` says 1.0.0-alpha.71 | registered |
| ~~the prompts~~ | **v3.3** (`tools/PROMPTS.sha256`; section "The prompts v3.3") | done, 2026-10-02 |
| ~~the seeds~~: draw **`final-cert-draw-2026-10-02`**, batches **`final-cert-batches-2026-10-02`**, A/B order **`final-cert-ab-2026-10-02`**, audit **`final-cert-audit-2026-10-02`**, second opinion **`final-cert-second-2026-10-02`** | in `config.json` (`seeds`, `ab_seed`, `audit.seed`, `adjudicators.second_seed`, `batches.seed`) | registered |
| ~~the frame commit~~ | **the commit that registers this plan**; `installF.sh final-cert-read-code-5` writes it into the code run as `COMMIT`, and `runF.sh draw` refuses to run without it | registered |
| ~~the exclusion lists' sha256~~ | `logs/inputs.sha256` of `runF.sh setup` (2026-10-02, code run `final-cert-read-code-4`), sha256 **`361cede0592e60d090b2b95d4cb2534cf2f4729f207ad026836dcd2c1a4ebf34`**; its lines in section 1 | registered |
| ~~Nima's decisions 1 to 5~~ | taken 2026-10-01; in `config.json` (`frame.picture_read_stacks` exclude, `learners.round9_guard_for_final` session, `bodypart_g3`, `panel.llm2_concurrency` 6, `frozen.rule`) | done |
| ~~the amendments of 2026-10-02~~ | decision 5 and the go condition (`config.json` `frozen.rule`, `scoring.go_condition`) | done |

Fixed by record 48 and not open: the ten axes (body part and post-contrast out), empty is wrong, N8 and decided none, the rules never vote, the audit and its bars, the three llm-1 raters with llm-2 dissenting, the two adjudicators with the raters' reasons, the main size 2,500, the supplement 250, kept-uncertain 600, the old void's kept 300, localizers 100, Hajek with Korn-Graubard, clean at 1 %, each make of 299 or more main reads at 3 %, System 1 round 9 trained after the seal, study B in the frame, the claim covering the frame only.

## Nima's decisions (taken 2026-10-01)

Nima, 2026-10-01: "Let's go with your recommendations." Recorded in record 48, "The final certificate's five open choices".

1. **The picture-read stacks leave the frame, by stack** (`config.json` `frame.picture_read_stacks`: `exclude`). They are every production campaign's items and pair sides, post-contrast read r1's singles, anchored read r2's panels and the production host's campaign items; their sessions stay. In the first dry run they were 5,979 stacks of the frame (clean non-localizers 4,911), and 18 to 28 drawn stacks were body-part gold items in five or six of the six gold rounds. Sealing those would have marked the label sets sealed (record 42). With the rule, no drawn stack is a campaign item (second dry run: 0 of 3,743). The claim covers the frame less those stacks. This amends record 48's design 1, under which picture reads stayed in.
2. **System 1 round 9's guard drops the final's sampled sessions, not their subjects** (`learners.round9_guard_for_final`: `session`). G3 by subject stays for sealed-1. By subject, round 9 would lose about 196,000 more stacks and 68 % of the archive would sit on sealed subjects; by session it loses the drawn sessions, about 61,000 stacks. Round 9's grade gives every line on the stacks whose subject has no training stack as well. This is a deviation in `learners-r9.md`, written before its `check` (listed there).
3. **This certificate's samples do not join record 50's G3 union** (`bodypart_g3.final_samples_in_union`: false). That union keeps a gold stack from training a graded body-part model; about 1,450 of the 3,218 gold items sit on the subjects the draw adds, and this certificate does not grade body part.
4. **llm-2 reads 6 at a time** (`panel.llm2_concurrency`: 6), resumable, dropping to 2 if the model server errors; the drop is logged and reported. At 6 I expect about 4 to 5 hours for 3,750 stacks (dev-4: 33 minutes for 1,000 at 8 with the shorter v2 prompt; dev-6: 15.3 s a stack at 2).
5. **The pack is the one dev-7 passes, as stored, with no further change** (`frozen.rule`). dev-7 grades pack 0.21.0 by its stored answers (job `pack-0210`, engine alpha.69), so that is what fills the blanks if dev-7 passes it. **Amended on 2026-10-02** (next section): dev-7 did not pass 0.21.0, and the pack certified is 0.23.0 from job `pack-0230`.

## The amendments of 2026-10-02

Nima, 2026-10-02, asked whether to start the certificate now or read a dev-9 first, answered "Certificate next (Recommended)". Recorded in record 48, "The final certificate goes on pack 0.23.0".

**The evidence.** dev-8 (`../2026-10-02-dev8-reference/grade.md`, part 2) graded pack 0.22 (stored, job `pack-0220`) at 7 of 900 non-localizers wrong or empty, all clean: a clean W rate of **0.66 %**, Korn-Graubard upper bound **1.55 %** (without its one dispute, 0.55 % and 1.39 %); kept-uncertain 0 of 332; localizers 0 of 100. The rate is near 0.5 %, the bound is not under 1 %. Pack 0.23.0 (`../2026-10-02-pack-0230/README.md`; kineuro/nils#344, engine 1.0.0-alpha.71) fixes dev-8's 6 rule-bug stacks and settles its dispute (a Siemens BLADE `*tseBNR` is VFA-TSE, X8-BNR); modelled on dev-8 for information only, it is at 0 of 900.

**The reason, as Nima saw it:** "0.22 is already at 0.66 % (0.55 % without the dispute) and 0.23 removes 6 of its 7 errors; about 78 to 87 % chance to pass. A dev-9 would use up kept-uncertain stacks the certificate needs."

**Amendment 1, of decision 5.** The pack certified is **MRI pack 0.23.0 as stored, from job `pack-0230`** (engine 1.0.0-alpha.71), with no further change. It is no longer the pack a development read passed: pack 0.23.0 has no development read of its own, and this certificate is its first grade on a read it took no fix from.

**Amendment 2, of the go condition** (record 48, design 5, as dev-7 registered it: the clean W rate and its Korn-Graubard bound near 0.5 %). **A near-0.5 % clean rate on the last development read is enough, without the bound.** dev-8's 0.66 % meets it.

**What follows in this plan:**
- **The pack:** 0.23.0, job `pack-0230`, engine 1.0.0-alpha.71 (`config.json` `frozen.expected`; written into `frozen` at the registration).
- **The exclusions:** dev-8 joins the header reads, out by session (dev-1 to dev-8; `tools/subst.py`, `runF.sh setup`), and `logs/inputs.sha256` covers its draw and its manifest.
- **The prompts v3.3** (section "The prompts v3.3"): v3.2 plus the reading guide's VFA-TSE line naming `*tseBNR…`, as the pack 0.23 study asks at the next read's registration.
- **The correction record:** `rulings.py build --record final` as it stands, X8-BNR included (section 7).
- **The packets:** unchanged (section 6). Pack 0.23.0 shows a reader no private element that the packets lack.
- **The frame, the costing and the kept-uncertain sample** from a third dry run after the reclassify (sections 1, 2 and 10).

## 1. The frame

`tools/frame.sql` (dev-7's query with the header reads apart from the picture reads) and `tools/frame.py`, in the research container as `nils_research` in a read-only transaction (`tools/runF.sh frame`).

**The strata** (record 48, design 2):
- **The registered stratum** is Phase 0's rule as dev-6 and dev-7 read it: the production host's stratum by key (dev-5's `v0keys.tsv`); study B's other stacks by the legacy storage server's v0 (`final-cert-frame-out`); an uncertain stack outside the production host takes dev-6's frame's stratum (job 228's disposition), else today's.
- **The final stratum:** a stack the frozen pack excludes today is void, whatever its registered stratum (NILS does not keep it, and a selection cannot hold it). Otherwise registered clean is **clean**, registered kept-uncertain is **kept-uncertain**, and a registered void stack the pack keeps is **void-kept**, the old void's reformats and scouts.

**Out of the frame** (design 1): every sealed stack (now or ever) and its session; every stack of a session holding a header-read stack. The header reads are dev-1 to dev-8 (dev-8 added on 2026-10-02) and remeasure-1 and -2 (their draw lists), production campaign `read-r1`, the production host's `read-r1-nima`, `read-r1-russell` and `reader-check` (502 keys), and the pilot's protocol-truth draw. The picture-read stacks themselves (decision 1). Counted, never excluded: sealed subjects, picture-read sessions, read subjects.

**The frozen-pack check:** `frame.py` stops unless every stored answer is the frozen pack from the frozen job, and unless no frame stack is unclassified.

**The dry runs** (2026-10-01, `runF.sh dry`, throwaway, the pack check lifted; the query takes 14 seconds over 550,680 stacks):
- **the first,** before Nima's decisions: production still stored pack 0.20.0 (job `pack-0200`), dev-7 had no draw yet, and the picture-read stacks stayed in;
- **the second,** after them (code run `final-cert-read-code-3`): every production stack outside the sealed ones holds **pack 0.21.0 from job `pack-0210`** (548,714 of 548,714; none unclassified), dev-7's 1,000 drawn stacks are among the header reads, and the picture-read stacks are out;
- **the third,** 2026-10-02, after the amendments and the NILS server's reclassify with pack 0.23.0 (code run `final-cert-read-code-4`, prereg, seed `dry-f`): every production stack outside the sealed ones holds **pack 0.23.0 from job `pack-0230`** (548,714 of 548,714; none unclassified), dev-7's and dev-8's 2,000 drawn stacks are among the 8,600 header-read stacks, and the picture-read stacks are out. This is the frame the draw will take if nothing is read or sealed before it.

| final stratum (registered) | archive |
|---|--:|
| clean (clean) | 418,642 |
| kept-uncertain (kept-uncertain) | 50,894 |
| void-kept (void, kept today) | 80,174 |
| void (void, excluded today) | 970 |

The same under pack 0.20.0, 0.21.0 and 0.23.0: no clean or kept-uncertain stack is excluded.

| the frame (third dry run, 2026-10-02) | non-localizers | Siemens | Philips | GE | other | localizers |
|---|--:|--:|--:|--:|--:|--:|
| clean | **230,208** | 162,072 | 34,715 | 33,416 | 5 | 69,620 |
| kept-uncertain | **6,932** | 2,275 | 417 | 4,238 | 2 | 20,325 |
| void-kept | 56,434 | 24,998 | 216 | 31,204 | 16 | 786 |

| the frame (second dry run, 2026-10-01) | non-localizers | Siemens | Philips | GE | other | localizers |
|---|--:|--:|--:|--:|--:|--:|
| clean | 238,875 | 167,165 | 36,654 | 35,051 | 5 | 71,234 |
| kept-uncertain | 7,502 | 2,529 | 417 | 4,554 | 2 | 21,652 |
| void-kept | 58,669 | 25,456 | 225 | 32,972 | 16 | 801 |

- The third dry run: 27,558 sessions of 5,088 subjects (the second: 28,493 and 5,128; the first: 29,410 and 5,160). The kept-uncertain non-localizers the draw takes 600 of: GE 4,238, Siemens 2,275, Philips 417, other 2.
- The third dry run equals the pack 0.23 study's frame with dev-8 out (`../2026-10-02-pack-0230/README.md`, taken under pack 0.22.0): the reclassify moved no stack between strata.
- Out of the clean non-localizers (a stack counted once): 91,371. The header-read sessions remove 75,870 (67,253 before dev-7), the sealed sessions 16,465, and the picture-read stacks 4,911 that nothing else removes.
- The clean frame went from the draft's 261,909 to 252,403 with dev-6's sessions, then to 238,875 with dev-7's sessions and the picture-read stacks, then to 230,208 with dev-8's sessions.

**The exclusion lists' sha256** (`logs/inputs.sha256` of the real setup, `runF.sh setup` on 2026-10-02 with code run `final-cert-read-code-4`; the file's own sha256 `361cede0592e60d090b2b95d4cb2534cf2f4729f207ad026836dcd2c1a4ebf34`):

| input | lines | sha256 |
|---|--:|---|
| `dev-5/source/v0keys.tsv` (the production host's stratum by key) | | `052f932c101439b81a742e8e3d155a02c7f8b28fb85d91518631770f660586da` |
| `final-cert-frame-out/scoring/frame.tsv` (study B by the legacy storage server's v0) | | `1f81d9842ee448fd757d8268046285875dc8ef3dfc169d1b6d1ee37e05a96845` |
| `dev-6/draw/frame.tsv` (job 228's disposition) | | `7e36ffeeb1edecf727759614be9e56792e5f1864c3b5529ebbe28c53088eb9db` |
| `dev-5/MANIFEST.json` | | `d0325cb184a5cfeca6cff7d3275e9c2a68026d95431a455d92ea8024ce96d90e` |
| `dev-6/MANIFEST.json` | | `ce03330b32d73a6961db7954b7bf7a18075db97b328383c5dedaea037fba7a97` |
| `final-cert-frame-out/MANIFEST.json` | | `689536a9539b3c2c4cbe4cd64aae7cb1d0c36d412abb7cf680552a59acb54cd3` |
| `final-cert-frame-2/MANIFEST.json` | | `867765a7408e99274a16d4255e1467f7efa334540f9c8d55d6f596ddd7ff3f47` |
| `dev-7/MANIFEST.json` | | `0b95c71fe8c58dfdbd17f9ff51793b132093a6b0c3af371a924c9264971510a8` |
| `dev-8/MANIFEST.json` | | `fb79c93ac183c04e3aa1184afb7850595729d08da51c7d4915a39b127838aedd` |
| `excl/dev-1.stacks` | 1,000 | `464714f8e4aaf8964475a3bff881bdffe7c491bfcf976a66dbb8a1301d4f7121` |
| `excl/dev-2.stacks` | 1,000 | `f9a7816d366d52326949a4d83b09b7f63c4e3f7dd5fc75823cc5ae5749a9b70b` |
| `excl/dev-3.stacks` | 1,000 | `caf25cc31f92f158c4a950e5a335afe675c7aafb45a639cb6adb48ee35e52501` |
| `excl/dev-4.stacks` | 1,000 | `46a97cc9ede360e72486cb3066672dfc1973e76ff14c78a11b3594382c7a5cad` |
| `excl/dev-5.stacks` | 1,000 | `4c589e0e311ed8b20155789b414bbd37e69b5f26d425b911bea197c97395b0ae` |
| `excl/dev-6.stacks` | 1,000 | `9928f6d1fc87acc268980855a2667b529e7c594b038acd8358ef64e80d8b8862` |
| `excl/dev-7.stacks` | 1,000 | `69e9aaaf35845d62f389e5ef6f2a6d9932728016e31a64212fc1945923b3d835` |
| `excl/dev-8.stacks` | 1,000 | `f8891ddc66897031b9e649e040565ad0dd4f7a0427b1baea8b124650de56bf44` |
| `excl/v0-campaign.keys` | 3,176 | `b1b43b8e8495b7d7bec1cfd8dc8685c7d345aeb3c6586891695b66984cc389b2` |
| `excl/v0-header.keys` | 502 | `46e292006beb3a1618da22d48dcb628665803023edc520cc0d642b547e505b27` |
| `excl/pc-anchored-r2.stacks` | 560 | `63d3aac17c810945e744b007c2cdf37b68bd10c277c0e8bb63f846e42fe5fe7f` |
| `excl/pc-single-r1.stacks` | 100 | `1c75f570ade9f68822fb70baf1770b7a8e47cc36f8c2341bf97377ba2982869e` |
| `excl/remeasure-1.stacks` | 300 | `fae065bc610b23468d467cf4795f9b42ecfcedbf6f2593157e57cd3ecdf00663` |
| `excl/remeasure-2.stacks` | 300 | `cb18a565fa32049f650c735756159890686fcc3d7ce04b5fed429f8600b7d374` |

## 2. The draw

`tools/draw.py RUN SEED`, seed **`final-cert-draw-2026-10-02`**. Five samples, each with exact inclusion probabilities and no caps (caps would break them):

| sample | from | size | design |
|---|---|--:|---|
| **main** (the claim) | clean non-localizers | 2,500 | proportional to make x field (strata whose share is under one read pooled into one stratum, every stratum at least one read); within a stratum a systematic draw from a seeded start over the stacks sorted by era, stored technique and key; pi = n_h / N_h |
| **supplement** (apart) | clean non-localizers in the rare cells | 250 | the cells (make, field, era, technique) the main sample reaches less than once in expectation; 250 over the makes by the sum of N_c^0.75, within a make by N_c^0.75 (the draft's `design.py` allocation); within a cell the first n_c in draw-key order; independent of the main sample, so a stack in both is read once and counted in both |
| **kept-uncertain** | kept-uncertain non-localizers | 600 | as main |
| **void-kept** | void-kept non-localizers | 300 | as main |
| **localizers** (apart) | localizers of the three kept strata | 100 | proportional to stratum x make, systematic over field, era, technique and key |

A drawn stack whose packet has no header is listed and left out, never swapped (dev-7's rule).

After the draw: `tools/weights.py` checks every weight against the frame alone, `tools/impact.py` counts what the seal would take (section 5), and `tools/disjoint.sql` counts the drawn stacks on production against every exclusion. Sealed stack, sealed session, header-read stack and header-read session must all be 0, or I stop.

**The dry draws** (throwaway seeds, never kept: `dry-f` on the third dry frame, the frame as registered; `dry-d` and `dry-e` on the second; `dry-a` to `dry-c` on the first):

| | **dry-f (third frame)** | dry-d | dry-e | dry-a to dry-c (first frame) |
|---|--:|--:|--:|--:|
| unique stacks (main and supplement shared) | **3,744 (6)** | 3,743 (7) | 3,743 (7) | 3,746 to 3,749 (1 to 4) |
| main: Siemens / Philips / GE / other | **1,760 / 377 / 363 / 0** | 1,749 / 383 / 367 / 1 | 1,749 / 383 / 368 / 0 | 1,722 / 407 / 371 / 0 |
| supplement: GE / Philips / Siemens / other; rare cells reached | 79 / 99 / 71 / 1; 165 of 304 | 80 / 97 / 73 / 0; 157 of 304 | same | 82 / 94 / 74 / 0; 161 of 305 |
| **kept-uncertain: GE / Siemens / Philips** | **367 / 197 / 36** | 365 / 202 / 33 | same | 347 / 219 / 34 |
| void-kept: GE / Siemens / Philips | 166 / 133 / 1 | 168 / 131 / 1 | same | 171 / 128 / 1 |
| localizers: clean / kept-uncertain / void-kept | 77 / 22 / 1 | 76 / 23 / 1 | 76 / 24 / 0 | 76 / 23 to 24 / 0 to 1 |
| sessions (with 2 or more drawn; most) | 3,363 (321; 6) | 3,412 (292; 5) | 3,391 (310; 5) | 3,369 to 3,407 (301 to 323; 5) |
| subjects (with 2 or more; most) | 2,286 (891; 12) | 2,341 (919; 8) | 2,299 (924; 8) | 2,314 to 2,336 (883 to 919; 9 to 10) |
| drawn stacks that are campaign items | 0 | 0 | 0 | 46 to 77 |

- **Kept-uncertain is met in every make** on the third frame: the 600 drawn (GE 367, Siemens 197, Philips 36) are under a tenth of its 6,932 non-localizers (GE 4,238, Siemens 2,275, Philips 417), so dev-8's shortage, which was its own frame's (sealed subjects out), does not reach this draw.

- **Every make of the main sample has 299 or more reads,** so all three are held to 3 %.
- **The rare cells** (second frame): 304 cells under 96 stacks each, 6,288 stacks, 2.6 % of the clean frame (the draft: 311 cells, 3.1 %). "Other" makes hold 4 cells and 5 stacks and got no read on dry-d; on the third frame the rare cells hold 6,129 stacks in 304 strata, and dry-f gave "other" one supplement read.
- **Kish's n** (dry-f): main 2,499.6, supplement 223.0, kept-uncertain 599.9, void-kept 299.8, localizers 99.9. On dry-d, the main sample was self-weighting with one pooled read of 37 stacks carrying weight 37 against about 96, and the supplement 227.9.
- **The main sample's mix** (dry-a) equals the frame's to a tenth of a percent by make, field and era. By cohort, study A 77.2 % (frame 78.0 %) and study B 9.0 % (8.6 %).

## 3. The weights

`draw.py` writes `draw/weights.tsv`, one row per stack and sample: its design stratum h (make x field or `pooled`; the rare cell for the supplement; stratum x make for the localizers), n_h, N_h, pi = n_h / N_h and w = N_h / n_h. A stack in both main and the supplement has a row for each. `weights.py` recomputes the pooling and every N_h from the frame alone and stops on any difference. The weights of each sample sum to its frame, so the Hajek denominator is the frame's size. Written before any answer is read.

## 4. The seal

One selection holds every drawn stack once and is sealed once (`tools/seal_export.py`, `tools/seal-server.sh`). Which sample a stack belongs to stays in the sealed draw, never in the registry. From the seal on, every door withholds what a system said of these stacks, and the reading starts only after it.

The kit (`runF.sh export`, in the run's `seal/`):
- `final-cert.select.json`, the selection document in `seal_samples.py`'s shape;
- `stack-ids.txt`, `frozen-job.txt`, `frozen-pack.txt`, `name.txt`, `summary.json`;
- `seal-server.sh` and `KIT.sha256` over all of them.

`seal-server.sh` stops with exit 3 and writes nothing unless:
- every file matches `KIT.sha256`;
- the installed MRI pack is the frozen pack, and every drawn stack's stored answer is the frozen pack from the frozen job;
- no drawn stack is sealed or excluded, no selection of that name exists, and no classify job is unfinished.

It then saves the selection, seals it, and checks that exactly the drawn stacks are sealed under `selection:final-cert@1`. It prints one line.

**What it changes:** in the production registry, one selection with its version and about 3,750 `sealed_stack` rows, with their audit. **Rollback:** a sealed sample is unsealed only by a recorded certificate (record 48), so a mistaken seal is mended by restoring the snapshot of step 1.

**Nima's commands** (after I say the draw is sealed in evidence and give `KITSHA`, the sha256 of `KIT.sha256`):

1. On the development laptop, snapshot the registry's datasets (recursive: the database is the child the registry's database dataset, as every `before-alpha*` snapshot holds it):

   *(The operator's command is kept in the private record.)*

2. On the development laptop, push the kit into the NILS server, checked on the compute server, and seal as nils with the engine unit's environment:

   > **Warning: `NILS_PACK_DIR` is required.** Without it the CLI finds a stale pack 0.8.0.

   *(The operator's command is kept in the private record.)*

   It prints:

   `text
   engine nils 1.0.0-alpha.71; pack 0.23.0; job pack-0230 0.23.0 <n>; selection final-cert@1; drawn <n>; sealed now <n>; by selection selection:final-cert@1 <n> ; sealed outside the draw 0; stderr lines 0
   rc=0
   `

   `<n>` is the number of drawn stacks with a header (about 3,744; `seal/summary.json`). `rc=3` means a check stopped it and nothing was written: tell me. `rc=4` means the seal does not cover exactly the drawn stacks: tell me before anything else.

## 5. What the seal takes from training

`tools/impact.py`, counted on each dry draw before any seal. The first three draws are what led to decisions 1 to 3; dry-d and dry-e are on the frame as decided, and dry-f on the frame as registered (dev-8 out, pack 0.23.0).

| | dry-a | dry-b | dry-c | dry-d | dry-e | dry-f |
|---|--:|--:|--:|--:|--:|--:|
| subjects drawn; of them already sealed (sealed-1) | 2,333; 719 | 2,314; 736 | 2,336; 741 | 2,341; 756 | 2,299; 743 | 2,286; 774 |
| subjects the draw adds | 1,614 | 1,578 | 1,595 | 1,585 | 1,556 | 1,512 |
| archive stacks on them | 201,172 | 197,716 | 196,230 | 196,292 | 196,659 | 188,236 |
| archive stacks on sealed subjects, before (175,715) and after | 376,887 | 373,431 | 371,945 | 372,007 | 372,374 | 363,951 |
| archive stacks in the drawn sessions (what round 9 loses under decision 2) | 60,896 | 61,273 | 61,644 | 60,883 | 60,745 | 59,802 |
| drawn stacks that are body-part gold items (rounds touched) | 18 (5) | 27 (6) | 28 (6) | 0 | 0 | 0 |
| body-part gold items on added subjects (of 3,218; outside record 50's union by decision 3) | 1,477 | 1,485 | 1,475 | 1,411 | 1,470 | 1,412 |
| drawn post-contrast pair items | 4 | 7 | 8 | 0 | 0 | 0 |

**What round 9 needs from the draw** (`../2026-09-24-phase-0/learners-r9.md`, section 0, step 7):
- the samples sealed on production before round 9's `check`: the selection `final-cert@1`, whose stacks the research views then drop;
- the frozen pack and job, since round 9 is taught and featurized by the pack the final certificate certifies. `learners-r9.md` expected 0.20, so a deviation there names the frozen pack (0.23.0, job `pack-0230`, by the amendments of 2026-10-02) before its `check`. Its note of 2026-10-01 still names 0.21.0 and dev-7, and is brought up to date when the deviation is written;
- dev-7 and dev-8 among its guard's seeds, as development reads graded after round 8;
- decision 2: its guard drops the drawn sessions, not their subjects, with every grade line also on the stacks whose subject never trained; written into `learners-r9.md` as a deviation before its `check` (listed there on 2026-10-01).

## 6. The packets

dev-7's and dev-8's packets unchanged (`tools/hdr.sql`, `sess.sql`, `build_packets.py`, `shown.json`; the identifier checks), built on the development laptop by `tools/prep.sh` after the draw is pulled and sealed in evidence, from `source/reread.json` (dev-7's, brought up to date).

**Pack 0.23.0 shows a reader no private element the packets lack** (checked 2026-10-02 on kineuro/nils `f9b7daa`): `packs/mri/private.yml` is unchanged since pack 0.21.0 (`55ea193`), so `header.private` carries the same eight elements. Its new session pass (`passes/session_dti.yml`) reads the same sibling fields as the first (the maker, the image type, the six texts, the orientation, rows, columns, slice count and series number) and the frame of reference, all of which `sess.sql` already gives. The NILS server only reclassifies (no re-read), so `source/reread.json` stays dev-7's. So `hdr.sql` and `shown.json` gain nothing; only their notes name pack 0.23.0.

**The dry build** (the dry-a draw in a scratch folder on the development laptop, deleted after): 3,747 packets of 3,747 stacks in 16 seconds; no stack without a header; identifier, personnummer, 8-digit date and answer-key checks passed on every packet; 10.2 KB a packet on average (48 KB at most), 38 MB in all; a median of 15 session entries. 994 packets said "the fingerprint is at revision 0" and 185 "GE's series plane" not read, as production stood before the NILS server's chain.

## 7. The reading

As dev-7 (`../2026-10-01-dev7-reference/plan.md`, "What is the same as dev-6"), with:
- **The three llm-1 raters** (`claude-tags`, `claude-family`, `claude-adversarial` on the prompts v3.3, below), 60 stacks per agent, the same batches for each rater (`tools/batches.py`, seed **`final-cert-batches-2026-10-02`**), non-localizers and localizers never mixed, the samples mixed so no rater can tell them apart. Each agent in its own folder holding only its batch (`tools/agents.py`, under a scratch folder on disk, mode 700). The dry build: **63 batches (61 non-localizer, 2 localizer), 189 rater agents of 40 to 60 stacks.**
- **Waves** (`tools/agent_plan.py`): one rater's 63 agents at a time, so a usage limit stops at most one rater part-way (dev-6's `claude-tags` pass stopped on the account's limit with every batch part-way). Each agent's input is about 198,000 tokens (the most 225,000), estimated at 3.5 bytes a token.
- **llm-2 dissenting** (`tools/chain.sh`, run `27b-v33`, thinking off, temperature 0, max 3,000 tokens, prompt as the raters'), 6 at a time (decision 4), resumable, dropping to 2 if the model server errors; started as soon as the packets exist, beside the raters. Its client timeout is 120 s (`TIMEOUT`), as dev-8 finished its run after requests hung at the model server's front door for 900 s each (dev-8's deviation 1).
- **Routing, the audit and the adjudicators** as dev-7 (`tools/route.sh`, `route.py`, `to_inputs.py`, `adj_batches.py`, `settle.py`, `check_adjudication.py`, `triage_flags.py`), with this plan's seeds. `route.py` refuses blank seeds. Scaled from dev-6 (273 adjudicator-1 and 224 adjudicator-2 items per 1,000 stacks): about 1,020 and 840 items, about 35 and 28 agents of 30. dev-8 needed fewer (163 and 204 per 1,000), which would give about 610 and 765.
- **The correction record:** `rulings.py build --record final` (`../2026-09-28-certificate/tools/rulings.py`) as it stands at the registration commit, with every rule after dev-7 (X7-MEAN, X7-MEANMAG, X7-SWIMAG, X7-BAREB) and after dev-8 (**X8-BNR**: a Siemens sequence name starting `*tseBNR` settled other than VFA-TSE takes VFA-TSE), built from the settled reference and the packets alone into `final-cert-read-corrections`, sealed and mirrored before any rule answer is read; the localizers' record beside. At this plan's 2026-10-02 update its sha256 is `669123d57620e65628b695e364fe21c88fc3dfc6cb09550615b619f75f456048`.

### The prompts v3.3

Composed by `../2026-09-27-header-judge/tools/compose_prompts_v33.py` from the v3.2 files dev-8 registered, each checked first by the sha256 dev-8's plan lists (all five matched). Against v3.2, v3.3 changes three things and nothing else:
- the title says v3.3 (2026-10-02, after dev-8);
- **the reading guide's VFA-TSE line** names a Siemens BLADE `*tseBNR…` beside N12's `*tseBR…`, with `*tseBN…` staying TSE and `*tseBNIR…` IR-TSE (record 48, "After dev-8's grade"; `../2026-10-02-pack-0230/README.md`, "Outside the pack"). The reading guide (`../2026-09-27-read-r1-notes/reading-guide.md`) carries the same edit;
- **the traps' Siemens RESTORE line,** which said RESTORE comes "only" from `*tseR` or `*tseBR`, names `*tseBNR` too, so that the trap does not contradict the guide (the traps are amended where a later ruling supersedes one, as for v3 to v3.2). This is a second line beside the one the pack 0.23 study named (section 13, "The prompts' trap line").

The schema and the constraints are v3.1's, unchanged: pack 0.23.0 adds no value. The prompts are copied into the run's `prompts/` and checked against `tools/PROMPTS.sha256` before any rater reads a packet:

| file | sha256 |
|---|---|
| `prompt-v3.3.md` | `5aacf4bf0ac7ef75d13001b449dd2561f923ef2849d99fc9bf210e43879b7e10` |
| `prompt-v3.3-family.md` | `426c6df88f9b4a7dee2541d50f5d037cd9e130b62317ac470dfef7f9ad6f0ecb` |
| `prompt-v3.3-adversarial.md` | `baec60d83e54a1b63661289152f884c64761aa459377334293d511a3fceff3d7` |
| `schema.json` | `7b595f512eac6a745b46055ba7c505721ed5265c5ed81134a92cb114ce38d410` |
| `constraints.json` | `b36f3e15e72578388a2221a7fc70ef660b74aec0db6b3e85e464c7ae53971cac` |
| `adjudicator-v3.3.md` | `c22e980851931cf41284dd7801a4f224eff9f5fa9d34dac59f7edb7bc76fcdd3` |
| `adjudicator-2-v3.3.md` | `f2747e4a12df7ac49a77c870372feee2175095f0ddd9bc3027334d892f27b477` |
| `adjudicator-schema.json` | `603cbc6dd831f8036bbf24bd5f3f59f43f18407cdd754ca194fb8f0ed9e309f5` |

v3.2's, checked before composing: `prompt-v3.2.md` `72929195…`, `prompt-v3.2-family.md` `e315b3a4…`, `prompt-v3.2-adversarial.md` `748b0742…`, `adjudicator-v3.2.md` `44ca31b2…`, `adjudicator-2-v3.2.md` `8c7ba892…`.

## 8. The estimand, the lines and the bars

**The estimand:** the share of the frame's clean non-localizers with at least one of the ten certificate axes wrong or empty under the frozen pack's stored answers, against the settled reference with its correction record. Empty is wrong; the lenient line is beside.

`tools/lines_final.py` (standard library; `tools/stats_kg.py` is dev-7's exact and Korn-Graubard code, copied):
- **main, the claim:** the Hajek estimate (equal to the plain rate) with the Korn-Graubard one-sided 95 % bound over the design strata. Variance by Taylor linearisation, with replacement (no finite-population correction, which is conservative), n* = min(n, p(1-p)/v x (z/t_df)^2), df = n minus the strata, n* = n when no stack is wrong. The pass reading: the bound at or under 1 %.
- **per make:** each make's domain of the main sample, Hajek and Korn-Graubard; a make with 299 or more main reads is held to 3 %.
- **beside the claim:** exact Clopper-Pearson, Wilson at Kish's n, Kish's n and n*, the Jeffreys aggregate over makes and over make x field (20,000 draws, seed 20260928), and a session-clustered bootstrap (2,000, seed 20260928), since about 300 sessions hold two or more drawn stacks.
- **the supplement, apart:** per rare cell n, x and the exact bound, sorted by bound; per cell group (make x technique family); its own Hajek with Korn-Graubard over the cell groups.
- **kept-uncertain and void-kept:** each its own Hajek and Korn-Graubard bound, by make beside. The rules plus System 1 test runs in the grade (`certify.py`).
- **the localizers, apart:** provenance and technique by family, no bar.
- **The pass condition** (intersection-union, so no multiplicity correction for the gate): the main bound at or under 1 % and every make of 299 or more main reads at or under 3 %.

**Tested** on the dry-a draw (the first frame; the second's main sample differs by under 25 reads a make) with simulated outcomes (`tools/simulate.py`; seeded, never an answer). Every line, table and bar is computed end to end. The chance to pass, over 1,000 simulated samples on the draw's own structure:

| true error | pass (clean at 1 % and every make at 3 %) | clean at 1 % | Siemens at 3 % | Philips at 3 % | GE at 3 % |
|---|--:|--:|--:|--:|--:|
| 0.3 % in every make | 99.8 % | 99.9 % | 100 % | 100 % | 99.9 % |
| **0.5 % in every make** | **86.3 %** | 87.2 % | 100 % | 99.8 % | 99.1 % |
| 0.7 % in every make | 41.9 % | 42.3 % | 100 % | 97.7 % | 95.9 % |
| 1.0 % in every make | 4.5 % | 4.6 % | 100 % | 88.4 % | 84.6 % |
| Siemens 0.3 %, GE and Philips 1.5 % | 28.5 % | 50.1 % | 100 % | 59.7 % | 53.5 % |
| Siemens 0.4 %, GE 2.5 %, Philips 0.5 % | 9.2 % | 38.5 % | 100 % | 99.8 % | 10.7 % |

At a true 0.5 % the design passes 86 % of the time (the draft's exact figure for 2,500: 87 %). The per-make bars cost about one point there, and they catch a make that is bad while the archive's rate hides it (the last row). The end-to-end run at 0.5 % (12 of 2,500 wrong) gave a Korn-Graubard bound of 0.78 % with n* 2,496, equal to the exact bound, as a self-weighting sample should.

## 9. The grade (after the read)

Registered like dev-7's grade: `grade.md` part 1 with its tools, committed before any rule answer on the final samples is fetched. The fetch is a production operation by Nima on the NILS server with `--unsealed-access` and `NILS_PACK_DIR` set, as the sealed-1 checkpoint's was. It takes the frozen pack's stored answers and votes, and System 1 round 9's checkpoint. The grade runs once, and nothing found on it is fixed and graded on it again.

## 10. Cost and wall time

Counted on the third dry run's draw (`dry-f`, the frame as registered): 3,744 unique stacks, 3,644 non-localizers and 100 localizers.

| step | where | size | time |
|---|---|---|---|
| setup, frame, draw, weights, impact, disjointness, seal kit | The research container | 550,680 stacks read, about 3,744 drawn | about 2 minutes (the frame query: 14 s) |
| pull, seal in evidence, mirror | The development laptop, Bifrost | about 70 MB | about 10 minutes |
| snapshot and seal | Nima, the compute server and the NILS server | one selection, about 3,744 rows | about 5 minutes |
| packets and batches | The development laptop | about 3,744 packets, 63 batches (61 non-localizer, 2 localizer) | under a minute |
| llm-2 | the model server, the 27B by a cold swap | about 3,744 stacks | about 4 to 5 h at 6 (decision 4); 16 h if it drops to 2 |
| llm-1 raters | Claude agents | 189 agents, about 37 million input tokens | 3 waves of 10 to 20 minutes each, 45 to 60 minutes; 15 to 30 minutes if all at once |
| routing | The development laptop | | minutes |
| adjudicators | Claude agents | about 47 to 63 agents, about 5 to 7 million input tokens | 15 to 20 minutes |
| settle, flags, manifests, mirror, correction record | The development laptop, Bifrost | | about 45 minutes |

- **Claude agents:** about 236 to 252 runs. The adjudicators are scaled from dev-8 (163 adjudicator-1 and 204 adjudicator-2 items per 1,000 stacks: about 610 and 765, 21 and 26 agents of 30) and from dev-6 (273 and 224: about 1,020 and 840, 35 and 28 agents). Against dev-8's 64 runs and dev-6's 68 for 1,000 stacks, that is about 3.5 to 3.9 times a development read. The token counts are lower bounds, since a turn re-reads its context. If a wave hits the usage limit, its partial answers are kept apart unread (dev-6's rule) and the wave reruns, under the same account or another (the hub's `claude-account` skill).
- **The critical path is llm-2.** From "go" to a settled reference: about 5 to 6 hours with llm-2 at 6, about 17 to 18 hours if it has to drop to 2.
- **Then:** round 9 trains (`learners-r9.md`), and the grade takes about an hour once the reference and its correction record are sealed.

## 11. From the amendments to "the read runs"

1. ~~**The go condition.**~~ Met as amended on 2026-10-02 (dev-8's clean W rate 0.66 %, the rate alone; "The amendments of 2026-10-02").
2. ~~**The pack**~~ (decision 5 as amended), done 2026-10-02: the NILS server is updated to engine alpha.71 and reclassified with pack 0.23.0 as job `pack-0230` (output in the NILS server's working folder, a `done` file holding the rc), started after Nima's go. I check read only from the compute server (`pct exec 110 -- test -f …/done`) until `done` holds rc 0. Seen at 06:52:29Z with rc 0; the job (id 367) finished at 08:51:31+02, no classify job is unfinished, and the NILS server's engine is 1.0.0-alpha.71.
3. ~~**The prompts.**~~ v3.3, composed and hashed on 2026-10-02 (section 7, "The prompts v3.3").
4. ~~**The dry frame and the setup.**~~ Done 2026-10-02 (sections 1 and 12). dev-7's and dev-8's draws are in the evidence mirror. On the development laptop, install the uncommitted tools; then, in the research container, the dry frame (every stored row must be `pack-0230`), and the setup that writes the exclusion lists' hashes:

   *(The operator's command is kept in the private record.)*

   *(The operator's command is kept in the private record.)*

   The counts are taken from the dry folder (sections 1, 2 and 10), then:

   *(The operator's command is kept in the private record.)*

5. ~~**The registration:**~~ Done by the commit that registers this plan. I fill every blank (`config.json`: `frozen`, `seeds`, `ab_seed`, `audit.seed`, `adjudicators.second_seed`, `batches.seed`, the exclusion hashes; this plan's sections 1, 2 and 10), change the header to REGISTERED, commit as nima-ch and push. That commit is the registration and the frame commit.
6. ~~**The committed tools to the research container, then the frame and the draw**~~ (after Nima's go for the draw, 2026-10-02: "Go ahead"). Done 2026-10-02 with code run `final-cert-read-code-5` (`57a7b48`): the frame check passed (548,714 of 548,714 `pack-0230`), 3,750 stacks drawn (main 2,500, supplement 250 with 0 shared, kept-uncertain 600, void-kept 300, localizers 100), every weight checked, disjointness 0 on every exclusion:

   *(The operator's command is kept in the private record.)*

   *(The operator's command is kept in the private record.)*

   The frame stops unless every stored answer is the frozen pack's. The draw prints its counts, the weights' check, the impact and the disjointness counts, which must be 0 where required, or I stop.
7. ~~**Into evidence:**~~ on the development laptop, `mirror.sh pull final-cert-read the research folder`, `evidence.py init` and `seal`, `mirror.sh push`. I give Nima `KITSHA`. Done 2026-10-02: run `final-cert-read` sealed (86 files, manifest `43cfafdbf2178c82ad20f0bae890546c13f0ede5fee62ac8f0650eab93fd394f`) and mirrored; `KITSHA` `5c381f69d4c9505a6b6922a50ab4aaa9513f08dc51210d535ca6d0d417d8c102`.
8. ~~**The seal:**~~ Nima's two commands (section 4). From here the samples are sealed. Done by Nima on 2026-10-02: snapshots `<snapshot>` and `<snapshot>`; the seal printed `engine nils 1.0.0-alpha.71; pack 0.23.0; job pack-0230 0.23.0 3750; selection final-cert@1; drawn 3750; sealed now 3750; by selection selection:final-cert@1 3750 ; sealed outside the draw 0; stderr lines 0` and `rc=0`. Checked read only through the research role: all 3,750 drawn stacks are sealed.
9. **The packets and batches:** `tools/prep.sh` on the development laptop, then the manifest after the packets and the mirror.
10. **The read runs:** llm-2 starts (`CONC=6 tools/chain.sh`, detached; `CONC=2` again if the server errors), and the three rater waves run (`agents.py raters`, `agent_plan.py`, one wave at a time, `agents.py collect raters`, `validate.py`). Then routing, the adjudicators, settling, the flags, the manifests, the correction record (section 7). Round 9 may train from step 8 on.

## 12. The dry runs (2026-10-01 and 2026-10-02; throwaway seeds, nothing kept)

| tool | where | what ran | result |
|---|---|---|---|
| `frame.sql`, `subst.py`, `frame.py` | The research container, `runF.sh dry`, code runs `final-cert-read-code` and `-code-2` (prereg), then `-code-3` with Nima's decisions | the real frame query on production, read only, pack check lifted; first with pack 0.20.0 stored and dev-7 absent, then with pack 0.21.0 stored, dev-7 excluded and the picture-read stacks out | section 1; 14 s |
| `draw.py`, `weights.py`, `impact.py`, `campitems.sql` | The research container, seeds `dry-a`, `dry-b`, `dry-c` (first frame), `dry-d`, `dry-e` (second) | five full draws | sections 2 and 5; every weight checked |
| `frame.sql`, `subst.py`, `frame.py`, `draw.py`, `weights.py`, `impact.py` | The research container, `runF.sh dry`, code run `final-cert-read-code-4` (prereg), seed `dry-f`, 2026-10-02 after the NILS server's reclassify (job `pack-0230` id 367 finished 08:51:31+02, rc 0) | the third dry frame (dev-1 to dev-8 out, every stored row `pack-0230`) and one full draw | sections 1, 2, 5 and 10; every weight checked |
| `runF.sh setup` | The research container, code run `final-cert-read-code-4`, 2026-10-02 | the run folder's exclusion lists and `logs/inputs.sha256` | section 1 (kept: it is the run's) |
| `seal_export.py` | The development laptop, the dry-a draw | the kit for 3,747 stacks | one selection of 3,747 ids |
| `seal-server.sh`, `test_seal.sh` | The development laptop, a scratch Postgres registry made by `nils synth` (alpha.69, 898 stacks, classified with pack 0.21.0 as job `pack-test`) in the test container, scratch HOME on disk | A: a wrong frozen job; B: a stack sealed before; C: a clean kit of 60; D: C again | A and B stop with exit 3 and write nothing; C seals exactly the 60 under `selection:final-cert-c@1`; D stops; the database dropped |
| `prep.sh`, `hdr.sql`, `sess.sql`, `build_packets.py`, `batches.py` | The development laptop, a scratch folder, through the research role | the dry-a draw's 3,747 packets | section 6 |
| `agents.py`, `agent_plan.py` | The development laptop, scratch | 189 rater folders, 150 MB | section 7 |
| `lines_final.py`, `simulate.py` | The development laptop | simulated outcomes on the dry-a draw, 1,000 replicates per scenario | section 8 |

**2026-10-02:** a tool check with code run `final-cert-read-code-4` (prereg) and the throwaway seed `dry-f`, while the NILS server's reclassify was part-way (347,424 stored rows `pack-0230`, 201,290 still `pack-0220`), only to prove the tools with dev-8 out: 8,600 header-read stacks (dev-7's lists and dev-8's 1,000), the clean frame 230,208 and kept-uncertain 6,932, as the pack 0.23 study's frame. Its counts are not the frame's and are not used; the dry folders were removed. The third dry run, after the reclassify, is section 1's.

Not run dry: `route.py`, the adjudicators' tools and `chain.sh`, which are dev-7's unchanged but for names, seeds and the concurrency. The dry folders in the research container (`runF.sh dry-clean`), the scratch packets, agent folders, seal kits and simulated outcomes on the development laptop, and the scratch database were deleted on 2026-10-01 once the counts above were taken.

## 13. Deviations from the approved draft and record 48

- **The main sample is 2,500**, not the draft's 2,000 (design 4).
- **No caps per series, session or subject** in any sample: the draft named none for the final, and caps would break the exact probabilities. Clustering is reported, and a session-clustered bootstrap sits beside the claim.
- **A stratum whose share is under one read is pooled,** so every frame stack has a chance; the draft's table gave such strata 0.
- **The void by today's disposition applies to every stratum:** a clean or kept-uncertain stack the frozen pack excludes is void too (none in either dry run).
- **One selection, sealed once,** for all five samples, so a stack in both main and the supplement is sealed once.
- **The localizers are drawn over all three kept strata** (the draft: clean and kept-uncertain).
- **Nima's decisions 1 to 3** (2026-10-01) change record 48's frame (the picture-read stacks out, amending design 1), round 9's guard for these samples (by session) and record 50's G3 union (these samples not in it).

Written 2026-10-02, before the registration, from the amendments of that day:
- **Nima's amendments** (2026-10-02, "Certificate next (Recommended)"): the pack certified is 0.23.0 as stored from job `pack-0230`, not the pack a development read passed (decision 5), and the go condition is the near-0.5 % rate on the last development read without its bound (record 48, design 5). Reason: dev-8's 0.66 % (0.55 % without its dispute), pack 0.23.0 removing 6 of its 7 errors, and a dev-9 using up kept-uncertain stacks this certificate needs.
- **dev-8 joins the header reads,** out by session, a development read the plan did not foresee. `runF.sh setup` and `subst.py` take its draw, and `logs/inputs.sha256` covers its draw list and its manifest.
- **The prompts are v3.3,** not v3.1.
- **The prompts' trap line:** besides the reading guide's VFA-TSE line, which the pack 0.23 study named, v3.3 amends the traps' Siemens RESTORE line, which said "only" `*tseR` or `*tseBR`, so the trap does not contradict the guide.
- **llm-2's client timeout is 120 s** (`chain.sh` `TIMEOUT`), from dev-8's deviation 1; judge.py's default was 900 s.
- **The snapshot before the seal is recursive** (`zfs snapshot -r`, section 4): the registry's database is the child dataset the registry's database dataset, which the plan's first command would have left out, so the rollback would not have held the seal's rows. Found on 2026-10-02 by a read-only `zfs list` on the compute server, and corrected before the registration.
- **The the research container commands of section 11 run in a login shell** (`bash -lc`), whose profile sets the registry's connection; with `bash -c`, the tool check of 2026-10-02 (section 12) found psql trying a local socket. The commands were corrected before any registered run.
- **The dry run no longer goes without a development read's draw:** `subst.py`'s dry escape for a missing dev-7 list is removed, and the dry run requires dev-7's and dev-8's draws.

Written during the read (2026-10-02), each before any rule answer on a drawn stack was read (`reference.md`, "The interruptions and reruns"; the run's `logs/agent-notes.txt`):
- **The usage limit stopped the reading** from 10:15 to 11:30 UTC. Two `claude-adversarial` batches (029, 031) were part-way and 31 had not started. Each part-way batch was answered once more by a fresh agent with the same task text, its first output kept unread in `raters/_first/`; the rest ran as registered. The design is unchanged.
- **The permission classifier refused one agent's read and another's writes** (`claude-tags` 041: a read of its own packets that the harness had saved outside its folder; `claude-family` 021: two of its own output writes). Nothing was bypassed and no answer was taken from an agent's reply. Both batches were answered once more in full by a fresh agent, so that no batch mixes two agents' answers; the first outputs and two earlier partial fixes are kept unread in `raters/_first/`.
- **One legality failure was answered once more** under the plan's rule (one `claude-adversarial` answer), its first answer kept in `raters/_first/`.
- **The waves overlapped once:** the reruns of `claude-tags` 041 and `claude-family` 021 ran during the `claude-adversarial` wave, after the stop.
- **The adjudicators ran at most 20 at once,** adjudicator-1's 19 agents first and adjudicator-2's 22 as slots freed.
- **The grade's tools** (`tools/grade/`) were written after the seal and before any rule answer was fetched, and are registered with `grade.md` part 1, as dev-7's and dev-8's were.

Written at the grade (2026-10-02), after Nima's fetch (`grade.md`, part 2, "The checks before scoring"):
- **The engine was alpha.72 at the fetch, not alpha.71.** The reader update of 12:39 UTC changed no pack:
  - the installed pack is 0.23.0;
  - alpha.72's MRI pack tree is byte-identical to alpha.71's.
  - The stored answers are job `pack-0230`, made on alpha.71 and unchanged across the fetch.
  - The derived five were computed with alpha.71's binary as registered.
- **The fetch reported "unfinished jobs 2":** the engine's two resident lane workers (jobs 370 and 371, `worker` and `pipeline-worker`), started at the 12:39 restart.
  - Neither is a classify job or a picture build. Each has run no job.
  - The registered check (no classify job unfinished) held.
  - The stored jobs were the same before and after the fetch.
- **A reading aid,** `tools/grade/readout.py`, was written after the fetch to lay each wrong stack out for the read by kind. It changes no count.
- **Two sensitivity lines** were computed after the grade as information only: without the reference faults, and without the faults and the disputes. They have no bar, the registration named none, and they are not in a sealed run.

## Privacy

- The packets carry no personnummer, patient name, patient id, birth date or address, checked on every packet before any rater sees one, as dev-7.
- No UID leaves a query. The frame, the draw and the seal kit hold production ids, the cohort, the make and model, the md5 key and the cell parts. They stay in the research container, the evidence runs and the NILS server's kit folder.
- Every command over the registry writes its rows to a file, never to a terminal. Nothing per stack enters git.

## Files

| file | what |
|---|---|
| `plan.md`, `config.json` | this plan (registered 2026-10-02); the config with every blank filled (the strata's `size` stays null, as dev-7's and dev-8's: the frame gives it) |
| `tools/frame.sql`, `subst.py`, `frame.py` | the frame: header reads out by session, the strata by today's disposition, the frozen-pack check |
| `tools/draw.py`, `weights.py`, `impact.py`, `campitems.sql`, `disjoint.sql` | the five samples with exact probabilities; the weights' check; what the seal takes; the disjointness check |
| `tools/runF.sh`, `installF.sh`, `q.sh` | the steps in the research container; the code run through Bifrost; the read-only runner |
| `tools/seal_export.py`, `seal-server.sh`, `test_seal.sh` | the seal kit, Nima's the NILS server script, its test on a scratch registry |
| `tools/hdr.sql`, `sess.sql`, `build_packets.py`, `shown.json`, `prep.sh`, `PROMPTS.sha256` | dev-7's and dev-8's packets; the prompts v3.3 checked by sha256 |
| `../2026-09-27-header-judge/tools/compose_prompts_v33.py` and the v3.3 files | the prompts v3.3, from dev-8's v3.2 |
| `tools/batches.py`, `agents.py`, `agent_plan.py`, `chain.sh` | batches, one folder per agent, the waves, llm-2 |
| `tools/to_inputs.py`, `route.py`, `route.sh`, `adj_batches.py`, `settle.py`, `check_adjudication.py`, `triage_flags.py` | dev-7's routing, adjudication and settling |
| `tools/lines_final.py`, `stats_kg.py`, `simulate.py` | the lines and bars; their test and the power |
