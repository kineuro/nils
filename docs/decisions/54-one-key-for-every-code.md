# 54 · One key for every subject code

> **Note (2026-10-08):** the function this record called KI_ID_GEN is named **the subject code generator** everywhere (record 55 B2, Nima's ruling). The name is changed below; the function, the key and the codes are not.

Opened 2026-10-06. Nima ruled the same day that NILS builds subject codes with the group's one key and one function everywhere: the subject code generator, the keyed 8-byte BLAKE2b of the 12-digit personnummer with the group's registry key, written as 16 hex characters. The ruling is in the group's data rules. This record carries it into the engine, says where each place's personnummer comes from, plans the production registry's move, and checks the function against a known truth.

**The answer, in short:**
- **One pull request, kineuro/nils#369** (branch `ki-id-gen`, head db101f6, every check passed on 2026-10-06). It is open and waits for review: no merge, no tag, no release.
  - S1: `nils setup` and `nils init` make `blake2b-8` registries by default, and setup takes the key from the group's key file (`--reg-key-file`).
  - S2: a personnummer is coded as its twelve digits.
- **Two engine facts made this small.** The engine already had `blake2b-8`, which is the subject code generator byte for byte (`pseudonym.rs`, pinned by a test). The `key add` path stores the key's bytes as given. So the engine only needed a default, a way to hand it the key file, and one normal form for the number.
- **The check against a known truth, without the key: done.** On one study whose code file is known, NILS's own code path, run in a lab registry under a lab key, gives the subject code generator's code for every person, by both routes. The keyed half is Nima's to run.
- **The production registry is not touched.** Its migration is planned below and waits for Nima's go. All of its 5,895 subject codes are `blake2b-32` codes, so every one of them changes.

## The ruling and what it means for the engine

generator(pn) = `hashlib.blake2b(pn.encode(), key=REG_KEY.encode(), digest_size=8).hexdigest()`, with pn the twelve digits.

| Piece | Before | Now (kineuro/nils#369) |
|---|---|---|
| The function | `blake2b-8` existed and equals the subject code generator (keyed BLAKE2b, 8-byte digest, hex) | unchanged |
| A new registry's scheme | `blake2b-32`, hardcoded in setup; `nils init --scheme` defaulted to it | `blake2b-8` by default in both; `--scheme blake2b-32` still works |
| The key | a passphrase typed, generated or read from a file | the same, or `--reg-key-file FILE`: `REG_KEY=<value>` or the value alone, mode 600 required, refused before anything is written when wrong, never shown, only its fingerprint |
| The identifier hashed | the identity rule's value exactly as read | under id type `personnummer`, the twelve digits; every other type as read |
| An existing registry | its scheme is fixed at init | still fixed; setup refuses a rerun that names another scheme |

## Rulings I propose

| | Proposal |
|---|---|
| R1 | **Default for every new install is `blake2b-8`.** With the group's key file it is the subject code generator. An install with no key file (a site outside the group, a laptop, a test) still gets `blake2b-8`, keyed by its own passphrase. Its codes are then 16 hex characters that are not the group's codes, which is right: they are not the group's people |
| R2 | **A personnummer is what the id type says, not what the value looks like.** Only identifiers filed under the id type named `personnummer` are normalised. I rejected a shape check alone: a 10 or 12 digit value with a valid check digit is common among study and registry ids (one in ten random ten-digit numbers passes Luhn), and guessing would give a person two codes. So a place whose PatientID holds a personnummer declares an identity rule of type `personnummer` |
| R3 | **One written form: twelve digits.** Spaces and the one separator (`-` or `+`) are dropped. A ten digit number gets its century: the latest year ending in its two digits that is not after this year, a hundred years earlier when that day is still to come this year, and a hundred years earlier again when the separator is `+`. A coordination number keeps day plus sixty as written. A twelve digit number is never changed except for its separator |
| R4 | **A value of that type that is not a personnummer is refused, not hashed as read.** Hashing it as read would make a code that looks like the person's and is not. In the identity rule it does not answer: the rule goes to its next source and records an `identity_unparsed` diagnostic (shape only), and the study UID fallback stands for it when nothing else answers. In a map import the row is a conflict that names the column and never the value. "Not a personnummer" means another shape, a date that is no date (month 13, day 0, 29 February outside a leap year, coordination day past the month's end) or a wrong check digit |
| R5 | **A lookup of a personnummer is a lookup of its twelve digits**, however the caller wrote it. A number typed into a selection or a map finds the identity the digest filed |

R3's century rule is reckoned against today, as the tax agency's own rule is. Over the identified trees I surveyed, every personnummer is already written as twelve digits, so the rule decides nothing there; it matters for maps typed by people.

## What PatientID holds, place by place

I read the first readable file of up to 300 folders of each identified and de-identified tree under the group's source studies (fifteen trees of eleven studies), as the research account, and counted shapes (digits as D, letters as A). Values were never printed. The table per study is kept in the private record.

What this says:
- **Four studies carry the personnummer**, in the identified tree only. One value of 256 read in one of them is twelve digits that are not a valid personnummer. Under R4 it would be refused, and someone must look at it before that study is coded.
- **Every tree NILS reads now holds a site or study id, never the personnummer.**
- **Six studies have no personnummer anywhere in their DICOM.** Their codes can come from the personnummer only through a map that someone holding the numbers provides.

## Where the registry gets the personnummer (step 3)

The standing ruling (record 26): the registry never points at an identified file. Identified data sits in `derivatives/dcm-original`, read only by the pseudonymiser, which writes `derivatives/dcm-anon`, the only tree NILS digests.

| Route | How the code is made | Fits the ruling | Needs | Covers |
|---|---|---|---|---|
| **A. The pseudonymiser** | The dataset is declared `arrives identified` with an identity rule of type `personnummer` on PatientID. The pseudonymiser reads `dcm-original`, derives the subject code generator of the twelve digits, and writes `dcm-anon` with that code in PatientID. The digest then reads `dcm-anon` with the verbatim rule | yes: this is the design's own route, and the pseudonymiser is the one reader of the originals | the key in the registry; a writable place for `dcm-anon` | the four studies with the number in `dcm-original` |
| **B. A map import** | A map of the site id (identifier column) to the personnummer (`canonical:personnummer`) is imported; each subject's code derives from the number, and the files in `dcm-raw` or `dcm-anon` are found by their site id | yes: the registry reads only de-identified files, and the numbers are sealed in the linkage store under the registry's key, as every identifier already is | a map per study, made where the numbers are (research account, mode 600) and deleted after import | any study, including the six without the number in DICOM, once a map exists |
| C. A code map, made outside NILS | site id to the subject code generator code, computed by a script with the key and imported as a `code` column | yes, and no personnummer ever enters NILS | the key outside NILS, and the code trusted as given | any study with a map |

**I recommend A for the four studies whose identified tree carries the number, and B for the rest.**
- A reads the person straight from the file, so a misfiled session (study H has several under the wrong folder) is coded by who it is, not by where it sits. That makes A the stronger route wherever it is possible.
- B is the only route for studies without the number in DICOM. Its map is only as good as its pairing, and the study H file shows how a map can be wrong: the old code file paired 318 of 388 persons with the wrong study H id.
- C keeps the number out of NILS altogether, but it gives up the linkage store's one guarantee: that a code is derived from an identifier the registry holds and can show. I would keep it for a site that will not hand over numbers.

Nothing in A or B needs a registry to point at an identified file, so I did not stop for a question here.

I proved both A and B on the real study H numbers in a lab registry, under a lab key (step 5 below).

## The production registry's move (step 4, plan only)

Two ways were weighed: re-keying the registry in place (every stack id kept, the old code filed as an alias) or a new registry under the group's key (the archive read again, every id-keyed artefact mapped by DICOM UIDs). I recommended the re-key. Record 55 B1 replaced this step: production starts from an empty registry with the group's key. The inventory of what carried a code is kept in the private record.

## Step 5: the check against a known truth

The known truth: one study's corrected code file (its own id, the personnummer and the subject code), held by the research account only. I never printed it or copied it out. Every number below is a count.

- **Without the key (done).** Comparing persons by an unkeyed hash of the twelve digits, the file's 388 persons are the 388 persons in the study's identified tree, as sets. 13,818 of 13,898 series folders sit under that person's own folder; the rest are misfiled sessions the study's own check had found. One person appears only in a later series folder of a session, so any per-session reading must read every series folder, not one file per session.
- **NILS's own code path (done, under a lab key).** Route B, a map import of the file: 388 rows, 388 subjects, 0 conflicts, and the NILS code equals the subject code generator of that row's personnummer under the same key for **388 of 388**. Route A, the pseudonymiser over 10 sessions of the identified tree: 16,604 files written, 9 refused (no DICOM), 0 held, and the codes written equal the generator's for the 10 persons, as sets. The lab registries, the copied sessions and the lab key were removed afterwards.
- **With the key (not run here).** The same check with the group's key is one command for Nima, from the machine that holds the key. The permission system refused my copy of the key, and per the wave's rule I did not try another way.

**Evidence:** an evidence run in the archive (scripts, counts as logs, the engine binary's hash and commit).

## Decisions for Nima

| | Decision | My recommendation |
|---|---|---|
| D1 | `blake2b-8` as every new install's default, keyed by a passphrase where no key file is given (R1) | yes |
| D2 | A personnummer is the id type `personnummer`, never a shape guess; an invalid one is refused, not hashed (R2, R4) | yes |
| D3 | The route per study: A (pseudonymiser) for the four studies whose identified tree carries the number; B (map) for the others | yes; and look at the one value that is no valid personnummer first |
| D4 | Under a `personnummer` rule and the group's key, should a subject coded from the number still be marked provisional? Record 26 marks a subject coded without a map as provisional. Under the subject code generator the number is the map | no longer provisional; that is a small engine change I left out of #369 until you rule |
| D5 | Subjects of the six studies with no personnummer in DICOM: wait for a map per study, or keep them out of the new codes | wait for maps; nothing codes them from a site id under the group's key |
| D6 | The production registry: M1, re-key in place (a new slice), or M2, a new registry | M1, after pack 1.0.1's fresh read is sealed and graded |
| D7 | Where route A writes `dcm-anon` for the four studies. The identified trees sit in the source pool, which data enters only through the group's data bridge | ask first whether the pseudonymiser may write `derivatives/dcm-anon` beside `dcm-original` in the source pool, or whether `dcm-anon` goes to a working place |
| D8 | Where the group's key file lives on the production guest for `--reg-key-file` and for the re-key | a root-only file listed in the operators' map, removed after setup if the key store keeps its own copy |
| D9 | The keyed half of step 5 | run `keyed-run.sh` |
| D10 | Review and merge kineuro/nils#369 | after CI passes and your read of the diff |

## Note, 2026-10-07 (Nima)

The generator is not to be called the subject code generator. It is the **subject code generator**, and NILS is not only for one institution: every site uses its own constant key, and ours is the group's. The engine already takes a key file per install, so this changes names, not behaviour. The rename (record, pull request, flags and docs) is done when this work resumes in the next wave, together with the production move.

## Amended by record 55 (2026-10-08)

- **The name** is the subject code generator, never the subject code generator, in this record, the pull request and the group's rules; every site uses its own constant key, the group's being its registry key file (Nima, 2026-10-07).
- **Step 4 is replaced.** The NILS server's registry is test and development and is not re-keyed; production starts from an empty registry with the group's key ([55](55-ready-for-production-and-the-paper.md) B1).
- **The PatientID choice** joins the slice: the pseudonymiser writes the subject code or a chosen ID type, by scenario (55 B2). The open rulings D1 to D10 above become the defaults of B2's slice in the Wave 7a spec and are confirmed with Nima when the slice is built.
