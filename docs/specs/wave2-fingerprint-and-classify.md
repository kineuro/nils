<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Wave 2: fingerprint and classify

The specification of the second wave of the NILS rewrite. It follows
`wave1-parse-and-digest.md`, which built the registry this wave reads, and the
design record it cites by id (`docs/decisions/`, D12, D7, C2, C11, C12, C14,
C15). Every section written before the work says what will be built; the blocks
headed "Settled while building" are amended in as it lands, the same way Wave 1
was written.

## 1. What Wave 2 delivers

The same binary, `nils`, gains

1. a **fingerprint** pass that turns the registry's stored fields into the
   values a classifier reasons over, once, columnar and reproducible, stored
   beside the stack (`nils fingerprint`);
2. a **pack loader**: classification knowledge as a versioned, diffable,
   third-party-shippable bundle, loaded by manifest, refusing to load a pack
   whose own corpus fails (C11);
3. the **MRI pack**, v0's accumulated knowledge carried over in meaning: the
   seven persisted axes, the flags, the tiers, the exclusion groups, the branch
   pipelines and the physics windows;
4. **`nils classify`**: a resumable job that routes each stack to a pack by
   modality, evaluates it, and writes the verdict **with its evidence**, which
   v0 computed and threw away;
5. the **cross-stack passes** v0 runs before and after classification, made
   deterministic against a declared reference instead of voting against
   whatever the registry happened to hold (C14);
6. **decisions that survive**: a human or agent verdict outranks a rule, is
   keyed at its scope, and a re-classification emits a new review item rather
   than overwriting it (C15, D7);
7. the **modality seam**: an axis belongs to a pack and not to the engine, a
   stack whose modality has no pack is recorded `no_pack` rather than pushed
   through the MRI classifier as v0 does, and a second modality is shown to
   work end to end by a minimal pack in the test suite. The CT and PET packs
   themselves are held; what this wave owes them is that the day they are
   written, no Rust is.

The gate is the same shape as Wave 1's: v1 against v0 on the live corpus, stack
by stack and axis by axis, every disagreement classified, on the baseline host
(§11).

**The stance on v0.** This wave carries over v0's *knowledge*, not its
behaviour. Where v0 is wrong (a flag that can never be true, a cascade that
exists twice and disagrees with itself, a name that drifts between two tables),
v1 is right, and the difference is declared before the run. Where v0 is wrong
and it changes nothing (an `or` swallowed by a conditional expression, whose
values another clause catches anyway), that is worth knowing and is not worth
declaring, and the two are not filed together. Where v1 is better rather than merely correct (one
intent cascade, a declared reference pool, a pack per modality, evidence on
every row), the difference is declared too. What is not allowed is a difference
nobody can explain: every disagreement in the parity diff is investigated to a
named cause, and a cause is a line of v0 or a line of v1, never a class label
(§11.5).

Not in Wave 2: anonymization and BIDS (Wave 3), the server, the query AST and
the MCP surface (Wave 4), the CT and PET packs, which are on hold and take the
seam of item 7 when they are wanted, and the model sidecars, whose seam this
wave declares and does not fill.

## 2. Words

- **Fingerprint**: the derived, per-stack view a classifier reads: normalized
  text, parsed numbers, flags, and the acquisition's physics, computed once from
  the registry and stored. Not a hash.
- **Axis**: one dimension of the answer (base, technique, modifier, construct,
  provenance, acceleration, contrast agent, body part). **Axis value**: what an
  axis resolves to, from that axis's vocabulary.
- **Flag**: a named boolean fact about a stack that rules are written against
  (`is_3d`, `has_fat_sat`), derived by the pack's grammar from the fingerprint.
- **Tier**: v0's ordered scan within an axis (exclusive flag, keywords,
  combination, fallback); the first tier that matches decides, and the tier
  fixes the confidence.
- **Exclusion group**: a set of values of which at most one may hold, with the
  order that resolves a conflict.
- **Clause**: one condition of a rule, with the tier that fixes its confidence.
  The first clause that holds is what the evidence cites.
- **Rule set**: an ordered list of rules, the axes they may decide, and an
  optional condition for entering at all. An axis file is one written
  compactly; a **branch** (v0's word for a route with its own taxonomy: SWI,
  SyMRI, EPIMix/NeuroMix, STAGE) is one with an entry condition. There is no
  third kind.
- **Pass**: a step that looks beyond one stack (at the series, the study, the
  session or a reference pool) before or after per-stack classification.
