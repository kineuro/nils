# Changelog

All notable changes to NILS v1 are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The first release will be 1.0.0; until then pre-releases are tagged `v1.0.0-alpha.N`. The history of v0, the 0.x line, is in [kineuro/nils-legacy](https://github.com/kineuro/nils-legacy).

## [Unreleased]

### Added

- The repository: licenses, contributor terms (CLA and DCO), security and trademark policies, templates and CI, as the design record decided (`docs/decisions/`, R1 to R8).
- The engine skeleton (Wave 1 spec, §14, slice 1): the `engine/` Cargo workspace with `nils-dicom`, `nils-registry`, `nils-digest` and the `nils` binary, on Rust 1.98.0 pinned; CI formats, lints and tests on SQLite and Postgres 16 and builds `nils` on the six release targets; a tag now attaches the six binaries and their checksums to the release. The synthetic corpus generator moved from the spike to `tools/synth/`.
- The reader and the walker (Wave 1 spec, §14, slice 2). `nils-dicom` reads Part 10 files with or without the preamble and bare implicit or explicit VR data sets, stops before Pixel Data, carries the field catalogue (v0's 171 columns with their sources, fallbacks, converters and sensitivity classes, rendered to `docs/reference/catalogue.md`), the Enhanced functional-group and private per-frame fallbacks, the six DWI private values by creator block, the character set handling, the seven quarantine classes and the diagnostics. `nils-digest` walks a root with a pool of listing threads (symbolic links skipped, an unlistable directory a `walk_error`, an unlistable root the job's failure), filters names with the `files` knob and feeds a pool of parsers. `nils digest <root> --dry-run` reads everything and prints the report (counts per quarantine class and diagnostic kind, studies, series, subjects, modalities, SOP classes, transfer syntaxes, character sets, rate, peak RSS); `--describe` prints the knobs; `--json` for both. Over the nmosd corpus (508,045 files) it refuses the 134 files the spike refused, at 62,300 files/s on eight workers.
