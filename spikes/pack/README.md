<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# The pack format spike (C11)

**Question.** Can classification knowledge be expressed as **data**, a
versioned, diffable, third-party-shippable pack, or does it need a code
escape hatch? The ratification of `docs/decisions/15` closes C11 "by a
prototype before Wave 2", and this is the prototype. It has to answer before
`docs/specs/wave2-fingerprint-and-classify.md` is ratified, because §5 and §6
of that specification are the answer written down.

**Criteria, written before the work** (Wave 2 §5.1, §13). Express the three
hardest pieces of v0 0.5.3 in the format, and evaluate them against real
stacks:

1. **The unified flags.** All 138 of them, over the five parsers and their 220
   predicates. *Passes when* every flag of every stack in the live corpus
   equals v0's.
2. **One branch with its own taxonomy.** SWI: seven outputs of one acquisition,
   an ordered dispatch whose order is the knowledge. *Passes when* the base,
   construct, technique, confidence and cited evidence equal v0's on every
   stack the branch routes.
3. **The physics-vote pass.** The bins, the widening schedule, the
   intent-scoped reference, the compatibility filter, the minimum. *Passes
   when* the verdict, the method and the match count equal v0's for every stack
   in the corpus, against the same reference.

*A note on the three.* The record's own wording (`docs/decisions/15`, C11)
named the parsers and flags, the MP2RAGE TI rule, and "the SWI, SyMRI and
EPIMix branches". The MP2RAGE TI rule is inside criterion 1, as two of the
seven helper flags. SyMRI and EPIMix are, structurally, the same thing as SWI:
an ordered first-match dispatch over flags and text substrings, with local
helpers that are flags by another name, so a third and fourth branch would
have tested nothing a second one does not. The physics vote replaced them
because it is the one piece that is **not** an expression: it is an algorithm
with a reference pool, a widening schedule and a tie-break, and it is where a
declarative format is most likely to fail. Trading two easy pieces for the hard
one makes the criteria stricter, not looser.

Nothing may be expressed in code: the evaluator may know `any`, `all` and
`not`, and it may know what a token is, but it may not know what SWI is. A
piece that cannot be expressed is a finding, and §5 and §6 are amended and
re-ratified before the wave begins.

**Rules.** The corpus never leaves the private hosts, and this report publishes
counts and rates, never a description, a path or an identifier. v0 is private
and is never copied into this repository: `referee.py` imports it from wherever
it is on the host. v0 on `fg` runs untouched; the export is read-only
(`export.sh` forces `default_transaction_read_only` for the session) and its
intermediate copies are deleted when the run is done.

## The harness

**`pack/`** is the prototype pack: `parsers.yml` (5 parsers, 220 predicates),
`flags.yml` (138 flags and the 7 helpers v0 keeps as methods on its context),
`branches/swi.yml` (10 rules) and `passes/physics_vote.yml`. It is not the MRI
pack: there are no axes in it, because the axis layer is not what C11 is in
doubt about.

**`rust/`** is `packeval`, a loader and an evaluator of about 2,000 lines. It
knows the ten atoms and three combinators of §"The language" below and nothing
else. Three modes, each writing one row per stack: `flags` (the true flags),
`branch` (the verdict and its evidence), `vote` (the filled answer, the
widening method, the match count and which pool answered).

**`referee.py`** runs v0's own code over the same rows and writes the same
columns. Three of v0's modules (`classification/core/context.py`,
`classification/branches/swi.py`, `sort/gap_filling.py`) import nothing but
the standard library, so the referee runs them exactly as the pipeline does,
without standing v0 up. **`compare.py`** diffs the two and names what
disagrees.

**`run.sh`** is the protocol: build, run both sides of each mode, diff, exit
non-zero on any disagreement.

## Hosts and data

CT 110 `nils` on Asgard (an LXC container, 64 cores, 256 GB, the corpus on the
NVMe pool `fast`), rustc 1.98.0, Python 3.13. The data is a read-only export of
the two tables v0's classifier reads and writes, `stack_fingerprint` and
`series_classification_cache`, from the live v0 metadata database on `fg`:
**518,365 stacks**, 386,468 MR series, one site, five vendors, a decade.

