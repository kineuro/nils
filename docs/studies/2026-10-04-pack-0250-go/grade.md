# Certificate 2's go read: pack 0.25.0 (stored) on p-go

2026-10-04. Certificate 2's go condition (`../2026-10-02-final-certificate-2/plan.md`, section 5, choice 2, DRAFT) read once for MRI pack 0.25.0, on the protocol sweep's held-out go read p-go (`../2026-10-02-protocol-sweep/plan.md`, section 9). This file holds counts only. Stack ids, values and the wrong stacks stay in the evidence runs.

## Part 1: the registration

**This part and `tools/` are committed before any stored 0.25.0 answer on a sweep stack is converted, read or scored. The commit is the registration.** At that commit:
- p-go's error list from the sweep's grade is sealed and unread (`protocol-sweep-go`), and no pack was fixed from p-go (`../2026-10-03-pack-0250/README.md`: "p-go was never opened");
- pack 0.25.0's stored answers on the sweep's drawn stacks are fetched and sealed unread (`pack-0250-go-rules`, below);
- the scorer and the lines have been dry-run on synthetic parties made from each record's corrected reference (below).

> **Warning: one shot.** p-go is graded once for pack 0.25.0, as registered here. Whatever the count, the go rule below is not amended, and p-go's error list stays sealed until the go decision it serves is taken.

### What is graded

- **Pack 0.25.0's answers as stored** on the pre-production registry, read through the research views (read only, role `nils_research`).
- **The job:**
  - every one of the sweep's 5,530 drawn stacks holds one stored classification row, from job **397, `pack-0250-timing`**, pack `mri` 0.25.0;
  - that job is a rerun of job 396 (`pack-0250`) with the same engine (1.0.0-alpha.75), pack and data, and is the latest row of each stack. The two jobs' classify arguments are the same; the engine version is the run's record, not a registry field;
  - job 393 (`pack-0250`, failed) left no row.
  
  The registry holds 552,685 stored rows, all from job 397.
- **Stored means the physics vote is in the answers.** No vote is modelled and no replay stands in for an answer.
- **The fetch:** `tools/grade/fetch-research.sh`, run in the research container on 2026-10-04 before this registration. It reads the stored axes, votes, evidence and the pack's voters (rule index to rule set, rule and axis). It stops unless every drawn stack is the one job above, and again if the jobs change during the fetch. They did not.
  - The rows went to files, never a terminal: 1,406 voters, 58,048 axis rows, 59,391 evidence rows and 11,062 vote rows.
  - They are sealed unread in the evidence run `pack-0250-go-rules`, 22 files, manifest sha256 `3d9133db4a269aab2b3a94167bc550035381a963a66cbc5d293500f2385c94f3`, mirrored.
  - Only its comments changed after the run.
- **The party:** `tools/grade/views_to_fetch.py` writes the answers in the shape dev-5's `stored.py` reads, and `stored.py` runs unchanged with pack 0.25.0's labels. `keep_read.py` keeps the 5,526 stacks with a packet.

### The two records

