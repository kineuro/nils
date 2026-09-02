# Changelog

All notable changes to NILS v1 are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The first release will be 1.0.0; until then pre-releases are tagged `v1.0.0-alpha.N`. The history of v0, the 0.x line, is in [kineuro/nils-legacy](https://github.com/kineuro/nils-legacy).

## [Unreleased]

### Added

- The repository: licenses, contributor terms (CLA and DCO), security and trademark policies, templates and CI, as the design record decided (`docs/decisions/`, R1 to R8).
- The engine skeleton (Wave 1 spec, §14, slice 1): the `engine/` Cargo workspace with `nils-dicom`, `nils-registry`, `nils-digest` and the `nils` binary, on Rust 1.98.0 pinned; CI formats, lints and tests on SQLite and Postgres 16 and builds `nils` on the six release targets; a tag now attaches the six binaries and their checksums to the release. The synthetic corpus generator moved from the spike to `tools/synth/`.