The vote is measured against the cache as it stands *now*, which is v0's state
after its own gap filling ran. Both sides see that same pool, so the comparison
tests the algorithm and not the history, which is the point: it is the
algorithm the pack has to be able to configure.

## Results

| criterion | stacks compared | disagreements |
|---|---:|---:|
| 1. the unified flags (138 per stack) | 518,365 | **0** |
| 2. the SWI branch (6 fields per stack) | 17,054 | **0** |
| 3. the physics vote (6 fields per stack) | 518,353 | **0** |

**Cost.** The same 518,365 stacks, 220 predicates and 145 flags each: the pack
evaluator takes **5.5 s**, v0's hand-written Python takes **30.0 s**. Reading a
data language is 5.4 times faster than running the code it replaces, on one
core, which settles the objection that a pack must cost something. The vote takes **37 s**:
518,353 stacks against a 397,700-stack reference in 9,603 bins, with the full
widening schedule.

**What the corpus exercised.** The SWI branch fired 8 of its 10 rules
(phase 4,198, magnitude 3,865, swi 3,751, minip 3,554, fallback 1,314, mip 293,
projection 78, qsm 1); `r2star` and `qsm_source_echo` are for a vendor this site
does not have, and are unexercised here. That is a real limit of the test and
is why those two rules ship with pack corpus cases rather than field evidence.

## Findings

**1. Three flags in v0 can never be true.** `is_multi_echo`, `is_multi_ti` and
`is_multi_fa` compare a `stack_key` that the query loading a fingerprint never
selects, so all three are `False` for every stack v0 has ever classified. The
value they look for exists: **57,128 stacks, 11.0 % of the corpus**, carry one
(56,891 `multi_echo`, 237 `multi_ti`, 0 `multi_flip_angle`). Any rule written
against those flags has never fired. The pack carries the rule v0 meant, and
the difference is declared (Wave 2 §11.1); it becomes measurable when v1's
fingerprint carries the stack key, in slice 1.

**2. A dead `or` in the synthetic test.** In `parse_scanning_sequence`,

```python
"has_synthetic": any([...]) or "SE (SYNTHETIC)" in seq.upper() if seq else False
                 or "IR (SYNTHETIC)" in seq.upper() if seq else False,
```

A conditional expression binds looser than `or`, so this parses as
`(A or B) if seq else ((C or D) if seq else False)`, and the `else` arm is only
reached when `seq` is falsy, where it yields `False`. The `IR (SYNTHETIC)` test
is unreachable.

**It costs nothing, and saying so is part of the finding.** The token test in
front of it already catches every value the unreachable half would have, because
the tokenizer strips the parentheses first: `IR (SYNTHETIC)` tokenizes to
`{IR, SYNTHETIC}` and v0 answers true through the first clause. Checked against
v0 rather than reasoned about. So this is dead code and a defeated intention,
not a difference the gate will ever see. The pack transcribes the behaviour and
its corpus pins both halves, including the one input where neither clause fires
(`IR(SYNTHETIC)`, no space, no token).

**3. Two functions in one module decide the SWI output by different rules.**
`apply_swi_logic` is what the pipeline calls; `detect_swi_output_type` is a
second copy of the same ordered dispatch, exported and covered by the test
suite, called from nothing. Its phase rule omits the `not has_qsm` guard the
real one has. A branch in a pack cannot drift from itself this way, because
there is one declaration and the evaluator is the only reader.

**4. The vote's tie is broken by the database's row order.** `Counter.most_common`
orders equal counts by insertion, insertion follows the reference query, and
the query has no `ORDER BY`. Two runs over the same data can differ. The pack
declares `on_tie: none`, which is a different answer and a stated one.

