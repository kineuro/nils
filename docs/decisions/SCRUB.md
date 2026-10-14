# What this copy leaves out

This directory is a copy of the private record `kineuro/nils-design`, made on
2026-09-02 under R7 of [15](15-ratification.md) §5, and widened on 2026-10-13 to the
records the NILS paper cites (26, 34, 36, 39, 40, 41, 47, 48, 50, 53, 54, 55 and 56;
[55](55-ready-for-production-and-the-paper.md) K11, test T15). R7 asks for a scrub pass with
a written list; this is the list. The pass is a script in the private repository
that asserts the count of every substitution it makes, so a refresh after an
amendment fails instead of leaking, and it ends with a check that none of the
removed terms remain in any file.

Nothing removed changes a decision, a verdict or an id. Where a sentence lost a
fact, the sentence says so ("its size is not part of this copy") rather than
pretending the fact was never there.

## Removed

| What | Where | In this copy |
|---|---|---|
| The production host's name and size (cores, memory), and the ratio between it and the baseline host | 00 principles 5 and 10, 02 (the budget), 12 §2 Host row, 12 §3 correction 1, 12 §4 D6, and every "on <host>" in 04, 09, 10, 11, 12, 13 and 15 | "the production host", "a large shared server", "many times that size", "a much larger machine" |
| The storage pool's name and the layout of the raw DICOM path | 12 §4 D6, 12 §5 D17, 15 §2 C6, 15 §4 | "the storage server", "the raw DICOM dataset of each study" |
| The two cohort names in the origin-hole example | 12 §2 Origin hole row, 12 §4 D4 | "one cohort", "another cohort", "the origin hole" |
| The row counting identifying fields in the live registry, where they are served from, and the default auth mode | 12 §2 | removed; D13 states the requirement instead |
| How many API tokens v0 has issued | 12 §2 Identity row, 12 §4 D8 | removed |
| What v0's token verifier accepts | 12 §4 D1 | "more lenient than a verifier should be" |
| D13's premise: the live registry's identifying fields with their shares, where the anonymization workbook is written and how it is protected (with file and line references), the digest truncation, the UID default, the transformations v0 lacks, the auth default | 12 §5 D13 | one paragraph that says v0 was built for one trusted host; the requirement list that follows is unchanged |
| The share of subjects with a birth date | 15 §8 C35 | "nearly every subject" |
| Where the Wave 1 gate's corpus and the two development corpora live, the baseline host as built, and where the v0 pseudonym key is kept | 11 (Wave 1, opened 2026-09-02) | one sentence that says these stay in the private record |
| The reason the record is private, stated as what it exposes | 10 (repositories), 15 §5 R7 | restated as the operators' detail |
| Links to rendered pages in a private workspace, and the tooling note | README | removed |
| The group's deployment of Wave 5: the host, the guest, the mounts and pools, the identity provider's application and group bindings, the portal entry, the deploy path, and the places table of that deployment | 20 §6 | one paragraph that says these stay in the private record; the roles and rules are the specification's |
| Where the viewer study's corpus lives and which workstations run it | 20 §7 | "the group's own largest scans, on the group's own machines" |
| The paths of the checkouts read, the site's stylesheet path, the pool named in Q22, and the group's chat channel by name | 20 §2, §3, §5, §10 | "the Metabase checkout", "the 0.5.0 checkout", "the group's site stylesheet", "the group's data", "the group's chat channel" |
| The group's machines in the cited records (26 to 56): the production host, the storage and compute servers, the laptops, the containers by number, their addresses, ports, users, paths and update commands, the backup NAS, and the public names of the group's own services | every cited record | by role: "the production host" (v0's), "the NILS server" (the group's install), "the research container", "the model server", "the development laptop", "the compute server", "the storage server", "the source pool", "the backup NAS"; a paragraph that was only deployment detail says the private record keeps it (55 B1 "What the NILS server is today", 40's update and clean-up steps, 54 step 4) |
| The names of the cohorts and studies | 09, 11, 17, 20, 26, 34, 39, 40, 48, 50, 54, 55, and the specifications and spike notes outside this directory | a letter per study, the same letter in every record (study A to study M; the multicentre study's site in the protocol list as site B1; the photon-counting CT study by its two sites). The key is in the private scrub script. Other sites of the multicentre study keep their country codes (DE1, IT3), which name a site, not a person. File names that carried a cohort's name became generic (11's script and adjudication file). A disease name that means the disease stays (15's "ALS onsets"); the abbreviation of photon-counting CT is spelled out |
| 54's survey per study: the PatientID shapes and counts of each identified and de-identified tree, the registry's subjects per source tree, the inventory of everything that carried a code, and the step-5 check's study-level detail (folder and series names, the code file's history) | 54 | a summary in counts: four studies carry the personnummer in their identified tree, six carry it nowhere; the check's result (388 of 388 by map import, 10 of 10 by the pseudonymiser, under a lab key) is kept |
| How v0 protected its pseudonym map | 26 | "how that map was protected is not part of this copy", as in 16 |
| The teacher of the writing course | 55 | "a course on scientific English". Russell Ouellette and Adithya Raju Ganeshan keep their names (55, ruling 15); the readers stay R, D and N |
| The operators' private notes and procedures by name (memory notes, skills, the hub's map and rules), the evidence archive's paths, and links to rendered pages in a private workspace | 47, 48, 50, 54, 55, 56 | "our operating notes", "our build procedure", "the operators' map", "the evidence archive"; links removed |
| A test's synthetic value written in the shape of a personal number | 53 | described, not written |
| The compute and storage servers by name in the records published before the paper | 00, 11, 12, 15, 19, 20 | "the compute server", "the storage server", from 2026-10-13 (55, ruling 15); a coordination server named in 14 became "an open-source coordination server" |
| Em dashes | every record | the house's punctuation: a heading's dash became a middle dot, a table's lone dash "none", a pair of dashes in one sentence parentheses, a single dash a colon or a comma |
| Links to records that are not in this copy | every record that cites one | the link's text stays, its target is dropped; studies (`studies/...`) and reports are paths in the private repository and are cited as such |

