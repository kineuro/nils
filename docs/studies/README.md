# The certificate studies

The plans, grades and reference reports behind the two header certificates of MRI
pack 0.23.0 and 0.25.0 (released as 1.0.0), with a summary of every evidence run
they cite. They are the public copy of studies in the private design repository
`kineuro/nils-design`, made for the NILS paper ([decision 55](../decisions/55-ready-for-production-and-the-paper.md),
K11 and test T15; ruling 15 says what the copy carries). The records they follow are
in [`../decisions/`](../decisions/README.md): [39](../decisions/39-two-ways-of-knowing-a-stack.md)
designed the certificate and [48](../decisions/48-the-reader-and-two-learners.md) set
how agents read the reference.

## What is here

| Folder | File | What it holds |
|---|---|---|
| `2026-10-01-final-certificate-read/` | `plan.md` | Certificate 1, registered before its draw: the frame, the strata, the samples, the reference read, the estimators, the pass condition, its simulated power, and every deviation with its date |
| | `grade.md` | Its grade, written before any rule answer was fetched (part 1) and filled once (part 2): pack 0.23.0 fails at 32 of 2,500 (1.28 %, upper bound 1.72 %) |
| | `reference.md` | How the reference was read: raters, adjudicators, the audit and its joint-error bounds |
| `2026-10-02-final-certificate-2/` | `plan.md` | Certificate 2, registered before its draw on a fresh frame, with the selective claim |
| | `grade.md` | Its grade: pack 0.25.0 passes at 7 of 5,000 (0.14 %, upper bound 0.26 %), each make under 3 % |
| | `reference.md` | Its reference read, audit and correction record |
| `2026-10-04-pack-0250-go/` | `grade.md` | The go read before certificate 2: 6 of 1,497 |
| `evidence/` | `<run>.json` | One summary per evidence run the files above cite (45 runs) |

## What the copy leaves out

The same rules as the decision record's copy ([`../decisions/SCRUB.md`](../decisions/SCRUB.md)):

- **The group's machines, addresses and paths.** Hosts and containers are named by role
  ("the NILS server", "the research container", "the compute server", "the development
  laptop"), paths by what they are ("the research folder", "the NILS server's working
  folder"), and ZFS snapshot names as `<snapshot>`. A command that an operator ran on
  those machines is replaced by a line saying the private record keeps it. Tool file
  names that carried a container number read `server` instead (`seal-server.sh`,
  `fetch-server-final.sh`).
- **The names of the cohorts and studies,** replaced by the same letters as in the
  decision record (study A to study M).
- **Em dashes,** by the house rule.

The counts, rates, bounds, seeds, commit and file hashes, job and run names, and every
deviation are kept as written.

## The evidence summaries

Every read, training round and grade wrote an evidence run on the group's own
machines, sealed by a manifest of every file's sha256 and mirrored. The runs hold
clinical header packets and the raters' answers, so they stay there. Each
`evidence/<run>.json` publishes:

- the run, its purpose, its study, when it was created and when it was sealed;
- the number of files and bytes, by folder;
- `manifest_sha256`, the sha256 of the private `MANIFEST.json`, so anyone given access
  to the evidence can check that it is the run these files describe;
- `sha256`: the hash of every file whose name names no stack (code, prompts, logs,
  aggregates, counts);
- `per_stack_files_by_folder`: the count of files named by a registry stack id
  (packets, rater answers, adjudications), which this copy does not list one by one.

File names that carry a host name are renamed in the summaries, with their hashes
unchanged: a container number reads `server`, the previous prototype's host reads `v0`
(`v0-campaign.keys`, `v0keys.tsv`), and the development laptop's name reads `laptop`.

The protocol documents the reference used and the stock of the archive stay private
(ruling 15).
