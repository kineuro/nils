# 11 — Build order, gates, and the do-not-forget list

## Standing rules

- **v0 keeps running on the production host untouched** until every gate below is green. It is the
  oracle (principle 10); decommissioning it early forfeits the regression asset.
- Every wave opens by writing the real spec for its slice (this folder only records
  decisions) and closes at a **gate**: a measurable comparison against v0 or a
  working consumer, not a demo.
- Waves are sequential where they share the registry; the agent track is parallel
  by design.

## Waves

**Wave 0 — ground.** v0 feature work frozen (F1, since 2026-09-02). The public repo
restarted as `kineuro/nils` on 2026-09-02 (15 §11; R1 to R8; AGPL-3.0-only, 10). CI skeleton:
multi-platform binary builds, the two-backend test matrix, the scaled performance benchmark harness (D6). Accepted 2026-09-02 from
[12](12-review-devils-advocate.md): the language spike (C1, judged on speed and
maintainability together), the baseline host with v0 measured on it (C6), and the
pack-format prototype (C11) before Wave 2 opens.

**Wave 1 — parse and digest.** The Rust core: walk, parse, extract, stack
signatures, registry schema (both backends), ingest batches, `nils digest` with
resume. *Gate:* parse-and-compare on the production corpus — field-level agreement with
v0's extraction on 37.5M instances, with every divergence classified (v0 bug, v1
bug, or accepted change); the performance budget met on the 8-core/64 GB baseline
host (C6). Identity linkage, the pseudonym scheme with the v0 key, and the identity
rule land here (C3, C36, C37, D13). *Opened 2026-09-02* with the language decision
(D2: Rust). The spec is `docs/specs/wave1-parse-and-digest.md` in `kineuro/nils`
(pull request 2), public from its first draft because it is the engine's own
documentation and cites this record by id; what it cannot say stays in the private
record: where the gate's corpus and the two development corpora live, the baseline
host as built, and where the v0 key comes from. Order of work in the spec's §14: eight
slices, skeleton to budget, C11 beside slices 6 to 8. Slice 1, the skeleton, merged
2026-09-02 (`kineuro/nils` pull request 3; Rust 1.98.0 pinned, 02); main now
requires the engine checks and the six builds. Slice 2, the reader and the walker,
merged 2026-09-02 (pull request 4): `nils digest --dry-run` over nmosd on a development container on Asgard
sees 508,045 files, parses 504,247 and quarantines 3,798, of which 3,664 are SOP
classes outside v0's nine (secondary capture above all) and the reader's own
classes refuse exactly the spike's 134 (124 `not_dicom`, 10 `missing_uid`), with
no diagnostics; 62,300 files/s at eight workers from cache, peak RSS 851 MB. What
building the table settled is amended into the spec (§5.3, §6.1 to §6.3): six v0
keywords that never existed in the dictionary and were therefore always null (two
accepted changes come from it: `phase_encoding_direction` from its standard tag,
the PET radiopharmaceutical fields from their sequence), the creator-aware private
blocks, the fallback chain's stop rule and the empty element as null. The mix corpus arrived on 2026-09-03 and closed the slice's open
item: 196,086 files seen, 194,090 parsed and 1,996 quarantined, every one of
them classified, and the classes are one, `parse_error/truncated`; nothing was
`not_dicom`, unreadable, without a UID, or of a SOP class or modality the reader
refuses. The corpus spans sixteen manufacturers and seventy-seven models over
2001 to 2026, six character sets, three SOP classes (MR, Enhanced MR, CT) and
four transfer syntaxes among the files that parse, JPEG 2000 in three quarters
of them. The truncated files are worth their own line, because they are not a
transfer fault: they sit under one top-level directory of the corpus, their file
meta reads cleanly, their sizes run from 33 to 190 KB with no alignment to
suggest a block-level cut, the copy on the scratch is byte-identical to what the
bridge delivered (compared file by file with checksums), and v0's registry holds
an instance for every one of the 1,996, so they were whole when v0 read them and
lost their tail afterwards, where they came from. It is the same shape as the
nmosd files a later writer rewrote, and the same lesson: v0's rows describe
files that have since changed, and only v1 describes them as they are. Slice 3, the
schema and the writer, merged 2026-09-02 (pull request 5): the schema declared
once and rendered for both dialects, the registry home (`nils.toml`, the key
store, `NILS_REGISTRY`, `NILS_DSN`), the writer with its three caches and
per-field hashes, resume, jobs and `nils status`, and the corpus generator as an
example of `nils-dicom`. The synthetic million (1,011,783 files, 2,000 refused by
design) on the same development container with 64 workers from cache: identical counts on SQLite, Postgres
`COPY` and Postgres `INSERT` and equal to the manifest; 32 s, 65 s and 70 s
(31,300, 15,500 and 14,500 files/s), 2 GB peak RSS once the batch queue was
capped at sixteen (4.1 GB before), 538 MB of SQLite and 1,015 MB of Postgres for
the million; the second pass over the unchanged tree 66 s on SQLite and 37 s on
Postgres, bound by touching a million `source_file` rows. The Postgres server's
merge on one core is the wall, so `COPY` stays the default and the spec's §15
question is closed; whether the Postgres registry wants more than one writer
connection is slice 8's, if the baseline host shows the same wall. The spec's
amendments are the blocks headed "Settled while building the writer (slice 3)"
(§4.2, §5.2, §9.1, §10, §11, §12.6, §13) and the numbers in §9.2. Not in slice 3
and known: `linkage_meta` does not yet hold the registry id (slice 4 adds it with
the store's first rows), the identity rule is fixed to PatientID with the
StudyInstanceUID fallback until slice 4, SIGINT is slice 6's, and the second-pass
cost on SQLite is slice 6's number to bring down. Slice 4, identity, merged
2026-09-02 (pull request 6): the two schemes, the subkeys derived from the
registry's one key, the linkage store with its id types, identities, audit rows,
linkages and the validate-then-apply import, the rule as a YAML file
(`--identity-rule`), and the writer's per-batch resolution through the store.
What building it settled is amended into the spec as the blocks "Settled while
building identity (slice 4)" (§4.2, §7, §11, §13, with §3, §9.2 and §15): one
fallback and it is StudyInstanceUID; a pattern read for its `id` group alone; one
identity per type per subject, with `linkage link` as the only merge; imported
subjects without a digest; `registry_id` in `linkage_meta` and the refusal of a
store that belongs to another registry; three collision reasons (`identity`,
`display-code`, `batch`), each rolling the batch back, opening
`identity.collision` and failing the job with the code and the item and never an
identifier; the operating-system user as the actor until Wave 4's doors; the
schema version held at 1 through the alpha; `serde-saphyr` in place of the
archived `serde_yaml`. On the same development container: nmosd under `blake2b-8` with a throwaway key
gives 44 subjects and 82 studies in 32 s, and the same tree again with
`--restart` matches all 44 and creates none in 26 s; the million on SQLite takes
35 s against 32 s with identical counts and 212 KB of linkage store. Not in slice
4 and known: `linkage purge` comes with `custody` (slice 6), the
`ingest.quarantine` items are slice 6's, `review apply` is Wave 4's, and the
gate's check that every v0 code comes out of a v1 digest under the v0 key with
the maps imported (§12.4) is slice 7's, with the compare tool. Slice 5, stacks,
merged 2026-09-02 (pull request 7): the signature, the key, the index and the
orientation as v0 computed them, the `stack` row with the class, the confidence
and the fourteen values as read, the instance's `stack_id` set by the transaction
that files it, `stacks` in the report and the dry run, `stacks_created` in
`written`. What building it settled is amended into the spec as the blocks
"Settled while building stacks (slice 5)" (§4.2, §8, §9.1, §11, §14): the
canonical string behind the key and the pinned key of the empty signature; the
thirteen series-level columns a signature is also made of (`image_type`,
`image_orientation_patient`, the seven MR fields, the three CT fields,
`series_type` on PET) left out of `field_disagreement`, derived from the
catalogue rather than listed, because the series row holds the first instance's
value as in v0 and the stacks hold every value the series has; `orientation_oblique`
once per stack created, for a known class under 0.9, never for the unknown one;
the stack cache keyed by series id and stack key, with one keyed select per batch
on a miss. On the same development container: v0's partition of nmosd, recomputed with v0's own two
modules (`spikes/stacks/`; the modules sit beside the script and are not
committed), equals v1's on every one of the 2,165 series both hold, 2,534 stacks
against 2,534 v0 groups over 493,708 instances, and the 3,180 instances only v0
holds are the non-image SOP classes §5.3 refuses; the digest with stacks takes
32.5 s (15,600 files/s, 32 workers, 1.7 GB peak) and the same tree again 25.8 s,
creating nothing; 34 stacks are oblique and none has an unknown orientation. The mix half followed on 2026-09-03, once the
corpus landed on the scratch (196,110 files, 23 GB, moved from the exchange
inbox and compared against it file by file with checksums: no difference): the
partitions equal v0's there too, over the whole corpus once the reader was
repaired (below): 4,120 stacks against 4,120, every one matched by membership,
531 multi-stack series identical, over 196,086 instances of 2,567 series, and
every instance v1 holds is one v0 holds. Three stack rows differ, and not by order:
they are Enhanced MR series of 500 instances each where v0 stored nulls for the
MR timing, echo and coil values and v1 reads them from the frame groups, which
is what the catalogue's fallback chain is for. The stack *index* order matches
on 250 of the 513, which is a walk-order difference and no bar. The pack-format
prototype (C11) may start. Slice 6, jobs and
custody, merged 2026-09-03 (pull request 8): the cancel token every stage
holds (one signal stops, two abort), SIGINT and SIGTERM in the binary with exit
code 130, the takeover of a job whose process is gone, `reparse_from` on a
failed batch (its last second read again, which repairs a registry commit that
lost its linkage commit), the `NILS_DEBUG_STOP` test hook, `nils custody`
with its reference page, `nils quarantine list`, the `ingest.quarantine`
review items with `nils review list | show`, and `nils linkage purge`. What
building it settled is amended into the spec as the blocks "Settled while
building jobs and custody (slice 6)" (§5.3, §7.4, §10, §13, §14, with §5.2 and
§9.2): a cancelled run marks nothing gone; one review item per batch and class
with the count as evidence and never a path, filed by a cancelled run too and
never by a kept quarantine; a purge asks at a terminal, refuses without
`--yes` elsewhere, keeps the id types, the read audit and the subjects, records
itself as a `linkage-purge` job that `status` lists, and is not undone by a
digest that finds the file unchanged; custody omits nothing (C38) and redacts
a DSN password everywhere; `review apply` is Wave 4's and `doctor` waits.
The number slice 3 left it: the second pass over an unchanged tree was never
the writer's touch (two seconds of twenty-five on nmosd) but the parsers'
queue, half a million unchanged files sent through it one at a time (25 s on
32 workers, 5 s on one); the resume stage now batches them for the writer
itself, and the second pass takes 4.7 s on nmosd against 25.8 s and, for the
million, 5.4 s on SQLite and 18.2 s on Postgres against 66 s and 37 s. On the
same development container over nmosd: a run killed after 40 commits, one killed inside a
transaction, one stopped by SIGINT and one aborted each resume to the
uninterrupted run's 44 subjects, 82 studies, 2,165 series, 2,534 stacks,
493,708 instances, 508,045 file rows in the same statuses and 44 identities,
and `custody --json` reads the same counts from every home; what differs is
how many `ingest.quarantine` items a resumed tree holds (one to six), since a
run files one per class it quarantined itself. Not in slice 6 and known:
`review apply` (Wave 4), `doctor`, the mix checks (the copy), and the
Postgres second pass (18 s of round trips for a million lookups and touches)
is slice 8's on the baseline host. Slice 7, the compare tool, merged
2026-09-03 (pull request 9): `tools/v0-compare/`, Python over DuckDB, reads v0
through `export.sh`'s read-only CSV export (the session forced read-only, F1)
or a DSN and v1 through either scanner, and measures §12 the way the spec
asks: instances paired on the SOP UID and every one missing on either side
classed by what the other side knows of it, fields compared after a symmetric
normalization with the residual read back as shapes and grouped by pattern,
stack partitions matched by membership, codes and studies per code, and v1's
sessions against v0's events. What building it settled is amended into the
spec as the blocks "Settled while building the compare tool (slice 7)" and
"Settled in slice 7" (§5.2, §12.1 to §12.4, §12.7, §14): the walker's `files`
knob as a union (`dcm,no-ext` is v0's "all" mode, so both sides see the same
candidates); the adjudication file (TOML; `divergence`, `partition` and
`instance` rules with glob patterns) and the one rule the bars apply to its
classes, that a group classed `accepted` or `v0-bug` is excused from the bar it
would fail while a `v1-bug` and an unclassified group count in full, so the
99.9 percent floor is measured net of what the record has explained and the
classification bar is unchanged; the two series columns that carry the first
instance's value and follow the walk order (`media_storage_sop_instance_uid`,
`image_position_patient`) classed `accepted` by the tool itself, found when
two digests of one synthetic corpus disagreed on seven of eight series; v0's
`study.modality` against v1's `modalities_in_study` declared accepted per run;
`linkage-csv` for the import with collisions counted, and `--key-file` classing
each v0 code `key-consistent`, `cohort-hashed`, `no identifier` or `other`
without an identifier or a key ever leaving the process. The tool has a gate of
its own in CI (`v0-compare`, both backends): a v0-shaped export projected out
of a synthetic registry with injected divergences in every class, a clean
projection passing, the CSV round trip through `nils linkage import` filing
every code as already known, and the key classes right with the right key and
`other` with a wrong one; the corpus generator gained `--same-day-percent` for
the session check. v0's export (fifteen zstd CSVs, 1.26 GB, no names: nine
cohorts, 5,322 subjects, 35,198 studies, 386,488 series, 518,887 stacks,
37,531,598 instances, 142,033 events, 10,881 identifier rows) reached the host
that runs the gate through the bridge on 2026-09-03, verified, and was deleted
from the host it came from.
The line counts `export.sh` printed while copying were 22 studies and 3,497
instances above those row counts, because a value holding a newline spans
several lines of a CSV; the script says "lines" now, and the numbers above are
the extract's.