Both start from the sweep's sealed, settled reference (`protocol-sweep`, manifest `d229f482…0c1818858`). Both add pack 0.25.0's own pack-stands record (`rulings.py pack --record final`: N7 from the rules' answer) and score N8.

| record | its corrections | role |
|---|---|---|
| **primary** | the current final record, **final-31**: `rulings.py build --record final` (nils-design `9e7c187`, `rulings.py` sha256 `69186ffa4349bafe76f1c61ac39d714d25bd92a8490769e220ae552d26d9ec8d`, round 6's X10 rules) over the sweep's settled reference | **the go line** |
| sweep | the sweep's sealed correction record, `protocol-sweep-corrections` (manifest `66f9b280…6c38c654`) | beside it, as information, comparable with 0.23.0 and 0.24.0 in `../2026-10-02-protocol-sweep/grade.md`, part 2 |

**Why the primary line takes final-31.** Certificate 2 grades by the current record. The X10 rules were built from the sweep outside p-go, never from p-go.

**How the records combine, and why it does not matter on p-go.**
- The pack 0.25.0 study scored its sweep replays as settled cells, then final-31, then the pack-stands record (`pack-0250-replay/code/lib23.py`, `reference`). The sweep's grade scored settled cells, then the sealed record, then the pack-stands record. Both records are the same tool's output over the same settled reference: the sealed one at the sweep's version (`fa94d9d`), final-31 today.
- The primary line follows the pack 0.25.0 study: settled, final-31, then the pack-stands record built against final-31.
- `tools/grade/records.py` compares the two records from the reference side alone, on the non-localizers:
  - **On p-go:** final-31 holds all 37 of the sealed record's cells with the same values and adds 5 (X10-TFE 3, X10-MAGICREAL 1, X10-REGSET 1). Applying the sealed record and then final-31 gives the same p-go reference as final-31 alone: 0 of 7,464 cells differ.
  - **Outside p-go:** 47 cells are the same, final-31 adds 33, and 6 sealed cells are gone (D10 2, R3-FEEPI 1, V28 3, the three narrowings of the pack 0.25.0 study).
  - The localizers' record is the same in both (DR6, 2 cells).
- final-31 here equals the pack 0.25.0 study's final-31 on every one of its 80 sweep cells outside p-go.

### The scorer

- **`grade7.py`** as the final certificate and the sweep ran it, sha256 `6941e2344ebb1ddbf684073d8a57a52787f57657eda1b59ac7f6c39f7d5b4d0a`:
  - the ten axes;
  - an empty answer is wrong;
  - a base decided as nothing is the answer `none`;
  - N8, with the localizers apart (`locbad.py`, on provenance and technique by family).
- **The `cert/` copy, `cert25`:** the sealed checkpoint's `cert/`, whose `derive.py` also derives `contrast_mix` and `base_basis` in its one `nils pack derive` call, as the sweep's `cert24`. `run.sh bin` checks that it differs in that line only.
  - `derive.py` sha256 `cf2e502977004e62c8c3365ba7b0ac3a0a2aaf1e4ce14aae1a75940741d5e8e5`.
  - The tree, as the sha256 of its sorted file list: `610d4b4ad5be31d62b3eaab844c95130cb7036caa84690bcbbb3277bbe5331c9`.
- **The engine and pack the scorer derives with:**
  - alpha.75's `nils-linux-x86_64`, sha256 `041a78b6…4f513412`;
  - alpha.75's `packs.tar.gz`, sha256 `598191af…9866a433`, which holds MRI 0.25.0;
  - both checked against the release's SHA256SUMS.
- **dev-5's `stored.py`**, unchanged: sha256 `17496d632f8c00d2ff63f43bfee646cac96e5b13f47fb8db2c2d5692894b0ec4`.

### The line and the go condition

- **p-go:** the 1,497 clean non-localizers read of 1,500 drawn: Siemens 1,049, Philips 227 and GE 221.
  - The 3 left out for an identifier shape (the sweep's deviation 1) are missing. The Hajek denominator stays the frame.
  - A stack is wrong when any of the ten axes is wrong or empty.
- **The line:** `tools/grade/lines_sweep.py`, the sweep's, unchanged:
  - Hajek over make x field, with the Korn-Graubard bound (the final certificate's `kg` in `lines_final.py`);
  - the exact bound beside;
  - per make (GE, Philips, Siemens).
- **Under both records.** The primary line decides; the sweep line is information.
- **The go condition, as drafted:** go when pack 0.25.0 has **at most 9** wrong or empty of p-go's 1,497 clean reads on the primary line (a Hajek rate of about 0.60 % or less, a bound of about 1.14 %).
  - The number goes to Nima, who decides.
  - If it is above 9, it is reported as no go, and the rule is not amended.
- **Beside it:** 0.23.0 stored and 0.24.0 modelled on p-go, from `../2026-10-02-protocol-sweep/grade.md`, part 2.
- **The rest of the sweep** (p-dev, cover, outside p-go, the localizers) comes out of the same script. It is seen development data, reported as information only.
- **p-go's wrong stacks and the full bad lists** go to the evidence run `pack-0250-go`, sealed and unread. Only counts leave it.

### Method decisions, taken before any answer is read

Each is the reading that does not favour the pack, or the established one where the two coincide.

1. **The votes are decoded by the pack's voters.** The sweep took every empty-valued vote as a base vote, which was true of pack 0.23.0. Here each vote's rule index is looked up in `nils.classification_voter` for MRI 0.25.0, and the run stops if one is not found. A base vote with an empty value marks a base decided as nothing, as `nils pack replay` marks it (`none`).
2. **A base that only the rule set `cant_tell` decided as nothing is scored as empty, not as `none`.** Pack 0.25.0's `cant_tell` empties an axis so that a person is asked. That is not a ruling that the stack has no base. `views_to_fetch.py` writes such votes under the axis `base!cant_tell`, which `stored.py` does not read. Their count is reported.
3. **The primary record's mechanics** follow the pack 0.25.0 study. On p-go they equal the sealed record followed by final-31, cell for cell (above).
4. **The stored answers were fetched before this registration** and sealed unread, as the sweep fetched 0.23.0's at its draw. They come from production's current header reads. The reference was read from the sweep's packets. Each stored answer is graded as it stands.
5. **The 3 p-go stacks without a packet stay missing,** as in the sweep's grade.

### The dry run (2026-10-04, before this commit)

`run.sh dry`. The parties are made from each record's corrected reference by `dryparty.py`, the sweep's, unchanged:

| party | record | non-localizers wrong | p-go wrong | localizers wrong |
|---|---|--:|--:|--:|
| the corrected reference | primary | 0 of 5,427 | 0 of 1,497 (KG 0.20 %) | 0 of 99 |
| the corrected reference | sweep | 0 of 5,427 | 0 of 1,497 | 0 of 99 |
| 5 planted technique errors | primary | 5 stacks | 3 | 0 of 99 |
| 5 planted technique errors | sweep | 5 stacks | 3 | 0 of 99 |

- **The planted errors:** 3 of the 5 stacks fall in p-go and 2 outside it, and each is counted once.
- **The vote decoding** was tested on a three-stack synthetic input against the real voter list:
  - a `cant_tell` empty base vote was kept out;
  - another rule set's empty base vote was read as decided nothing.

No stored answer had been converted or read.

### The code

- **`tools/grade/`:**
  - `run.sh` (steps bin, record, stored, dry, stands, grade, lines, pack);
  - `fetch-research.sh`;
  - `views_to_fetch.py` (the sweep's, with the voters);
  - `records.py`;
  - from the sweep, unchanged: `keep_read.py`, `lines_sweep.py`, `locbad.py`, `dryparty.py`.
- **`tools/`:** `lines_final.py` and `stats_kg.py`, the final certificate's, unchanged.
- **From elsewhere, unchanged:**
  - dev-5's `stored.py`;
  - `../2026-09-28-certificate/tools/rulings.py`;
  - `../2026-09-24-phase-0/tools/r7/grade7.py`;
  - the sealed checkpoint's `cert/` (as `cert25`).

## Part 2: the grade (2026-10-04)

Run once, as registered in `8b99fc1`. The evidence is in `pack-0250-go-grade`. p-go's wrong stacks and the full bad lists are in `pack-0250-go`, which is sealed and unread. Only the counts below left it.

**The answer:** pack 0.25.0, as stored, has **6 wrong of p-go's 1,497 clean reads** on the primary line. That is a Hajek rate of 0.40 %, with a Korn-Graubard bound of 0.79 % (exact 0.79 %). **It meets the drafted go condition of at most 9.** The decision is Nima's.

> **Warning: the go rests on the current record.** Under the sweep's record alone, the same answers have 11 wrong (0.73 %, bound 1.21 %), above 9.
> - The 5 stacks between the two lines are the 5 p-go stacks where final-31 adds a cell: X10-TFE 3, X10-MAGICREAL 1 and X10-REGSET 1. These are Nima's rulings of 2026-10-03 on the 11 questions, written as record rules from the sweep outside p-go.
> - Under the primary line all 5 are right, and no stack is wrong under the primary line alone.
> - The registration named the primary line as the go line. This warning is so that Nima decides knowing it.

### The conversion

- **The votes:** 136,898 stored votes on the 5,530 drawn stacks, every one decoded by the pack's 1,406 voters.
- **Empty base votes:**
  - 503 stacks hold an empty base vote from a rule set other than `cant_tell`;
  - 456 of them have an empty stored base, so the base is decided as nothing (`none`);
  - 20 stacks are excluded.
- **Method decision 2** (the `cant_tell`-only base): it touches 2 stacks of the whole sweep, neither in p-go. It changed no p-go outcome.
- **The pack-stands record:** 3 cells under each record (N7: Magnitude 2, Magnitude with ND 1).

### The lines

Hajek estimate with the Korn-Graubard upper bound, exact upper bound beside. p-go counts its 1,497 clean non-localizers read; the 3 without a packet are missing.

| p-go | n | **0.25.0 stored, primary (final-31)**: wrong, Hajek, KG, exact | 0.25.0 stored, sweep record: wrong, Hajek, KG, exact |
|---|--:|--:|--:|
| all clean | 1,497 | **6, 0.40 %, 0.79 %, 0.79 %** | 11, 0.73 %, 1.21 %, 1.21 % |
| GE | 221 | 1, 0.45 %, 2.15 %, 2.13 % | 2, 0.91 %, 2.86 %, 2.82 % |
| Philips | 227 | 1, 0.44 %, 2.09 %, 2.07 % | 5, 2.20 %, 4.59 %, 4.58 % |
| Siemens | 1,049 | 4, 0.38 %, 0.87 %, 0.87 % | 4, 0.38 %, 0.87 %, 0.87 % |

**Beside them, from `../2026-10-02-protocol-sweep/grade.md`, part 2** (the sweep's record):

| p-go | n | 0.23.0 stored: wrong, Hajek, KG, exact | 0.24.0 modelled: wrong, Hajek, KG, exact |
|---|--:|--:|--:|
| all clean | 1,497 | 17, 1.13 %, 1.70 %, 1.70 % | 13, 0.87 %, 1.38 %, 1.38 % |
| GE | 221 | 2, 0.91 %, 2.84 %, 2.82 % | 3, 1.36 %, 3.51 %, 3.47 % |
| Philips | 227 | 2, 0.88 %, 2.76 %, 2.75 % | 3, 1.32 %, 3.40 %, 3.38 % |
| Siemens | 1,049 | 13, 1.24 %, 1.96 %, 1.96 % | 7, 0.67 %, 1.25 %, 1.25 % |

- **Like for like** (the sweep's record), p-go goes from 17 (0.23.0) to 13 (0.24.0) to 11 (0.25.0).
  - Siemens drops from 13 to 7 to 4.
  - Philips is 2, 3 and then 5. 4 of its 5 are among the 5 stacks the record moves (the fifth is GE): under the primary line Philips is 1.
- **Every make is under its 3 % certificate bar** on the primary line. Philips is not under it on the sweep's record (4.59 %).

**The rest of the sweep, seen, information only.** 0.25.0 was built from these stacks' errors, so these are training numbers, never a grade.

| line | primary (final-31) | sweep record |
|---|--:|--:|
| p-dev, all 1,000 | 1 | 6 |
| p-dev clean, 788 | 1 (0.13 %, KG 0.60 %) | 5 (0.63 %, KG 1.33 %) |
| cover: clusters wrong of 2,944 | 31 (0.10 % of the frame) | 54 (0.16 % of the frame) |
| outside p-go, unweighted, of 3,930 | 32 (0.81 %) | 60 (1.53 %) |
| localizers, of 99 | 0 | 0 |

The outside-p-go bad lists are in `pack-0250-go-grade` (`scoring/lines-*/private/`), as development evidence. They were not read for this file.

### Against the go condition

- **Primary line: 6 of 1,497, at most 9 allowed: go, as drafted.** It is 3 under the line. A Hajek rate of 0.40 % with a bound of 0.79 %, under the 1 % certificate bar.
- **For certificate 2's planning** (`../2026-10-02-final-certificate-2/plan.md`, choice 1), a true clean rate near 0.4 % passes main 5,000 almost surely, and 2,500 about 95 % of the time.
- **What it does not settle:**
  - certificate 2 draws a new frame, so these 6 do not carry over;
  - the margin depends on the record (above);
  - Nima's other choices (sizes, the selective claim) are still open.

The number goes to Nima. The rule is not amended.

### The evidence

| run | what | files | manifest sha256 |
|---|---|--:|---|
| `pack-0250-go-rules` | pack 0.25.0's stored answers, votes, evidence and voters on the sweep's drawn stacks (job 397) | 22 | `3d9133db4a269aab2b3a94167bc550035381a963a66cbc5d293500f2385c94f3` |
| `pack-0250-go-grade` | the code, final-31 over the sweep, the dry run, the grade aggregates and lines under both records, the outside-p-go bad lists, the logs | 86 | `4ecd4d98a161078a26b186acea8e9827257111ae3711f534c972d09b8030967e` |
| `pack-0250-go` | the converted answers, the parties, the pack-stands records, the full bad lists and p-go's wrong stacks under both records, unread | 25 | `9a8d422c2805188a509c1be381b12e7a3bcf232ca7777944ac65eccf68548e66` |

All three are sealed and mirrored to the research container.

### Deviations

None from part 1. One note: `fetch-research.sh` ran before the registration under the name `fetch-server.sh`. Only its comments changed when it was renamed and committed. The SQL it ran is kept in `pack-0250-go-rules` (`logs/`).