- **Pack**: the versioned bundle of vocabulary, grammar, branches, passes and
  corpus that judges one modality. **Overlay**: a provenance-scoped amendment to
  a pack (a site's word for "no contrast"), applied at classification time,
  never edited into the pack (C2).
- **Evidence**: what made a verdict, stored with the verdict: the field, the
  matched token, the tier, the rule id.
- **Decision**: a human's or an agent's verdict on a review item, at a scope,
  outranking the rule's (C15).
- **The two corpora** (C12): the **v0-parity corpus**, generated from the live
  classification cache, which is machine output and gates *reproduction*; and
  the **verified corpus**, adjudicated by a human during this wave, which is the
  moat and gates *correctness*. They are never called the same thing.

## 3. The shape of the code

Two crates join the workspace of Wave 1.

- `nils-pack`, the pack format: manifest, vocabulary, grammar, branches,
  passes, corpus; loading, validation, versioning and the overlay merge. It
  knows nothing about a registry: a pack plus a fingerprint yields a verdict,
  which is what makes it testable and shippable.
- `nils-classify`, the pass over a registry: the fingerprint builder, the
  modality router, the batch pipeline, the evidence and decision writers, the
  cross-stack passes.

`nils-dicom` gains nothing; `nils-registry` gains the tables of §4; the `nils`
binary gains `fingerprint`, `classify`, `pack` and `review` verbs. The
dependency direction stays one way: `nils-pack` knows the fingerprint's shape
and nothing else.

## 4. The fingerprint

### 4.1 What it is for

v0 recomputes, per stack and inside the classifier, what it needs from the raw
fields: it re-splits `ImageType`, re-parses `ScanningSequence`, re-derives the
same booleans, for every detector, every time. The fingerprint pass does that
once and writes the result down, for three reasons: a rule then reads a value
rather than a parser; a classification becomes reproducible from stored inputs
(the same fingerprint and the same pack version give the same verdict, on any
host, later); and the columnar shape is what makes the evaluation of most rules
a vector operation rather than a Python loop (D12's "compile where the structure
allows").

### 4.2 What it holds, and the line that decides it

**The fingerprint holds what is true of the file. The pack holds what is true
of the knowledge.** Everything below follows from that one line, and so does
what is deliberately absent.

What it is, in one sentence: **the five-table join a classifier would
otherwise do per stack, materialized and typed once**. Per stack, in
`stack_fingerprint`:

- **Folded text**: the six fields v0 joins (series description, protocol name,
  sequence name, body part examined, series comments, image comments), each
  kept as its own column and once more as the join, folded to one form: Unicode
  NFKC, case, whitespace collapsed, the accents and the Nordic letters kept.
  Folding is a fact about text.
- **The multi-valued fields as read**: `ImageType`, `ScanningSequence`,
  `SequenceVariant`, `ScanOptions` and `ImageOrientationPatient`, the stack's
  own value where it has one and the series' otherwise. They are not tokenized
  here: a parser declares its own `strip` and `split` (§6.1), and two packs may
  disagree about where a token ends, so tokenizing is knowledge too.
- **Physics**: the numbers a rule compares (echo time, repetition time,
  inversion time, flip angle, echo train length, b-values, field strength,
  slice thickness, spacing, matrix, number of averages, pixel bandwidth), typed,
  in the units the DICOM carries, with nulls kept as nulls.
- **Shape**: the geometry facts (2D or 3D acquisition, the orientation class,
  the number of instances in the stack, the stack's signature and index, how
  many stacks its series has, and, when that is more than one, **why the series
  split**: the first signature column its stacks disagree on, in v0's order.
  v0 computes that reason, stores it on the stack, and then never selects it
  into the fingerprint its own classifier reads, which is why three of its
  flags have never fired.
- **Provenance**: manufacturer, model, station and the implementation writer
  (its class UID and version name), folded the same way as the text. Not
  SoftwareVersions: the registry does not carry it, and adding a column to the
  catalogue is a Wave 1 change, not a fingerprint's to make.
- **Contrast**: the administration fields v0 joins into its contrast blob,
  folded, since a contrast agent's name is a fact about the study.

Deliberately **not** in it, and this settles the open question §14 carried:

- **Flags** are not stored. They are derived by the pack's grammar, so storing
  them in an engine table would need a namespace per pack and a rewrite on
  every pack version. The prototype settles the cost objection: 518,365 stacks,
  220 predicates and 145 flags each, in 5.5 seconds on one core. Deriving them
  when a pack is loaded is cheaper than keeping them true.
- **The semantic normalization is not applied here.** v0 folds *and* rewrites
  in one pass at extract time: `ir` becomes `inversion-recovery`, `mpr` beside
  `3d` becomes `mprage`, seventeen boilerplate tokens are dropped. Those are
  claims about MRI, not about text, so they belong to the pack (§6.4) and run
  when a pack does. The practical difference is large: in v0, correcting the
  token map means re-extracting the archive, because the rewritten text is what
  was stored. In v1 it is a pack version, a diff and a re-classification.

The fingerprint is derived data. It is never the truth about a file: the
registry's columns are, and the fingerprint carries the batch and the epoch that
produced it, so a change in derivation is visible and re-runnable.

**Settled while building (slice 1).** Two names had to be kept apart. v0's
`stack_key` is the reason a series split; v1's `stack.stack_key` is the
signature hash and means something else entirely, so the fingerprint calls the
hash `signature` and the reason `split_reason`, and a pack author cannot
confuse them.

The reason itself is derived from the stacks rather than copied from v0, and
the two agree on all 2,165 series of the nmosd corpus. On the mix corpus they
agree on 2,559 of 2,567, and the eight are v0's, with a named cause: v0 writes
the key in its **sort** step and not in extraction, so a series that was
extracted and never sorted has none. Across the whole live corpus that is 19
multi-stack series (8 of the 9 CT ones, 11 of 87,973 MR ones), and every one of
them is a series with no classification row, while every keyed series has one.
The correlation is exact, both ways. v1 derives the reason wherever the stacks
are, so the gap closes.

### 4.3 When it runs

`nils fingerprint` is a job like `digest`: over a source, a batch or the whole
registry, resumable, writing in bulk. A digest may run it inline (`--fingerprint`)
so that one pass over new data both ingests and derives. A stack whose fingerprint
exists and whose inputs have not changed is skipped, on the same epoch rule the
digest uses (§5.2 of Wave 1): the registry's own columns carry the epoch that last
wrote them, and a fingerprint older than its stack's is stale.

### 4.4 What it costs

The bar, on the baseline host of C6: the fingerprint of the live corpus's
518,887 stacks takes no more than the digest that produced them, and holds the
same memory ceiling (§11.4). It reads the registry, not the files: no DICOM is
opened in this pass.

## 5. The pack (C11)

### 5.1 What v0 keeps where, and what that means for the format

There is no pack in v0. Its knowledge sits in four places at once: eight YAML
files loaded from beside the code (2,348 content lines of vocabulary), ~10,000
lines of Python that is the grammar (five parsers, the unified flags, the tiers,
the exclusion groups, the branch pipelines with their own output tables, the
physics windows, the semantic normalizer, the intent cascade), one YAML file
that looks authoritative and is never read (`acceleration-detection.yaml`; the
detector's lists are Python constants), and per-cohort keyword deltas in the
application database. Nothing is stamped on a classified row: no rule version,
no timestamp, no provenance. Re-running the sort replaces the row silently.

So "carry the YAML over" was never the job. The job is to express the grammar
as data too, or admit a code escape hatch. C11 asks for that decision before
this wave, with a second criterion: adding a modality must need no code from us
and none at all from a user.

**The decision this specification takes**: the pack language expresses the
parsers, the flags, the axes with their tiers and exclusion groups, the physics
windows, the branches and the intent cascade, all of them data. The cross-stack
passes are the one thing it does not express as expressions, because they are
algorithms, not predicates: a pack *declares* a pass as a configured instance of
a kind the engine provides (§7). Adding a modality then needs vocabulary,
grammar and pass configuration, and no code. Adding a new *kind* of pass needs
engine code, and that boundary is written down rather than discovered.

The ratification asks for that decision to be **taken by a prototype, before
the wave**, and it was. `spikes/pack/` expresses the three hardest pieces of v0
(the unified flags, one branch with its own taxonomy, the physics-vote pass)
in the format of §5 and §6, and evaluates them against the live corpus with
v0's own code as the referee: **518,365 stacks × 138 flags, 17,054 branch
verdicts and 518,353 votes, with no disagreement in any field**, at 5.5 seconds
against v0's 30. Reading the knowledge as data is not a compromise; on this
evidence it is faster than the code it replaces.

The prototype widened the language in two places, and §6 and §7 below are
written as it left them: a flag may carry an inline parser atom (§6.2), because
v0 twice reaches past its own named predicates into a token set; and a pass
filter reads a **candidate** as well as a stack (§7.2), because "does this
technique fit this ScanningSequence" is a question about an answer being
proposed. Neither changes §5.

### 5.2 The bundle

```
packs/mri/
  pack.yml            # identity: name, semver, modality, contract version, axes provided
  parsers/*.yml       # tokenizers and their named predicates (§6.1)
  flags/*.yml         # named booleans over parser predicates and fingerprint scalars (§6.2)
  axes/*.yml          # one file per axis: values, tiers, keywords, physics, exclusion groups
  rules/*.yml         # rule sets written longhand: the routes with their own logic (§6.5)
  intent.yml          # the directory_type rule set (§6.6)
  order.yml           # the order the rule sets run in
  passes/*.yml        # configured instances of the engine's pass kinds (§7)
  corpus/*.yml        # expectation fixtures: a fingerprint in, the axes out
```

A pack is identified by `name@version`, its version is a semantic version, and
**every classified row records the pack name, version and the contract version
that evaluated it**. That single column is what turns re-classification from a
blind overwrite into a diff: "pack 2.1 changes 3,412 stacks, here they are".

The engine refuses to load a pack whose own corpus fails, and refuses a pack
whose contract version it does not implement. A vocabulary change (an axis
value, a directory type, an identifier namespace) is a **major** version bump,
because a federated question asked for pack 2 must not be answered by pack 3's
vocabulary (D26).

### 5.3 Overlays (C2)

An overlay is a provenance-scoped amendment: a list of additions and removals
against named, editable buckets of a pack, keyed by ingest batch, site or
scanner. v0 has this shape already, per cohort, for keyword lists only, as
deltas rather than snapshots, and its merge rule is worth carrying exactly:

    effective = (defaults + added, de-duplicated case-insensitively,
                 first occurrence kept in order) minus removed

with the original spelling preserved, because v0's contrast vocabulary contains
`" -k"` and `" -gd"` whose leading space is load-bearing. What changes in v1:
the scope is provenance, never a selection (C2), and the overlay is recorded on
the classified row beside the pack version, so a row judged under an overlay
says so.

Editable buckets are declared by the pack, not by the engine. v0 permits
keywords only, and keeps physics, flags and priority orders global; a v1 pack
may open more, and what it opens is part of its contract. A bucket is a named
list in the pack, referenced where a list of terms is wanted
(`any_token: {bucket: diffusion_tokens}`), so an overlay names a bucket and
never a path into the pack's internals.

**Settled while building (slice 2).** Whose cases judge what. The first
overlay written for a test made its pack fail to load, because the pack's own
corpus said the site's new word was not an agent and the overlay had just made
it one. Running the author's cases against a site's amendment turns the
author's claim into a constraint on the site, which is the opposite of what an
overlay is for. So: **the pack's corpus judges the pack, always and
un-overlaid, and an overlay carries its own cases and is judged by those.** An
overlay with no cases is refused, on the same reasoning that refuses a pack
with none: it changes verdicts, so it says what it changed.

## 6. The grammar

### 6.1 Parsers

A parser turns one DICOM string into named booleans. v0 has five (`ImageType`,
`ScanningSequence`, `SequenceVariant`, `ScanOptions`, `SequenceName`) and about
250 predicates across them. Every one is a token-set membership test or a
substring test over the raw upper-cased value:

```yaml
parsers:
  image_type:
    field: ImageType
    tokenize: {case: upper, split: '[\\/\s]+'}
    predicates:
      is_original:    {token: ORIGINAL}
      has_diffusion:  {any_token: [DIFFUSION, ADC, FA, TRACEW, ISODWI, EXP, EADC]}
      is_error:       {any: [{token: ERROR_MAP}, {substring: "MR ERROR MESSAGE"}]}
```

The atoms, and there are no others: `token`, `any_token`, `all_tokens` and
`tokens: {gt: n}` read the token set; `substring`, `any_substring`, `prefix`
(with `trim_start`), `equals`, `matches` (a regular expression) and `empty`
read the case-folded raw value; `any`, `all` and `not` compose them. The token
atoms and the raw atoms are kept apart because v0 mixes them deliberately and
the difference is load-bearing: a backslash survives in the raw value and
never in a token. Ten atoms and three combinators covered v0's 220 predicates
and did not grow again through its flags, its branches or its passes.

### 6.2 Flags

A flag is a named boolean over parser predicates and fingerprint scalars. v0's
107 documented unified flags are `any`/`all` over the parsers with a few
numeric conditions; the expression language is exactly that, and no more:

```yaml
flags:
  has_se:
    any: [scanning_sequence.has_se, scanning_sequence.has_fse,
          image_type.hint_se, sequence_name.is_se, sequence_name.is_tse]
  is_mp2rage:
    all:
      - sequence_name.is_mp2rage
      - {field: mr_inversion_time, lt: 1800.0}
```

Grammar: a flag is a reference (`parser.predicate`, or another **flag** by
bare name), or `{any: [...]}`, `{all: [...]}`, `{not: ...}`, or a comparison on
a fingerprint field (`eq, ne, lt, le, gt, ge, present`), or a match on a text
field (`{text: f, case: raw|lower|upper, <atom>}`), or an inline parser atom
(`{parser: image_type, token: SW_M_FFE}`), which is there because v0 twice
reaches past its own named predicates into the token set. A clause of a rule
set may also read an axis an earlier rule set decided
(`{axis: provenance, is: SWIRecon}`), which is how a route is entered and how
the intent cascade is written. Nothing else.

Flag-to-flag references are what removes v0's separate notion of a "helper":
the seven booleans it keeps as methods on its classification context (the
b-value test, the MP2RAGE context and its two inversion tests, the two text
fallbacks, the denoised-uniform test) are flags like any other here. The
loader orders flags by dependency and refuses a cycle by name.

A missing value makes a comparison false, never an error, which is v0's
`try/except` written once. A reference to a name that does not exist fails the
pack at load, naming the path that is wrong, not at run on the 400,000th
stack.

### 6.3 The rule set, and why there is only one kind of them

v0 has two machines for deciding an axis and they are the same machine.

A **detector** scans an axis's values in priority order and, for each value,
tries an exclusive flag, then a keyword hit, then a combination of flags; the
first value that matches any of the three wins, and which of the three matched
fixes the confidence (0.95, 0.85, 0.75). A **branch** scans its own rules in a
deliberate order and, for each rule, tries a flag, then a text hit; the first
rule that matches any of them wins, and which matched is the evidence. Value
major, tier minor, in both. The prototype expressed the second as data and
matched v0 on all 17,054 stacks it routes (`spikes/pack/`); the first is the
same shape with a different file in front of it.

So v1 has one concept.

- A **rule** is an ordered list of **clauses**. The first clause that holds
  fires the rule and is what the evidence cites; the clause's tier fixes the
  confidence unless the rule states one.
- A **rule set** declares the axes it `decides`, an optional `enter_when`, and
  its rules in order. The first rule that fires decides.
- A pack's rule sets run in declared order. An axis a rule set decided is not
  decided again; a multi-valued axis collects and is resolved at the end.

That is the whole evaluation model. v0's `skip_base_detection`,
`skip_construct_detection` and `skip_technique_detection` booleans, threaded
through the pipeline by hand, are gone: "already decided" is the mechanism, and
no rule set has to know that another one exists.

**An axis file is a rule set written compactly.** Writing four hundred keyword
rules longhand would be worse than v0, so the axis keeps its dense form and the
loader expands it: one rule per value, in `order`, with up to three clauses in
v0's order.

```yaml
axis: technique
order: [MDME, 3D-TSE, HASTE, ...]        # first value that fires wins
values:
  3D-TSE:
    label: SPACE                          # what the row stores
    family: SE
    keywords: [space, cube, vista, "3d tse", "3d fse", fase3d, mvox, isofse]
    detection:
      exclusive: is_space                 # clause 1, tier exclusive  (0.95)
                                          # clause 2 is `keywords`    (0.85)
      combination: [is_tse, is_3d]        # clause 3, tier combination (0.75)
```

Keywords are matched as case-insensitive substrings against the normalized text
of §6.4: no word boundaries, no regex, because that is what v0 does and the
gate is against v0.

Two v0 problems are fixed here rather than carried. Its rule *key* and its
stored *name* differ (`3D-TSE` against `SPACE`) and its inference tables are
keyed on a mixture of the two; in v1 the key is the identity, `label` is the
display string, and every table is keyed on the identity. And its constructs
drift between the branch tables and the intent cascade (`SWI` against
`SWIProcessed`, `Phase` against `PhaseMap`), so that two parts of one program
disagree about the same value; in v1 a rule that sets a value outside its
axis's declared vocabulary, or an axis its rule set does not `decide`, fails
the pack at load.

Physics windows are rules like any other, with a numeric clause instead of a
flag, kept in their own block so that the decorative copies v0 carries cannot
drift from the ones that are read:

```yaml
physics:
  ir:  [{when: {ti: {lt: 300}},  value: T2w, confidence: 0.75},
        {when: {ti: {ge: 300, le: 1500}}, value: T1w, confidence: 0.70}]
  se:  [{when: {tr: {lt: 1000}, te: {lt: 30}}, value: T1w, confidence: 0.70}]
```

Multi-valued axes (modifier, construct, acceleration) collect every match in
order and then apply **exclusion groups**, which keep the highest-priority
member of each group: `IR_CONTRAST: [FLAIR, STIR, DIR, PSIR, IR]`. Collection
and exclusion belong to the axis, not to the rule set that produced a value,
because they are facts about what the axis means. The result is sorted and
de-duplicated before it is stored, as v0 does.

### 6.4 Text and its normalization

Keywords are matched against a blob the **pack** builds from the folded text of
§4.2, in twelve ordered steps that are all data: literal removals, three
classes of character replacement (meaningful to a word, separator to a space,
noise to nothing), lower-casing, spacing around `+` and `-` so that a trailing
plus survives as a token, a final character filter, tokenizing on space and
underscore, de-duplication, boilerplate removal, unconditional token
replacement, conditional token replacement, and a join.

Three of them are load-bearing in ways that are easy to get wrong, so they are
called out rather than left to a reader of the config:

- **De-duplication keeps the first occurrence and preserves order.** A
  description that repeats a word loses the repeat, and a two-word keyword only
  matches if the surviving tokens end up adjacent.
- **`+` and `-` are tokens.** They carry contrast meaning, which is why v0's
  contrast vocabulary contains `" -k"` and `" -gd"` whose leading space is
  load-bearing, and why step 4.5 exists at all.
- **The conditional replacements read the whole token set**, so `mpr` becomes
  `mprage` only when `3d` is also present. That makes the pass order-sensitive
  in one direction only: unconditional replacements run first, and their output
  is what the conditions see.

v0 runs this at extract time and stores the result; v1 runs it when a pack is
loaded, for the reason §4.2 gives. The steps and their order are transcribed
exactly, because the gate compares against v0 and a different order gives
different answers on real text.

### 6.5 A different route of logic is a file

Some acquisitions are not variations on the normal questions; they need
different questions entirely. One SWI acquisition yields seven outputs and
none of them is asking "which contrast weighting is this". v0 calls this a
**branch** and writes it in Python: five of them, `swi.py`, `symri.py`,
`epimix.py`, `stage.py` and a Dixon table, about 1,865 lines, each returning a
`BranchResult` of overrides plus three booleans telling the pipeline what to
skip.

In v1 a branch is a rule set with an `enter_when`. Nothing else distinguishes
it.

```yaml
rules: swi
enter_when: {axis: provenance, is: SWIRecon}
decides: [base, construct, technique, directory_type]
order: [minip, mip, phase, swi, projection, r2star, magnitude, qsm, fallback]
```

The order is the knowledge, not the plumbing: SWI tries MinIP, MIP, Phase,
SWI, R2*, magnitude, and QSM **last**, because one vendor stamps `psd/QSM/me`
into every output's description, so `qsm` in the text says nothing until the
specific output labels are ruled out. That reasoning survives as a comment
beside the order it explains, instead of a docstring on a function.

**This is the answer to "why bother having branches at all".** They earn their
place only if adding a route is cheap, and the shape above is what makes it
cheap. To classify an experimental sequence that needs its own logic:

1. write `rules/<name>.yml` with an `enter_when`, the axes it `decides`, and
   its rules in the order that matters;
2. add whatever vocabulary it needs to the axis files, which is a major
   version bump (§5.2) because a vocabulary is a contract;
3. add its cases to the pack's corpus, which the engine checks at load.

No Rust, no release of the engine, no pipeline stage to insert, no skip flag to
thread. It ships as a pack version, it is diffable against the version before
it, `nils pack diff` says which stacks it changes, and `nils explain` names it
on every stack it touched. If it turns out to be wrong, the pack rolls back and
the rows say which pack decided them.

If a route ever needs something this cannot express, that is a finding of the
same kind the prototype produced, and it is answered by widening the language
or by adding a pass kind (§7.2), not by adding a Python file to the engine.

### 6.6 Intent

`directory_type` is not read from the file; it is synthesized from the other
axes, in a six-priority cascade: provenance first, then construct sets, then
BOLD text gated on an EPI technique, then base and modifier, then the remaining
constructs, then a provenance fallback, else `misc`. v0 has **two** copies of
this cascade that disagree, the pipeline's and a simplified one inside gap
filling, so a stack can be given one intent when classified and another when
completed.

It is a rule set like any other: it `decides: [directory_type]`, it runs last
because it reads what the others decided, and its clauses read axes
(`{axis: base, is: T2w}`) rather than flags. There is one declaration, and both
the classifier and the passes evaluate it, which is what makes the two copies
impossible rather than merely discouraged.

## 7. The passes (C14)

### 7.1 What v0 does

Three things happen around per-stack classification.

**Before**, a *session rescue*: stacks are grouped by subject and study date
(the date, not the study, so that a brain and a spine split across two studies
on one day are one session), and if no stack in the group is `ORIGINAL\PRIMARY`,
the `ORIGINAL\SECONDARY` ones are treated as primary instead of being excluded.
The decision is in memory only, and it depends on the composition of the batch
being sorted.

**After**, five phases: a field-strength normalization that rewrites a shared
table for every cohort; an orientation-confidence flag; an acquisition-type
fill; a physics vote that fills a missing base or technique; an SWI re-route; a
cross-stack contrast-duplicate check; an incomplete-4D detection; and a DWI
enrichment from vendor private tags.

The physics vote is the one C14 names. Its reference pool is **every classified
MR stack in the whole database**, with no cohort filter and no ordering, split
into one pool per `directory_type` plus a global fallback, which limits the
damage but does not bound it. Stacks are binned by
rounded physics (TR to 100 ms, TE to 5 ms, TI to 100 ms, flip angle to 5°,
slice count ceiled to twenties), the bin is searched and then widened by one and
two bins (single dimension, then the pairs TE+TI and TR+FA, then a relaxed TI
search for IR), a vote is taken over `(base, technique)` pairs, the winner must
be compatible with the stack's `ScanningSequence`, two matches are enough, and
the winner is written. A tie is broken by the order the reference rows arrived
in, which is the order the database returned them. So the answer a stack gets depends on what was sorted
before it, sorting one cohort can change another, and nothing about one stack
explains its own result. v0 also force-sets the review flag on every stack this
step touches, which is why a human who cleared a flag sees it return.

### 7.2 What v1 does instead

A pass is a **configured instance of a kind the engine provides**, declared by
the pack, and it runs against a **declared reference** rather than whatever the
registry happens to hold:

```yaml
pass: physics_vote
kind: nearest_neighbour_vote
reference:
  scope: pack_corpus + registry_snapshot   # named, versioned, recorded on the row
  filter: {modality: MR, directory_type_not: excluded}
key:  {tr: {round: 100}, te: {round: 5}, ti: {round: 100},
       flip_angle: {round: 5}, slices: {ceil: 20}}
widen: {max_distance: 2, pairs: [[te, ti], [tr, flip_angle]]}
decide: {min_matches: 2, on_tie: none, writes: [base, technique]}
emit: {evidence: always, review_item_below: 0.7}
```

Three properties follow, and they are the point of the change. The result is
**reproducible**: the reference is a named snapshot, so the same stack against
the same reference gives the same answer next year. It is **explainable**: the
vote is written as evidence (how many neighbours, at what distance, by what
method) instead of appearing as a value from nowhere. And it is **honest about
uncertainty**: a weak vote becomes a review item rather than a silent fill.

A pass kind may offer its filter a **second subject**. `nearest_neighbour_vote`
does: its compatibility filter judges a *candidate* technique against the
stack's own `ScanningSequence`, so the atoms `family` and `candidate_empty`
read the candidate while every other atom reads the stack. What a kind offers
is part of that kind's contract, declared with it, and it is the seam between
"a pack expresses predicates" and "the engine owns algorithms".

The other phases become their own kinds: `group_rescue` (the session rescue,
with its grouping key declared), `value_normalize` (field strength, with the
standard values and tolerances as data), `derive_fill` (acquisition type),
`reroute` (SWI), `duplicate_detect` (contrast), `shape_check` (incomplete 4D),
`enrich` (DWI b-values, phase-encode direction, direction count). A pack that
declares none of them gets none of them.

Every numeric convention the algorithm depends on is in the pack, including the
ones a language would otherwise decide silently: `key.tr.rounding: half_even`
is there because Python rounds a half to even and Rust rounds it away from
zero, which puts a TR of exactly 50 ms in a different bin. A pack that does not
carry its conventions is not portable, only untested on the other
implementation.

Two v0 behaviours are dropped deliberately, and the gate will show them as
differences: a pass no longer rewrites a table shared with other cohorts (field
strength is normalized into the fingerprint, not into `mri_series_details`), and
a pass no longer sets the review flag on everything it touches; it sets it
when it has something to say.

### 7.3 What the other phases turned out to be

Written after slice 6, from v0's own code rather than from its documentation.
v0 has eight phases around per-stack classification, and the wave assumed each
one would become a pass kind. Reading them, only one of them changes what a
stack is classified as:

| v0 phase | what it actually does | what it is in v1 |
|---|---|---|
| physics vote | fills a missing base and technique | the `nearest_neighbour_vote` pass, verified |
| session rescue | marks `ORIGINAL\SECONDARY` stacks of a session with no primary, in memory, so exclusion skips them | a fact about a session, which belongs in the fingerprint |
| SWI re-route | rewrites construct and technique for SWI outputs | already a route, decided by the rules (§6.5) |
| field-strength normalization | rounds field strength to standard values, in a table shared with every cohort | a fingerprint field |
| acquisition-type fill | fills `MRAcquisitionType` from other fields | a fingerprint field |
| DWI enrichment | reads b values and phase-encode direction from vendor private tags | the reader's, in Wave 1 |
| orientation confidence | raises a flag | a question for a person |
| contrast duplicates | raises a flag on two identical stacks that both say PRE, or both POST | a question for a person |
| incomplete 4D | raises a flag when slices hold different numbers of instances | a question for a person |

Three of them fill fields, three raise questions, one is a route and one is a
fact about a session. So a pass kind is needed for exactly one of the nine
things v0 does after classifying, and the wave implements that one.

The three that fill fields belong to the fingerprint, where they are computed
once and are the same for every reader, rather than to a phase that rewrites a
table shared with every cohort. The three that raise questions are review
items, and what v1 changes about them is not the detection but the emission
(§8.2): v0 sets its review flag on everything it touches. The session rescue is
the interesting one: as a phase it depends on which stacks happen to be in the
batch, so the same stack classified twice can be classified differently, which
is the whole complaint of C14. As a fingerprint field, computed from the
session and not from the batch, it is a fact and stays one.

Carrying those four into the fingerprint and the queue is not this wave's work
and is not needed for the gate, because none of them changes an axis. They are
written down here as the wave's finding, with what each one becomes, so the
decision is a decision and not an omission.

## 8. Evidence, review and decisions (D7, C15)

### 8.1 Evidence is stored

v0 computes evidence and confidence and then discards both: the upsert writes
the verdict alone. Every axis result in v1 carries its evidence, and the
evidence is written:

```
classification_evidence: stack_id, axis, value, tier, confidence,
                         rule_id, source (field or flag), matched (the token),
                         pack, pack_version, overlay, pass (when a pass wrote it)
```

That is what a review queue shows a human, and what makes an agent's
confirmation an informed act rather than a rubber stamp (D7). It is also what
makes a pack diff readable: two versions disagree, and the evidence says which
rule changed its mind.

### 8.2 Review items

The emitter is the classifier, the kinds are v0's reasons made explicit
(`{axis}:{missing|conflict|low_confidence|ambiguous}`, the body-part and intent
reasons, the pass reasons), and the item carries the evidence that produced it.
The policy of D7 decides what may be auto-resolved and by whom; the default is
human-only.

v0 flags 84 percent of its stacks, mostly for "no keyword evidence" rather than
for doubt, which is why its queue is unusable. v1's emission thresholds are
per kind and declared by the pack, and the gate reports the queue's size as a
number that matters: a pack that flags everything has failed even if it agrees
with v0.

### 8.3 Decisions outrank rules

A decision is recorded at a scope (this stack, this series, this subject, this
provenance) with who made it and why. On re-classification the rule's verdict is
computed as usual and then **the decision wins**, the difference is recorded as
evidence, and a new review item is emitted only when the rule's new answer
disagrees with the decision (C15). Nothing overwrites a human.

v0 protects exactly one axis this way (body part, through a committed profile in
the application database) and lets the next sort revert everything else. That is
the behaviour C15 was written against, and the gate will show it as a class of
difference: stacks where v0 reverted a human and v1 did not.

## 9. The pipeline

`nils classify` is a job of the same shape as `digest` (Wave 1 §10): resumable,
cancellable, one row in `job`, batches with their own ids, an epoch bump at the
end.

1. **Select** the stacks in scope (a source, a batch, a study, the registry),
   skipping those already classified by the same pack version with the same
   fingerprint epoch. This is the skip v0 gets wrong: its `skipClassified`
   keys on a column classification never writes.
2. **Route** each stack to a pack by modality. A modality with no pack is an
   explicit outcome, `no_pack`, recorded on the row; it is never a review item
   and never `misc`, which is what v0 does with every CT and PET stack it
   pushes through the MRI classifier.
3. **Evaluate** per batch, in parallel, against the loaded pack and its overlay:
   the rule sets in the pack's declared order, each entered when its condition
   holds, each leaving alone what an earlier one decided (§6.3).
4. **Write** verdicts, evidence and review items in bulk, in one transaction per
   batch, in the order the registry's foreign keys require.
5. **Pass** phases run after the per-stack phase, in declared order, each
   writing its own evidence.

Batch sizes and the memory ceiling follow Wave 1 §9.1: bounded queues, bulk
writes, no per-stack query. The bar is §11.4.

## 10. Knobs, diagnostics and the report

The knobs: `--pack`, `--pack-dir`, `--overlay`, `--reference` (which snapshot a
pass votes against), `--workers`, `--batch-rows`, `--scope`, `--force`,
`--dry-run`. Every one is recorded in the batch's config, as Wave 1 does.

The diagnostics are counted per batch and kind, with samples as shapes:
`axis_unresolved`, `axis_conflict`, `keyword_shadowed` (a keyword that can never
match because an earlier rule always wins), `overlay_unused` (a term the site
added that matched nothing), `pass_no_match`, `no_pack`. `keyword_shadowed` and
`overlay_unused` are new: they are how a pack is maintained, and they are the
diagnostics an agent reads to propose an overlay (C37).

The report says, per axis: how many stacks resolved, at which tier, with what
confidence spread, how many went to review and why. That report is the thing a
human reads after a classification run, and the thing the gate diffs.

## 11. The gate

### 11.1 Reproduction, against the parity corpus (C12)

The v0-parity corpus is the live classification cache: 518,887 stacks with their
axes, machine output, no verified column, 84 percent flagged. It gates
**reproduction**, and nothing else. The comparison tool of Wave 1 grows a
`classify` mode: v0's cache and v1's rows, joined on the stack, compared axis by
axis, every difference grouped by a pattern and classified as `v0-bug`,
`v1-bug`, `accepted` or `pass-difference`, with an adjudication file kept beside
the run exactly as §12.1 of Wave 1 defines it.

The bars:

- Every axis of every stack agrees, or its difference is classified.
- The classes that are **not** allowed to appear at all: an axis v1 leaves
  unresolved that v0 resolved from the same fingerprint; a stack v1 excludes
  that v0 classified.
- Differences that are **expected and declared before the run**: the passes
  (v1's vote is against a declared reference, v0's against the whole registry),
  the intent cascade (v0 has two copies that disagree), the dead flags (v0's
  `is_multi_echo`, `is_multi_ti`, `is_multi_fa` are permanently false because
  the query that loads a fingerprint never selects `stack_key`, so a rule that
  depends on them never fires in v0 and will fire in v1), the constructs whose
  names drift between v0's branches and its intent, the SWI output type that two
  functions in one module decide by different conditions, and the human
  decisions v0 reverts. Not on this list, deliberately: the synthetic-IR test a
  Python conditional expression swallows. It is unreachable code, but the token
  test in front of it catches the same values, so it changes no verdict and the
  gate must not be told to expect one.

Each of those is a case in the **verified corpus**, which is where the wave's
value accumulates.

### 11.2 Correctness, against the verified corpus

Built during this wave by stratified adjudication (axis value × provenance ×
manufacturer), seeded by what exists today: 113 acknowledgements, 284 body-part
labels, 5 override rows. A case is a fingerprint and the axes a human says are
right, with the reason. The pack ships its own cases; the private harness keeps
the ones derived from real data (C10).

The bar: the verified corpus passes entirely, and every disagreement adjudicated
during the parity diff has become a case in it.

### 11.3 The pack's own corpus

The engine refuses to load a pack whose corpus fails. That is a load-time check,
not a test-time one, and it is what makes a third-party pack safe to install.

### 11.4 Performance

On the baseline host of C6, from a cold cache: the fingerprint of the live
corpus's stacks costs no more than the digest that produced them; classification
of the same corpus sustains at least the digest's rate, since it reads rows
rather than files; peak resident memory stays under the Wave 1 target of 4 GB.
The CI benchmark gains a classify run over the synthetic corpus with the same
regression gate as §12.6 of Wave 1.

### 11.5 Every difference has a named cause

A class label is where the investigation of a difference is filed, not where it
ends. For every group in the parity diff the adjudication carries a **cause**:
the v0 expression that produced its answer or the v1 rule that produced ours,
quoted by file and line, and one sentence saying which is right and why. A group
whose cause is "walk order" or "v0 bug" and nothing more is not adjudicated; it
is an open item, and the gate does not pass with open items.

This is the bar that Wave 1's nmosd gate was actually held to, written down:
there, four mismatched studies looked like a classification difference and were
two unrelated causes: an import reading the wrong identifier table, and a
session duplicated on disk by an export that read a subject number out of a
folder name. Neither would have been found by classifying the difference and
moving on. Classification axes will produce more of that kind, not less, because
a wrong axis usually has an upstream reason.

## 12. CLI for Wave 2

```
nils fingerprint [--scope ...] [--force]            derive and store fingerprints
nils pack list | show <pack> | validate <dir>       packs, their versions, their corpus
nils pack diff <a> <b> [--registry ...]             what changes between two versions
nils classify [--pack ...] [--overlay ...] [--scope ...] [--reference ...]
nils overlay list | add | remove --scope ...        provenance-scoped amendments
nils review list | show | apply                     the queue, with evidence (D7)
nils explain <stack>                                the verdict, its evidence, its decisions
```

`nils explain` is small and load-bearing: one stack, every axis, the tier and
the rule and the matched token that decided it, the passes that touched it, the
decisions that outrank it. It is the answer to "why is this a T2w", which v0
cannot answer at all.

## 13. Order of work

The pack-format prototype (C11) is not a slice of this wave: it runs before the
specification is ratified, and `spikes/pack/README.md` is its report (§5.1).
The wave itself is eight slices.

1. **The fingerprint.** The tables, the pass, the normalizer, the parsers.
   *Done when:* the live corpus's stacks have fingerprints, the normalizer
   reproduces v0's twelve steps on a fixture set, and §11.4's cost holds.
2. **The pack loader.** Manifest, validation, versioning, overlays, the corpus
   check at load. *Done when:* an invalid pack is refused with the line that is
   wrong, and a pack whose corpus fails will not load.
3. **The rule set, and the axes as rule sets.** One evaluator, the compact axis
   form expanded into it, tiers, exclusion groups, physics windows, the seven
   persisted axes plus intent. *Done when:* the per-stack verdict matches v0 on
   the parity corpus for every stack that needs neither a route nor a pass, and
   the evaluator has one code path, not two.
4. **The routes.** SWI, SyMRI, EPIMix, STAGE and Dixon as rule sets with entry
   conditions, and nothing in the engine that names any of them. *Done when:*
   the route-decided stacks match v0's, and a sixth route is added to the pack
   by writing one file, with the diff of the engine empty.
5. **Evidence, review and decisions.** The tables, the emission thresholds, the
   precedence of C15. *Done when:* a decision survives a re-classification and
   the queue's size is a number the report states.
6. **The passes.** The kinds of §7.2, the declared reference, the evidence they
   write. *Done when:* two runs against the same reference give the same
   answers, and the difference from v0 is classified rather than accidental.
7. **The MRI gate.** The parity diff, the adjudication, the verified corpus, the
   performance run. *Done when:* §11's bars hold and every disagreement is
   either a fixed bug or a case in the verified corpus.
8. **The modality seam.** Not a second clinical pack, but the foundation one
   would stand on, tested rather than asserted. *Done when:* no axis name, no
   flag name and no vocabulary value of MRI appears anywhere in the engine
   crates; a stack whose modality has no pack is recorded `no_pack`, with no
   review item and no `misc`; a pack declares its own axes and the registry
   stores them without knowing what they mean; and a minimal two-axis pack for
   a second modality, synthetic throughout and living in the test suite, loads,
   routes and classifies with no change to any engine crate.

The seam is last on purpose. It costs a day if everything before it is right
and a rewrite if it is not, which is exactly what makes it worth measuring. The
CT and PET packs are on hold and are not this wave's work; when they are
wanted, this slice is the reason they will be vocabulary and not code.

## 14. Open questions carried into the wave

- **Which axes are shared and which belong to one modality.** Body part,
  provenance and intent are plainly shared; base and technique plainly are not.
  The CT and PET packs would settle it, and they are on hold, so slice 8 leaves
  it open on purpose: the seam must not assume the answer, and the minimal test
  pack must declare an axis of its own that no other pack has, to prove that it
  can.
- **What the pass reference is, exactly.** "The pack corpus plus a recorded
  registry snapshot" is the shape; whether the snapshot is a materialized table,
  a filtered view pinned by epoch, or a file shipped with the pack is slice 6's
  to decide and to record.
- **The sidecar seam.** A pack may declare that an axis prefers a model when one
  is present (body part today). Wave 2 declares the seam and does not implement
  a sidecar; the question is whether the declaration belongs in the axis or in a
  pass, and it is answered when the first sidecar exists.