The first gate run, nmosd on a development container on Asgard, ran
2026-09-03. v1 digested the tree in
`dcm,no-ext` at 32 workers: 508,045 files in 30 s, 504,247 parsed and 3,798
quarantined, 43 subjects, 82 studies, 2,165 series (all MR), 2,534 stacks and
493,708 instances. The compare took ten seconds. Eight of the ten bars pass and
every divergence is classified; the two that fail are the two §12.4 predicted,
and they are Nima's to settle, not the engine's: four of the 82 common studies
hang off another code (two under a v1 code v0 never had, two under a code that
is another v0 subject's; one v0 subject split over two v1 subjects, one v1
subject holding two v0 subjects), which also costs the session bar its two
sessions, since a study under another code lands in another (subject, date)
group. Both were run down on 2026-09-03 and neither is the engine's.

Two of the four were the import's: the linkage CSV had been built from v0's
identifier table, which holds 43 rows where the project's own map holds 44, so
two studies carried an identifier v1 had never been told about and it minted a
code for them. Imported from the project's map instead, the studies on the same
code go from 78 to 80 of 82 and the sessions from 42 to 43 of 44. The map needed
the many-to-one import of §7.4 to load at all: it gives one person two project
identifiers, from the era before the identifiers were renamed.

The other two were the corpus, and the corpus was wrong twice over. One
session existed **twice** on disk, under two subject directories: 10,895 files
each, the same 10,894 instances, the right directory carrying the right
identifier and the other carrying its own. The digest ingested whichever copy
its walk reached first per instance, which is why the study came out under the
wrong subject and why the two copies show as 10,515 duplicates on one side and
24 on the other. The identifier the files kept from before the rename says how
it happened: the subject's old identifier ended in the digits of another
subject's number, and one export run read those digits as that other subject.
v0 has the session right because it ingested it before the second copy existed
and its resume never re-read anything. Nima settled the anatomy by eye, the
project's session list agreed, and the two subjects have different personnummer
in the project's own list, so it was never an identity collision.

