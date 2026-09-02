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
