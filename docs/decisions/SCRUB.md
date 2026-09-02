# What this copy leaves out

This directory is a copy of the private record `kineuro/nils-design`, made on
2026-09-02 under R7 of [15](15-ratification.md) §5. R7 asks for a scrub pass with
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
| The reason the record is private, stated as what it exposes | 10 (repositories), 15 §5 R7 | restated as the operators' detail |
| Links to rendered pages in a private workspace, and the tooling note | README | removed |

The README was also adapted to its new place: links point into this directory,
and the "private by design" paragraph became the pointer to this file.

## Kept on purpose

- Counts and shapes from the live databases (12 §2, 13 §1): instances, stacks,
  subjects, cohorts, review flags, jobs, threads. They are the evidence behind the
  decisions and they identify nobody.
- Hardware the group has published elsewhere (Asgard), the group's partners (Vienna,
  Amsterdam), the names of the private repositories, and the v0 commit that code
  references point to.
- Facts about v0's code: its schema, its stages, where it mounts the Docker socket,
  what its agent's connection string looks like. v0's code is public in
  `kineuro/nils-legacy`; describing it discloses nothing new.
- Every decision, challenge, amendment and verdict, verbatim.

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