A script on the archive host (`scripts/fix-nmosd-duplicate-session.py` beside
the cohort's own maps) checks that the two directories hold the same instances
and the identifiers they should, and then moves the wrong copy beside the study
root rather than deleting it. It ran on 2026-09-03, on the archive and on the
gate's copy. **With the duplicate gone the nmosd gate passes every bar**: 82 of
82 studies on the same code, 44 of 44 sessions, every field at or above its
floor, 212 of 212 multi-stack partitions identical, every divergence
classified. The first gate run of the rewrite is green, and it took a reader
fix, a writer rule, an identity rule and a corpus repair to get there.

The run used a throwaway key, so `code_classes` says `other` for all 43;
with Nima's own key it stays `other`, and that is a fact about this cohort
rather than a fault: it arrives pseudonymized, the personnummer the key was
applied to is nowhere in the data, and the codes come from the project's map.
The key still classifies the cohorts that arrive with the personnummer in
`PatientID`.

What the run turned up, and what it did not, is worth keeping. The 128,880 v0
instances v1 has no row for are the whole of the difference at the instance
level, and every one of them was checked on disk: none is under this root.
Seventeen of the 43 subjects are listed under more than one cohort, and a v0
path is relative to its own cohort's root, so these are the shared subjects'
files under the other cohorts' roots, which this run did not digest. In the
other direction there is nothing at all: every v1 instance is in v0. Per field,
26 of the catalogue's columns carry divergent rows and all of them are
explained; the rest agree exactly. Three causes account for nearly everything.
A writer rewrote part of this cohort after v0 had read it (126 series over
three subjects transcoded from JPEG 2000 to Explicit VR Little Endian and
stripped of ContentDate, NumberOfFrames and more), and v0's resume skips a SOP
UID it already holds, so v0 never re-read those files: 32,696 instances where
v0's row describes a file that no longer exists in that form, and the same
shape without the ContentDate marker in the series-level columns
(`body_part_examined` 275 series, `series_date` and `series_time` 149,
`implementation_class_uid` and `implementation_version_name` 317, three subjects'
sex and six subjects' birth date, eight studies' time). v1 fills a null column
of a series, study or subject row from the first later instance that carries a
value (§9.1) where v0 kept the null: 74, 10 and 8 series for the three Siemens
DWI columns, 43 series' date and time, six subjects' sex, two studies' time.
And where the instances of a series disagree on a series-level column, each
side keeps its first instance's value and the walk order decides which one that
is: `sequence_name` (2 to 26 spellings per series), `spacing_between_slices`
(18), `acquisition_matrix` (17, one matrix the transpose of the other),
`image_orientation_patient` in 18 series and the thirteen stack-signature
columns in the multi-stack series the tool now classes itself. Two findings are
v0's alone. Its `phase_encoding_direction` column was null on every row it ever
wrote, because v0 looked the attribute up by a keyword DICOM does not have
(`PhaseEncodingDirection`; the attribute is `InPlanePhaseEncodingDirection`,
(0018,1312)), where v1 reads it on 1,747 of the 2,165 series. And in 171 series
v0 holds an orientation whose spelling is in no file, one digit longer per
component than the file's and equal to it to 1e-15, which never split a stack
because the signature carries the derived orientation class, not the raw
cosines.

Reading the run corrected two things in the engine besides. A field two files
of one row disagree on is decided by value now, not by which file arrived
first: the row keeps the smaller in the catalogue's canonical text order, on
every column of the subject, study and series rows (§9.1, amended in slice 7).
The run had shown how often that matters, with single series carrying up to
twenty-six spellings of `sequence_name`, two acquisition matrices, two slice
spacings and two orientations, each of them a race between the walk and the
workers. `min` is the rule because it needs no bookkeeping and no provenance
column: a resumed run, a late batch and a second digest of one tree reach the
same row. The stack row is left as it was, and the mix run says why that is
enough: over 6,558 stacks of both corpora not one differs by order, since the
signature pins the values.

And the second, from Nima's account of how v0's codes were really made: the
anonymization step mapped a personnummer to a subject code with the key, wrote
a project identifier into `PatientID`, and passed v0 a CSV of identifier to
code, so the key never entered v0. That is why the key reproduces none of the
nmosd codes: the personnummer is not in this data at all, and 42 of the 43
subjects' identifiers are exactly the `PatientID` the files carry today. Two
things follow. A person may hold **many** identifiers of one type, not one: a
personnummer is not for life, since a temporary number becomes a permanent one
with residency, several temporary ones may come before that, and the permanent
one changes when a legal sex change does. v1 refused the second one; it now
files them all on the one subject and counts them, and only an identifier that
maps to two codes is refused (§7.4). And where the anonymizer writes the
subject code into `PatientID`, which is the shape v1 takes from here, the
identity rule's `code: verbatim` files it as the code and derives nothing, so
the map stays where the key is and the linkage store holds no identifying value
at all for such a cohort; it needs a pattern on every field, so a bare
personnummer is never filed as a code (§7.3).

Reading that first report also corrected the tool, in the pull request that
followed (§12.1 to §12.3, §12.7): v1's SQLite registry is read by its declared
types rather than as text, since SQLite spells a REAL with fifteen significant
digits and a float32 widened to a double needs seventeen, which had made 574
series differ on `b1rms` alone; the thirteen stack-signature columns are derived
from the catalogue the way the engine derives them, and a divergence of one of
them in a series with several stacks is grouped apart and accepted by the tool
itself, as the two `VARIES_PER_INSTANCE` columns already were; a v0 path absent
from the compared root is reported apart when its subject is in several
cohorts; and `--fs-cap` bounds the on-disk check at a million paths instead of
stopping silently at a hundred thousand, which is what left 28,880 of the
128,880 unchecked the first time. The adjudication of the run lives with it on
the host (`adjudication-nmosd.toml`), one rule per group with the probe that
settled it. The run was repeated on the engine as it now stands, with the
decided fields and the repaired reader and with Nima's own key in place: the
same eight bars pass, every divergence is still classified by the same rules,
and the two that fail are still the four studies.

Still not in slice 7: the cold-cache run on the baseline host, then the cohorts
as the migration lands their raw trees, each report summarized here in counts
and shapes; the slice closes with the last of them.

**Wave 2 — fingerprint and classify.** The columnar fingerprint pass, the pack
loader, the MRI pack carried over, evidence storage, `nils classify`. *Gate:*
diff against the 518k-stack classification cache (the v0-parity corpus, machine
output); disagreements individually adjudicated and either fixed or recorded as
intentional pack corrections (each one becomes a case in the verified corpus, C12).
Because v0 stores no pack version, the diff must separate rule changes from step-4
gap-filling drift, which depends on ingestion order (C14).

**Wave 3 — anonymize and BIDS.** Strategies, audit, `dcm2niix` orchestration,
naming/collision rules, `nils anonymize` and `nils bids`. *Gate:* BIDS validator
clean on hand-verified reference selections, with the main acquisition per session
and contrast taken from the registry (C8, D16). v0 exports are compared for
information only: they are not valid BIDS (classification-derived filenames, three
open naming bugs), so "byte-identical against v0" is not the bar.

**Wave 4 — server and contracts.** The thin server: jobs, API, semantic catalog,
AST execution, selections, review items, auth modes, MCP, events
([05-contracts.md](05-contracts.md)). The web UI rebuilt on the job/queue model.
*Gate:* the CLI and the UI drive every stage through the same doors (contract
test), and an off-the-shelf MCP client can query, select, and work a review queue.
Additions from [13](13-query-and-agent-study.md): the AST fixtures of C4
grow to the 28 gold tasks and the ten question families (C16), each with a declared
grain; the affordance endpoints and result handles are part of the contract test
(C20, C21); the MCP shape is exercised by Flue's own client and by a third-party
client through the OAuth resource metadata (C22). From
[14](14-federation.md) (C33): the federation **primitives** land here because
they are cheap now and misery later: the registry epoch and pack versions in
capabilities, `local`/`federated` visibility in the catalog, disclosure
projections on result handles, the `federation.*` review kinds, `user@node`
principals and peer-key verification (C26 to C30). No daemon yet; the contract
test proves a projection suppresses and a `local` field never validates for a
federated principal.

**Wave 5 — nils-query MVP.** Notebook, saved selections, send-to. *Gate:* a real
study's cohort defined as a selection and exported end to end without a hand-written
manifest. Addition (C16): every gold task expressed in the notebook
reproduces its v0 result hash on the migrated registry, and the ten families are
expressible without an escape hatch, roles and picks included (C19).

**Wave 6 — segment rebased.** Port nils-segment onto contracts only (07). *Gate:*
a full annotation work — subset by selection, prep via seeded pipelines, rating,
adjudication, export — with zero database-level integration. This wave is the proof
of D1 and the contracts; treat its friction as contract bugs.

**Parallel track — agent.** From Wave 4's MCP: the Ask-query Flue pilot, then agent
v1 growing alongside (08). Never on the critical path; review-item policies for
agents open only after the human workflows are trusted. The pilot is time-boxed
with its exit criteria written first (C23, 08): the ten families through the
draft-selection loop on a local and a hosted model, gold hashes reproduced, fewer
turns per resolved question than v0, no harness upgrade costing more than a day.
`nils-evals` (C25) is built before the pilot so that it can be judged.

**Wave 7 — pipelines and packs widen.** BIDS-Apps runner hardened, starter catalog
seeded at boot, GPU leasing, descriptor tunables + QC hooks (09). CT pack designed
against real photon-counting data (04). v0 decommission plan executed. Added
(C31): the SLURM and Apptainer executor, with the Amsterdam cluster as its first
target if that is what their cluster is; *gate:* the starter pipeline runs on a
selection through their scheduler with full parameter provenance and rootless.

**Wave 8 — federation** ([14](14-federation.md), D25 to D29, ratified 2026-09-02). The
node daemon, manifests and pinned peers, the federated catalog and node profiles,
fan-out and merge, the daemon's MCP server, the scope chip (06), the agreement
template. *Gate:* the Stockholm–Vienna pilot: two nodes over a mesh, the
composition and protocol questions of 13 §2 answered at both at `count` with
k=5, one request needing a human at the other end, audit read at both ends,
nothing individual-level moved. It opens only after Wave 5, because a federation
of registries nobody can query locally is a federation of nothing.

## The do-not-forget list

Standing items that must not silently drop off between waves — check at every gate:

- The **license** (decided 2026-09-02, 10): AGPL-3.0-only with an SPDX header in
  every source file, Apache-2.0 in `contracts/` and the SDKs, `CONTRIBUTING` with
  the CLA and the DCO, all from commit one (15, R6).
- **Identity linkage** lands with the registry schema (Wave 1), not "later" —
  retrofitting merge semantics is misery.
- **CT/PET readiness** is a schema and pack-router concern from Wave 1 even though
  the packs come in Wave 7 — nothing may assume MRI.
- **Small-machine CI** from Wave 1 — the budget is a gate, not a note.
- **The custody page** (C38) ships with the first store that persists anything a
  user would ask about, which is Wave 1's registry; every later store is added to
  it in the wave that creates it.
- **The one generic CSV importer** replaces the six copies when clinical imports
  port (Wave 4) — do not port the copies.
- **Absence stories** written per app as each ships (D1) and tested: a
  contracts-only deployment must render no dead links and raise no errors.
- **Corpus hygiene**: every adjudicated disagreement in Waves 1-3 becomes a fixture;
  the corpus is the moat.
- **Which instance is "first"** for a series-level column its instances disagree
  on is decided today by the walk order and by which batch commits first, as it
  was in v0, so two digests of one tree can write different rows. The nmosd gate
  run shows how often that is real: `sequence_name` in series carrying up to 26
  spellings, `spacing_between_slices`, `acquisition_matrix`, the orientation, and
  the thirteen stack-signature columns wherever a series has several stacks.
  Nothing downstream reads those columns yet. Before Wave 2's fingerprint does,
  decide whether to leave them order-dependent and say so in the catalogue, or to
  make them deterministic by a rule the writer applies (the value of the instance
  whose path sorts first, say), which costs a comparison per file and an update.
- Migration of the live registry itself (v0 Postgres → v1 schema) is Wave 4's
  hidden deliverable — spec it there, with the production data as the rehearsal.
- **Federation stays optional at every gate** (14, D25): a deployment without
  `nils node` must show no scope chip, no federation tool, no endpoint, no error.
  The absence test of D1 applies to the daemon like to any app.
- **Ask Vienna and Amsterdam early** what they run, who the controller is, and
  which kind of cluster they mean (14 §7); the answers decide whether Wave 7's
  executor and Wave 8's pilot have real targets or stay declared seams.
- Amend this folder when reality wins an argument. An outdated decision record is
  worse than none.
