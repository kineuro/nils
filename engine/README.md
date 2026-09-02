# The engine

One Cargo workspace, four crates, one binary. What each crate holds is set out in the Wave 1 specification ([`docs/specs/wave1-parse-and-digest.md`](../docs/specs/wave1-parse-and-digest.md), §3) and the order they fill up in is its §14.

| crate | holds |
|---|---|
| [`crates/nils-dicom`](crates/nils-dicom) | the reader: open, stop before Pixel Data, the field catalogue, normalization, the refusal classes |
| [`crates/nils-registry`](crates/nils-registry) | the schema declared once, the SQLite and Postgres dialects, migrations, the linkage store, the key store |
| [`crates/nils-digest`](crates/nils-digest) | the walker, the pipeline, identity resolution, stack signatures, the writer, jobs, resume |
| [`crates/nils`](crates/nils) | the binary: the command line, configuration, `custody`, output formatting |

## Building

The toolchain is pinned in [`rust-toolchain.toml`](rust-toolchain.toml); `rustup` installs it on first use, and CI builds with the same file. `Cargo.lock` is committed and CI builds with `--locked`.

```sh
cd engine
cargo build --release
./target/release/nils --version
```

## Running

What the binary does so far (`custody`, `quarantine`, `review` and `linkage purge` land with slice 6):

```sh
nils digest <root> --dry-run              # walk, read every header, print the report; no registry needed
nils digest <root> --dry-run --json       # the report as one JSON document, progress as JSON lines
nils digest <root> --describe             # the effective knobs

mkdir myreg && cd myreg
nils key add k < key.txt                  # the pseudonym key, kept in keys/ and named, never printed
nils init --key k                         # a SQLite registry in the working directory
nils init --key k --backend postgres --dsn postgres://nils:secret@db/nils --schema nils
nils digest <root>                        # digest; the same command again resumes
nils digest <root> --workers 8 --walk-threads 8 --batch-rows 2000 --files dcm --name my-batch
nils status                               # the registry, the running jobs, the last batches
nils status --batch 3 --json              # one batch's counts

nils digest <root> --identity-rule rule.yaml   # which field names the person (spec §7.3); PatientID by default
nils linkage import codes.csv             # legacy codes: a header row, columns identifier and code (--id-column, --code-column)
nils linkage id-type add personal-number --description "the national number"
nils linkage id-type list
nils linkage show <code> --why "the audit reason"   # decrypts the subject's identifiers; writes read_audit
nils linkage link <code-a> <code-b> --evidence "the same person, renamed"
nils linkage unlink <id>
```

Every instance is filed under a stack of its series: v0's signature (spec §8), computed from the file alone, keyed, indexed in order of first appearance, with the orientation class and its confidence on the `stack` row. The report counts `stacks` beside `studies`, `series` and `subjects`, and a stack whose orientation is known but oblique (confidence under 0.9) counts `orientation_oblique` once.

A registry names one pseudonym key and one scheme at `init`: `blake2b-32` by default, or `--scheme blake2b-8` to continue a v0 registry with its key, so that every known person lands on the known code. The identifiers themselves live only in the linkage store, encrypted under a subkey of the registry's key; a subject holds one identifier per type, and two subjects that are one person are joined with `linkage link`. An identity collision (two identifiers on one code) rolls its batch back, opens a review item and fails the job with the code and the item, never an identifier.

A registry is a home directory: `nils.toml`, the key store and, on SQLite, `registry.db` and `linkage.db`. `--registry <dir>` names it for any command, else `NILS_REGISTRY`, else the working directory. On Postgres the two stores are the schemas `<schema>` and `<schema>_linkage`; `NILS_DSN` overrides the DSN in `nils.toml`, and `NILS_PG_BULK=insert` swaps the `COPY` bulk path for multi-row inserts. Exit codes: 0 done, 1 the command failed, 2 the arguments or the configuration are wrong, 3 another job holds the registry.

The report names nothing from inside a file but SOP class and transfer syntax UIDs, modality and character set codes, tag keywords and the reader's error texts; diagnostic samples are shapes (`series_mr.echo_time=9a`), never values. Progress goes to stderr every ten seconds. The field catalogue is [`docs/reference/catalogue.md`](../docs/reference/catalogue.md), rendered from `nils-dicom` by `cargo run -p nils-dicom --example catalogue -- --write` and checked by a test.

A synthetic corpus for trying it: `cargo run --release -p nils-dicom --example corpus -- --out /tmp/synth --instances 20000 --seed 1 > manifest.json`, described in [`tools/synth/README.md`](../tools/synth/README.md); the manifest's counts are what a digest of the tree must report.

## Checks

CI runs these on every pull request, in this order, and main requires them (`.github/workflows/ci.yml`). Run them before pushing:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The tests that need Postgres read `NILS_TEST_POSTGRES_DSN`; without it they say so and pass, so the suite is green on a laptop without a server. CI provides a Postgres 16 service and sets the variable. To run them locally:

```sh
docker run -d --rm --name nils-pg -e POSTGRES_USER=nils -e POSTGRES_PASSWORD=nils -e POSTGRES_DB=nils_test -p 127.0.0.1:5432:5432 postgres:16
NILS_TEST_POSTGRES_DSN=postgres://nils:nils@127.0.0.1:5432/nils_test cargo test --workspace --locked
docker stop nils-pg
```

## Targets and releases

`.github/workflows/engine-build.yml` builds `nils` natively on the six release targets (Linux, macOS and Windows, x86-64 and arm64) and runs `nils --version` on each; `ci.yml` calls it on every pull request and `release.yml` on every `v*` tag, which attaches the six binaries, `SHA256SUMS` and `VERSION` to the GitHub release. A tag's version must be the one in [`Cargo.toml`](Cargo.toml); the release job checks.

## Rules

- Every source file starts with `// SPDX-License-Identifier: AGPL-3.0-only`; `scripts/check-spdx.sh` checks.
- Rust 2024 edition; `rustfmt` with the defaults; `clippy` with warnings as errors. `unsafe` needs an explicit `#[allow(unsafe_code)]` with its reason beside it.
- A new Rust release is adopted by raising `rust-toolchain.toml` and `rust-version` together, in a pull request of its own.
- Dependencies are declared once in the workspace `Cargo.toml`, when the slice that uses them lands.
- Nothing here assumes MRI, and a stage's input is a predicate over columns, never a list carried in memory (design record, 02).
