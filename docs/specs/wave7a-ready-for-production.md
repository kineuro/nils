<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Wave 7a: ready for production and for the paper

The specification of the wave that finishes Wave 6 and ends with NILS ready for
real work and for publication. It follows `wave5-the-whole-desk.md` and the
waves of the design record that came after it (records 21 to 54, which shipped
without a spec of their own), and cites its record by item id
(`docs/decisions/55-ready-for-production-and-the-paper.md`: the decisions A1 to
L4, settled with Nima on 2026-10-08, and the wave's tests T1 to T36). The record
holds the decisions, the people and the deployment; this document holds what the
code does.

Wave 6 certified the header rules at 1 % (MRI pack 1.0.0: 0.14 % wrong on a
sealed sample, upper bound 0.26 %), certified body part and post-contrast from
the image on the scans they claim, and showed that a learned partner of the
rules, System 1, can accept 97 % of clean scans with 2 errors in 4,856. It did
not put System 1 inside the review loop, it did not build the shared place where
people check, pick and segment, and it left the desk a set of good pages that do
not yet make one product: a first dataset that is never pseudonymised, about
forty engine abilities with no control, and command-line steps in the middle of
desk flows. This wave closes those gaps and ends with a site's production
install on an empty registry.

The measure of the wave, in four sentences. Data enters NILS only through a
folder layout that says what it is, and a file that leaves pseudonymised says so
in its own header. The rules decide every answer and System 1 stands beside
them: a person is asked only where an axis that matters is left empty (and, on a
dataset with damaged headers, where System 1 does not confirm the rules), with
both suggestions and both reasons beside the question, and the error of what is
accepted is measured by a blind audit. Every model NILS serves is a replaceable
part with a card, fast on a CPU and much faster on a GPU, and the rules and the
models update without a new engine. A person finds every task in one obvious
place in the desk, and the people's work (answering, picking, checking a result,
settling) is one list on one mechanism.

> **Amended in place on 2026-10-16 by the audit of Wave 7a.** This specification
> was written on 2026-10-08, before the work. Its sentences about what the code
> does "today" describe the code of that day, and some of what it asks was
> changed by a ruling or by the build. Each place that changed carries its own
> note, naming the ruling or the record that changed it: the summary of System 1
> above and in section 1, and sections 3, 4.1, 5.1, 5.3, 6.1, 8.1, 8.2, 9.2,
> 11.2, 12.2, 13.1, 14, 17, 18, 19 and 20. Section 11.6 is new: the rules engine
> as built (record 56). The rulings are in
> `docs/decisions/55-ready-for-production-and-the-paper.md`, numbered from 1 on
> 2026-10-12 and dated before that.

## 1. What Wave 7a delivers

- **Data that comes in safely** (§5): the subject code generator as the default
  pseudonym scheme; the pseudonymiser writing the subject code or a chosen ID
  type into PatientID, by the dataset's declaration; a folder without the
  dataset layout never read as de-identified on any path.
- **Files that say what was done to them** (§6): the DICOM de-identification
  marks in every pseudonymised file, and the tag list the desk shows made exact.
- **Rows that leave under one rule** (§7): the quasi-identifier rule applied at
  every door that returns rows, and the release's own record of what it carried.
- **Releases that always name a scan** (§8): a name for every scan, never a
  refusal; the BIDS validator in CI; display composites kept out of analysis
  releases.
- **Rules that ship on their own** (§9): rules releases with their own version
  and an engine-compatibility range, taken by the update path.
- **Models as parts** (§10): several models per task, each with a card, chosen
  per site; label sets registered from outside the engine; scoring at arrival
  with a per-session slice cache.
- **The rules decide, with System 1 beside them** (§11): the rules decide every
  answer; System 1's answer and confidence are stored for every scan as a second
  opinion, and on a dataset with damaged headers the rules' answer is accepted
  only when System 1 agrees above its thresholds; both suggestions and both
  reasons in review, blind first where a read trains or measures; a blind audit
  stream for accepted scans; and the rules engine, where a change to the rules
  shows its effect before a person adopts it (§11.6). *Amended in place on
  2026-10-16 by record 56.*
- **The image models in production** (§12): body part's fast path; post-contrast
  served through its student where it is certified; the post-contrast label as a
  state with its minutes, its source and its flags.
- **A desk that is one product** (§13): one map, every task in one place; Review
  and Campaigns as one list of work over five job types; a page to browse
  subjects, sessions and scans; a reader that shows why the rules decided.
- **The assistant and the model gateway, measured** (§14).
- **An install that can be repeated** (§15): the keep-data fix, the install test
  at both ends of the wave, and a site's refresh to an empty registry.
- **The research behind the paper** (§16): the protocols the wave's studies
  follow, and how their models enter the product.

## 2. What it rests on

Everything of Wave 5 and of the records that followed it:
- the dataset layout, pseudonymisation as a real step and the identifier map
  (record 26), and the tag list the engine serves (record 28);
- the installer and the update path (records 21, 29, 30, 32), and a wave
  carried by a development channel rather than a release (record 31);
- the rules' certificate and its methods (records 39 and 48): a sealed sample, a
  reference read by agents and settled, the exact binomial bound;
