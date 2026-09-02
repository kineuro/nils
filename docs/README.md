# Documentation

Everything under `docs/` is licensed [CC BY 4.0](LICENSE).

`decisions/` is the design record of NILS v1: what is being built, why, and what was decided against, one numbered document per subject, with a register of every decision (`D1`, `D2`, ...) and every amendment (`C1`, `C2`, ...) that changed one. The record is amended the same day reality wins, and the code cites it. It is the public copy of a private record; what was removed, and why, is listed in `decisions/SCRUB.md`.

`specs/` holds one specification per wave of the build, written before the wave's code and amended while it is built: what the wave delivers, the schema and the commands, the order of work, and the gate it closes at. A spec cites the record by decision id and never repeats its reasons. `specs/wave1-parse-and-digest.md` is the first.

`reference/` holds pages rendered from the code and checked against it by tests, so that they cannot drift: `reference/catalogue.md` is the field catalogue, every column the digest writes with its source, converter and sensitivity class.
