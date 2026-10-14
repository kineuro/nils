# Certificate 2: the grade

**Result (part 2): the certificate passes.** Main 7 of 5,000 wrong (the stack left out counted), Hajek 0.14 %, Korn-Graubard upper bound 0.26 % against the 1 % bar; every make passes 3 %; the selective claim passes with coverage 97.1 %.

2026-10-04. `plan.md`, sections 6, 8 and 11. Certificate 2 grades MRI pack 0.25.0 by its stored answers (job `pack-0250-a76`, id 400, engine 1.0.0-alpha.76) on the 6,245 stacks drawn and sealed as `selection:final-cert-2-a@1` and `selection:final-cert-2-b@1`. This file holds counts only. Stack ids, values and the wrong stacks stay in the evidence runs.

## Part 1: the registration of the grade

**This part and the tools (`tools/grade/`, `tools/selG.sh`) are committed before the rules' stored answers on a drawn stack are opened, converted or scored. The commit is the grade's registration.** At that commit:
- the read's reference is sealed (`final-cert-2-read`, `reference.md`) and so is its correction record (`final-cert-2-read-corrections`, final-32);
- the rules' stored answers are fetched and sealed unread (`final-cert-2-rules`); nothing in them has been opened, converted or scored;
- the scorer, the lines and the selective grader have been dry-run on synthetic parties made from the corrected reference and on a synthetic trigger (below).

> **Warning: graded once.** Nothing found on this grade is fixed and graded on it again (record 48). A method question that comes up after a number is seen is reported, not decided.

### Verified before anything was opened (2026-10-04)

