<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Wave 7b: packs, the software place, workflows, and the paper's evidence

The specification of the wave after Wave 7a. It follows
`wave7a-ready-for-production.md` and cites its two records by item id:
`docs/decisions/57-pipeline-packs-software-and-workflows.md` (the decisions P1 to
P11, accepted on 2026-10-12, and the tests G1 to G15) and
`docs/decisions/58-the-papers-evidence-in-production.md` (the decisions P1 to P7,
accepted on 2026-10-18, and the tests E1 to E6). Both records number their
decisions P1 on, so where the two could be confused this document writes 57-P1
and 58-P1. The records hold the decisions, the people and the deployment; this
document holds what the code does. The two records and the wave's agenda were
settled with Nima on 2026-10-18.

Wave 7a ended with the first beta on a site's production install: data enters
through a declared layout, the rules decide every answer with System 1 beside
them, every model is a replaceable part, and the desk holds every task. Two
things it left as they were. A pipeline runs with its descriptor's defaults
whatever the data: a lesion pipeline over sessions that have no FLAIR reads
"done" and makes nothing, no option is chosen from the scan's facts, nothing
records who chose a value, no result is reused, and the engine installs no tool
but the converter. And the evidence of what the labels are worth after
deployment has no home in the product: the blind audit of Wave 7a never ran, a
person who checks a reference reads every axis from scratch, and no result says
which curation state made it.

The measure of the wave, in five sentences. Every pipeline carries a pack that
says what a session needs, which option each rule sets from the registry's
facts and the person's goal, and which check flags a result, and every unit
records who chose each value. Tools live in a software place the engine installs
into or registers from, laid out so that people keep running the same copy by
hand, with licences as recorded acceptances. Steps chain into workflows on one
page, every link checked before anything runs, and every result keeps two keys,
so that a step already made is never run twice and results that are not
comparable are never pooled silently. A person checks a reference by accepting
or correcting an answer already filled in, guarded by planted errors, and a
monthly audit read by local raters says what the accepted labels are worth.
Every result says which rules, which model and which human overrides made its
inputs, and a cohort recipe says how much of the archive it can use and how far
label error can move it.

## 1. What Wave 7b delivers

- **A pack per pipeline** (§5): a typed sibling of the descriptor with the
  session's needs, the knobs and who may set them, the rules that set them from
  the registry's facts, the checks, the licence and the resources; evaluated at
  pre-flight, recorded per unit with who chose each value; two keys on every
  result; status per unit in the pack's own words; cohort checks.
- **The software place** (§6): install by digest or register an existing copy,
  verified by probe, fingerprint and test run; licences as recorded
  acceptances; dependencies checked as the pre-flight checks them; Settings >
  Software and `nils software`.
- **Registration as a family** (§7): one entry for the person, three methods
  behind it, chosen per session by the pack's rules; transforms as derivatives;
  SAMSEG and segcsvd read an aligned FLAIR instead of trusting the header.
- **Workflows on one Pipelines page** (§8): steps chained, each link checked
  before anything runs, runs going per unit through the chain, every result kept
  with its keys and reused when a key matches, and a provenance door that
  answers which steps and which curation state made a result.
- **The assistant plans with the pack** (§9) and **every pack change shows its
  effect first** (§10); **the first packs** (§11).
- **Verify mode in campaigns** (§12): the proposal filled in, one key to accept,
  planted wrong values and a catch rate, answers that never train.
- **The audit stream with local raters** (§13): 150 accepted stacks a month,
  read by raters on the site's own model gateway, settled by a person in
  verify mode, graded with an exact bound.
- **The drift series** (§14), **the yield funnel and the error-budget line**
  (§15), and **the sealed protocol as a command** (§16, only on a partner's
  yes).
- **The desk in rounds** (§17) and **the maintenance track** (§18).

## 2. What it rests on

- The descriptors (`pipelines/*/nils.job.yml`, `contracts/job/v1`): a Boutiques
  0.5 subset with an `x-nils` block, the image pinned by digest, typed
  parameters, roles, outputs as derivative kinds, `needs`, `qc`, `secrets` and
  typed inputs (records 43 and 49).
- Status per unit (made, failed, skipped, left, each reason counted), a run that
  none of whose units can run refused, failure kinds with cause, advice and fix,
  the machine requirements before a run, and the converter installed by the
  engine (record 55, the rulings of 2026-10-12, built after the Wave 7a spec was
  written).
- Pipeline versions with a select, a fixing version that `replaces` the older,
  runs over a scope's main scans, releases that keep their picks, and a
  pipeline's numbers as typed columns the ask reads by detail level (record 49).
- Check a result, job type 3 (record 55, H3).
- Campaigns as Wave 7a left them: purposes `decide`, `train`, `measure` and
  `audit`, blind first where a read trains or measures, suggestions brought in
  per item, the axes question held to the pack's legal combinations, A/B items
  for a reference read by judges, one account per reader, agreement and
  adjudication, Results and Pace (records 45, 48, 50 and 55; Wave 7a spec
  §11.4, §13.2).
- The audit draw and its grade door (record 55, D7; Wave 7a spec §11.5), built
  and never run.
- The rules' replay (`nils pack replay`) and the rehearsal of a rule change
  with its effect report (record 56).
- The certificate of the rules and its exact binomial bound (record 48), and the
  rule that production is fixed and released, never certified again (record 55,
  ruling 27).
- A site's shared software folder as it stands outside NILS: Apptainer
  sandboxes with one launcher each, read by every staff container.

## 3. What Wave 7b does not do

- It changes no MRI pack rule and no sorting. Facts come from the classification
  as it stands.
- It re-hosts no image and keeps no image registry of its own; no code path
  pushes an image anywhere (57-P5).
- It runs no native install in the first build: a tool registered from a home on
  the host (a FreeSurfer installed natively) needs a pinned base image to run
  inside, and comes after.
- It builds no free-form workflow canvas (57-Q2); a workflow is a list of steps
  with its links shown and checked.
- It builds no GPU scheduling across machines and no cluster executor.
- It harmonises nothing and corrects no brain-age bias; both are analyses over
  results, not pipelines.
- It ships no WMH-SynthSeg, which exists only in development builds of its
  tool.
- No pack chooses a tool outside its family; which family a person runs stays
  the person's choice, with the assistant's help.
- It builds no documents-to-facts, installs no second site, runs no time study
  (record 55, ruling 16) and no hyper-registry (58, "What it does not do").
- It draws no new sealed sample and writes no new certificate (ruling 27); the
  monthly audit is production evidence, not a certificate.
- No audit read and no verify answer trains a model.
- Nothing leaves a site: the audit's raters run on the site's model gateway,
  only counts leave the research side, and an export for a paper holds no
  header text, path or subject code.

## 4. The rules every slice obeys

### 4.1 The development channel first, and the desk in rounds (A4; ruling 36)