**5. A rounding mode is part of the pack.** Python rounds a half to even and
Rust rounds it away from zero, so a TR of exactly 50 ms bins differently in the
two languages. Nothing in v0 records which it meant. `key.tr.rounding:
half_even` is now data, and it is the smallest example of the general rule: a
pack that does not carry a numeric convention is not portable, it is merely
untested on the other implementation.

**6. A pass filter reads two subjects, not one.** The expression language is
over *a stack*. The compatibility filter asks whether a **candidate** technique
fits the stack's `ScanningSequence`, a question about a stack and an answer
being proposed. The language gained two atoms (`family`, `candidate_empty`) and
the evaluation context gained a candidate slot. This is the one place the
prototype had to widen §6 as written, and it is the seam between "the pack
expresses predicates" and "the engine owns algorithms": a pass kind may offer
its own subject, and what it offers is part of the kind's contract.

**7. Flags need inline parser atoms.** `is_swi_processed` reaches past v0's own
named predicates into the raw token set for `SW_M_FFE`. Rather than invent a
predicate v0 does not have, the language allows `{parser: image_type, token:
SW_M_FFE}` inside a flag. Two places in v0 need it.

**8. Ten atoms and three combinators were enough.** The vocabulary stopped
growing at v0's 220th predicate and did not grow again through 138 flags, a
branch and a pass. That is the actual C11 answer: the size of the language is
bounded by the shape of the knowledge, not by the amount of it.

## The language

Against a **subject**, a parser's field or a text field an atom opens:

`token`, `any_token`, `all_tokens`, `tokens: {gt: n}`, `substring`,
`any_substring`, `prefix` (with `trim_start`), `equals`, `matches` (a regular
expression), `empty`.

Against the **stack**: `parser.predicate` and `flag_name` by name,
`{parser: p, <atom>}` inline, `{field: f, <eq|ne|lt|le|gt|ge|present>}`,
`{text: f, case: raw|lower|upper, <atom>}`.

Against a pass's **candidate**: `family`, `candidate_empty`.

Composed with `any`, `all`, `not`. A missing value makes a comparison false and
never an error, which is v0's `try/except` written once. A flag may name
another flag; the loader orders them and refuses a cycle by name. A reference
to something that does not exist fails the pack at load, naming the path
(`flags.has_se[3]`), not at run time on the 400,000th stack.

## What the pack does not express, on purpose

A pass is an algorithm (a binning, a widening, a vote), and the pack does not
write algorithms. It declares a **configured instance of a kind the engine
provides**, and every number the algorithm uses is in the pack. Adding a
modality then costs vocabulary and no engine code; adding a new *kind* of pass
costs engine code. That boundary is now written down (Wave 2 §7.2) rather than
discovered later.

## Verdict

**C11 is answered: classification knowledge is data.** All three criteria pass
exactly. Every one of 138 flags on every one of 518,365 stacks; every field of
every verdict on the 17,054 stacks the SWI branch routes; every filled answer,
widening method and match count on 518,353 stacks voting against a
397,700-stack reference. Not "close enough to adjudicate": identical, on the
whole live corpus, with no code that knows anything about MRI.

The language needed one widening against §6 as written (finding 6, a pass
filter's second subject) and one addition that was already implied (finding 7,
inline parser atoms). Both are folded into Wave 2 §6 and §7 rather than left
here, and neither changes the shape of §5. The rest of §5 and §6 stand as
ratified material.

Two things this spike does **not** answer, and Wave 2 must:

* **The axis layer.** Tiers, exclusion groups and physics windows are simpler
  than what was tested here (an ordered scan over the same atoms) but they
  are untested, and the intent cascade has an ordering of its own.
* **Whether a pack can be *written* by someone who is not us.** The format
  expresses v0. Whether a radiologist can read `axes/technique.yml` and add a
  value without help is a question about the format's ergonomics, and the
  answer comes when the CT and PET packs are written (Wave 2, slice 8).

The prototype pack, the evaluator and the referee stay here as the record.
`nils-pack` in the wave is written fresh against Wave 2 §5 and §6; where its
`expr.rs` looks like this one's, that is because this one was right.