| check | result |
|---|---|
| the three manifests against the git copies and the research container's mirror | equal: `final-cert-2-read` `4fa1b974…`, `final-cert-2-read-corrections` `da916921…`, `final-cert-2-rules` `757e12b0…` |
| the runs' files against their manifests | corrections 9 of 9 and rules 32 of 32 unchanged; the read 35,435 of 35,436: the one is `logs/checkpoints.log`, which holds the 18:13:54Z checkpoint line appended after the reseal (deviation 23). The file without that line hashes to the manifest's `577e6755…`, and the mirror's copy is that |
| the stored answers' job | every one of the 6,245 drawn stacks holds job `pack-0250-a76` (id 400), pack `mri` 0.25.0, before and after the fetch (`jobs.tsv` equals `jobs-after.tsv`; stderr 0 lines). The fetch ran at the draw, before the seal, read only (plan.md, section 9, deviation 3); no registry write |
| `grade7.py` | `6941e234…` |
| `rulings.py` | `83d220db…`, the same in the corrections run |
| dev-5's `stored.py` | `17496d63…` |
| alpha.76 | `nils-linux-x86_64` `6b590e07…` and `packs.tar.gz` `0b5c804e…`, checked against the release's `SHA256SUMS` and `config.json`; the pack is MRI 0.25.0 |
| `cert25` | `derive.py` `cf2e5029…` (one line differs from the checkpoint's), tree `610d4b4a…`; both equal `config.json` |

`tools/grade/run.sh verify` and `run.sh bin` do these checks and stop on a mismatch.

### What is graded

- **The pack's answers as stored** on the pre-production registry, read through the research views at the draw. Stored means the physics vote is in the answers: no vote is modelled and no replay stands in for an answer.
- **The reference:** the read's settled cells, then the final-32 correction record (`final-cert-2-read-corrections`), then the localizers' record beside, then the pack-stands record (`rulings.py pack --record final`, N7, built from the pack's own answers over final-32), and N8. Registered as the go read's primary line.
- **The scorer:** `grade7.py` (the ten axes; an empty answer is wrong; a base decided as nothing is `none`), `cert25` and alpha.76's engine and pack to derive the five derived axes, dev-5's `stored.py` unchanged. The conversion is the go read's: `views_to_fetch.py` decodes each vote by the pack's voters (1,406); a base that only the rule set `cant_tell` decided as nothing is scored empty.
- **The sample:** 6,245 drawn: main 5,000, supplement 250, kept-uncertain 600, void-kept 300, localizers 100 (5 stacks are in both main and supplement). 6,244 have a packet. **One main stack (Siemens) was left out for an identifier shape and is counted wrong** (deviation 8, extended by 20).

### The order of the steps

1. **Verify** (above; `run.sh verify bin`).
2. **Register** this part and the tools: commit and push.
3. **Install** the tools in the research container (a sealed code run) and **open `final-cert-2-rules`**, the grade's first act: check it against the mirror (`selG.sh check`), convert it to a party (`run.sh stored`), build the pack-stands record (`stands`).
4. **The main lines:** `grade7.py` (`grade`), then `locbad.py`, `lines_final.py` and `counts.py` (`lines`). One run.
5. **The selective claim:** in the research container, System 1 round 9 as frozen on the drawn stacks (`selS.sh s1 draw`, its frozen checksum checked before and after), the draw's party (`selG.sh party`), the trigger per stack (`selS.sh score draw`) and its output (`selG.sh out`). Then `sel_grade.py` (`run.sh sel`).
6. **Evidence:** the counts and aggregates to `final-cert-2-grade`, every file with a stack id to `final-cert-2-grade-private`, each sealed and mirrored.
7. **Part 2** of this file, and the development curve.

### The lines (`tools/lines_final.py` and `stats_kg.py`, unchanged)

- **Main, the claim:** the Hajek estimate on the 5,000 main reads (the sample is self-weighting within make x field) with the Korn-Graubard one-sided 95 % upper bound over the design strata. **Bar: 1 %.** Beside it: the exact Clopper-Pearson bound, Wilson at Kish's n, Kish's n and n\*, the Jeffreys aggregates over makes and over make x field, and the session-clustered bootstrap.
- **Per make:** each make's domain of the main sample, Hajek with Korn-Graubard. **Bar: 3 % for a make with 299 or more main reads.** A make below that is reported, not bounded.
- **Pass (intersection-union):** the main bound at or under 1 % and every held make at or under 3 %.
- **Kept-uncertain and void-kept:** each its own Hajek and Korn-Graubard bound, by make beside. No bar.
- **The supplement (the rare cells), apart:** per rare cell and cell group with exact bounds, and its own Hajek line. No bar.
- **The localizers, apart:** provenance and technique by family against their settled cells as the localizers' record leaves them. No bar.
- **Beside the primary line:**
  - the missing stack carried as not wrong (the frame as the Hajek denominator);
  - the lenient reading (a stack is wrong only through a wrong cell, an empty cell alone does not count): information only.
- **The counts:** wrong stacks of the main sample by make, field, cohort and era; wrong and empty cells by axis (grade7's own table); wrong stacks by sample.

### The selective claim (plan.md, section 6; deviations 20 and 21)

- **The trigger** is `combined`'s frozen score at **0.95**, with the pack's stored answers as its rules, accepting a stack only where `combined` keeps the pack's answer on all five axes; a stack System 1's export lacks goes to review. System 1 is round 9 as frozen (`FROZEN.sha256` `f34c3f01…`).
- **The claim (B):** among the accepted clean main stacks, at most **1 %** wrong or empty on the ten axes (Hajek over the accepted domain, Korn-Graubard 95 %); per make at 3 % where 299 or more are accepted.
- **The coverage bar:** the accepted share of the clean frame, by its Hajek estimate on the main sample, at least **85 %**. The lower 95 % bound is beside (one minus the Korn-Graubard upper bound of the review share).
- **The missing stack** counts wrong in the accepted domain whether or not the trigger accepts it (the primary reading, deviation 20); the trigger-decides reading and the as-missing reading are beside.
- **Beside it, described, no bar:** A (`rules-err`, its threshold set on the main sample to B's coverage), B's catch (the wrong main stacks sent to review) and the review share, and B at every threshold of the registered grid (information; the threshold stays 0.95).
- **Not a fallback:** the selective claim passing or failing does not change the main claim's result.

### The dry run (2026-10-04, before this commit)

`tools/grade/run.sh dry`, with alpha.76's engine and pack, `HOME`, `XDG_CONFIG_HOME` and `TMPDIR` in a scratch folder on disk (never `/tmp`). No stored answer was read.
- **The corrected reference itself as a party** (`dryparty.py`): 0 of 6,144 non-localizers wrong or empty; the pack-stands record empty; `lines_final.py` gives main 1 of 5,000 wrong, which is the missing stack counted wrong (Korn-Graubard 0.09 %), 0 in every other line, 0 of 100 localizers.
- **Five planted technique errors:** main 6 of 5,000 (the 5 and the missing stack), Korn-Graubard 0.24 %.
- **The selective grader** on a synthetic trigger made from the stack id alone (about 2 % with no row, 3 % failing `keeps`, scores 0.948 to 1): the three readings of the missing stack, the coverage and its lower bound, A's threshold search, the catch and the grid run, and the arithmetic was checked by hand on the planted cases.
- **The draw's party** (`selG.sh partytest`) on the protocol sweep's 3,930 stacks (job 400's fetch there): equal to the sweep's own party on all 3,930 stacks, none differing.
- **The research container's installed tools** are a sealed code run, `final-cert-2-code-10` (pre-registration copy). The grade runs the committed copy, installed after this commit.

### Method decisions, taken before any answer is opened

Each is the reading that does not favour the pack, or the established one where the two coincide.

1. **The missing stack counts wrong** on the main and per-make lines, and in the accepted domain of the selective claim whatever the trigger says (deviation 20).
2. **The selective claim's estimators** are as deviation 21: Korn-Graubard over the accepted domain, coverage by the Hajek estimate, its lower bound from the review share's Korn-Graubard bound, A at B's coverage.
3. **The error lists are not read** (deviation 22). If a claim fails, the count is the result and the lists wait for Nima.
4. **No other number is graded than the ones above.** If a result surprises, it is reported as it is; the method is not changed and the grade is not run again.

### The code

- **`tools/grade/`:** `run.sh` (steps verify, bin, missing, dry, stored, stands, grade, lines, sel, pack); `sel_grade.py`, `counts.py`, `dry_trigger.py` (new); from the go read, unchanged: `views_to_fetch.py` (`9cece14f…`), `keep_read.py`, `locbad.py`, `dryparty.py`.
- **`tools/selG.sh`:** the research container's side of the selective claim (`check`, `party`, `partytest`, `out`), over `selS.sh`, `sel9.py` and `sel_score.py`.
- **From elsewhere, unchanged:** `tools/lines_final.py`, `tools/stats_kg.py`, dev-5's `stored.py`, `../2026-09-28-certificate/tools/rulings.py`, `../2026-09-24-phase-0/tools/r7/grade7.py`, the sealed checkpoint's `cert/` (as `cert25`).

## Part 2: the results (2026-10-04)

Run once, as registered in nils-design `2ff54e4`. The stored answers were opened at the grade's first step, after that commit. Every number below is a count or a rate; the wrong stacks are in `final-cert-2-grade-private`, sealed and unread.

**The answer: the certificate passes, on the main claim, on every per-make bar and on the selective claim.**

| claim | bar | result | pass |
|---|---|---|---|
| **main** (clean non-localizers) | Korn-Graubard upper bound at most 1 % | **7 of 5,000 wrong**, Hajek **0.14 %**, Korn-Graubard **0.26 %** (exact 0.26 %) | **pass** |
| per make: GE | 3 % (760 main reads) | 1 of 760, 0.13 %, bound 0.62 % | pass |
| per make: Philips | 3 % (666) | 3 of 666, 0.45 %, bound 1.16 % | pass |
| per make: Siemens | 3 % (3,574) | 3 of 3,574, 0.08 %, bound 0.22 % | pass |
| **selective, B** (accepted error) | at most 1 % | 2 of 4,856 accepted wrong, Hajek **0.04 %**, Korn-Graubard **0.13 %** | **pass** |
| selective, B, coverage | at least 85 % | **97.13 %** (lower 95 % bound 96.71 %) | **pass** |
| selective, B, per make (3 %) | 3 % where 299 or more accepted | GE 0 of 711 (0.42 %), Philips 1 of 631 (0.75 %), Siemens 1 of 3,514 (0.14 %) | pass |

> **Warning: the 7 includes the stack left out.** One Siemens main stack has no packet (an identifier-shaped text in its header, deviation 8) and is counted wrong, as registered. Of the 7, 6 are read and wrong. With that stack carried as not wrong: 6 of 5,000, Hajek 0.12 %, bound 0.24 %. Neither reading comes near the bar.

**The statement, as registered:** of the clean stacks NILS keeps, outside the sessions any header read has touched, at most 1 % have any of the ten header axes wrong, at 95 % confidence (Korn-Graubard bound 0.26 %); and at most 3 % for each of Siemens, Philips and GE (bounds 0.22 %, 1.16 % and 0.62 %).

### The checks before scoring

- **The fetch:** every one of the 6,245 stacks held job `pack-0250-a76` (id 400, pack 0.25.0), before and after the fetch at the draw. The research container's working copy of the fetch equals the sealed run's file by file, and the mirror's manifest is the registered `757e12b0…`.
- **The conversion:** 6,245 stacks, 148,853 stored votes, every one decoded by the pack's 1,406 voters. 617 stacks hold an empty base vote from a rule set other than `cant_tell`, 581 of them a base decided as nothing (`none`). No stack's empty base vote came from `cant_tell` alone, so method decision 2 changed nothing. 0 stacks excluded. The pack-stands record holds 3 cells (N7: `Magnitude` to none 2, `Magnitude+ND` to `ND` 1).
- **The reference:** the settled cells with final-32 (127 cells on 103 stacks), the localizers' record (0 cells), N8 on 21 stacks.
- **The scorer** gave 0 wrong on the corrected reference itself in the dry run (the missing stack aside).

### The lines (`tools/lines_final.py`, unchanged)

| line | n | wrong | Hajek | Korn-Graubard upper | n\* | exact upper |
|---|--:|--:|--:|--:|--:|--:|
| **main, the claim (bar 1 %)** | 5,000 | 7 | **0.14 %** | **0.26 %** | 4,998 | 0.26 % |
| main, GE (3 %) | 760 | 1 | 0.13 % | 0.62 % | 757 | 0.62 % |
| main, Philips (3 %) | 666 | 3 | 0.45 % | 1.16 % | 663 | 1.16 % |
| main, Siemens (3 %) | 3,574 | 3 | 0.08 % | 0.22 % | 3,567 | 0.22 % |
| supplement (apart) | 250 | 4 | 1.56 % | 3.79 % | 217 | 3.62 % |
| kept-uncertain | 600 | 2 | 0.33 % | 1.05 % | 596 | 1.05 % |
| void-kept | 300 | 0 | 0.00 % | 0.99 % | 300 | 0.99 % |
| localizers (apart, no bar) | 100 | 0 | 0.00 % | 2.95 % | 100 | 2.95 % |

- **Beside the main line:** Jeffreys over makes 0.27 %, over make x field 0.35 %; session-clustered bootstrap 0.22 % (4,313 sessions); Wilson at Kish's n 0.07 % to 0.29 %; Kish's n 5,000. Every reading is far under 1 %.
- **Sensitivities (information only):**
  - the missing stack carried as not wrong: main 6 of 5,000, Hajek 0.12 %, bound 0.24 %; Siemens 2 of 3,574, bound 0.18 %;
  - the lenient reading (a stack wrong only through an empty cell is not wrong): the same 7. No stack is wrong through empty cells alone, and no wrong cell is empty.
- **Main, by make and field** (wrong of drawn; the missing stack is counted in its own make and field): GE 1 of 760; Philips 3 of 666; Siemens 3 of 3,574. By field: 1.5 T 2 of 3,011, 3 T 4 of 1,845, 7 T or more 1 of 122, 1 T or less 0 of 21. By cohort: study A 5 of 4,219, study B 2 of 247, the others 0. By era: 2009 to 2014 1, 2015 to 2020 3, 2021 on 2, unknown 1, up to 2008 0.
- **Kept-uncertain, by make:** GE 2 of 477 (0.42 %, bound 1.32 %), Siemens 0 of 112 (bound 2.64 %), Philips 0 of 11 (bound 23.84 %, reported, not bounded).
- **Void-kept, by make:** GE 0 of 171 (bound 1.74 %), Siemens 0 of 128 (bound 2.31 %), Philips 0 of 1.
- **The supplement:** 141 rare cells read, 5 stacks also in the main sample; 4 stacks wrong, each in a different rare cell. By cell group (make x technique family): Philips MIXED 1 of 7 stacks, GE OTHER 2 of 43, Siemens OTHER 1 of 40. The final certificate had 20 wrong of 250 (Hajek 7.64 %, bound 11.19 %).
- **The localizers:** 0 of 100 wrong on provenance and technique by family; the localizers' record corrected 0 cells.

**Per axis** (all 6,144 non-localizers; wrong cells, none empty):

| axis | provenance | technique | modifier | construct | base | role | directory type | disposition | convertible | quality |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| wrong | 0 | 2 | 3 | 6 | 2 | 2 | 3 | 5 | 0 | 0 |
| cells | 6,132 | 6,143 | 6,123 | 6,121 | 6,121 | 6,098 | 6,098 | 6,098 | 6,098 | 6,144 |

In all, 12 of 6,144 read non-localizers are wrong (23 cells): 10 of the 5,244 clean stacks of main and supplement, 2 of 600 kept-uncertain, 0 of 300 void-kept. `grade7.py`'s own unweighted reading of the clean stratum (main and supplement together) is 10 of 5,244, exact upper bound 0.32 %; its accept-the-rules line reads "fails" at 1 % and "passes" at 2 %. The registered pass reading is `lines_final.py`'s above, not `grade7.py`'s.

### The selective claim (B, threshold 0.95)

System 1 round 9 as frozen (`FROZEN.sha256` unchanged before and after) was run on the 6,245 drawn stacks: all 6,245 are in round 9's export, 3,519 of them in its training registry (teacher labels). `combined` with the pack's stored answers as its rules kept the pack's answer on all five axes for every stack (no stack failed `keeps`), and gave score 0 to the 91 stacks where the pack answers at 0.70 or more and System 1 differs.

| reading of the missing stack | accepted n | wrong | Hajek | Korn-Graubard upper | exact upper |
|---|--:|--:|--:|--:|--:|
| **forced** (primary, deviation 20) | 4,856 | 2 | **0.04 %** | **0.13 %** | 0.13 % |
| the trigger decides | 4,856 | 2 | 0.04 % | 0.13 % | 0.13 % |
| the stack as missing | 4,856 | 1 | 0.02 % | 0.10 % | 0.10 % |

- **Coverage:** 4,856 of 5,000 main stacks accepted, **97.13 %** by the Hajek estimate (plain 97.12 %), lower 95 % bound 96.71 %, bar 85 %. By make: GE 93.55 % (lower 91.89 %), Philips 94.75 % (93.10 %), Siemens 98.34 % (97.94 %). The trigger accepts the stack left out, so the three readings agree on the claim line.
- **Catch and review share:** 144 stacks go to review (2.87 %). **5 of the 7 wrong main stacks** are among them (71.4 %, plain and weighted); the 2 accepted wrong are the missing stack and one Philips stack.
- **A (`rules-err`),** its threshold set on this main sample to B's coverage (calibrated score 0.9675): 4,856 accepted, 6 wrong, Hajek 0.12 %, Korn-Graubard 0.24 %; by make GE 1 of 756, Philips 2 of 557, Siemens 3 of 3,543. It sends 1 of the 7 wrong stacks to review, B sends 5. No bar.
- **The grid** (information only; the threshold stays 0.95): B accepts 4,382 at 0.99 (coverage 87.65 %, 1 wrong), 4,690 at 0.98 (93.81 %, 2 wrong), 4,778 at 0.97 (95.57 %, 2 wrong), 4,856 at 0.95 (97.13 %, 2 wrong), 4,897 at 0.9 (2 wrong), and all 5,000 at 0.0 (7 wrong). The selective claim adds little on this pack: accepting everything is already under 1 %.
- **Against the plan's expectation** (section 6, the sweep): reading 2 predicted an accepted error near 0.48 % for pack 0.23.0's errors; pack 0.25.0's is 0.04 %, since its rules make far fewer errors.

### Against the final certificate and the go read

| read | pack | clean main or go read | wrong | Hajek | Korn-Graubard | note |
|---|---|--:|--:|--:|--:|---|
| final certificate (2026-10-02) | 0.23.0 stored | main 2,500 | 32 | 1.28 % | 1.72 % | fails 1 % |
| the go read, p-go (2026-10-04) | 0.25.0 stored, job 397 | 1,497 | 6 | 0.40 % | 0.79 % | meets the go rule |
| **certificate 2 (this grade)** | **0.25.0 stored, job 400** | **main 5,000** | **7 (6 read)** | **0.14 %** | **0.26 %** | **passes 1 % and 3 %** |

- On fresh archive-wide draws, the clean rate went from 1.28 % (0.23.0) to 0.14 % (0.25.0), about nine times lower; on every make as well (GE 1.38 % to 0.13 %, Philips 1.06 % to 0.45 %, Siemens 1.31 % to 0.08 %). The go read's 0.40 % (6 of 1,497, bound 0.79 %) is higher than this certificate's 0.14 %; the two samples are different draws, and the go read's bound allows the lower rate.
- Supplement: 20 of 250 (7.64 %) for 0.23.0, 4 of 250 (1.56 %) for 0.25.0. Kept-uncertain: 8 of 600 to 2 of 600. Void-kept: 6 of 300 to 0 of 300.
- The main sample was 5,000 against 2,500, so the bound tightens as well as the rate.

### What this does not settle

- The reference is the panel's reading of the header. The audit found 0 joint errors before the record and 2 against the corrected reference (bounds at most 0.54 %), so a joint error cannot hide much of the 0.14 %.
- The result is for the clean non-localizer stratum of the frame (150,517 stacks) with the picture-read stacks and the earlier reads' sessions out. Kept-uncertain, void-kept, the supplement and the localizers have no bar. Body part and post-contrast are certified on their own tracks.
- Pack 0.25.0 is certified as stored by alpha.76 on the pre-production registry, job 400. The same pack must be what production runs.
- Nima must decide (below): what is published and how, and whether the error lists are read.

### The evidence

| run | what | files | manifest sha256 |
|---|---|--:|---|
| `final-cert-2-read` | the read (resealed) | 35,436 | `4fa1b974976a4b7c5b76220a577e5252f1299fa1940deaad0dceb788edd8b780` |
| `final-cert-2-read-corrections` | final-32 over the read | 9 | `da9169214291546c928e186c4b84b87a3ad7de5bb3af5b12a40059c8fc5dc50d` |
| `final-cert-2-rules` | the stored answers, opened at this grade | 32 | `757e12b05deec381f273a065bebccbeda57a87a7ee8279475160ac1019c491c3` |
| `final-cert-2-code-11` | the committed tools as installed for the grade | 50 | see `../phase0-evidence/runs/` |
| `final-cert-2-grade` | the code as run, the lines, the selective claim and the counts (no stack id), the logs | 37 | `ea4a802851a967f6ec6b0d5457c80474b97e92e4c1e91ed0893c8b09bca30d7c` |
| `final-cert-2-grade-private` | the party, the stands record, grade7's bad lists, the wrong stacks by line, System 1's predictions and the trigger per stack: sealed, unread | 54 | `c03388ccaf71ce39f9953cae867820a640cd34ddd0c0fece1e72aac51be04608` |

All are sealed and mirrored to the research container. The wrong stacks were not read for this grade.

### Deviations

New ones 20 to 23 are in `plan.md`, written before the grade. None was added after a result.