The README was also adapted to its new place: links point into this directory,
and the "private by design" paragraph became the pointer to this file.

## Kept on purpose

- Counts and shapes from the live databases (12 §2, 13 §1): instances, stacks,
  subjects, cohorts, review flags, jobs, threads. They are the evidence behind the
  decisions and they identify nobody.
- The group's partners (Vienna, Amsterdam), the names of the private repositories, and the v0 commit that code
  references point to.
- The two development corpora by what they are, study D's corpus (by its letter, as
  every cohort is from 2026-10-13, decision 55 ruling 15) and the mix sample, with
  their file, series and subject counts and the results of running the engine over
  them (11, Wave 1, from slice 2 on). Where the corpora live is still removed.
- Facts about v0's code: its schema, its stages, where it mounts the Docker socket,
  what its agent's connection string looks like. v0's code is public in
  `kineuro/nils-legacy`; describing it discloses nothing new.
- Every decision, challenge, amendment and verdict, verbatim.
- In the cited records: Nima's own words, scrubbed of the above; the counts, rates,
  bounds and evidence-run names behind every certificate; the release and pull
  request numbers; the commit and manifest hashes; the names of the models used as
  raters and judges; the sign-in provider by name (Authentik, as in 00); the
  group's data bridge (Bifrost, a public repository); and the paths of the studies
  in the private repository, which say where the evidence is kept, not what it
  holds.

## The corpus review (C10)

C10 ([12](12-review-devils-advocate.md) §6, accepted in [15](15-ratification.md) §7)
says: the live-derived corpus stays on the production host; the public corpus is
synthetic or transformed, and the transformation is reviewed once, in writing,
before the first public commit. This is that review, for commit one and for this
copy of the record.

- **At commit one the repository holds no corpus and no fixture.** `packs/mri/` and
  `evals/` are README files; `spikes/lang/` states a question. Nothing derived from
  the live registry, its classification cache, the agent's transcripts or any DICOM
  file is in this repository.
- **This copy of the record was checked against the same rule.** No subject code,
  study identifier, UID, patient-level value, path on the storage server or free-text
  field from the live databases appears in it. The check is part of the scrub
  script: one pattern per class, run over every file, failing the pass on a hit.
- **The rule for what comes next.** Before the first fixture lands (the v0-parity
  corpus of C12, the spike's list of failing vendor files, an evals task set), the
  transformation that produced it is written down in `docs/corpus-review.md`: the
  input, the function applied to every free-text and identifier field, what was
  dropped, and who read the result. The pull request that adds the fixture links to
  that file, and the pull request template asks for it. The live-derived corpus is
  never a fixture here; it is a harness that runs on the production host and
  reports pass or fail (10).