- labels and models with a home (record 42), pipelines that make derivatives
  (record 43), review on one mechanism (record 45), the model gateway serving
  the group (record 47), analyses with help (record 49), body part from the
  image (record 50), the header's session pass and private elements (record 53),
  and the subject code generator (record 54, pull request #369).

## 3. What Wave 7a does not do

- It does not take on the master plan's own Wave 7: the hardened runner, the
  cluster executor, rules for CT, and retiring the previous prototype (record 55,
  section J). *Amended in place on 2026-10-16 by record 55, ruling 31 (i):* these
  were to be Wave 7b's. Wave 7b is now record 57 and the short clinical import,
  with the two items of record 56 that ruling 31 (b) moved there (§11.6), and
  the four come later, outside 7b.
- It does not let a model decide alone. The rules decide; System 1's answer is a
  second opinion, accepted on a dataset with damaged headers only where it
  agrees with the rules above the gate's thresholds, and staged for a person to
  confirm where the rules name nothing (§11.2). *Amended in place on 2026-10-16
  by record 56 and record 55, ruling 20:* this first said that a model's answer
  is accepted only where the rules agree and the gate's thresholds hold, and
  that the rest is a review question.
- It does not let a person's answer that trains or measures a model be anchored
  by the model: those reads are blind first (§11.4).
- It does not certify by re-grading. Every claim is graded once on a sample
  sealed before it was drawn; a missed bar is reported as missed. *Amended in
  place on 2026-10-16 by record 55, ruling 27:* after this release no claim is
  certified again; a production change is a fix, a check that nothing broke and
  a release (§4.1).
- It does not build manual segmentation (drawing masks); job type 4 is Wave 7b's.
- It does not change the ruled disclosure: the release is pseudonymous, the date
  is the date (record 38), sequence names stay visible (§7).
- It does not move any of the group's specifics into code: hosts, mounts, the
  identity provider's application and the people are the record's.

## 4. The rules every slice obeys

### 4.1 The development channel first (A4)

Every engine and desk slice is built from its branch onto a development install
through the development channel of record 31, tried there in use on a test
registry, fixed in quick rounds, and merged only when its owner has accepted it.
The desk is judged as a product in use, not as a set of passing tests. A
release closes one or more slices. *Amended in place on 2026-10-16 by record 55,
ruling 27 (2026-10-15):* this first added that no release is cut for a single
fix. From alpha.83 on, a production change is a fix, a check that nothing broke
(the tests and, for rules, the archive replay of §9.3) and a release, so a single
fix is released alone (alpha.83 to alpha.86), and no certificate is asked again.

### 4.2 Every model on CPU and on GPU (A5)

Every model NILS serves runs on a CPU alone at a speed stated on its card, and
uses a GPU when one is present and free, which must cut the time by a large
factor. One model version gives the same answers on both, or differs only
between an answer and "not sure", within a tolerance stated on its card and
tested as an equivalence class (§10.5). A GPU is taken under a lease: a minimum
of free memory and at most two processes on a card.

### 4.3 Registered before drawn

Every training round, every certificate and every experiment of §16 has its plan
written and committed before its data is drawn: the sample, the splits, the
measures, the bars, and what counts as a pass. A sealed sample is never seen by
the people or the models it will grade. A change after registration is written
beside the plan with its date.

### 4.4 Nothing of the site in the code

As in Wave 5 (its §4.1): nothing in the engine, the desk, the assistant or the
gateway may read a path, a host name, a group name or a pool of the group's own.
A site's refresh (§15.3) is a procedure over the installer's own setup record,
not a script of the group's.

## 5. Data that comes in (B2)

A dataset is a folder with two trees (record 26): `derivatives/dcm-original`,
which holds identified files and is read only by the pseudonymiser, and
`derivatives/dcm-anon`, which the digest reads. The previous prototype's
`derivatives/dcm-raw` is renamed `dcm-anon` when the dataset is declared. This
section makes three things true on every path into the engine: a folder
without that layout is never read as de-identified; the pseudonymiser writes
into PatientID what the dataset chose; and a subject's code is the subject code
generator's.

### 5.1 Where it stands

*Amended in place on 2026-10-16:* this list is the engine as the wave found it on
2026-10-08, with the lines of the code of that day. Sections 5.3 and 5.4 say what
the wave built in its place, and 5.3 names the ruling that changed it.

- The engine's two defaults disagree. `handling_of` defaults `arrives` to
  `identified` (`nils-registry/src/place.rs:464`), while `dataset_of` and
  `default_dataset` default to `deidentified` with the trees
  `{originals: null, anon: "."}` (`place.rs:521-532`, `695-700`). A null
  `dataset` column reads as the second everywhere (`place.rs:155`, `184-191`).
- `nils place add --role source` and `POST /api/places` declare the dataset and
  carry only `handling.arrives` (`nils/src/main.rs:2733-2738`,
  `nils/src/serve.rs:2533-2543`, `4093-4104`), so every path that does not name
  the arrival ends de-identified.
- `nils setup` makes its source place with a null dataset and never looks at
  the folder (`nils/src/setup.rs:5263-5288`, `9043-9062`). A digest of such a
  place reads the whole folder: the walker does not skip `derivatives/`, and
  with no originals tree the originals guard (`main.rs:3828`) cannot fire. An
  undeclared folder that holds `derivatives/dcm-original` is digested whole.
- The layout step `dataset::look` (`nils/src/dataset.rs:178-237`) renames
  `dcm-raw` on any declaration, moves loose entries into `dcm-original` for an
  identified dataset, and into `dcm-anon` only on `--move-into-anon`.
  `place add` prints "N loose entries beside derivatives/, not read" even when
  they are read (`main.rs:2631`).
- The pseudonymiser always writes the subject's registry code into PatientID
  (`nils-release/src/scrub.rs:131-137`, `nils-pseudonymize/src/rewrite.rs:134-145`).
  A pseudonymised tree is read back with PatientID taken verbatim as type
  `subject-code` only for identified datasets (`dataset.rs:520-543`, `557-563`).
- The pseudonym scheme is chosen once at `nils init --scheme`, `blake2b-32` by
  default, and `nils setup` always makes `blake2b-32` (`setup.rs:1340-1356`).
  Pull request #369 makes `blake2b-8` the default, adds
  `nils setup --scheme` and `--reg-key-file`, and normalises a value of type
  `personnummer` to its twelve digits before any lookup or code.

### 5.2 The subject code generator (record 54)

- **The code:** the keyed BLAKE2b with an 8-byte digest of the twelve-digit
  personnummer under the site's own registry key, written as 16 lowercase hex.
  The engine's `blake2b-8` is this function byte for byte. Every site uses its own
  constant key. It is called the subject code generator everywhere: code,
  flags, documents and messages.
- **The default:** `nils init` and `nils setup` make `blake2b-8` registries;
  `nils setup --reg-key-file FILE` takes the key from a file with strict format
  and permission checks and prints only its fingerprint; a rerun that names
  another scheme is refused.
- **The number:** a value of ID type `personnummer` is normalised to twelve
  digits; a value that is not a personnummer fails the identity rule or refuses
  its map row and is held, never coded as given.
- **Not provisional:** a subject whose code comes from a personnummer is a
  subject, not a provisional one.
- The open rulings of record 54 (its D1 to D10) are this slice's defaults and are
  confirmed with the record's owner when the slice is built.

### 5.3 Never read without the layout

**Amended in place on 2026-10-16 by record 55 (B2, the ruling of 2026-10-08 that
the layout is read from the structure).** This section first gave `arrives` a
fourth value, `undeclared`, and had every path that adds a source ask how its
files arrive: identified, de-identified, or left undeclared. Nima's ruling
replaced the question by the structure: adding data is the one moment its
structure is expected, and the folder says what it is. A dataset's arrival is no
longer declared, and a declaration that names it is refused, in words saying what
decides it. What was built:

- **A source place is a folder, and what the folder is decides what is read.** A
  folder that holds `derivatives/` is one dataset. A dataset's own
  `derivatives/dcm-raw` or `derivatives/dcm-anon` is a legacy place, read as that
  tree. Anything else is a root, added alone: `nils setup`'s source place,
  `nils place add --role source` and `POST /api/places` read nothing under it. A
  folder under a root becomes a dataset, a source place of its own, only when a
  person adds it (`nils place add-dataset ROOT FOLDER`, `POST /api/places` with
  `root` and `folder`, or the desk's Add a dataset), and only then is its
  structure read. `nils place folders` lists a root's folders by name, a page at
  a time, without looking inside one; `nils place folder` looks at one before it
  is added and changes nothing: whether it holds DICOM, whether it has
  `derivatives/`, and what it would be.
- **A dataset is what its folder holds.** Only `derivatives/dcm-original` is
  *identified*: the pseudonymiser reads it and writes `derivatives/dcm-anon`.
  Only `derivatives/dcm-anon`, or `dcm-raw`, renamed `dcm-anon` and shown, is
  *anonymised*: the registry reads it. Both is *identified with its anonymised
  copy*. Anything else, which is entries holding DICOM beside `derivatives/` or
  no tree at all, is *unknown*.
- **Nothing unknown is read.** Nothing of an unknown dataset, of a root, of a
  folder that holds a place's folder, or of any `derivatives/dcm-original` is
  digested, brought in or pseudonymised, and the refusal says why. The digest's
  walk never goes into a `dcm-original` below the folder it reads.
- **A move happens only on the person's word.** The entries of an unknown
  dataset that hold DICOM go into the tree the person names, `originals` for
  identified data or `anon` for data that is already anonymised, and only with
  the confirmation: `--move-into originals|anon` with `--confirm-move` on
  `nils place add` and `nils place set`, `move_into` with `confirm_move: true` on
  `POST /api/places` and `PUT /api/places/{id}`, or a confirmation in the desk.
  Without it the answer is a 409 that carries the layout and names what would
  move, and nothing is written. Entries that hold no DICOM stay beside
  `derivatives/`, unread. The rename of `dcm-raw` to `dcm-anon` stays automatic,
  as ruled, and is shown.
- **Anonymised data says what it holds before it is read:** what its PatientID
  holds (`patient_id`, §5.4) and how its subjects are found (`subjects`). Either
  `map`, through a map of subject codes to its ids, given or already in the
  registry, a file whose id no map names being held; or `generated`, the subject
  code generator making each code from the id as from a personnummer, every
  subject a subject and never provisional.
- **The migration.** Registry schema 80 makes every source place without a whole,
  current declaration undeclared until its folder is explored again: one that
  `nils setup` made with no dataset, one that the old default had read its folder
  itself, and an anonymised one that does not say what PatientID holds and how
  its subjects are found. No file is touched, and each keeps its rule, cohort and
  tag lists.

### 5.4 What PatientID holds

A dataset declares what its pseudonymiser writes into PatientID, and so what
the digest reads back from `dcm-anon`:

| Declaration | What is written | Where the value comes from | When it is unknown |
|---|---|---|---|
| `patient_id: subject-code` | the subject's code | the subject code generator, from the personnummer in `dcm-original` | the file is held until a map or a person decides |
| `patient_id: id-type:<name>`, a type the registry knows | the subject's value of that ID type | the linkage store, through the subject found by the generator's code | the file is held until a map gives the subject a value of that type |
| `patient_id: id-type:<name>`, a new type | as above | a map the person provides (§5.5), with `--make-types` | as above |

- The digest reads `dcm-anon` by the same declaration: a subject code verbatim
  as type `subject-code` (`anon_rule`, now for every declaration, not only
  identified datasets), or an ID type through the linkage store.
- A dataset that arrives de-identified declares the ID type its PatientID holds
  and needs only the map from that type to the subject code.
- "Code them anyway" (`POST /api/linkage/held/code`) queues the pseudonymise of
  the held files and answers with its job, as the desk already expects
  (`nils/src/linkage_doors.rs:411-451`). Today it answers `{place, files}` and
  queues nothing.
- `nils place set NAME --patient-id …` and the desk's Change dialog can change the
  declaration of a dataset that has not been pseudonymised yet.

### 5.5 Maps

The CSV map import stays as it is (`nils-registry/src/identity_map.rs`; `nils
linkage import`; `POST /api/linkage/imports`): column roles `identifier:<type>`,
`canonical:<type>`, `code` and `ignore`. For the scenarios of §5.4: a map of
`canonical:personnummer` and `identifier:<type>` gives subjects their values of
an existing type; a map of `code` and `identifier:<type>` with `--make-types`
makes a new type. A map releases the held files it names, and the next
pseudonymise writes them.

### 5.6 Tests

- **T4.** A test per path of §5.3: a folder with loose DICOM and no trees is
  added by each path; 0 files are digested before the answer; a folder that
  holds `derivatives/dcm-original` is never read past it.
- **T5.** A synthetic dataset per row of §5.4: every file of `dcm-anon` carries the
  declared value; every unknown subject's files are held.
- **T6.** The generator's code on known cases under a lab key equals the
  reference implementation's, 100 %.

## 6. What a pseudonymised file says about itself (B3, K5)

### 6.1 The marks

No file NILS writes says today that it was de-identified: none of (0012,0062),
(0012,0063), (0012,0064) or (0028,0303) appears in the repository. Every file the
pseudonymiser writes into `dcm-anon`, and every file a release writes, carries:

- **(0012,0062) PatientIdentityRemoved:** `YES`.
- **(0012,0063) DeidentificationMethod:** a short text naming NILS, its version
  and the writer (pseudonymise or release).
- **(0012,0064) DeidentificationMethodCodeSequence:** one item per option the
  writer's own policy applies, from DICOM PS3.16 CID 7050. The options a writer
  can state are the eight in the table below, each only while its condition
  holds.
- **(0028,0303) LongitudinalTemporalInformationModified:** `UNMODIFIED`.

**Amended in place on 2026-10-16 by record 55 (the ruling of 2026-10-10, item 1,
which keeps the scanner's identity in a pseudonymised copy and claims it as
113109) and by the review of the wave's merge of 2026-10-10.** This section first
listed five options: 113100 always, 113106, 113108, 113110 and 113111. What the
writers state is:

| Code | Option | Stated while |
|---|---|---|
| 113100 | Basic Application Confidentiality Profile | every direct identifier and every element of the ids category is removed, retained under a claimed option or cleaned as a description: always under the default plans |
| 113105 | Clean Descriptors Option | always: the descriptions the rules read stay, cleaned of the file's own IDs, accession number, study id, birth date and the words of its names |
| 113106 | Retain Longitudinal Temporal Information Full Dates Option | no date is removed, which is always: the date is the date (record 38) |
| 113108 | Retain Patient Characteristics Option | the age, sex, size or weight is kept |
| 113109 | Retain Device Identity Option | the device's identity is kept (its station name, serial number, UID, gantry or unique device identifier): by the pseudonymiser, since the rules and a decision for a scanner read it |
| 113110 | Retain UIDs Option | the UIDs are kept, not remapped: by the pseudonymiser, not by a release that remaps them |
| 113111 | Retain Safe Private Option | the pack's allowlist keeps private elements |
| 113112 | Retain Institution Identity Option | the institution's name or address is kept |

By default the pseudonymiser states 113100, 113105, 113106, 113108, 113109 and
113110, and a release states 113100, 113105, 113106 and 113108.

The option list is derived from the writer's policy, never written by hand, so
a change of policy changes the marks. The marks are added in `scrub::apply`
after PatientID (`nils-release/src/scrub.rs:131-137`), which the pseudonymiser
and the release share, with the writer's option set passed in. The release's
removal list (`nils-release/src/tags.rs:159-183`) keeps the four tags.

### 6.2 The tag list, exact

The engine serves its pseudonymisation tag list to the desk
(`GET /api/pseudonymize/tags`, `nils-pseudonymize/src/policy.rs:67-110`, record 28).
Its five known faults are mended in the same slice: the reason of a covariate
row is carried on the row; the remapping of mandatory rows is shown in `why`; the
document has a version key for caching; `fates` is usable as served; and
`opt_out` is marked not for display. The document gains a `marks` block that
lists the marks of §6.1 per writer. The contract (`contracts/openapi/v7`) is
amended in place for these additive fields.

### 6.3 Test

**T7.** Every file of a pseudonymised test dataset and of a release of it carries
the four marks, with the option list its writer's policy names.

## 7. Rows that leave the engine (K7, K9)

### 7.1 The quasi-identifier rule at every door

The catalog classes every field (`nils-catalog/src/lib.rs`): technical,
quasi-identifying, sensitive, identifying. A person's detail level (record 25)
says which classes they may see. Today the rule is enforced in some doors and
not in others:

| Door | Enforced today | Where |
|---|---|---|
| The values sampler (`GET /api/ask/catalog/{level}/{field}/values`) | yes: shapes only for quasi, sensitive and identifying fields | `nils-ask/src/affordance.rs:382-400, 462-480` |
| The profile door (`POST /api/ask/profile`) | yes, by class against the caller's detail | `nils/src/profile.rs:268-345` |
| Classify signals | yes: a text shown only if it covers 5 stacks of 3 subjects | `nils-classify/src/signals.rs:236-337` |
| A campaign item's header, the reader, pipeline summaries | yes, below quasi | `nils/src/file_header.rs:499-569`, `reader.rs`, `pipelines.rs:297-470` |
| Handle rows, export and MCP `rows` | relative to the run that made the handle | `ask_doors.rs:162-200, 1443-1487` |
| **Ask run, preview and jobs** | **no:** `check_class` lets quasi fields through at any detail | `nils-ask/src/validate.rs:1709-1724` |
| **MCP** | **no:** it proxies the doors above | `mcp.rs:181-250` |
| The CLI (`nils ask run`, `handles export`) | holds every class by design: a person at the keyboard of the registry | `ask_cli.rs` |

**The change.**
1. **First, the ruling of 2026-10-01:** sequence names are never hidden. The
   series description and the protocol name (`stack.text_series_description`,
   `text_protocol_name`) move from quasi-identifying to technical. The station
   name, `text_all` and the dates stay quasi-identifying.
2. **Then the rule in `check_class`:** a quasi-identifying field in a run, a
   preview or a job is projected as its shape below the quasi level, as the
   sampler does, and its use as a filter stays allowed. `Catalog::may_project_raw`,
   which states the rule and is called only from tests today, becomes the one
   place it is decided.
3. MCP inherits it. The CLI keeps every class and writes an audit row per run.
4. A release and a campaign export are governed by their own policy (the
   pseudonymous release of D13; record 38's date) and are outside this rule; their
   doors are listed in the custody page with what they carry.

**T13:** a test per door of the table, at each detail level, that the fields it
returns are the ones the person's level allows, and that sequence names are
returned at every level.

**Aggregates of a quasi-identifying field (the ruling of 2026-10-12).** Below
detail quasi, at plain, the ask answers a group's summary of such a field, an age
for one, as follows. A group is a group set's child, keyed by the group's key;
each limit counts the group's subjects that have a value, never its rows, so one
subject's five visits are one subject.

| Of the field, over a group | At detail plain |
|---|---|
| The mean (`avg`) | answered when the group holds 5 subjects or more (`MEASURE_K`, as a pipeline's measures under record 49 R4); held back below |
| The spread (`stddev` of the group's child) | the same |
| The count of distinct values (`distinct`) | the same |
| The min, the max, the median, a percentile | never answered: each is one person's own value. The min and the max are shapes; the median and a percentile are refused |
| The sum and the list | shapes |
| A count of rows or subjects, a comparison | answered, but a count of rows the field chose or grouped is held below 5 subjects (small counts, below) |

A held-back value is null, and every answer that holds rows names its columns
with a value held back and why, beside `shaped`: `held_back`, as
`[{column, why}]` with why `a group under 5 subjects`, on a run and its cached
answer, a preview, a handle and a page of it, and a job's result. A mean or a
count of distinct values made anywhere but as a group set's own binding reads a
row's own subject, session or stack, one person's rows, and stays a shape. What a
group's other bindings and the answer's columns make of a mean alone (a rounded
mean, a share of it) is held with it; made with a shaped value, it is a shape.

**Small counts (the ruling of 2026-10-12).** Below detail quasi, at plain, a
count of rows that a quasi-identifying field chose or grouped is answered only for
none or 5 subjects and more (`MEASURE_K`); between, it is held back: null, and
named in `held_back` with why `fewer than 5, or held beside a count of fewer than
5 so that it is not found by subtraction`. Counts of rows that technical and
clinical fields alone chose and grouped stay exact, and detail quasi and above see
every count. The chat bench found it: a plain reader rebuilt ages as counts per
age band, one band holding a single subject.

A quasi-identifying field chooses a set's rows where:

| The set | Example |
|---|---|
| a `where` reads one, through any comparison or function, or a binding made from one | `age >= 45`, `sex = 'F'`, `first between two dates` |
| it is a group keyed by one, or by a value made from one | a band of the age, the year of a date |
| it keeps the rows that have rows within a window of days (`has` with a window), or that have a partner a near finds (every near reads a window of days; an optional near does not filter, but everything read of its partner reads a day) | sessions with a score within 90 days |
| it keeps the rows that have rows of such a set (`has`, `attach`), narrows one (`from`), lies under one (`of`), combines one (`algebra`), or groups one | the sessions of the subjects aged 40 to 44 |
| a group's own `where` reads one | the bands whose mean age is over 40 |

A `pick` keeps the same number of rows whatever it orders by, so it chooses none.
A subject's ordinal, the visit number, is no quasi-identifying field.

What is held, at plain:

| The count | Held |
|---|---|
| `level: count` of such a set: its rows and its subjects | when it holds 1 to 4 subjects |
| `level: boolean` of such a set | null (neither 0 nor 1) when it holds 1 to 4 subjects |
| a count of a group set at `level: count` (its number of groups) | when the groups hold 1 to 4 subjects |
| a group's `_rows`, `_subjects`, `count` and `distinct`, where such a field chose the group's child or keys the group | when the group holds 1 to 4 subjects, every count of the group's row |
| a group's count of its own by such a field: a `count` of a value made from one (the subjects with a value), a `sum` or an `avg` of one read through a comparison (`n_female`, the subjects whose value is not 0) | when it counts 1 to 4 subjects, or the group's other subjects are 1 to 4, which its total less it would say |
| what the group's other bindings and the answer's columns make of a held count (twice it, a share of it), and a `share` measure of one | with it |
| a `share` measure over such a set | in every row, when the set holds 1 to 4 subjects |
| the diagnosis's funnel: a stage's rows and subjects, its keys, and what its clause group lost | when such a field chose the stage and it holds 1 to 4 subjects, when it dropped 1 to 4 from the stage before it, and when it starts from a set whose last stage is held |

**No count is found by subtraction.** The ask answers a group's total anyway,
as another question: the subjects of a cohort, the subjects with an age. So a
group's count held alone would be that total less the counts shown. I chose to
hold one count more, never the total, since the total is an ordinary question
of its own: in every line of groups (with one key, all the answer's groups; with
several, the groups that share every key but one) that holds a count, where the
line holds one count alone, or only counts under 5 (two groups of 1 would be
found from their sum of 2), the line's group with the fewest subjects of those
shown is held too, the first by key on a tie. A count held beside another is not
under 5 itself, so the why says both, and the desk and the chat never say which
it is: saying it would tell the reader the other one. With several keys, the
passes run three times over every key, so a count held for one line is not left
alone in another. The groups held are decided over every group of the answer,
before the group's own `where`, a page or a limit, so the rows a page shows never
change which are held.

What the desk and the chat see: a held count is null in the rows (an empty cell),
and the answer's `held_back` names its column with the why, on a run and its
cached answer, a preview, a handle and a page of it, and a job's result; the
diagnosis's stages carry `held_back` too, with null `rows` and `subjects`. A
handle records `quasi_counts` in its suppression, so no answer cached while such
counts were numbers is served again.

**Small lists (the ruling of 2026-10-13).** A list's length is a count, so the
rule for counts holds for lists too. Below detail quasi, at plain, an answer that
lists the rows of a set a quasi-identifying field chose (by the table above) is
answered only where the set holds none or 5 subjects and more; where it holds 1
to 4, no row is answered at all. That covers every level that returns one row
per member, `level: record` and `aggregate`, and a group set over such rows,
whose number of groups says something of the few subjects under it (the subjects
under the group set decide). The subjects are counted over the whole set, before
an `after`, a limit or a page, in the same read as the list, so a page never
decides it and the rows are never read.

| What the answer says | Where |
|---|---|
| no row, and every column named in `held_back` with why `a list of fewer than 5 subjects that a quasi identifying field chose; its length is a count, so no row is shown` | a run and its cached answer, a preview, a handle (which has no page: page 0 answers 404 as an empty handle's does), a job's result, MCP |
| `row_count` null (never 0, which would say none), `pages` 0, `truncated` false | a run, its cached answer, a handle, the list of handles, a kept set, a job's result |
| the stage's `rows` and `subjects` null, no keys, `held_back` | the diagnosis's funnel with keys; a pick's ties are null |

A handle frozen from such an answer holds no row and no key, so no later page,
export or reader can read one. It is no source either: a set read from it, a start
from it and a promotion of it into a cohort are refused (409, `handle N holds no rows`), since
either would answer none where there were some. The CSV export of it writes the
header alone and says why. A list of the rows that technical and clinical fields
alone chose (the visits numbered 6, a scanner's sessions) is answered at plain
whatever its length, and detail quasi and above see every list. A handle records
`quasi_lists` in its suppression, so no answer cached while such a list held its
rows is served again. Only the ask's own reads for itself are not held: a share's
denominator, a group's spread, the values sampler and a selection frozen for a
pipeline, a campaign or a seal, whose rows never reach the reader.

> **Limit: plain detail still filters.** A filter, an order and a count read a
> quasi-identifying field itself at every detail, and the ask holds each answer to
> these rules, not a person's series of questions. So repeated questions can
> narrow a value: asked again with a narrower filter (ages 40 to 50, then 40 to
> 45), a count of 5 or more says how many people are left in the range, a
> count of the ages outside the range less the whole says the ones inside, and a
> group's mean moves by what the narrower filter took out. A row's own count of
> its subject's rows (a subject's visits in a year) is one person's and is
> answered. The real protection is who is given plain detail (record 25):
> give it only to people who may see such counts and summaries of the registry.

### 7.2 What a release records about itself (K9)

Three items record 26 left for later join this wave's de-identification work:
- **Which UIDs a release carries:** the original or remapped ones, by release
  (record 17); the choice is recorded on the release row and in the marks of §6.1
  (113110 or not).
- **The series description in pseudonymised folder names:** `dcm-anon`'s folder
  names carry the series number and the series description reduced to letters,
  digits and hyphens, since sequence names are not identifying (§7.1); to be
  confirmed with the record's owner when the slice is built.
- **A merge proposed from birth date, sex and overlapping visits:** the identity
  station of the assistant (§14) proposes a merge of two subjects when these agree;
  it proposes only, and a person merges.

## 8. Releases that always name a scan (C4, B7, C3)

### 8.1 A name for every scan

Today a release groups stacks by (subject, session, datatype)
(`nils-release/src/run.rs:3668-3670`). A group that shares one name gets `run-n`
only when `bids/repeat.rs` (`one_acquisition`) finds no difference and the suffix
admits a run; otherwise each stack is refused its BIDS name, routed to
`sourcedata/` as DICOM under its descriptive name (`run.rs:3741-3790`,
`bids/place.rs:181-190`), and raised as a `release.shared_name` review item. The
descriptive layout adds a plain counter (`name.rs:266-309`).

The rule of the wave: **a release is never refused for a name conflict.** The
refuse-and-route branch is replaced by, in order:
1. **A true repeat** (one session, the same in everything but the time) stays a
   run, as today.
2. **A difference among the classification axes** is named by the axis's value.
3. **Any other difference** is named by the property and its value, taken from
   `differs` (the slice thickness, the repetition time, the matrix and so on).
4. **The last fallback** is a plain number, never called a run.
5. **Body part stays in the name,** as the first group of the `acq-` label
   (`packs/mri/bids.yml:157-179`) and as the descriptive layout's prefix.

**Amended in place on 2026-10-16 by record 55 (C4, the rulings of 2026-10-08:
how a name is built, the three ways to name a stack, FLAIR is a modifier, and two
styles with a full sidecar).** This section first described two modes, a BIDS
mode that put a difference into `acq-` and an informative mode that carried a
`diff-` entity (for example `diff-SliceThickness3`). The rulings replaced them,
and the `diff-` entity does not exist: strict BIDS admits no entity of ours, and
the validator refuses one. A release names its stacks in one of three styles,
chosen per release:

- **Descriptive:** v0's grammar as before, with `_` between its slots.
- **Full BIDS** (`--naming full`, the default): the suffix is the base contrast,
  or a construct suffix BIDS defines; whatever has an entity of its own goes to
  it (`ce-`, `rec-`, `part-`, `echo-`, `inv-`); every other axis is spelled in
  `acq-` in v0's slot order, joined by `+` (body part, with the brain implicit
  and the spine `SC`; orientation; 2D or 3D; the modifiers; the technique; the
  construct): `acq-Ax+2D+FLAIR+IRTSE_T2w`.
- **Minimal BIDS** (`--naming minimal`): `acq-` holds only 2D or 3D, the
  modifiers and the technique: `acq-2D+FLAIR+IRTSE_T2w`. Body part is in a
  minimal name only where it is what separates two stacks.

In both BIDS styles FLAIR, like STIR, DIR, PSIR and FatSat, is a modifier and
never the suffix. A release recorded under the earlier `bids` or `informative`
reads as `full`.

A name conflict (rules 2 to 4 above) adds what separates the stacks: the value of
a classification axis where they differ on one, in that axis's own slot;
otherwise the measured property and its value (the slice thickness, the
repetition time, the matrix and so on) after the pack's slots; as the last
fallback a plain number, which is never a `run-`. In a BIDS name all of it is in
`acq-` (`acq-Ax+2D+FLAIR+IRTSE+3mm_T2w` in the full style,
`acq-2D+FLAIR+IRTSE+3mm_T2w` in the minimal one); in the descriptive layout it is
a `_` slot of its own before v0's counter (`Ax_T2w_2D_FLAIR_TIRM_ND_3mm`). True
repeats keep `run-`. No text, such as a protocol name or a series description,
ever makes a name. A scout or a working scan, which never takes its BIDS name,
takes no part in a name conflict.

The release's record lists every name that a difference or a number decided,
with the property and the values, and its README says how many there are. A
`release.shared_name` item is raised only if two stacks differ in nothing the
engine can see and are not a repeat. Every BIDS sidecar carries a `NILS` object
with every classification axis, the descriptive name and the acquisition as the
registry measured it, and every `scans.tsv` a `nils_name` column.

### 8.2 The validator in CI (B7)

CI runs `tools/release-check/gate.sh` (`.github/workflows/ci.yml:276-277`), whose
bar 2 is a structural check against a copy of the BIDS schema; the official
`bids-validator` runs only if it is on the path (`tools/release-check/check.py:101-140`),
and CI installs none. CI installs a pinned version of the official validator,
and the gate runs it on the fixture release in both BIDS naming styles, full and
minimal (§8.1; *amended in place on 2026-10-16 by record 55, C4*, where it said
both naming modes): 0 errors, with warnings listed in the job's summary. A
release on the development install's test data is validated the same way by hand
before the slice merges (T12).

### 8.3 Display composites (C3)

Synthetic-MR maps saved in colour for viewing are read today as screenshots:
set aside and unclassified. The MRI pack gains a value of its own for them,
"display composite": they stay classified and findable, and a release's default
selection leaves them out unless they are asked for. Shipped as a rules release
(§9). The rule for vessel pictures made from a TWIST-VIBE scan in the same
session (angiography, not DCE) waits for a real case and is noted in the pack's
documentation.

**Tests:** T12, T17, T18.

### 8.4 SyMRI and the DICOM export (C4, ruling of 2026-10-09)

SyMRI is anatomical and stays together, as the previous prototype kept it
(`anat/SyMRI/`): its maps and synthetic contrasts are NIfTI under
`sub-*/ses-*/anat/SyMRI/` with their BIDS names, and its multi-dynamic
multi-echo or QALAS acquisition, a working scan that is not an image to convert,
is kept as DICOM. The pack names the folder and what goes in it
(`packs/mri/bids.yml`, `folders`: provenance SyMRI, technique MDME or QALAS), and
`.bidsignore` lists it. `--synthetic folder` is the default; `anat` and
`derivatives` keep their earlier meaning.

A BIDS release also writes a DICOM export inside the BIDS folder, as the previous
prototype's DICOM tree did: each converted stack's scrubbed slices under
`sourcedata/dicom/`, at the path its NIfTI has, in a folder named after the NIfTI
file without its extension, one file per slice. Every DICOM a release writes
is under `sourcedata/dicom/`: the scouts and working scans that went to
`sourcedata/sub-*/ses-*/` before are now at
`sourcedata/dicom/sub-*/ses-*/<datatype or localizer>/<name>/`, so that
`sourcedata/` can hold other kinds of source beside the DICOM later.
`--dicom all|folders|none`;
`all` is the default because the prototype exported every stack. The export is
part of the stack's state (an `@dicom` entry among its extensions), so a re-run
moves it with a rename, drops it with the stack, and a handover packs it.

## 9. Rules that ship on their own (B5, C2)

### 9.1 Where it stands

The packs travel inside the engine's release: `release.yml:54-76` tars `packs/`
into `packs.tar.gz`, covered by `SHA256SUMS`. An update compares each pack by
version and digest against the engine release's tarball, keeps the site's own
packs and edited first-party packs, and moves replaced packs to `<dir>.previous`,
one update deep (`nils/src/packs.rs`). A pack's manifest (`packs/mri/pack.yml`)
names its `pack`, `version`, `contract` and `modality`, and no engine; the loader
refuses only a pack contract above its own (`nils-pack/src/pack.rs:34-52, 313-320`).
Every rules fix therefore needs an engine release: twenty-five in Wave 6's eight
days.

### 9.2 Rules releases

- **A channel of its own.** A pack is released under its own tag
  (`pack-mri-v1.0.2`), with its tarball, `SHA256SUMS` and the same signing as the
  engine's assets. The development channel of record 31 serves packs beside the
  engine.
- **An engine range.** `pack.yml` gains `engine: ">=1.0.0-alpha.80, <2.0.0"`. The
  loader refuses a pack outside the running engine's range with a named reason,
  and the update path never installs one, the way a desk release's contract
  floors are checked today (`nils/src/releases.rs:34-60`).
- **The update path.** `nils update --check`, `nils update` and the supervisor
  read the pack channel beside the engine and desk channels
  (`packs::status` and `refresh`, `packs.rs:359-380`). The site's own packs and
  edited first-party packs are kept as today, and `.previous` stays one deep.
- **The desk.** Settings › Parts lists each pack with its version, its newer
  release and an update control.
- **Versions.** The certified rules are MRI pack 1.0.0, the paper's named release;
  production fixes ship as 1.0.1, 1.0.2 and on. *Amended in place on 2026-10-16 by
  record 55 (B5, ruling 31 (f)):* production runs the newest rules release, 1.2.3
  today, and 1.0.0 is reachable by the pin, which is for reproducing the paper; a
  site does not pin otherwise.

**As built (2026-10-10).** The pack contract is the compatibility key: every
engine already refuses a pack whose contract is above its own, so a rules
release that needs a later engine declares the contract that engine
introduced. The `engine` range is pack contract 10, an optional key for what
a contract cannot say (an engine fix a rules fix relies on, an upper bound);
the loader refuses it in a pack that declares less, so an engine at 9 refuses
such a pack by its contract. The MRI pack names no range yet; it gains one
with the release that needs it. A release carries `pack-<name>.tar.gz` (holding
`packs/<name>/`), `pack-<name>.VERSION` and `SHA256SUMS`, is published by
`rules-release.yml` and is never the repository's latest release. The engine
finds a pack's releases in the GitHub listing by their tag, or on a channel of
a deployment's own by `latest/download/pack-<name>.VERSION`. The update path
reads a release's `pack.yml` before it installs it; the engine's own release
stays the floor, and a pack's own release goes in where it is at least as new
as the copy the engine carries, in the same single swap. `nils update --part
rules` takes the rules alone and starts nothing again, since the engine reads
a pack again whenever its files change. The setup record notes the rules in
use (`[rules.<name>]`). An engine in a container reads the packs its image
carries.

*Amended in place on 2026-10-16 by record 55 (B5 and the ruling of 2026-10-10,
item 4; ruling 31 (f)):* the MRI pack names its engine range from 1.1.0 on (now
`>=1.0.0-alpha.82, <2.0.0`, pack contract 10). Its first rules release on a tag of
its own is `pack-mri-v1.2.0`; 1.0.2, 1.0.3 and 1.1.0 shipped inside it, and
`pack-mri-v1.2.1` to `pack-mri-v1.2.3` followed. The certified 1.0.0 and 1.0.1
were published by hand on 2026-10-16 as `pack-mri-v1.0.0` and `pack-mri-v1.0.1`,
from the packs that engines alpha.78 and alpha.79 shipped (the commits that made
them are older than the release workflow), so that an install can be pinned to the
rules the paper reports.

### 9.3 The release discipline (C2)

Three standing rules, written into the release procedure:
1. The protocol truth table follows every ruling.
2. Every reading test's stored answers are saved before new rules run over the
   archive.
3. A production rules fix is replayed over the whole archive (`nils pack replay`
   on the flat header packets), every scan whose answer changes is listed and
   checked, and only then is the fix released.

**Tests:** T9, T16.

## 10. Models as parts (D4, A5, E5, E6)

### 10.1 Where it stands

The engine registers models with a card (`contracts/model/v1/card.schema.json`:
name, version, kind, digest and task required; slot, encoders, `trained_on`,
threshold, pack version, artifact, free-form metrics, preprocessing, params,
intended use, limits and runtime optional) and moves them through registered,
admitted (after a recorded check), promoted and retired; promoting a model
retires the one promoted before it in the same task and slot
(`nils-registry/src/model.rs`). A model's answers become derivatives and
proposals: `proposals::ingest` makes `<axis>:model` review items, stages
decisions authored by the model at or above its card's threshold (only admitted
or promoted models may author), and a person commits them
(`nils-registry/src/proposals.rs:482`, `review.rs`). The card has no field for
speed or for a GPU (a pipeline's descriptor declares its GPU need,
`contracts/job/v1/nils.job.schema.json:94-99`); nothing chooses a certified
model by default; a label set can be registered only if the registry wrote it,
and `trained_on` must name such a set (`model.rs:586-591`).

### 10.2 Several models per task, one chosen

- **The card gains:** `tested` (a list of claims, each with its value, its bound
  and the evidence run it comes from); `speed` (stacks a second on CPU and on
  GPU, with the hardware each was measured on); `gpu` (`none`, `optional` or
  `required`); and `equivalence` (the class and tolerance of its CPU and GPU
  answers, §10.5). The contract is amended additively.
- **Several admitted models per task.** Promotion no longer retires the model
  promoted before; each task and slot has a **chosen** model instead, a pointer a
  site's admin sets, which defaults to the model a recorded certificate names
  (`certificate.model_ids`). Choosing is audited. A retired model's files stay
  where its card says, so a past result can be run again.
- **Where it is chosen:** the Models page under Pipelines (§13.1) shows each
  task's models with their cards side by side and the chosen one, and lets an
  admin choose.

### 10.3 Label sets from outside (E6)

A new door, `POST /api/label-sets/register` (and `nils labels register`), takes a
label set made outside the engine: its `labels.tsv`, its provenance and the
sha256 of every file. Its rows are keyed by DICOM identity (Study, Series and
SOP Instance UIDs, and the stack key within a series), so it means the same in
any registry. It is identified by name, version and the digest of its canonical
`labels.tsv`, like a set the engine wrote. The sealed check compares its rows by
DICOM identity with every sealed sample and refuses a set that overlaps one; a
card's `trained_on` may then name it.

### 10.4 Scoring at arrival, and the slice cache (E5)

- **A step after classify.** A bring-in chains pseudonymise, digest, fingerprint
  and classify and nothing after (`nils/src/chain.rs:52-90`); pipelines run only
  on request over a frozen selection. The chain gains a last step, `score`: for
  each task whose chosen model's pipeline declares `on_arrival: true`, it runs the
  pipeline over the batch's new stacks, under the grants the chain recorded.
- **The slice cache.** The pictures a model reads from a scan are cached per
  session, keyed by the SOP Instance UID and a sha256 of the file's content (the
  engine's own file fingerprint is size and time, `schema.rs:341-356`, and its
  `fingerprint` means the header record), and by the model input's version. It
  lives in the site's working place, since the pixels are clinical, and never
  leaves the site. A change of a model's input version invalidates its entries; a
  stated retention keeps the cache bounded. Because the key is DICOM identity, the
  cache survives a new registry (§15.3).
- Body part's plane cache (2 kB a stack) is written by this step.

### 10.5 The same model on CPU and on GPU (A5)

The admission suite runs a model on a fixed equivalence set on CPU and on a GPU
and records the class of the difference (§16.3) on its card; a model whose class
is worse than its card states is not admitted. Exported and quantised forms are
tested the same way, on the file that ships. A GPU is taken under the lease rule
of §4.2.

### 10.6 Site models and public models

A model trained on a site's own data is a site model: it is registered with its
files copied into the site's model store (`nils model keep`), and it never
leaves the site. A model trained only on public data may ship in a public
release, with its card and the sha256 of its files, and an install registers it
from the release.

**Tests:** T22, T23, T26.

## 11. System 1 in the review rule, and the rules that decide (D3, D6, D7; record 56)

### 11.1 System 1 as a pipeline

System 1 runs as a pipeline whose model is a site model (§10.6), never part of
the public engine. It runs over every stack at arrival (§10.4) and over the
archive on request, and for each stack and each axis it writes its answer, its
calibrated probability, and a reliability score for the rules on that header (the
error model of §16.2 with the signs of damage: missing fields, unknown words, an
unseen vendor, a fallback clause). These are derivatives authored by the model,
through the existing proposal path.

### 11.2 The gate

**Amended in place on 2026-10-16 by record 56 (accepted 2026-10-09), record 55
(the ruling on D3 of 2026-10-09, and rulings 20 and 27).** This section first put
every stack and axis through the gate. The gate is chosen per dataset, not per
scan:

- **A dataset with normal headers:** the rules decide. Review holds only the axes
  that matter that the rules left empty (§11.6), and System 1's opinion is a
  second opinion beside those questions; it sends nothing there.
- **A dataset flagged as having damaged or foreign headers** (§11.6): the full
  gate applies, as this section first described it for every dataset. For each
  stack and each axis the model's card gives thresholds for, the rules' answer is
  **accepted** when System 1's answer equals it, System 1's probability is at
  least λ1, and the rules' reliability is at least λ2; λ1 and λ2 per axis come
  from the frozen model's card, set by Learn then Test (§16.2). An accepted answer
  stays the rules' decision and records both authors: the rules' version and
  System 1's model id. Every other stack and axis becomes a review question:
  `<axis>:disagree` when the two answers differ, `<axis>:unsure` when they agree
  below a threshold. An axis a person decided is not asked again, and a stack of a
  sample sealed now never becomes a review item.
- **System 1 never fills an answer silently** (ruling 20, 2026-10-14): where the
  rules name nothing on an axis of a flagged dataset and System 1 is confident at
  its threshold, its answer is staged on the gate's question for a person to
  confirm.
- **When the gate runs.** It runs once System 1 stands, which is once its model is
  admitted and chosen for its task (ruling 27, 2026-10-15: its admission and its
  replay stand for a certificate, and a certificate that is recorded is shown and
  is no longer required). With no model chosen it raises nothing. It runs on
  request (`POST /api/review/gate`) and after every System 1 run.

A pair of a rules version and a System 1 version is a round; a new rules release
or a new chosen model starts a new round, and the gate's thresholds belong to the
pair. `GET /api/review/rounds` reports each round, what it accepted and what it
sent to Review, by kind.

### 11.3 Both suggestions and both reasons

A review question carries:
- **the rules' suggestion and reason:** the clauses and fields that decided, and
  the text each matched, also for the axes that did resolve (today the signals
  sample text only for stacks an axis left unresolved, `nils-classify/src/signals.rs`);
- **System 1's suggestion and reason:** its answer, its probability, the fields
  that mattered most (each field's effect measured by leaving it out), and up to
  five similar settled stacks, drawn only from settled stacks outside every sealed
  sample and shown for display, never as votes.

### 11.4 Blind first where a read trains or measures

- Every review question carries a mode: `routine`, where both suggestions and both
  reasons show at once, or `blind_first`, where the person answers from the
  evidence (the image, the header text, the fields) and the suggestions show only
  after a provisional answer.
- Campaigns whose answers train a model or measure an error are `blind_first`
  always. In the queue a random share of questions is `blind_first`; the share is a
  site setting, one in five by default.
- Every answer records whether it was given blind and the time it took. **Only blind
  answers become training labels** (§16.2); the rest are decisions.
- A blind read hides NILS's own answers (the suggestions, the rules' values,
  System 1's scores, the matched words) and never the header text a reader needs
  (record 48).

### 11.5 The audit stream

Every round, a random 1 % of the accepted stacks is drawn by a seeded draw keyed by
DICOM identity and read blind: by agent raters outside the engine, as the
reference of record 48 was read, with a person settling their disagreements. The
engine provides the draw (`POST /api/audit/draws`, recording the round, the seed
and the frame) and an audit campaign kind whose answers are marked never to train.
The accepted error of the round and its bound are computed from the settled
answers and recorded beside the round.

**Tests:** T19, T20, T21.

### 11.6 The rules engine (record 56)

*Added on 2026-10-16, as built.* Record 56 (accepted on 2026-10-09) turned the
review rule round: the rules decide, a person is asked only where they cannot,
and an error is mended in the rules, not scan by scan. This section is what was
built for it. The gate it leaves to System 1 is §11.2, and the pack's own
releases are §9.2.

**What goes to Review.** A scan becomes a question only when an axis that matters
is left empty by the rules or, on a dataset flagged as damaged, when System 1
does not confirm them (§11.2). The axes that matter are those that the release's
names, the main-scan pick or a release read; the list comes from the code, and a
test fails if it drifts from what the release reads. An axis is asked about when
it is missing only if it holds one value, has no default and is not an image
model's; for the MRI pack that is the base alone. Never a question: a conflict
between the pack's rules (the pack's order decides, and the rule that lost is
kept as evidence), a rule's low confidence, body part and post-contrast (each has
its own operation and its own review, named in the pack's `review.by_model`), a
scout's base, the split note, and an equal-rank disagreement inside the pack,
which is a defect of the pack and goes to its quality report. The pack is silent
on a localizer, an excluded stack and a display composite. Not ruled yet: the
session pass and the physics vote still raise a question (`<axis>:session`,
`<axis>:vote`) for an answer below their threshold of 0.70, and a broken
constraint raises `classify.*`.

**Questions are answered in bulk.** `GET /api/review/questions` gathers the
sort's own questions that wait for a person into plain ones over a dataset's, a
cohort's or the registry's scans: an axis and what is in dispute on it ("Which
base?", "T2w or FLAIR?"), each counting its distinct scans and naming the two
answers its keys give. `GET /api/review/questions/{key}/scans` pages one
question's scans, each with its picture, NILS's answer, the rules' reason with
the rules it was decided over, System 1's answer and any answer staged for a
person. `POST /api/review/questions/{key}/answer` gives one value to the scans
the person saw, with the exceptions set apart, as one decision per scan through
the review spine in one transaction, and then re-sorts those scans alone, at once,
by the pack and the overlay that sorted them. A question about several axes at
once, and an item an open campaign holds, are answered elsewhere. A blind-first
question hides NILS's own answers on its tiles, never the header text, and an
answer is marked blind only where nothing of NILS's or System 1's answers was on
screen. A stack of a sample sealed now is in no count.

**A rule change is a list of typed operations.** The patch contract
(`contracts/patch/v1`) names the operations most changes to a pack are:
`add_words` and `remove_words`, `move_rule`, `move_set`, `set_priority`,
`add_value`, `add_rule` (a rule built from templates: words in a field, a tag
equal to a value, a physics range, a flag or an axis value, alone or combined; on
an axis that holds several values, `nothing: true` makes the stack hold none of
them where the conditions hold), `silence`, `by_model` and `map_name`, and four on
the main-scan pick, which are pack edits only:
`set_candidates`, `remove_border`, `set_near_tie` and `set_runner_up_within`. Each
carries its scope, its reason and its evidence. A scope of the whole pack is a
pack edit and ships as a rules release (§9.2); `site:NAME`, `dataset:NAME` and
`scanner:KEY=VALUE` make an overlay, and an overlay never leaves its scope. The
engine applies the operations in order to a copy of the pack's own documents,
builds the result with the pack's own loader, refuses an operation that cannot
apply with a plain reason, and lets the pack's corpus and the patch's own cases
judge the result. `nils pack apply` checks a patch and, with `--out`, writes a
pack edit as the pack's next version, each file rewritten only where it changed,
so its comments stay.

**Its effect is shown before anyone decides.** `nils pack rehearse` and
`POST /api/packs/{name}/rehearse` (pipelines:work or review:work) sort the
registry in scope twice, once by the pack and once by the patched pack, and write
nothing. The report has, per axis, the stacks that move from one value to another,
each row with its datasets, sites, makes, scanners and example stacks; the names,
descriptive and BIDS, that change, before and after; the main-scan picks that
change, by role, each side scored by its own pack's pick; the Review questions
that appear and go, by kind; against the answers people settled (Review answers
and a campaign's close), the right answers it turns wrong and the wrong ones it
turns right, measured on the rules' own answer; and the scope and what the change
ships as. A stack of a sample sealed now is never read. A scope of more than 2,000
stacks is rehearsed as a job, and one of more than 200,000 is refused.

**The pack's quality report** (`GET /api/packs/{name}/quality`, review:see) is for
whoever tunes the pack: the equal-rank disagreements by rule pair; the overrides,
which rule the pack's ranking put above which, by rule pair, with the stacks,
datasets and rules versions; and System 1's confident disagreements with the
rules once a System 1 model serves. It can be read for one dataset.

**A proposal keeps a rehearsal until a person decides.**
`POST /api/packs/{name}/proposals` (pipelines:work or review:work) keeps typed
operations with their effect report as a proposal, a pack edit or a scoped change
but not both, with the approval card a person decides on: the stacks it moves, its
largest moves, the answers it fixes and breaks, every answer it breaks named with
its scan, whether the pack's cases hold, and the questions before and after. A
proposal is stale once the pack version or the scoped changes in force have moved
after it was rehearsed, and it is rehearsed again before it can be adopted.
`POST .../proposals/{id}/decide` adopts or rejects it, with review:work and
data:work, and it is a person's: an agent or a model is refused. Adopting a pack
edit writes the pack's next version beside the pack, the version it replaces kept
whole under `<packs>.versions`, and records it as a rules version to release;
adopting a scoped change keeps it for every sort to apply to its scope alone.
Either way the re-sort is queued, and it writes on the proposal what it did
(`applied.measured`: the stacks judged, those whose answer changed and on which
axes, and their answers before and after). Every check that can refuse runs
first, and the adoption, its re-sort's job, the audit row and the handles it
invalidates are written in one transaction.

**An overlay is adopted only as a proposal** (ruling 26, 2026-10-15).
`POST /api/overlays` keeps an overlay as a proposal, rehearsed over the overlay's
own origin with the scoped changes in force, and `POST /api/overlays/{id}/adopt`
goes through the proposal's decide path: the same stale check, transaction and
approval card, and a person's. A newer adopted version of an overlay retires the
older; adopting an older one again is a rollback, done by proposing it again; any
adoption makes the open proposals stale until they are rehearsed again; and
withdrawing an adopted overlay is audited. `nils overlay` lists, shows, exports
and refuses; it no longer adopts.

**The damaged-header flag** (`GET` and `PUT /api/datasets/{name}/headers`, data:work
to set) is a person's flag with a reason or else the signs. As built it reads one
sign: at least half of the dataset's MR scans carry neither a series description
nor a protocol name. Ruling 31 (c), 2026-10-16: the two other signs of record 56
(words the pack's vocabulary does not know, and a manufacturer not seen before)
are to be built and the flag set at ingest, with thresholds proposed from the
evidence for Nima's yes before they ship.

**The audit stream** (§11.5) is built: `POST /api/audit/draws` draws a round's
sample and makes the campaign that reads it blind, and
`POST /api/audit/draws/{id}/grade` works out the accepted error with its exact
bound from the settled answers. No round has been drawn: the raters' registered
check, which comes first, has not been passed.

**The desk.** Review's Rules page has four tabs. Proposals shows a proposal on its
approval card with its effect and the scans of any answer it breaks, adopted with
a press and its re-sort followed to its counts. Quality shows the pack's quality,
each row with the change that would mend it. Change a rule has one form for each
typed operation, generated from the patch schema, whose effect is shown before it
is proposed. Words keeps the site's own words for the rules' lists, as before. A
dataset's detail says how many rule fixes were proposed from its read, and the one
chat draws the same card. The assistant's `tune-sorting-words` skill writes
`add_words` and `remove_words` instead of raw overlay documents and proposes them;
no station can decide.

**What guards it.** Sealed samples never feed the loop. An overlay never leaves its
scope. Every rules version stays available. A change that turns any right answer
wrong says so on its card. Only blind answers train System 1.

**Not built, and moved to 7b by ruling 31 (b), 2026-10-16.** Two things. The first
is the automatic grouping of Review answers and audit errors into proposals
(record 56, section 5.3). What exists is that the overrides and the equal-rank
disagreements are kept on every classification and grouped by rule pair on the
Quality tab, each with a fix drafted, and that a person or the assistant drafts
the rest. The second is the full effect report for a free-form edit of a pack's
files (record 56, sections 5.3 and 5.5). The rules releases 1.0.2 to 1.2.3 were
free-form edits, each replayed over the archive (§9.3), which counts the stacks
that moved by axis, and `nils pack rehearse` takes typed operations or an overlay
only. Until the report exists for a free-form edit, every pack release is
replayed over the archive.

**Tests:** T20.

## 12. The image models in production (E1 to E4, G5, F2, F5)

### 12.1 Body part's fast path

The cascade in front of the certified body-part model (a one-slice student, a
24-frame reader for what it defers, and the certified path for the rest) passed its
class III test and reads about 8 files a stack; with the plane cache a re-pass is
minutes. It ships as the next release of the body-part pipeline, which needs each
file's geometry in the stack manifest (`stacks.schema.json`, a contract change)
and the open engine pull request that writes it. It becomes the chosen body-part
model on an install only after a pass on the development install's test data.
Every stack gets a body region or a reason code: `sealed`, `not-mr`, `no-geometry`,
`not-sure` (below the threshold) or `unreadable`.

### 12.2 Post-contrast, served where it is certified

The certified post-contrast model is served through its student, as a pipeline: it
answers only on T1-type brain (the header's T1 family and the body-part model's
brain), and says "not sure" everywhere else. What the student defers goes to the
full pipeline (segmentation, bias-field, vessel filter), pinned as an image built
from the research environment; the licences of its tools' weights decide whether
that image may ship beyond the site. *Amended in place on 2026-10-16 by record 55,
ruling 22:* it ships. The image is published to the project's container registry
where its licences allow, and a release ships the pipeline's descriptor pinned to
the published digest (§20, question 10). Each faster form of §16.3 replaces a part
only after its equivalence test.

### 12.3 The post-contrast label

The label NILS stores becomes:

| Field | Values |
|---|---|
| `contrast_state` | `pre`, `post`, `by_definition`, `undetermined` |
| `minutes_since_injection` | a number, or none |
| `contrast_source` | `t1_anchor_time_rule`, `header`, `image`, `reader` |
| `contrast_prior_72h` | true when the subject had contrast within the previous 72 hours |
| `asl_after_contrast` | true for an ASL acquired after contrast, which is invalid for perfusion |

- **The time rule** runs as a session pass of the rules: each scan of a session is
  `pre` or `post` by its time against the session's clock, set by the last
  pre-contrast and first post-contrast T1 whose answer is certified (the image model
  at high confidence, or a settled read). A session with no T1, or with only a post
  T1, is `undetermined`, never `pre`.
- **Its checks run as code,** each with a reason code: an earlier injection the same
  day or days before; two exams in one session; bolus and test-bolus runs; a split
  dose or preload; clock errors (the time against the series number); derived series
  (they take their source's time); sessions without a usable T1; a mislabelled post
  T1. A check that fires makes the scan `undetermined` with its reason.
- **The 72-hour flag** needs the subject's earlier sessions, which a session pass
  cannot see, so the engine gains a subject-level step after the session pass.
- **By definition:** DSC, DCE and contrast MR angiography are `by_definition`. PSIR,
  vessel-wall T1 and PD scans count as the T1 family for the image model.
- **Error levels by use** are documented with the label: at most 1 % where it decides
  inclusion in pooled quantitative analysis (FLAIR segmentation sets, ASL blood flow,
  quantitative maps); 2 % for ADC, QSM and TOF; 5 % where it is descriptive (T2, PD,
  DWI, SWI).

**Tests:** T24, T25, T27.

## 13. A desk that is one product (H2 to H5, K5, K6)

A walkthrough of the desk from its code (record 55, H1) found every task it does
and every one it lacks. Its findings drive this section; each round below is built
on the development install and accepted in use before it merges (§4.1).

### 13.1 The map

| Section | Pages |
|---|---|
| Home | what needs the person, each tile a link to where they act; what runs; setup until it is done |
| Data | Datasets and cohorts (one page); Browse (subjects, then sessions, then scans); Pseudonyms |
| Query | Cards; Selections (a list, new); Exports |
| Review | Work (one list, §13.2); Main scans; Subjects; Rules (§11.6); Rounds; Label sets |
| Pipelines | Catalog; Runs; Jobs with their logs; Models (§10.2); a form to add a pipeline |
| Release | Releases; hand over; withdraw; session schemes |
| Assistant | Conversations; Plans waiting (the inbox); Standing grants |
| Settings | Overview; Parts (with Engine, Desk, Language models and the Assistant's stations set in under it); Places; People; Backups; Audit; Setup (reachable from Home until setup is done) |

*Amended in place on 2026-10-16 by record 55 (the desk's design of 2026-10-09,
confirmed that day; ruling 20 of 2026-10-14):* the rows for Data, Query and Review
are the sections as built. They first read, for Data, Datasets with a "Do all"
button and Cohorts as two pages; for Query, Ask; and for Review, one list of work,
Picks and Label sets. Ruling 20 undid the rename of Query to Ask.

**How the shell changes.**
- Sections are registered in `web/src/home/placeholders.ts` (one grant and one
  engine door each) and `web/src/sections.ts`; side sub-pages carry no grant of
  their own; Review, Campaigns and Pipelines switch sub-pages with chips inside the
  page. Every section's sub-pages move into the side, so a section's predicate
  becomes any-of (Pipelines shows for `pipelines:see` or `models:see`; Review for
  `review:see` or `campaigns:see`) and each side page carries its own grant.
- **Models under Pipelines:** `#pipelines/models[/<id>]`; the model page's links
  change with it. No engine change.
- **Campaigns into Review:** `#review/campaigns/<id>/rate|adjudicate|settle|pairs|anchored|gallery`
  and `#review/label-sets/<n>`, within the route grammar of `web/src/routes.ts`;
  every link to `#campaigns` is rewritten. A person who holds only `campaigns:*` (a
  rater) sees their campaigns and no queue, rules or identifiers, and the page does
  not read the queue for them. The grants themselves do not change.
- **Words:** one word per thing. "Sort" is classification only; "Read" is the
  digest; "Pseudonymise" is the step and its button; "Query" names the query
  section, whose cards keep their name (*amended in place on 2026-10-16 by record
  55, ruling 20*: it first said "Ask"); "Language models" replaces "Kvasir" on
  the page (the part keeps its name on Parts); "People" and "Backups" replace
  "Identity" and "Database".
- **Nothing hidden while empty:** the Picks, Proposals and Asked chips show with a
  count of zero and a line that says what would fill them. Home's tiles are links.
- **The dark theme:** progress and selection colours that nearly vanish are mended
  in the theme files.

**The rounds, in order.**
1. **One safe way in for data** (§5): one dialog, Data's "Add a dataset", which
   says what a folder is before anything reads it. Setup's DICOM step
   and Places' "Add a place" with the source role lead to it. *Amended in place on
   2026-10-16 by record 55 (B2, the ruling of 2026-10-08):* the dialog first always
   asked how the files arrive. It is now a finder, never a list of every folder,
   since a root may hold thousands: pick the root (skipped when there is one), type
   part of a folder's name or browse, fifty at a time, and pick a folder. NILS
   looks inside it once and says whether it holds DICOM, whether it has the two
   trees, how many entries with DICOM lie loose beside them, and the state the
   dataset would get (§5.3). One Add button, and a folder with no DICOM asks "Add
   anyway?" first. The dialog asks only what the structure cannot say: for
   anonymised data, what PatientID holds and how its subjects are found; for
   identified data, the kind of identifying ID its PatientID holds (one that is
   the same everywhere, which the key codes with no map, or a hospital or study
   ID, whose subject codes a map gives) and what becomes of an ID no map names. A
   dataset whose files lie beside its two folders is finished from the dataset's
   one button: it asks whether they are identified or already anonymised, names
   the entries and the folder, and moves them only once the person confirms.
2. **A dataset's steps as a line:** Pseudonymise, Read, Sort, each with its count,
   its button and its last job; "Do all" runs the chain; the Pseudonymisation page
   has its own button.
3. **No command line inside a flow:** a picture is built when it is first asked for
   (a missing pyramid queues its build and the reader waits for it, instead of
   answering 404), and `pyramid build` gains `--force`; "Save as a selection" on a
   card, with the selections list door below; a pick run as part of the bring-in and
   a button for it; session schemes made in the release form.
4. **One pattern:** the words, the side, the empty states and the theme above.
5. **The missing controls, most used first:** hand over and withdraw a release (the
   doors exist); change one's password and an admin's reset; remove a person; the
   job and unit logs; every campaign kind made in the desk; standing grants and the
   plans inbox (§14); the assistant's drafts shown as cards only when kept; saved
   selections named with `@` in the chat. Restore stays a printed command (D50):
   Backups shows the procedure and the exact command to copy.

**New doors the map needs.**
- `GET /api/ask/selections` (a list; today only `PUT` and `GET` by name exist).
- `GET /api/subjects`, `GET /api/subjects/{id}/sessions` and
  `GET /api/sessions/{id}/stacks`, paged and filtered, under the person's detail
  level, for Browse.
- `GET /api/jobs/{id}/log` and `GET /api/runs/{id}/units/{unit}/log` (units already
  write `log.txt`).
- `GET /api/cohorts/{name}/picks`: each session's roles, picked, doubted or missing,
  for the heatmap.
- The desk server: `POST /desk/people/{name}/password` (self, or an admin's reset in
  local mode) and `DELETE /desk/people/{name}`.

### 13.2 Review: one list of work over five job types (H3)

**Where it stands.** Every campaign item stands on a review item: a selection or a
handle raises one `campaign.<kind>` review item per item, and a review source adopts
the items it names (`nils-registry/src/campaign.rs`). An open campaign holds its
items, and the review doors refuse to apply them. The engine knows eight question
kinds (axis, axes, pick, form, derivative, free, pair, anchored; A/B is a mark on the
source), but its create door draws only stack, session and review items, so pair,
anchored and A/B campaigns are made from the command line. Only axis and axes
questions close into decisions, and pick into picks.

**One list.** Review's first page is one list of work: the machine's open questions,
grouped by kind with their counts, and the campaigns, with their state and the
person's own share; `campaign.*` review items get a family of their own. The desk
builds the list from the existing doors (`GET /api/review/summary`,
`GET /api/campaigns`, `/mine`); an engine door that returns both follows only if the
two reads are slow. A queue is worked by one person, item by item; a campaign by
several readers, blind, with their agreement measured.

**The five job types,** each with its screen, all saving through the same mechanism:

| Type | What a person does | Screen | State in 7a |
|---|---|---|---|
| 1. Answer a question about a scan | an axis, several axes, a form | the rate reader | finished: every kind made in the desk |
| 2. Pick a session's main scan | choose the stack for a role | the session board | finished, with the cohort heatmap |
| 3. Check a result | accept, reject or flag a pipeline's output | the check reader: the result over the scan it was made on and the visit's main scans | new |
| 4. Draw or correct a mask | edit a segmentation | an annotation app | Wave 7b |
| 5. Settle a disagreement | two answers side by side | the adjudication workspace; the A/B reader | finished |

- **Nothing typed by hand:** the create door draws pair, anchored and A/B items, and
  the desk picks selections, handles and review kinds from lists.
- **Type 3, new** (the check-a-result build spec of 2026-10-11). A check is a
  campaign whose question is "is this result right?" (`kind: check`). Its item is one
  result, a mask a pipeline registered, read as one of the part sets its output
  declares (each lesion a connected component of a label, each structure a label of
  a label map), or at part grain one item per lesion. Every mask records its grid and
  the scan whose grid it is on, tied `same`, `world` or `none`; a result that cannot
  be drawn is left out of a check and counted, never drawn approximately. A check
  never holds a sealed scan's result (the rulings of 2026-10-12, item 6): a result
  made from a stack of a sample sealed now is left out first and counted, never
  named, and a check made before that holds one withholds its item from every
  rater without the audited grant to read sealed stacks, and its close writes
  nothing of it.
  - **Panels:** the scan the result was made on, then the visit's main scans by the
    roles the check declares (T1w and FLAIR by default, at most 8), frozen when the
    check is made. The viewer draws the result as a mask layer on each scan's own
    picture pyramid (fill, outline, opacity, hidden at a key), and a scan is never
    resampled. On a scan of a sample sealed now a panel says its series description,
    never its role.
  - **The answer:** a verdict, accepted, rejected or flagged, with the reasons the
    check declares (one to seven, the keys 3 to 9), on the whole result, on a lesion,
    or as a point for a missed lesion; the lesions the rater had in focus are kept.
    Two raters are compared on the verdict, an agreed flag is the union of their
    flags, and a different verdict goes to the adjudicator.
  - **The close** (`closes_into: check`) writes, through the one write path, a
    decision on the result (scope `derivative`) and on each lesion it judged (scope
    `derivative_part`), each a person's, and the status (accepted, rejected or
    flagged, with the decision's id) on the derivative and its parts. A person may
    also check one result directly, and withdraw it; a later check withdraws the
    earlier decision, and nothing is deleted.
  - **What reads it:** a run's derivative input takes no rejected result unless its
    descriptor asks for any (`checked`), and the ask's measures carry the status of
    the run's results of each unit, so "where the segmentation was accepted" is one
    filter.
  - **Making one** is a button on the run page: a run's results (all, not yet
    checked, or the units its declared checks flagged), named results, or a
    selection's newest, with a preview that writes nothing. A flagged result waits
    for correction, which is type 4 (Wave 7b).
- **The two proofs** of Wave 6's gate run in the desk on the production install: a
  body-part curation round and a per-axis review, with zero database-level
  integration and no table named for a kind of QC.

### 13.3 Segmentation checks, learnt from a working prototype (H4)

A segmentation QC tool in daily use outside NILS was read as the model for type 3:
what is looked at, the keys, what is recorded, the pace. Type 3 is designed from
that reading (the check-a-result build spec), with nothing asked of its author, and
keeps what worked: one press per answer, the lesion in focus zoomed with its anchor
inside it, the mask at an opacity the rater sets, a short note. It adds what the tool
lacked: a check made from a run in two presses, any campaign's raters, agreement and
adjudication, the lesions seen recorded, a missed lesion as a point in the same
check, and a status on the result that later runs and the ask read. The first real
set is checked by its first user (T33), so the checking moves into NILS. Manual
segmentation (type 4) waits for Wave 7b.

### 13.4 The reader (H5)

- **The glitches first.** Blank thumbnails, black or gray screens and a horizontal
  line were reported in three body-part rounds; some had causes fixed since (dead
  registry connections after a database restart, scout overlays, zero-spacing
  repeats, YBR colour, single-plane stacks), and the gray screens have none
  recorded. They are reproduced on the development install at the current version,
  with the affected pyramids rebuilt (`pyramid build --force`), and fixed before the
  next reading sitting.
- **Why the rules decided, beside the image.** The rate reader already reads the
  rule, the clause, the flags, the header values and the matched words, but shows
  them in a drawer, closed by default. They move into the side panel beside the
  image, open. Queue items read `GET /api/stacks/{id}/why`; a rater keeps the
  campaign item's own door, which needs only `campaigns:see`. A blind read keeps the
  header text and hides the rules' answer (record 48).
- **Fixed size and two views.** A picture box of a fixed size per screen class, so a
  big screen shows the same layout every time; a compact and an expanded view for
  the whole reader, not only for the rows of an axes question.

**Tests:** T31, T32, T33, T34.

## 14. The assistant and the model gateway (B6, K1)

- **The stations measured.** Seven stations (concierge, ask-help, keyword-tune,
  analysis-plan, run-read, identity-check, operator) run on the site's local model.
  Each is measured on its own test questions on the development install with the
  local model: the bench (`bench/`, a scrubbed corpus with recorded golds) gains a
  script for its evaluations, and fixtures for the two stations never measured live
  (analysis-plan's cohorts and selections; run-read's planted runs). The desk's
  picker lists every station a person may use. *Amended in place on 2026-10-16 by
  record 55 (B6, the ruling of 2026-10-09 "one chat"; T10 as changed on
  2026-10-10; ruling 6 of 2026-10-12):* the stations became skills of one chat,
  one agent and one voice, with about ten namespaced tools mounted from the
  person's grants for the whole session, writes only proposed and executed from an
  approval card by application code, and sub-agents only for read-only fan-out.
  The bench holds conversations graded by pass^3, with routing, task-switching,
  approval-bypass and injection suites, and the chat is measured and improved on
  the model server's default model, qwen3.8-flash-next.
- **One model, no surprise swaps.** The model gateway cold-swaps a card's models and
  returns a card to its default after ten idle minutes, so a site whose default is
  not the assistant's model pays a swap of one to two minutes on the first request
  after a quiet spell. The fault behind the old "sleep and wake" (a model server's
  memory saver crashing on hybrid models) is fixed by a newer server image with a
  release-and-resume driver; until then the assistant's model is the card's default.
- **Thinking as text.** The reasoning splitter knows `<think>` but not `<thinking>`,
  runs only on the gateway's own path and not on its pass-through doors, and is set
  per backend rather than per model. It learns `<thinking>`, is set per model, and its
  three identical copies (the gateway, the assistant, the desk) become one shared
  module. A reproduction test sends a request after a swap on each door.
- **Standing grants and the plans inbox** get their desk screens (§13.1, round 5) over
  the assistant's existing routes (`/grants`, `/plans`, `/inbox`).
- **The five open engine issues** on the draft and query doors (#94, #98, #99, #100,
  #101) were fixed with tests in alpha.38 and never closed: each fix is verified
  against its issue, and the issue closed. #94's remaining part (the text an axis
  matched, for axes that resolved) is built for §11.3.
- **K9's merge proposal:** the identity station proposes a merge of two subjects whose
  birth date, sex and visits agree; it proposes, and a person merges.

**Tests:** T10, T11.

## 15. The installer, and a site's refresh (B4, B1)

### 15.1 Keeping data on uninstall

`nils uninstall --keep-data` deletes the setup record (`setup.rs:18612-18618`), which
holds, among the rest, which database schemas this install made and the sign-in, place
and model-server facts; it also deletes the model gateway's whole directory (its models,
keys and sealed credentials). The next setup reads only the record, so a later purge
leaves the adopted schemas behind and every choice must be given again. With the fix,
keep-data keeps everything a later setup needs: the registry, the record's facts under
the kept directory (read by the next setup), the desk's people, and the gateway's
models and sealed credentials; the plan it prints says so.

### 15.2 The install test

The install test sets NILS up on a fresh virtual machine in fourteen shapes (a system
install with Postgres and a supervisor; user installs with SQLite and Postgres; docker;
rootless podman; a reinstall over a running install; an update from an old release;
purge then install; no linger; a refused drop; the gateway and assistant under podman;
podman with Postgres; keep-data then install; a model server). It runs at the start of
the wave and on the beta candidate, all fourteen passing, and gains a fifteenth: sign-in
through an OIDC provider.

### 15.3 A site's refresh

No path today makes an empty registry while keeping the rest: setup always makes a fresh
registry key, and datasets, ID types and maps live in the registry, not in the setup
record. A refresh is therefore new work:

1. **Save the declarations:** `nils place export` writes every dataset's declaration
   (its folder, arrival, PatientID choice, ID type and cohort) to a file.
2. **Archive:** the old registry's schemas are dumped and kept, and stay readable as a
   separate read-only database.
3. **Refresh:** `nils setup --new-registry --reg-key-file FILE` makes an empty registry
   under the site's key with the subject code generator's scheme, and keeps the setup
   record, the desk's people and groups, the gateway's sealed credentials, and the
   places.
4. **Re-declare:** `nils place import` reads the saved declarations.
5. **Check:** every part healthy, sign-in working, the first datasets brought in.

The deployment's specifics (which install, which places, which people) are the record's.

**Tests:** T8, T1 to T3 in the record.

## 16. The research that runs beside the code

Four of the wave's results are research before they are code: System 1's model
and its robustness certificate, the post-contrast model's speed and its third
round, body part's fine mode and its new classes, and two explorations. Each
runs in the group's research environment on its own copy of the archive, under
a plan registered before its data is drawn (§4.3), and reaches the product only
through §16.6.

### 16.1 Keys that survive a new registry

A site's refresh (§15.3) gives every stack a new id. Everything a study must
keep across it is therefore keyed by DICOM identity, never by registry ids: the
training guards and seals (which sessions and subjects a model may never see),
label sets (§10.3), the slice cache (§10.4), the audit's frame (§11.5) and every
sealed sample. A study reads the archived registry it started from; the
production registry is only read for the checks it is named in.

### 16.2 System 1 (D1, D2, D5)

**What it learns from.** The whole archive's headers (about half a million MR
stacks) and damaged copies of them. The labels are the rules' answers, which
Wave 6 found a better teacher than a label model's posteriors (the label model's
weighting of the rules' votes is tried as a variant), and the settled answer
keys of the earlier reads with their corrections. A review answer becomes a
label only when it was given blind (§11.4). The guard keeps every sealed
sample's sessions and subjects out of every training and pretraining step,
label-free steps included.

**The damage.** Seven families: abbreviations learned from the archive, typos,
console languages, vendor-name swaps, anonymisation dropout as in DICOM PS3.15,
vendor omissions, and number errors; plus natural pairs from the archive (the
same physics named differently across sites), weighted highest. A copy's target
is its original's; each copy is classified by the engine in a scratch registry
with the archive as context, so the rules' physics vote sees what it sees in
production. Before any training the families are split into those used for
training and those held out for the certificate (a language family, a real
anonymiser's output, whole vendors and extreme field strengths, one cohort's
private naming, compound damage). A probe checks that the model has not learned
the generator rather than the headers.

**The models, in order.**
1. **The cheap model:** gradient boosting on character n-grams of the text
   fields, the physics fields, the coded fields and the session's siblings, one
   head per question over its legal values, with beta calibration; trained on
   the training families' damage. It is the first served if it passes.
2. **The encoder,** which has to beat the cheap model on development data before
   it is graded: a byte-level model of 20 to 40 million parameters (6 to 8
   layers, width 384 to 512, at most 64 fields) that reads a header as a set of
   (field, value) pairs, numbers on a log scale with a missing token.
   Pretraining is JEPA-style: from a damaged header, predict a frozen language
   model's embedding of the intact header, with a small moving-average term; the
   embedding's effective rank is watched, since a learned metric collapsed once
   in Wave 6. Heads per question, legal combinations only, a consistency loss
   across the damaged copies of one header, and calibration after quantisation,
   on the file that ships (Wave 6 saw an exported model move about 5 % of its
   answers when quantised).
3. **The ablation:** the same encoder without the JEPA step. If it is as good,
   the frozen target has not earned its place.

**The gate's thresholds** (§11.2) are chosen by Learn then Test on the training
families' damage, never on a sealed sample: a fixed sequence of thresholds from
strict to loose, each tested with an exact binomial bound, stopping at the first
failure. They are frozen with the model.

**The robustness certificate** is graded once, on a sealed sample whose sessions
and subjects no 7a model has seen, damaged by the held-out families. The claim:
at most 1 % wrong among the scans the gate accepts, at 95 % confidence, by the
exact Clopper-Pearson bound (a 1 % claim needs at least 299 accepted scans with
no error, about 474 with one). Reported beside it: the share accepted, the rules
alone under the same damage, and a three by three table (clean, training
damage, held-out damage; against seen sites, held-out sites, held-out vendors),
with public headers as external data for the vendors and field strengths the
archive lacks. The honest claim names the damage: "on this held-out damage".

### 16.3 The image models' speed (E1, E4)

Every faster form of a certified model is used only after an equivalence test
against it, registered before the draw, in the classes Wave 6 defined: class I,
the same answer always; class II, the same answer within a stated tolerance on
the model's score; class III, every change is between an answer and "not sure".
The post-contrast student is tried with few slices (a slice choice and the
stack's geometry as inputs, the teacher's arm logits as its target) and a
deferral rule whose upper bound is at most 0.5 %. Each candidate gets its own
test; a draw that has served many candidates is replaced by a fresh one. Speed
is measured on CPU alone and with a GPU (§4.2).

### 16.4 Post-contrast (F2, F3, F4)

- **The time rule's label** (§12.3) is checked by code against the eight failure
  modes, and its anchors are T1 scans with a certified answer. No person labels
  a non-T1 scan; the disagreements between the rule and earlier settled reads are
  examined first.
- **The image model's third round:** the earlier round's proposals frozen before
  training ("not sure" for the scan types and field strengths it failed on, a
  wider band between the header's contrast tag and the image, an adjudicated
  reference before an item counts as an error), then a fresh test read sized by
  a power calculation per make and field strength before the draw, from subjects
  outside every earlier read, read in one sitting with an account per reader.
  Claims are made only for the groups with enough scans.
- **Beyond T1-type brain:** a feasibility verdict per family. Labels come from
  open labelled data under a licence check, or from sessions whose T1 is certain
  and a later scan surely post; no manual labelling of non-T1 scans. FLAIR (with
  a paired pre-contrast FLAIR) and ASL are the families where image evidence is
  worth pursuing.

### 16.5 Body part (G1, G3, G4, G6)

- **The fine mode:** where the model misses, and why, from development data
  only; then a crisp boundary measured in the image (which part of the spine is
  visible, from an anatomical landmark), slices chosen by plane and field of
  view with the field of view as an input, and the stronger student design. A
  new boundary changes the label space, so the fine mode is certified again on a
  fresh blind read.
- **Neck as its own class,** from a targeted search and a quick read, with the
  question of whether the coarse mode keeps neck within spine settled in the
  plan.
- **CT,** explored: which CT scans the archive holds once they are
  pseudonymised (§5), public labelled CT sets for tests, and whether the same
  approach carries over.
- **JEPA pretraining for the image encoders:** one small encoder pretrained on
  the archive's own unlabelled slices with MR damage on the input side, then
  body part's head and post-contrast's student fine-tuned on top; a registered
  experiment against today's models on unseen tests, under a CPU size cap.

### 16.6 How a study's result enters the product

A study hands over: the frozen model files with their sha256; a card
with the tested numbers, each citing its evidence run (§10.2); the registered label set
it trained on (§10.3); the equivalence test of any faster form; and its speed on
CPU and on GPU. The model is then registered, checked by the admission suite,
and chosen for its task by the site (§10.2). A model trained on a site's own
data is a site model: it lives in that site's model store and never leaves it;
a model trained only on public data may ship in a public release.

## 17. The gate of the wave and its closing bars

The wave's tests are the record's T1 to T36, written before the work started
and graded once in the closing report; a missed test is reported as missed,
never redefined. Those the code must meet are restated here with the record's
ids. "The development install" is an install that follows the development
channel (§4.1) on a test registry; "the production install" is the site's
install after its refresh (§15.3). Each repository runs its own gate in CI as
before, and the tests below are added to it where they can run there.

A change to a test is written beside the original, with its date and the ruling
(record 55, A3), and the original stands. *Amended in place on 2026-10-16 by
record 55:* the tests below that carry a change were changed by rulings after this
section was written.

**Data in, data out**
- **T4.** A folder without `derivatives/dcm-original` or `derivatives/dcm-anon`
  is never read as de-identified: 0 files digested before the person's answer,
  through every path that adds a source (§5.3). A CI test per path.
- **T5.** PatientID holds what the dataset chose, in each scenario of §5.4: 100 %
  of files in each scenario's test dataset.
- **T6.** Subject codes equal the subject code generator's on known cases: 100 %.
- **T7.** 100 % of pseudonymised files carry the de-identification marks of §6.1.
- **T13.** The quasi-identifier rule holds at every door that returns rows (§7.1):
  a test per door; sequence names stay visible.
- **T12.** A BIDS release passes the official validator with 0 errors in both
  naming modes, in CI on every change to the release code (§8.2). *Changed on
  2026-10-08 (record 55, C4, "the three ways to name a stack"):* the two modes are
  the two BIDS naming styles, full and minimal (§8.1).
- **T18.** No release is refused for a name conflict (§8.1): 0 refusals on the
  development install's test data and on the production install's first release.
- **T17.** Display composites are classified and left out of analysis releases by
  default (§8.3).

**Parts that update on their own**
- **T9.** A rules release reaches an install without an engine release, and a
  rules version outside the engine's range is refused (§9).
- **T16.** Every production rules fix of the wave is replayed over the archive
  and its moved scans listed and checked before it is released (§9.3).
- **T22.** Every served model has a complete card that names its registered label
  set; the site picks one model per task; earlier versions stay runnable (§10).
- **T23.** Every served model's speed on CPU and on GPU is on its card, and its
  answers are the same on both within the card's tolerance (§4.2, §10.5).
- **T26.** A dataset brought in gets its image answers without a manual run, and
  a re-score after a model update reads the slice cache (§10.4).

**System 1 and the image models**
- **T19.** System 1's robustness certificate (§16.2): at most 1 % wrong among
  accepted scans under held-out damage, by the exact bound, graded once.
- **T20.** The review rule (§11): every scan carries System 1's answer and
  confidence; the accepted and reviewed shares are reported; review shows both
  suggestions and both reasons, blind first where a read trains or measures.
  *Changed on 2026-10-10 (accepted that day):* on production, the scans sent to
  Review are reported by kind, and none is sent for a pack conflict, a rule's low
  confidence, body part or post-contrast (record 56); System 1's opinion is shown
  beside each question once D2 confirms it, or reported as not yet; Review pages
  stay blind-first for reads that train or measure.
- **T21.** The blind audit's first round (§11.5): its raters pass their check
  first; the accepted error and its bound are reported; 0 audit reads train.
- **T24.** Body part's fast path: a cold pass of the archive in about an hour on
  the production install, its equivalence class held; every scan has a body
  region or a stated reason (§12.1). *Changed on 2026-10-13 (ruling 11):* a cold
  pass of the archive in at most about 1.5 h on a card, the equivalence class held,
  every scan with a body region or a stated reason; the bar does not apply to a CPU
  alone. *Changed again on 2026-10-15 (ruling 29):* the production half is graded
  on one dataset; the pass at archive scale stays the one graded in the research
  container, and production proves the run, the stated reason of every scan with
  no region, and that its answers are stored.
- **T25.** Post-contrast answers only on T1-type brain and "not sure" elsewhere; a
  pass's time is measured against the hour; each faster form passed its
  equivalence test (§12.2, §16.3). *Changed on 2026-10-13 (ruling 11):*
  post-contrast's pass in under 4 h on a card, improved after 7a; the bar does not
  apply to a CPU alone.
- **T27.** Every scan in a session with a T1 anchor carries the post-contrast
  state, the minutes since injection, the label's source and the 72-hour flag,
  with the failure-mode checks run (§12.3). *Changed on 2026-10-16 (ruling 30,
  beside ruling 29):* its production check is graded on a calibrated cohort that
  receives contrast, once that cohort is on production, together with the audit of
  the post-contrast time rule (ruling 25); it stays owed until then.
- **T19, T28, T29, T30** grade the research of §16 once each.

**The desk and the people's work**
- **T31.** Every task has one place in the desk's map; no command-line step is
  left inside a desk flow; every section is reachable from the side; each round
  was accepted in use on the development install (§13.1).
- **T32.** Review and Campaigns are one list of work; job types 1, 2, 3 and 5
  work; Browse and the heatmap exist; a body-part curation round and a per-axis
  review run in the desk with zero database-level integration (§13.2).
- **T33.** Job type 3 is used on a real segmentation QC set (§13.3). *Changed on
  2026-10-11 (ruling 7 of that day):* type 3 works end to end on test data. A run
  writes masks, a check is made from the run, items are answered and adjudicated,
  the campaign closes, each result's status is written as a person's decision, and
  a rejected mask is left out of the run's outputs.
- **T34.** The reader shows no blank or black screen on a test round of 200 items,
  shows the rules' evidence beside the image, and has its fixed-size, compact and
  expanded views (§13.4).

**The parts around the engine**
- **T10.** Every assistant station meets its bar on its own test questions with
  the local model (§14). *Changed on 2026-10-10 (accepted that day), and graded on
  the model of ruling 6 of 2026-10-12:* the one chat on its bench with the local
  model. Each skill's questions (the stations' test questions carried over) are at
  or above their bar of record 19 or their last measurement, none worse, graded by
  pass^3, and the routing, task-switching, approval-bypass and injection suites
  pass.
- **T11.** The model gateway's sleep-and-wake fault and the thinking-as-text fault
  each have a reproduction test that passes, and the five open engine issues are
  closed by merged fixes (§14).
- **T8.** The install test passes in all fourteen shapes at the wave's start and
  on the beta candidate (§15.2).
- **T1, T2, T3.** The production install stands after its refresh, the archived
  registry is readable for research, and every evidence run's inputs are
  archived (§15.3; the deployment's specifics are the record's).
- **T15.** This spec and the records the paper cites are public before the paper
  is submitted.

The record's T14 (the previous prototype's past releases), T35 (the paper) and
T36 (the end-of-wave checklist) are graded in the record.

## 18. Defaults settled in this spec

- A dataset is what its folder holds, and a folder with neither tree is unknown
  and never read (§5.3). Loose entries move only on confirmation; `dcm-raw` is
  renamed `dcm-anon` as ruled, and shown. *Amended in place on 2026-10-16 by
  record 55, B2 (the ruling of 2026-10-08):* this first said that a place's arrival
  defaults to `undeclared` and that an undeclared place is never digested.
- A dataset declares `patient_id: subject-code` or `patient_id: id-type:<name>`
  (§5.4); an unknown subject's files are held.
- `nils init` and `nils setup` make registries of the subject code generator's scheme
  (§5.2).
- The marks' option list is derived from each writer's policy (§6.1).
- The series description and protocol name are technical fields (§7.1).
- A release names its stacks in one of three styles, descriptive, full BIDS and
  minimal BIDS; a difference and the fallback number go into `acq-` in the two BIDS
  styles and into a slot of their own in the descriptive one; a fallback is never
  called a run (§8.1). *Amended in place on 2026-10-16 by record 55, C4 (the
  rulings of 2026-10-08):* this first said that the BIDS mode spells a difference
  and the fallback number into `acq-` and that the informative mode carries
  `diff-<Property><Value>` and a plain `_<n>`.
- A pack's own tag is `pack-<name>-v<version>`, and its manifest names an engine range
  (§9.2).
- A task's chosen model defaults to the model a recorded certificate names (§10.2).
- A label set's rows, a sealed sample's members, the slice cache and the audit's frame
  are keyed by DICOM identity (§10.3, §10.4, §11.5, §16.1).
- The slice cache's key is the SOP Instance UID, the file's sha256 and the model
  input's version; it lives in the site's working place (§10.4).
- One review question in five is blind first, as a site setting; only blind answers
  train (§11.4). *Noted on 2026-10-16:* the share was never ruled (§20, question 4).
- A round is a pair of a rules version and a System 1 version; its audit reads 1 % of
  its accepted stacks (§11.2, §11.5).
- A stack with no body region carries one of `sealed`, `not-mr`, `no-geometry`,
  `not-sure`, `unreadable` (§12.1).
- Restore stays a printed command (D50); Backups shows it (§13.1).
- Settings' landing page is Overview; Engine, Desk, Language models and the Assistant's
  stations sit under Parts; Setup is reached from Home until setup is done (§13.1).

## 19. Order of work

A slice's letter names its repository or place: **A** the engine, **B** the desk, **C**
the model gateway, **D** the assistant, **P** a pipeline package (in `pipelines/`), **R**
a rules pack (in `packs/`), **S** a study or a test campaign, **W** the site's own
deployment (whose specifics are the record's). Each slice is built from its branch onto
the development install, tried in use, then merged as one pull request, or several
stacked where slices share code (§4.1). The steps follow the record's road.

**Step 1, the ground**
- **A1** keep-data keeps what a later setup needs (§15.1).
- **S1** the install test at the wave's start, with the OIDC shape added (§15.2).
- **A2** the subject code generator: #369 rebased and renamed (§5.2).
- **A3** never read without the layout: the layout read from the folder's
  structure, confirmed moves, the migration (§5.3; *amended in place on
  2026-10-16 by record 55, B2*, where it said `undeclared` and detection on every
  path).
- **A4** what PatientID holds; "code them anyway" queues its job (§5.4).
- **B1** one safe way in for data (§13.1, round 1).
- **A5** the five open issues verified against their fixes and closed (§14).
- **S2** the research plans registered: System 1's damage split and its cheap model,
  body part's failure analysis, the time rule's checks, the post-contrast speed round
  (§16).
- **D1** the stations measured: the bench's evaluation script and the missing fixtures
  (§14).

**Step 2, what comes in and what goes out**
- **A6** the de-identification marks and the exact tag list (§6).
- **A7** the catalog reclassification, then the quasi-identifier rule at every door (§7.1).
- **A8** what a release records about itself; **D2** the identity station's merge
  proposal (§7.2).
- **A9** names never refused (§8.1); **A10** the official validator in CI (§8.2).
- **B2** a dataset's steps as a line (§13.1, round 2).
- **A11** pictures built on demand and `pyramid build --force`; **A12** the selections
  list door; **A13** a pick run in the bring-in; **B3** "Save as a selection", session
  schemes in the release form (§13.1, round 3).

**Step 3, parts that update on their own**
- **A14** rules releases: their channel, the engine range, the update path; **B4** packs
  on Parts (§9); **R1** display composites, as the first rules release (§8.3).
- **A15** the card's new fields and a chosen model per task; **B5** Models under
  Pipelines with the choice (§10.2).
- **A16** label sets from outside (§10.3).
- **A17** scoring at arrival and the slice cache (§10.4); **A18** the CPU and GPU
  equivalence in the admission suite (§10.5).
- **P1** body part's fast path, with the stack manifest's geometry (§12.1).

**Step 4, the people's work**
- **A19** the create door draws pair, anchored and A/B items; **B6** Review's one list,
  campaign families, pickers instead of typed handles (§13.2).
- **A20** a derivative's QC status and type-3 items; **B7** the overlay and the type-3
  screen (§13.2, §13.3).
- **A21** the browse doors; **B8** Browse. **A22** the picks door; **B9** the heatmap.
- **B10** the reader: the glitches first, then the evidence, the fixed size and the views
  (§13.4).
- **B11** one pattern: words, side, empty states, theme (§13.1, round 4).
- **A23** the log doors; **B12** the people doors; **B13** the missing controls (§13.1,
  round 5).

**Step 5, the research rounds** (from step 1, beside steps 2 to 4)
- **S3** System 1: the cheap model, the encoder and its ablation, the frozen choice, the
  robustness certificate graded once (§16.2).
- **S4** body part: the new boundary, neck, the pretrained encoder, a fresh read graded
  once (§16.5).
- **S5** post-contrast: the speed round, the third round of the image model with its
  fresh read, the feasibility verdict (§16.3, §16.4).
- **S6** CT, explored (§16.5).

**Step 6, System 1 and the label in the engine**
- **P2** System 1 as a pipeline (§11.1); **A24** the gate, review questions with both
  suggestions and both reasons, blind-first modes and answer flags (§11.2 to §11.4);
  **B14** the review screens for them; **A25** the audit draw and the audit kind (§11.5).
- **R2** the time rule as a session pass with its checks; **A26** the subject-level step
  and the post-contrast label's fields (§12.3).
- **P3** post-contrast served through its student, with the pinned fallback image (§12.2).
- **C1** the reasoning splitter shared, `<thinking>` known, set per model; **C2** the
  release-and-resume driver when the server image allows it (§14).
- **A27** `nils place export` and `import`, `nils setup --new-registry` (§15.3).

**Step 7, the beta candidate**
- A release of every part; **S1** the install test again; **D1** the stations measured
  again.

**Step 8, the site** (**W**)
- The site's refresh (§15.3), its first datasets, its production checks, and the closing
  report that grades every test once.

## 20. Open questions carried into the wave

Each is settled with the record's owner when its slice is built, and written into the
record with its date.

**Amended in place on 2026-10-16 by the audit of Wave 7a.** Ten of the eleven were
settled, each in the record with its date, and the answer to each is below with
its ruling. The eleventh, question 4, was never ruled. The rulings are in
`docs/decisions/55-ready-for-production-and-the-paper.md`: those of 2026-10-12 and
after are numbered, the earlier ones are dated.

1. Record 54's open rulings D1 to D10, which are §5.2's defaults until then.

   **Answered, one by one.** D1 (`blake2b-8` as the default) and D2 (a personnummer
   is an ID type, an invalid one is refused) were built as §5.2's defaults, and D10
   (review and merge) was done: the content of pull request #369 reached main with
   part one. D4 (a subject coded from a personnummer is not provisional), D7
   (`dcm-anon` is written beside `dcm-original`, in the study's own folder) and D8
   (the key file is a root-only file on the production install) were ruled on
   2026-10-08 (record 55, B2). D5 (the subjects of studies with no personnummer in
   their DICOM) was settled the same day by the ruling that anonymised or coded
   data must have its subjects resolved, by a map or by a code made from its ID
   (§5.3). D3 (the route per study), D6 (re-keying the production registry) and D9
   (the keyed half of the check) were overtaken: B1 starts production from an empty
   registry under the group's key (2026-10-08), and ruling 21 of 2026-10-14 has Nima
   bring datasets in one by one and test the key first on numbers he makes up.
2. How each naming mode spells a difference and the fallback number (§8.1).

   **Answered** by the rulings of 2026-10-08 (record 55, C4): three styles,
   descriptive, full and minimal; a difference or a fallback number goes into
   `acq-` in the two BIDS styles, `run-` is kept for true repeats, and a plain
   number is last.
3. Whether `dcm-anon`'s folder names carry the series description (§7.2).

   **Answered** by ruling 18 of 2026-10-14 (K9): yes, as
   `<SeriesNumber>-<description reduced to letters, digits and hyphens, about 40
   characters>`, the number alone when the description is empty; new writes only,
   existing trees never renamed, and a resumed run keeps its tree's scheme.
4. The share of blind-first review questions, and that only blind answers train (§11.4).

   **Not ruled.** The default of this specification is built and in force: one
   question in five is read blind first, a site setting (`review.blind_share`), and
   only blind answers train. Record 55 proposed it on 2026-10-08 under D3 "to
   confirm with Nima"; D6 was ruled that day, that a read whose answers train or
   measure is answered blind first; and record 56 (accepted 2026-10-09) lists "only
   blind answers train System 1" among its guards. No ruling confirms or changes
   the share.
5. The robustness certificate's sample: the earlier certificate's sample with the
   training guard, and whether an untouched sealed sample joins it (§16.2).

   **Answered** by D2 (2026-10-08): the earlier certificate's sample, kept out of
   every training step by the guard, graded once. Ruling 22 of 2026-10-14 records
   that D2 was graded on certificate 2's samples, unsealed by ruling 19, and on a
   second, untouched sample of a research guard's unread stacks, which was never a
   registry seal.
6. Where the minutes since injection are counted from: the first post-contrast T1, or
   the bolus start (§12.3).

   **Answered** by ruling 5 of 2026-10-11 (the time-label plan's T1): from the bolus
   start where a bolus series exists, else from the first post-contrast T1's first
   instance, with which one recorded.
7. Whether the coarse body-part mode keeps neck within spine (§16.5).

   **Answered** by ruling 8 of 2026-10-12 (B6): yes, neck stays within spine in both
   modes until 100 trainable neck stacks from at least 50 subjects exist.
8. What keep-data keeps of the model gateway's state (§15.1).

   **Answered** by the ruling of 2026-10-08 (B4): all of it, the models, the store,
   the sealed credentials and the assistant's key.
9. Where the archived registry stays readable after a refresh (§15.3).

   **Answered** by B1 (2026-10-08) and ruling 21 of 2026-10-14: in a separate
   read-only database beside the new registry. The old registry stays as the "test"
   registry and is archived whole.
10. The licences of the post-contrast fallback's tool weights (§12.2).

    **Answered** by rulings 20 and 22 of 2026-10-14. Ruling 20 had the image
    site-built and never re-hosted; ruling 22 replaced it: every model's image is
    published by the release, and the fallback's, which carries the parts of
    FreeSurfer 8.2.0 that the Synth tools run on, is published openly where their
    licences allow. It is published to the project's container registry with the
    notices and licences of its parts inside it (`NOTICE.full`), and no licence key.
11. Whether the assistant's model becomes the card's default before the
    release-and-resume driver exists (§14).

    **Answered** by ruling 6, item 3, of 2026-10-12: the one chat is measured and
    improved on qwen3.8-flash-next, the stations' model and the model server's
    default ("lets focus on improving flash-next"). The release-and-resume driver
    exists (Kvasir #40) and falls back to a cold swap; the warm swap waits for a
    model-server image with the upstream fix.
