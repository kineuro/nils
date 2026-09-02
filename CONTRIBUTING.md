# Contributing

NILS v1 is pre-alpha and developed in the open. Issues, questions and pull requests are welcome from the first commit; what follows is how they move.

## Before you start

- Read the design record in [`docs/decisions/`](docs/). Every decision has an id (`D5`, `C18`); a change that touches one cites it, and a change that contradicts one amends the record in the same pull request.
- Open an issue before a large change, so the direction is agreed before the code exists.
- Never put patient data in an issue, a pull request, a test or a fixture: no names, no identifiers, no UIDs, no folder paths from a clinical system. Counts and shapes are fine.

## How work flows

- `main` is protected. Code lands by pull request with a green CI run, rebased onto `main` (linear history, no merge commits). Maintainers may commit directly to `docs/` so that the decision record stays cheap to amend.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/): `feat(registry): ingest batches (D4)`, `fix(walker): quarantine unreadable files (D17)`, `docs: amend D13`. Cite the decision id when there is one.
- Every source file starts with an SPDX header:

  ```text
  // SPDX-License-Identifier: AGPL-3.0-only
  ```

  under `contracts/` and `sdk/`:

  ```text
  // SPDX-License-Identifier: Apache-2.0
  ```

  CI checks both (`scripts/check-spdx.sh`).
- Releases are tags: `v1.0.0-alpha.N` until the first release, `v1.0.0` after. A tag builds the binaries and publishes the matching section of [`CHANGELOG.md`](CHANGELOG.md) as the release notes, so every pull request that a user would notice adds a line under `Unreleased`.
- The engine is Rust, on the toolchain pinned in `engine/rust-toolchain.toml`. CI formats (`cargo fmt --all --check`), lints (`cargo clippy --workspace --all-targets -- -D warnings`), tests on SQLite and Postgres 16, and builds `nils` on the six release targets; `engine/README.md` says how to run the same checks before pushing. Text hygiene, SPDX headers and sign-offs are checked on every file.

## Licensing your contribution

The repository carries two licenses, and each part has its own agreement.

### The engine: CLA

Everything outside `contracts/` and `sdk/` is [AGPL-3.0-only](LICENSE). A contribution to it needs the [NILS Contributor License Agreement](CLA.md), signed once. On your first pull request a bot asks for it; sign by posting this comment on the pull request:

```text
I have read the CLA Document and I hereby sign the CLA
```

The signature is recorded in the `cla-signatures` branch and covers every later contribution. Why a CLA on an AGPL project is explained in [CLA.md](CLA.md).

### The contracts: DCO

`contracts/` and `sdk/` are [Apache-2.0](contracts/LICENSE), and a contribution to them needs no agreement, only the [Developer Certificate of Origin](https://developercertificate.org/): sign off each commit that touches them.

```text
git commit -s
```

CI rejects a commit that touches `contracts/` or `sdk/` without a `Signed-off-by` line (`scripts/check-dco.sh`). A pull request that touches only the contracts is not asked for the CLA.

## Security

Vulnerabilities go to the channel in [SECURITY.md](SECURITY.md), never to a public issue.

## Conduct

Be decent; argue about the work, not the person. The maintainers close what falls short of that.