Every engine and desk slice is built from its branch onto a development install
through the development channel (record 31), tried there in use on a test
registry, fixed in quick rounds, and merged only when its owner has accepted it.
The desk's look and feel is a track of its own: as many build-and-look rounds on
the development install as it takes, the owner checking the feel as a user, and
one release once a round of UI work is settled, never one per fix. Real runs on
real data go to the site's compute host.

### 4.2 Fix, prove, release (ruling 27)

A production change is a fix, a check that nothing broke (the tests and, for
rules, the archive replay of §10), and a release. No sealing, no certificate and
no grading again. The maintenance track of §18 runs under this rule beside the
wave, in small releases.

### 4.3 Registered before drawn

As in Wave 7a (§4.3): the audit raters' check and every audit round (§13), the
drift series' runs (§14), the funnel's pilot (§15, registered and run by the
paper's own work) and the paper's verify campaigns (§12) have their plan written
and committed before their data is drawn: the sample, the measures, the bars and what counts as a pass. A change
after registration is written beside the plan with its date.

### 4.4 Nothing of the site in the code

As in Wave 5 and Wave 7a: nothing in the engine, the desk, the assistant or the
gateway reads a path, a host name, a group name or a pool of the site's own.
The software place is a path the setup record holds; a site's shared folder is
registered, never assumed.

### 4.5 Rules 1.0.0 stays pinnable

The paper pins the certified rules. Every release this wave or the maintenance
track makes keeps `pack-mri-v1.0.0` installable and never changes the
certificate's answers or the sealed packets (record 48). A change to the MRI
pack goes through the archive replay first and reports its moves (§10). The four
campaign purposes of Wave 7a keep their behaviour exactly, since the production
campaigns of that wave closed under them; `verify` is added beside them (§12).

### 4.6 Nothing leaves a site

The audit's raters are models on the site's own gateway. The research side
exports counts and manifests, never packets or rows. A campaign's export for a
paper (`of: counts`, §12.6) holds counts, digests and rates.

## 5. The pack (57-P1 to 57-P4)

### 5.1 The pair

`nils.job.yml` stays the knob layer: the image, the command, the inputs as
Boutiques knobs with their constraints (`requires-inputs`, `disables-inputs`,
`value-choices`, groups), the outputs and `x-nils`. `nils-pack.yaml` beside it
holds what NILS adds, under its own contract `contracts/pipeline-pack/v1`, and
pins the descriptor by the digest the catalog already computes
(`pipeline.descriptor_sha256`). **A pipeline version is the pair.** Changing
either file makes the name's next version, and `replaces` decides, as today,
whether the older stays selectable. A pack that does not load refuses the
version at the catalog, with the loader's words, as a descriptor with an
unpinned image is refused today.

### 5.2 The pack's sections

| Section | What it holds |
|---|---|
| `needs` | Per role, `hard` and `soft` needs. Each need: `when` (an expression over facts), `words` for the person, `source` (a paper, a maintainer's statement, or `policy`). `hard_if` makes a need hard only under a goal or switch. Needs on links (`aligned(flair, t1w)`) and on the host (`ram_gb`, `gpu_memory_gb`, a licence file) live here too. |
| `knobs` | For each Boutiques input: `what`, `default`, `cost`, `changes` (the outputs it changes), `comparability` (`none`, `additive`, `breaks`) and `set_by` (`person`, `rule`, or `fixed`, never shown). |
| `goals` | The person's choices: `goal` (`fast`, `standard`, `research`) and the pack's own switches (`lesions: [on, off]`). |
| `variants` | The sibling pipelines a rule may route a session to, inside one family (§7). |
| `rules` | Ordered; each `id`, `when`, `set` (knob values) or `route` (a variant), `why`, `source`. First match per knob wins. |
| `checks` | Each `metric`, `op`, `value`, `action` (`fail`, `flag`, `drop`), `source`; fixed thresholds as today's `x-nils.qc`; `cohort: true` with `stratum` and `min_sessions` for the checks of §5.8. |
| `licence` | `spdx`, `redistributable`, `site_file` (`env`, `mount`), `terms` (address and version), `accept` (whether an administrator must accept). |
| `resources` | Memory and minutes per unit as the input drives them, `gpu` (`none`, `optional`, `required`) and its memory. |
| `comparability` | The knobs and facts that enter the comparability key (§5.6). |
| `tests` | A small public input and the outputs expected, run when the tool is installed or registered (§6.4). |
| `source` | The image's OCI reference with `@sha256`, the version `probe` (a command and the text expected) and the `key_files` the fingerprint reads (§6.4). |

A short example (part of the SAMSEG pack):

```yaml
nils_pack: 1
pipeline: {name: samseg-lesions, descriptor_sha256: "<digest>"}
goals: {goal: [fast, standard, research], lesions: [on, off]}
needs:
  flair:
    hard_if: "lesions == 'on'"
    hard: [{when: "present(flair)", words: "no FLAIR picked, so no lesions", source: "<paper>"}]
  link:
    hard: [{when: "aligned(flair, t1w)", words: "the FLAIR is not registered to the T1w", source: "<paper>"}]
knobs:
  pallidum_separate: {comparability: breaks, set_by: rule}
  threshold: {default: 0.3, comparability: breaks, set_by: fixed}
rules:
  - {id: pallidum-needs-t2-or-flair, when: "not present(flair) and not present(t2w)", set: {pallidum_separate: false}, source: "<paper>"}
checks:
  - {metric: flair_t1_brain_dice, op: ">=", value: 0.85, action: flag, source: policy}
```

### 5.3 The expression language (57-P2)

A small language with no side effects: comparisons (`==`, `!=`, `<`, `<=`,
`>`, `>=`, `in`), `and`, `or`, `not`, and the named tests `present(role)`,
`aligned(a, b)`, `min(...)` and `max(...)`. No loops, no user functions, no
arithmetic beyond `min` and `max`. The engine parses and type-checks every
expression when the pack is loaded: an unknown fact, a wrong type, or a value
outside a fact's vocabulary refuses the pack at load. One evaluator in the
engine serves the pre-flight, the runner, the desk's doors and the assistant,
so all four read the same answer. After the rules run, the engine validates the
invocation against the descriptor's Boutiques constraints: a rule can never
produce an invocation the descriptor forbids.

### 5.4 The facts

Facts come from the registry, never from a pack's own reading of a header.

| Level | Facts |
|---|---|
| A stack in a role | `base`, `dimension` (2D or 3D), `voxel_max_mm`, `voxel_min_mm`, `slice_thickness_mm`, `field_strength_t`, `post_contrast` (the post-contrast label of Wave 7a), `vendor`, `body_part`, `skull_stripped`, `defaced` |
| A session | the roles present; `aligned(a, b)` from a registration step or a check; `age_years` where detail allows |
| A subject | the sessions that pass a pipeline's hard needs |
| A cohort | `diagnosis` from the clinical pack's imports |
| The host | cores, memory, GPU and its memory, the runtime |
| The person | `goal` and the pack's switches |

A fact the registry cannot tell is `unknown`. A hard need on an unknown fact
skips the session with "cannot tell: <fact>", and a person may let it run (the
unit then records the person as the one who chose). A soft need on an unknown
fact adds its caveat. The facts a rule read are kept with the unit (§5.5) and
shown at the reader's detail level, so an age appears only where detail allows
(record 55, K7).

### 5.5 Who chose (57-P2)

Every unit records, for each knob, `{value, by, rule, facts}`, where `by` is
`default`, `rule:<id>`, `person:<user>`, or `person:<user>` with
`proposed: assistant` when a person accepted the assistant's proposal, and
`facts` are the facts the rule read. The run's units door returns them; the
run page shows them (§17). Replaying the pack's rules on the recorded facts
gives the recorded values (G3).

### 5.6 The two keys, and no silent pooling (57-P3)

Every result carries two keys the engine computes from the pack and the run:

1. **The comparability key:** the tool, its version, the image digest or the
   registered install's fingerprint (§6.4), the pack's version, the variant,
   every knob whose comparability is `breaks` with its value, the input
   contrast set, the converter and its version for a BIDS input, and,
   recursively, the comparability keys of the steps it read.
2. **The result key:** the comparability key plus every remaining knob value
   (`additive` included) and the input stacks by content, the sha256 of each
   input file. It is what reuse matches (§8.5).

**The ask never pools across comparability keys silently.** A measure over
results with more than one comparability key answers per key, or says it holds
several and refuses to sum them, in the same words wherever it is read: the
ask, a page, a release, an export. A test per door (G5).

### 5.7 Status per unit, from the pack (57-P4)

The status per unit of Wave 7a is the base. The pack adds:

- **Skipped** units name the hard need they lack in the pack's words ("the T1 is
  2D with 5 mm slices; recon-all needs 3D at 1.5 mm or finer").
- **Routed** units say which variant a rule sent them to and why.
- **Caveats** from soft needs sit on the unit and travel with its results into
  the ask.
- **Flagged** units name the check, its value and its threshold. A flag is never
  a failure: the result is kept, and a person may check it (job type 3).
- **Cohort checks** read "not yet: 12 of 30 sessions" below their stratum size.

The pre-flight's answer gains, per unit, `{status, need, words, variant,
caveats}` and, per knob, `{value, by, rule}`; the unit door gains `knobs`,
`caveats`, `flags` and `keys`. `nils pipelines validate <dir>` validates a
descriptor and pack pair offline with the loader's words.

### 5.8 Cohort checks (57-Q8)

A cohort check (the Euler number against the cohort; MRIQC's metrics as robust
z-scores) runs within a stratum of site, vendor, field strength, sequence and
post-contrast state, when the stratum holds at least 30 sessions; a robust z
above 3 flags. A cohort check may flag a unit after its run has closed, and the
flag says when it came. Thresholds are adjusted after the first real runs, each
change replayed (§10).

**Tests:** G1, G2, G3, G4, G5.

## 6. The software place (57-P5)

### 6.1 The layout

One copy for NILS and for people, in a plain folder in the layout a site already
uses, extended with modules and manifests:

```
<place>/
  <app>/<version>/image            the SIF or sandbox, or a manifest naming image@sha256 on docker or podman
  <app>/<version>/manifest.json    source and digest, pack hash, size, installed or registered, by whom, when, verified when
  <app>/<version>/bin/<cmd>        one command per tool entry point
  <app>/bin/<app>-run              the launcher people use today
  bin/                             links for people's PATH
  modules/<app>/<version>.lua      Lmod modules: module use <place>/modules
  licences/<app>/                  site licence files, readable by the engine's account alone, mounted read-only
  externals.yaml                   registered installs
```

Wrappers find the place from their own location, so moving the folder does not
break them. On docker or podman the image stays in the runtime's store; the
manifest records `image@sha256` and the wrappers call the runtime. The engine
runs the same image file people run: the Apptainer runtime's image cache of
Wave 7a becomes the place.

### 6.2 Install

`nils software install <app>@<version>` pulls the image by its digest from the
pack's `source`, unpacks it where the host needs a sandbox, writes the manifest,
the wrappers and the module, and verifies it (§6.4). A pull records the digest
it asked for and the digest it got; they are equal or the install fails.

### 6.3 Register

`nils software register <app>@<version> --kind sif|sandbox|image --path <path>`
records an existing install as Spack records externals: the spec, the kind, the
path, `managed: false`, in `externals.yaml` and a manifest. The engine never
updates or removes what it did not install; `nils software remove` on a
registered install is refused (409) with that reason.

### 6.4 Verification, for both ways

- a version probe the pack declares (`recon-all --version`, `build-stamp.txt`,
  `mri_synthseg --help`), compared with the text expected;
- a fingerprint: the sha256 of a SIF, or of the pack's `key_files` in a sandbox
  or a home (FreeSurfer's `bin/` and `models/`), so that libraries added for a
  viewer do not change it;
- the pack's test run on its small public input.

The manifest records `verified_at` and the three results. A run records
"installed (or registered) install, version X, fingerprint Y", so a result
always says what made it. A registered install whose key-file fingerprint
equals the pinned image's is the same tool for the comparability key (57-Q4);
one that differs has its own key.

### 6.5 Licences as recorded acceptances

- An acceptance is an event: who, when, which terms (address and version), and,
  for a site file, the file's sha256. Its content is never recorded.
- A site file (the FreeSurfer licence) stays a secret input (record 49, R3),
  placed under `licences/<app>/`, mounted read-only into its pipeline alone with
  the environment variable the pack names (`FS_LICENSE`; `FS_ALLOW_DEEP` the
  same way).
- A run of a tool whose terms need an acceptance is refused before it starts
  (409, naming the terms' address) until an administrator has accepted them.
  A non-commercial tool (pyment, CC-BY-NC-4.0) is accepted once per site by an
  administrator in their own name (57-Q5).
- The engine never re-hosts an image whose terms forbid it. It pulls by digest
  from the tool's own source or registers a copy the site holds, and has no
  code path that pushes an image anywhere (G9).

### 6.6 Dependencies, checked as the pre-flight checks them

At install and before each run, each item is answered ok, warn or missing with
its fix: the runtime and its version, the GPU and its memory, memory, disk, the
converter, the licence (G10). The requirements module of Wave 7a is the one
implementation.

### 6.7 Doors, the command and the desk

- `GET /api/software`: each package with version, licence (accepted by whom and
  when), size, place, how it came (installed or registered), status (installed,
  missing, update available, failed verification) and its needs, checked.
- `POST /api/software/install`, `POST /api/software/register`,
  `POST /api/software/{app}/{version}/verify`,
  `POST /api/software/{app}/{version}/accept`,
  `DELETE /api/software/{app}/{version}` (only what the engine installed).
- `nils software list | install | register | verify | accept | remove` is the
  same at the keyboard. The converter's install door of Wave 7a becomes one
  package among the others; `nils tools install dcm2niix` stays as an alias for
  one release and then goes, and the run failure's advice names
  `nils software install dcm2niix`.
- **Settings > Software** (administrators) lists the packages and offers
  install, register a path, verify, accept a licence, and remove.

### 6.8 A site's shared folder (57-Q1)

The apps a site already holds in its shared software folder stay the site's and
are registered as they are, with no byte changed (G8). The engine writes only a
subfolder of its own, read-only to everyone else, so that staff see both. Who
writes where is the setup record's, never the code's (§4.4).

**Tests:** G7, G8, G9, G10.

## 7. Registration as a family (57-P6)

**A family** is one entry for the person and several pipelines behind it, with
the pack's rules choosing per session. The catalog lists a family as one row
that opens to its members; a run over a family records, per unit, the member it
was routed to and the rule that chose. Registration is the first family; the
same shape serves cortical reconstruction, brain age and lesions later.

**The smallest set:** two pinned images, three methods. ANTs 2.6.x (rigid with
mutual information across contrasts in 3D, SyN to a template, lesion masks, high
resolution) and FreeSurfer 8.2.0 (SynthMorph, contrast- and resolution-agnostic
with no skull stripping; `mri_robust_register` and `mri_robust_template`,
inverse-consistent and robust to lesion change across visits; SynthSeg for the
checks).

**The tasks and their main rules:**

| Task | Rule |
|---|---|
| Within a session, across contrasts (FLAIR, T2, PD, post-contrast T1 to the T1w) | Both 3D at 1.5 mm or finer: ANTs rigid, Mattes MI, SynthStrip mask, `-j 0`. Either 2D or slices over 2.5 mm: SynthMorph rigid where the pinned model has it, else affine with a flag. |
| Across a person's visits | Same contrast, 3D: `mri_robust_register --iscale` for two visits, `mri_robust_template` for three or more, rigid. Protocol changed or 2D: SynthMorph, flagged. Atrophy at research grade: ANTs SyN in the halfway space. |
| To a template | 3D T1w at 1.2 mm or finer: `antsRegistrationSyN.sh -t s` to MNI152NLin2009cAsym, after N4 and skull stripping, a lesion mask as `-x` when lesions are on. Otherwise, or goal fast: SynthMorph joint, `-r 0.5`. |
| Labels back to the person | The stored transform inverted, nearest-neighbour or GenericLabel; native SynthSeg preferred where it covers the region. |
| Refuse | Not brain or head, or either image's SynthSeg QC below 0.65. |

**Outputs** are derivatives: the transform in the tool's native form (ITK
`.mat` and `.h5`, LTA, a displacement field and its inverse), named BIDS-style
(`_from-FLAIR_to-T1w_mode-image_xfm`), and the aligned image. Both carry the
typed output of §8.2: kind `transform` or `image`, the role and contrast they
stand for, and the grid they are on.

**Checks:** SynthSeg QC of both inputs at 0.65; label Dice after alignment
against the cohort's 5th percentile per task; folding above 0.01 % of brain
voxels; inverse consistency above 0.1 mm; a rigid sanity check (determinant
near 1, no flip, translation under 50 mm, rotation under 30 degrees within a
session). The last three are `source: policy`, calibrated on the site's data.

**What it changes:** SAMSEG and segcsvd take the registration's aligned FLAIR as
a typed input, and the header-only resampling goes. The 0.85 brain-mask check of
Wave 7a stays as the check on the link. Production runs plan on the CPU
(SynthMorph affine about 72 s, deformable about 15 min single-threaded); a card
is taken under the lease of Wave 7a when free.

The family runs a pilot on the research side, graded by its checks, before
SAMSEG and segcsvd depend on it in production (G11).

## 8. Workflows, results kept, and provenance (57-P7, 57-P8, 58-P5)

### 8.1 One Pipelines page

Catalog and Running become one Pipelines page with no tabs: the pipelines and
families (each with its version select and its last run), the workflows, and
the runs going and done, every row opening its own page by a click anywhere on
it. A workflow is added here by hand: steps chained, each step a pipeline or a
family, an output wired to the next step's input. Planning with the assistant
lives on the Assistant page, and its plan opens here as a workflow filled in,
which the person checks and starts.

### 8.2 The workflow document

`nils.workflow.yml`, under `contracts/workflow/v1`, versioned as a pipeline is
and kept in the catalog: `steps` (each a pipeline or family, its version or
`current`, the goal, the knobs a person may set) and `links` (an output of one
step to an input of the next). Outputs are typed by their derivative kind
(`mask`, `image`, `transform`, `table`) and, newly, by the role and contrast
they stand for and the grid or space they are on (`grid: t1w`, as SAMSEG's map
already declares). A change makes the document's next version; `replaces`
decides whether the older stays selectable.

### 8.3 Links checked when the workflow is planned (G12)

`POST /api/workflows/{id}/plan` (and `nils workflow plan`) answers, before
anything runs, each link's verdict and each unit's pre-flight:

- **Satisfied:** registration of FLAIR to T1w before SAMSEG meets SAMSEG's
  `aligned(flair, t1w)` need.
- **Missing:** SAMSEG with lesions and no registration step is accepted only with
  the header-alignment check on, and the plan says so.
- **Redundant:** N4 before SAMSEG or recon-all is flagged, since both correct
  bias themselves; N4 before a FLAIR lesion model is flagged as possibly
  harmful.
- **Refused:** a link whose output type does not meet the next input (a T1w mask
  into a FLAIR input) is refused with both types named.
- **Fed:** SynthSeg before segcsvd is a typed input already; the plan names which
  SynthSeg (model, input contrast) the segcsvd key will carry.

### 8.4 Runs go per unit through the chain

A session's next step starts when its previous step is done, without waiting
for the whole selection. A unit skipped or failed at one step is skipped
downstream with the same reason. Cohort checks run when a step's stratum is
complete. The run's line sums each step's units.

### 8.5 Results kept, and reuse (G6)

Every result is kept with its two keys (§5.6): its files as derivatives by path,
its values as typed numbers per subject, session and stack, and the knob values
with who chose them. **A step whose result key matches a kept result is not run
again:** the run records "reused from run N" for that unit and links the
result. A result is not reused when a person rejected it in a check (job type
3), when a newer version replaced its pipeline because the older was wrong
(`replaces`), or when its files fail their sha256.

### 8.6 The provenance door, with the curation state (57-P8, 58-P5)

`GET /api/provenance/{subject|session|stack}/{id}` (and `nils provenance`)
answers, for a subject, session or stack, which steps ran on it, with which keys
and which results, and, for every result, **its curation state:** the MRI pack
version and digest that classified its inputs, the label set digest, System 1's
model version, and whether a person overrode an axis on an input, and which. A
change of the rules version on an input marks the result `stale`, in the same
words at every door (the ask, a page, a release, an export), as the comparability
rule of §5.6 is worded. The assistant reads the door through the ask at the
reader's detail level.

**Tests:** G6, G12, E5.

## 9. The assistant plans with the pack (57-P9)

plan-analysis reads the packs and the workflow documents. Asked for an analysis,
it proposes the pipeline or workflow, the selection or the scope's main scans,
the goal and any knob the person may set, and which sessions will be skipped or
routed and why, from the pre-flight. It explains an option in the pack's own
words and sources. It never sets a value itself: a person accepts its proposal,
and the unit records the person with the proposal noted (§5.5). It runs on the
stations' model with the pack as its context (G13).

## 10. A pack change shows its effect first (57-P10)

Every pack change, rules or needs, is replayed over the registry before it
ships, as record 56 does for the MRI pack: how many sessions change from run to
skipped, which variant they are routed to, which knob values change and from
what to what, and which kept results it would make stale. The replay is
read-only. The first packs are written after the archive's fact counts per
session, which exist (57-Q7), so each rule is written knowing how many sessions
it touches (G14).

## 11. The first packs (57-P11)

One row per tool; the needs, the rules that matter and the checks, each with its
source in the record's research.

| Tool | Needs (hard; soft) | Rules that matter | Checks |
|---|---|---|---|
| **recon-all** (FreeSurfer 8.2.0) | Hard: T1w, 3D, every voxel side 1.5 mm or finer (1.3 to 1.5 mm flagged and in the comparability key). Soft: non-contrast; adult; MS lesions. | No T1 meets the hard needs: route to recon-all-clinical. 3D post-contrast T1: SynthSR then recon-all, flagged, recon-all-clinical as fallback. Finer than 0.9 mm at research grade: `-hires`. `-3T` only as a cohort's fixed choice. Ventricle volume z above 3: `-bigventricles`. Research grade: `-qcache`. Fixed: `FS_V8_XOPTS=1`, `mris_fix_topology -threads 1`. | Euler number z below -3.29 within the cohort; the input's SynthSeg QC below 0.65; `SegmentedIntraCranialVolume` of 0; fsqc outliers at 10 subjects or more. 32 GB. |
| **recon-all-clinical** | Any single scan; adults. Soft: thick slices. | The variant recon-all's rules route to. | SynthSeg QC. 3 to 4 h, 32 GB. |
| **SAMSEG** | Hard: T1w; inputs on one grid; with lesions, a FLAIR (else T2). Soft: non-contrast T1. | Lesion mode only with a lesion-bright contrast, pattern 0 for T1 and 1 for FLAIR or T2, in input order. `--pallidum-separate` only with a T2 or FLAIR. Threshold 0.3 fixed. No N4 before it. | `seg.mgz` and `samseg.stats` exist; intracranial 0.9 to 2.3 L (policy); FLAIR on T1 at 0.85 (policy); lesion volume above 0 in controls flagged. |
| **SynthSeg** | Any single scan. | `--robust` for clinical, 2D, thick or non-T1 input, one choice per study (robust and not are different networks). Off only for goal fast on a 3D T1 near 1 mm, marked not comparable. | Any of the 10 QC scores below 0.65 flags the session; a structure's volume is kept only where its own QC is 0.65 or more. 24 GB. |
| **SynthStrip** | Any single scan. | Border 1 mm; `--no-csf` off unless a registration mask needs tight edges. Both in the comparability key. | Mask volume outside the cohort's 1st to 99th percentile; mask touching the image edge; more than one component (policy). |
| **MRIQC** 24.0.2 | Hard: 3D T1w, not skull-stripped, no FLAIR; a 3D T2w in its own invocation. Soft: voxels over 1.5 mm, post-contrast, defaced. | Always `--no-sub --notrack -m T1w`, `--species human`, float settings fixed. | `fber` or `qi_1` of -1 fails the unit; `wm2max` outside 0.6 to 0.8 flags; robust z above 3 within the stratum flags, from 30 sessions; the bundled classifier never decides. |
| **N4** (ANTs 2.6) | Hard: positive intensities, 3D. Soft: a brain mask; T1w; slices 3 mm or thinner. | `-b [180,3]` fixed; SynthStrip mask as `-x` where one exists; shrink 2 for 1.5 to 3 mm, 1 above 3 mm; fewer levels for goal fast, marked not comparable. Never before recon-all, SAMSEG or a FLAIR lesion model (a link warning). | Field positive inside the mask; convergence falling per level; CJV not worse after. |
| **segcsvd** rc03 | WMH: FLAIR up to 5 mm hard, over 3 mm soft; a SynthSeg 2.0 map. PVS: 3D non-contrast T1 up to 1.2 mm hard, over 1.0 mm soft. T1 and FLAIR aligned (§7). Soft: 1.5 T; MS. | Thresholds 0.35 fixed; `skip_mask_and_bias` false; the WMH prior from the FLAIR, else empty and tagged not comparable; MS labelled "not validated for MS"; PVS research only with a lesion mask as prior. The undocumented positionals frozen and hidden. | Probabilities in 0 to 1; mask equals probability above threshold; WMH above 7 % of ICV or 0 flags; PVS of 0 flags; PVS inside the WMH prior flags; alignment at 0.85. |
| **SynthBA** 0.2.1 | T1w, T2w or FLAIR, 2D or 3D up to 6 mm; ages 6 to 95. Soft: 2D; 2D T1; post-contrast; MS. | Model `g`; template `t1` for T1w, `t2` for T2w and PD, one fixed choice per study for FLAIR; input order: 3D non-contrast T1, 3D FLAIR, 3D T2, 2D FLAIR, 2D T2, 2D T1. | Mask volume against the cohort; a prediction outside the training ages. |
| **pyment** 5.1.0 | Hard: 3D non-contrast T1 up to 1.5 mm; ages 3 to 97; the FreeSurfer licence. Soft: finer than 0.7 mm; 1.5 T; Philips; MS. | FastSurfer forced to 1 mm when the finest voxel is under 0.98 mm; `multi-2025` by default, both weights at research grade. | Conformed voxel 1.0 mm; crop 224 x 192 x 224; the loader check; a prediction outside the training ages; the install test on ds000030. Non-commercial. |
| **LST-AI** | 3D T1 and FLAIR; MS. | The lesion family's first choice for MS with a 3D T1 and FLAIR (57-Q6); whether its denormal-float flush is on is decided by one same-conversion repeat (§18) and then fixed in the pack. | Lesion volume and count against the cohort; the T1-FLAIR alignment at 0.85. |
| **Registration** | §7 | §7 | §7 |

Brain age is reported raw, with its configuration key and the published error
for its input type; the age-bias correction is an analysis, not a pipeline.

## 12. Verify mode in campaigns (58-P1)

### 12.1 The purpose

A fifth campaign purpose, `verify`, beside `decide`, `train`, `measure` and
`audit`, which keep their behaviour exactly (§4.5). A verify campaign's items
carry a suggestion (`suggest: imported` or `rules`), and the reader is shown it
first, filled in on every asked axis. This is the one exception to blind-first
(record 55, D6); the planted values of §12.3 are what make it safe.

### 12.2 The screen

One stack per screen, with the header text as the reader shows it today and
every asked axis filled in with its proposal. One key accepts the whole item.
Changing an axis offers the other legal values first, held to the pack's
constraints as the axes question is held today. Where two sources disagree on an
item, both are shown as A and B, unlabelled, and the reader picks one or neither
(the A/B items of record 48).

### 12.3 Planted values

A campaign is made with `planted: {share, seed, from}`: a seeded share of its
items (10 % by default) carries one axis whose proposal is replaced by a
plausible wrong value, kept legal with the item's other axes. `from` is a
confusion table (rows of true value, wrong value and count, with no stack
identities), made on the research side from a development read; the commonest
wrong value for the true value is planted, and where no table is given a random
other legal value is. The seed and the planted list are recorded with the campaign, never shown
to a reader, and opened at the close. The reader is told that planted values
exist, not where.

### 12.4 Blind, and training

An answer given in verify is recorded `blind: false`, as the rule of Wave 7a
(§11.4) already does for an answer given after a suggestion showed, so **a
verify answer never becomes a training label.** Where the campaign closes into
decisions, the answer is a decision with the reader as its author; where it
closes into nothing, it is a reference's check.

### 12.5 Results and Pace

Results report the **catch rate**, the share of planted items whose planted
axis the reader did not accept, beside the agreement, overall and per reader. A
campaign is reported against a registered bar of 90 %: one under it is reported
as such and its result never stands alone. Results and Pace name a reader by
their account's name, never the sign-in subject (a fix of Wave 7a's list).

### 12.6 The export for a paper

`POST /api/campaigns/{id}/export` with `of: counts` writes counts, digests (the
frame's and the planted list's sha256), the catch rate and the agreement: no
header text, no path, no subject code. `of: outcomes` and `of: answers` stay as
they are for the site's own use.

### 12.7 Doors

Making a campaign takes `purpose: verify` and `planted`; `GET /api/campaigns/{id}`
reports `planted: {share, count, opened}`; the answers door is unchanged; the
grade of §13.4 reads the settled answers as today.

**Test:** E1.

## 13. The audit stream, with local raters (58-P2)

### 13.1 The draw, by count

`POST /api/audit/draws` gains `count` and `since`: a fixed number of stacks (150
for the monthly audit) drawn from the stacks accepted since the last draw, in
place of a round's share. The frame is as before (MR, sorted under the rules in
force, no sort question waiting, not of a sample sealed), the draw is keyed by
seed and DICOM identity so the same seed draws the same stacks in any registry,
and the draw records its frame size and digest and the rules' answer each drawn
stack was accepted with.

### 13.2 The raters, and their check

The raters are two local models of different weights, one item read by both:
the stations' model with thinking on and the site's second model, acting as
agents outside the engine through the campaign doors as Wave 7a built them. Two
raters of one model agree when both are wrong, which is also why the settle of
§13.3 does not rest on their disagreement. Before the first round they are
checked once against a key made by another model family: the certificate's
registered reference (the settled read that graded the rules) and the protocol
printouts, through the grade of the registered local reproduction of that
reference, never against a key the raters' own model produced. The raters pass
when the local reference is fit (every axis under the 2 % shared-error bar) and
meets the bar of Wave 7a's T21 on answered cells (at most 3 % of answered cells
wrong and at least 90 % answered on every axis), under one vocabulary for the
key, the prompts and the served rules (the words of the key are the served
pack's). The check is an evidence run with its manifest; it waits for the local
reproduction's grade. The human-adjudicated subset of the paper's audit campaign
re-checks the raters. If they miss, the rounds are read by people alone in
verify mode and every report says so; no bigger model is tried.

### 13.3 The settle

Every stack where a rater's answer differs from the accepted answer goes to a
person in verify mode, plus a seeded 10 % of the stacks where both raters agree
with it: a campaign of purpose `verify` over those items, with the accepted
answer and the differing answers shown as A and B, unlabelled. A disagreement
between the raters alone decides nothing. The settled answers are the round's;
a stack not settled stands as the raters' agreement with the accepted answer.

### 13.4 The round's report

`POST /api/audit/draws/{id}/grade` reports, as today, the stacks read, the wrong
ones (every settled difference from the accepted answer is a rules error), the
exact one-sided 95 % bound and the wrong by axis, and gains counts by
vendor and acquisition year and the registered rule `{max_wrong: 1, passed}`.
Six rounds pool to 900 stacks. A month over the rule opens a rule review through
the loop of record 56, never a certificate (ruling 27). These reads never train
a model (D7).

**Test:** E2.

## 14. The drift series (58-P3)

On the research side, with the replay tools of record 56: the replay's summary
grouped by acquisition year, vendor and scanner model, from the fingerprint
fields the archive extract already carries; one evidence run per rules version
released since 1.0.0, each with a manifest (binary digest, pack digest, extract
digest, chunking); the moves of each version relative to 1.0.0 per group, as a
table of counts. Nothing in the engine changes; a grouping flag on the counting
script. Counts only leave the research side (E3).

## 15. The yield funnel and the error-budget line (58-P4)

For a selection and a pipeline or family, the funnel gives the subjects,
sessions and stacks left after each query filter of the selection and after each
hard need of the pack (§5.2, §5.7), by acquisition year, sequence (base) and 2D
or 3D, and a last line that multiplies the certified upper bound of each axis
the selection filtered on (the certificate's per-axis bound; the stack bound
where an axis has none) by the count filtered on it and reads "at most N stacks
misplaced by label error". `GET /api/funnel?selection=<id>&pipeline=<id>` and
`nils funnel <selection> <pipeline>` return the same numbers; the desk shows
them beside the selection. **The definition is fixed by a pilot before anything
is built:** a registered pilot on existing labels, run on the research side by
the paper's own work, not by a slice of this wave (§4.3); the record is amended
with its definition before the slice starts (E4).

## 16. The sealed protocol as a command (58-P6, stretch)

Only on a partner's yes. On a site's own install, `nils audit draw --seed <s>
--count <n> --packets <dir>` makes a seeded draw and writes header packets as
text for a verify campaign; `nils audit export` writes counts, the bound and the
reader-disagreement count under the minimum-cell rule of record 14 (k = 5). It
is the first half of a label-quality passport a site returns about its own
labels (E6, graded only if built).

## 17. The desk in rounds (ruling 36)

The desk's look and feel is a track of rounds on the development install (§4.1):
a list of screens written from the owner's screenshots and the task table of
Wave 7a, one round at a time, the owner checking the feel as a user, and one
release once a round is settled. The screens this wave adds join the rounds:

- Settings > Software (§6.7);
- a run's units with each knob's value and who chose it, the unit's caveats,
  its skip or route reason and its flags (§5.5, §5.7); the goal and the pack's
  switches at the start of a run;
- the Pipelines page without tabs, the workflow as a list of steps with its
  links shown and checked (§8.1, §8.3);
- the provenance panel on a result, with its curation state (§8.6);
- the verify screen and Results with the catch rate (§12), the first round of
  the track, because the paper's audit campaign needs it by December;
- the funnel beside a selection (§15);
- the import map dialog that shows shapes and counts only (§18).

No gate test of its own; the G and E tests reach the screens they name.

## 18. The maintenance track (ruling 27)

The fixes found while closing Wave 7a that change what the code does, each
shipped as a small release with its tests (§4.2), the privacy fix and the
release fix first:

- **The import map dialog** shows each column's shape (its type, its length
  range, how many values and how many distinct) and never a value.
- **Releases build only what changed:** a model image is rebuilt only when its
  package's files changed since the last release, else the published image is
  reused by digest; the release reuses the pull request's platform binaries; the
  tag gate accepts the pull request's green checks when the pull request was up
  to date with main (its squash is the same tree).
- **System 1 at arrival on a fresh install:** the arrival descriptor carries
  what the trust gate needs, so a kept model's checked code is trusted by its
  admission and a fresh install scores at arrival without a hand-registered
  version.
- **Shared-name questions** are raised only for the stacks a release writes, so
  a test release never spends another dataset's questions; a group is asked once.
- **One acquisition written twice** (equal acquisition times, equal fingerprint
  fields) is one stack with several paths, written once.
- **System 1's parts** run several at once within the lane's budget, and the
  featurising is batched at the next retrain.
- **A run's output folder** left by a previous registry is moved aside
  (`<id>.before-<when>`), as the run folder already is.
- **The supervisor's update** does not roll back during a migration that
  reports progress; the settle time waits for the migration.
- **The model gateway** holds its token limit with thinking on and caps the tool
  calls of one turn, so a looping check holds no slot.
- **A release's speed** is measured (converter, the share's writes, fsync) before
  a whole-dataset release.
- Campaign Results and Pace name a reader by account name (§12.5); the sources
  door gets single-flight; the post-contrast hint query is made fast; the
  supervise test's race is fixed; the converter failure's advice names the
  software place (§6.7); `located()` accepts `--chunk` from a door.
- **LST-AI's denormal flush** (a unit from 157 to 54.5 min) is decided by one
  same-conversion repeat and, if adopted, fixed in its pack (§11).

Rules changes among them go through the replay (§10) and keep 1.0.0 pinnable
(§4.5).

## 19. The gate of the wave and its closing bars

The wave's tests are record 57's G1 to G15 and record 58's E1 to E6, written
before the work started and graded once in the closing report; a missed test is
reported as missed, never redefined (record 55, A3). "The development install"
follows the development channel on a test registry; "the production install" is
the site's; "the research side" is the site's research container. Each
repository runs its own gate in CI as before, and the tests below are added to it
where they can run there.

**The pack**
- **G1.** Every starter pipeline has a pack; 100 % load under
  `contracts/pipeline-pack/v1`; a fixture pack with an unknown fact, a wrong
  type or a rule that breaks a Boutiques constraint is refused at load. CI.
- **G2.** On a fixture set with a case per need of each first pack, 100 % of
  sessions a hard need excludes are skipped with the need's words, 0 run; every
  soft need's caveat is on its unit and in the ask. The development install.
- **G3.** 100 % of units of every run record each knob's value, `by` and the
  facts read; replaying the rules on the recorded facts gives the same values.
- **G4.** Each first pack's checks on a fixture of known bad outputs (SynthSeg
  QC under 0.65, MRIQC `fber` of -1, a FLAIR moved 15 mm, an empty PVS mask) give
  the specified status, 100 %; cohort checks say "not yet" below their stratum.
- **G5.** A measure over results with two comparability keys is answered per
  key or refused, at every door that returns it; a test per door. CI.

**Workflows and reuse**
- **G6.** A workflow run again over the same inputs runs 0 steps and says
  "reused" for each unit; changing one `breaks` knob reruns exactly that step and
  the steps after it; a rejected result is never reused.
- **G12.** The plan of every workflow in a link fixture set gives the specified
  answer: satisfied, missing, redundant, refused; 100 %.

**The software place**
- **G7.** Each first pack's tool installed by digest and verified (probe,
  fingerprint, test run); usable by hand from `bin/` and `module load` as a
  person; NILS runs the same file (sha256 equal). The development install and a
  lab install.
- **G8.** On the production install, the site's existing shared apps registered
  with no byte changed (checksums before and after equal); verified; a run
  records "registered, version, fingerprint"; removing one is refused.
- **G9.** Each acceptance recorded (who, when, terms, file sha256); a run
  needing an unaccepted licence refused before it starts; the licence never in
  an output or a log; no image pushed anywhere (no such code path).
- **G10.** Each missing item (runtime, GPU, memory, disk, converter, licence)
  reported per item with its fix, at install and at pre-flight, on a fixture
  per item.

**The family, the assistant, the effect**
- **G11.** The registration family's pilot: each task's rule picks its method;
  transform, aligned image and checks written; on the moved-FLAIR fixtures the
  alignment check passes after registration where the header path failed it,
  100 %. The development install, then the research side on archive copies.
- **G13.** plan-analysis on its test questions with the stations' model names
  the right pipeline or workflow and the right skipped sessions with their
  reasons, at or above the bar set when its questions are written.
- **G14.** Every pack change in the wave replayed on the archive before release;
  changes released equal changes replayed; the fact counts written before the
  first pack.
- **G15.** One real workflow on the production install after the release
  (registration then SAMSEG with lesions; SynthSeg then segcsvd) on a production
  dataset: every unit's status explained, 0 units "done" with nothing made.

**The paper's evidence**
- **E1.** On the development install, a verify campaign of 200 items with 10 %
  planted from a fixture confusion table: every planted item recorded and its catch reported in Results; the
  proposal shown first on every item of a verify campaign and on no item of a
  measuring or training campaign before its answer (a test per door); every
  verify answer `blind: false` and in no training label set; the export of
  §12.6 holds no header text, path or subject code, checked by a scan of its
  files. CI where it can run.
- **E2.** The raters' check against the certificate's registered reference and
  the protocol printouts, through the local reproduction's grade, reported
  before any round, under one vocabulary; on the production install, 150 stacks
  drawn by seed, read by both raters, every difference from the accepted answer
  and a seeded 10 % of the agreements settled by a person in verify mode, the
  wrong count and exact bound reported with counts by vendor and year; the
  human-adjudicated subset re-checks the raters; 0 audit reads in any training
  set.
- **E3.** One evidence run per rules version since 1.0.0 (seven today, and the
  wave's), each reproducible from its manifest (the same counts on a rerun); the
  moves of each version relative to 1.0.0 by year, vendor and scanner model;
  counts only. The research side.
- **E4.** For the registered cohort recipe on the production install, every
  funnel row equals a hand count on the research side (0 differences), and the
  same numbers at every door that returns it (desk, command, ask).
- **E5.** Every unit of G15's workflow answers its curation state at the
  provenance door; a rules-version change on an input marks the result stale in
  the same words at every door (a test per door).
- **E6.** Only if built: on a lab install, `nils audit draw --seed` draws the
  same stacks from the same seed twice, makes the verify campaign, and exports
  counts, the bound and the disagreement count with no cell under 5.

Record 58's P7 (a documents-to-facts feasibility test) has no test; its result
is a study.

## 20. Defaults settled in this spec

- The pack is `nils-pack.yaml` beside `nils.job.yml`, under
  `contracts/pipeline-pack/v1`, pinning the descriptor by `descriptor_sha256`; a
  pipeline version is the pair (§5.1).
- The expression language is comparisons, `and`, `or`, `not`, `present`,
  `aligned`, `min` and `max`; an expression that does not type-check refuses the
  pack at load (§5.3).
- A fact the registry cannot tell is `unknown`; a hard need on it skips with
  "cannot tell" (§5.4).
- `by` is `default`, `rule:<id>` or `person:<user>`, with `proposed: assistant`
  where it applies (§5.5).
- Cohort checks need 30 sessions per stratum and flag at a robust z above 3
  (§5.8).
- The software place's layout is §6.1's; the engine's own subfolder of a shared
  folder is read-only to others; `externals.yaml` holds registered installs;
  `nils tools install dcm2niix` stays as an alias for one release (§6).
- A family is one catalog row that opens to its members (§7).
- The workflow document is `nils.workflow.yml` under `contracts/workflow/v1`;
  typed outputs carry kind, role, contrast and grid (§8.2).
- The provenance door is `/api/provenance/{kind}/{id}`; a result whose input's
  rules version changed reads `stale` (§8.6).
- The goal defaults to `standard`, chosen per run; a cohort may fix it (57-Q3).
- A verify campaign plants 10 % of its items by default from a confusion table,
  a random other legal value where none is given, and is reported against a
  catch-rate bar of 90 %; the export for a paper is `of: counts` (§12).
- The monthly audit draws 150 stacks by count since the last draw, read by two
  raters of different weights; every difference from the accepted answer and a
  seeded 10 % of the agreements are settled by a person; the round passes with
  at most 1 wrong (§13).
- The minimum cell of an exported count is 5 (§16).

## 21. Order of work

A slice's letter names its repository or place: **A** the engine, **B** the
desk, **C** the model gateway, **D** the assistant, **P** a pipeline package (in
`pipelines/`), **R** a rules pack (in `packs/`), **S** a study, a registered plan
or a research tool, **W** the site's own deployment (whose specifics are the
record's). Each slice is built from its branch onto the development install,
tried in use, then merged as one pull request (§4.1). Slices in one step run
side by side where they touch different parts; the paper's dates (record 58)
bind the steps that carry its items. If the wave must be cut, reuse (§8.5) and
the assistant's planning (§9) go first; the provenance door stays (record 57,
risk 8, as amended).

**Step 1, the ground (to mid November)**
- **A1** the pack: the contract, the loader, the expression language, the facts
  over the registry, rules at pre-flight recorded per unit, the two keys, cohort
  checks (§5). G1 to G5.
- **A2** the software place: the layout, manifests, install by digest, register,
  verify, acceptances, wrappers and modules, `nils software`; the converter
  moves into it (§6). G7, G9, G10.
- **A3** verify mode: the purpose, planted values, the catch rate, the export
  (§12); **B1** the verify screen and Results with the catch rate, the first desk
  round (§17). E1.
- **A4** the first maintenance release: the import map dialog and releases that
  build only what changed (§18).
- **B2** the desk rounds' first list, written from the screenshots and the task
  table (§17).
- **S1** the audit raters' check, registered now and run after the local
  reproduction's grade, against the certificate's registered reference and the
  protocol printouts (§13.2). **S2** the drift series' tools and its first runs
  (§14). The funnel's pilot is the paper's own work, not a slice (§15).

**Step 2, the packs and the audit (November)**
- **P1** the first packs of §11, each replayed before it ships (§10); **R1** the
  descriptor fixes the research found (SAMSEG's pallidum rule, segcsvd's key)
  ship with them. G2, G4, G14.
- **A5** the audit draw by count and the round's report (§13.1, §13.4); **W1**
  the first round on the production install on 1 December. E2.
- **B3** Settings > Software; a run's units with who chose, caveats and reasons;
  the goal and switches at a run's start (§17).
- **A6** the second maintenance release: System 1 at arrival, shared-name
  questions scoped, the twice-written acquisition, System 1's parts in parallel,
  output folders moved aside, the supervisor's settle (§18).

**Step 3, the family and the workflows (December)**
- **P2** the registration family: the two images pinned, the three methods, the
  rules, transforms as derivatives, the checks (§7); **S4** its pilot on the
  research side. G11.
- **A7** workflows: the document, typed links, link checks at plan time, per-unit
  chaining, reuse by key, the provenance door with the curation state (§8); then
  SAMSEG and segcsvd read the registration step. **B4** the Pipelines page
  without tabs and the workflow list; the provenance panel. G6, G12, E5.
- **C1** the gateway's token limit with thinking on and the call cap (§18).

**Step 4, the funnel and the assistant (January)**
- **A8** the funnel and the error-budget line, from the pilot's definition
  (§15); **B5** the funnel beside a selection. E4.
- **D1** plan-analysis reads packs and workflows; the ask reads provenance and
  keys (§9). G13.
- **S5** the drift series refreshed with the wave's rules versions (§14). E3.

**Step 5, the close**
- **W2** with the owner's go: the site's shared folder registered on the
  production install (§6.8), the release, the first real workflow. G8, G15.
- **S6** records 57 and 58 scrubbed into `docs/decisions/` of this repository
  beside this spec, and the closing report that grades G1 to G15 and E1 to E6
  once.
- Record 58's P6 on a partner's accepted invitation; P7 as research after
  step 1 has started.

## 22. Open questions carried into the wave

Each is settled with the record's owner when its slice is built, and written
into the record with its date.

1. The funnel's exact definition, after the pilot (§15): which filters are
   rows, how the per-axis bounds combine where a selection filters on several
   axes.
2. The confusion table each verify campaign of the paper plants from
   (`planted.from`, §12.3): which development read it is made from, on the
   research side.
3. The policy thresholds of the registration family's checks (§7), calibrated
   on the pilot.
4. Which key accepts an item on the verify screen, and whether a change of one
   axis re-asks the derived axes (§12.2).
5. The name of the engine's own subfolder in a site's shared software folder
   (§6.8), the setup record's.
6. Whether a family's row in the catalog expands in place or opens its own page
   (§7, §8.1).
7. Whether an audit round's differences are settled by one person or by two
   with a third to adjudicate (§13.3).
8. Record 58's P6, which exists only if a partner says yes (§16).
