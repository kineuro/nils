<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# The gate

The gate of Wave 4b (`docs/specs/wave4b-the-ask.md`, section 13): the
questions the engine must answer, the rows it must answer them with, and
the questions it must refuse to pretend it can answer.

Everything here is data. `gate.yml` names each fixture, the family it
belongs to, the file that holds its document and the outcome it is allowed
to have; `fixtures/` holds the documents; `expect/` holds what each one
returned when the canonicals were taken. Every row in `expect/` is rendered
by the engine's own renderer, which is the declared normalisation: integers
as digits, doubles at nine decimals with trailing zeros trimmed, dates as
text, subject codes digested, in the answer's own order.

## Running it

The gate runs on a registry the generator writes, never on an archive:

```sh
nils --registry /tmp/gate key add k
nils --registry /tmp/gate init --key k
nils --registry /tmp/gate synth --seed 11 --subjects 48
nils --registry /tmp/gate session rebuild
nils --registry /tmp/gate ask gate --pack-dir packs
```

For Postgres, `init --backend postgres --schema gate` with `NILS_DSN` set.
Continuous integration does both on every pull request, and the canonicals
are one file for each backend: the two agreeing is each run agreeing with
the file.

`--write` takes the canonicals from the run instead of checking against
them. Only ever after reading the diff the run printed, and never to make
a red gate green.

## The three outcomes

A fixture is allowed to be one of three things, and there is no fourth:

- **passes**: the document runs and its rows match the canonical.
- **deferred**: the question needs a clause this wave staged, named in the
  fixture. Today that is `pair`, the one to one matching of family 6.
- **out of language**: the question is one that section 14's limits name.
  Today that is free text beyond the fingerprint's own columns.

A fixture that fails for any other reason fails the gate.

## What a fixture proves

Each canonical carries the answer's content hash, its columns, its rows and
the funnel's last stage per named set, so a fixture that drifts says both
what came back and where the subjects went. A hash alone is not an oracle,
which is why the rows are here.

## The Wave 4c fixtures

Wave 4c (`docs/specs/wave4c-the-assistant.md` §6.8) adds eight fixtures, each of
which failed on `main` when it was written. They live in two places, because
the gate runs in one process and some of them are about a door:

- **In the gate itself**: `write-refusal`, always first in the run. It opens the
  ask reader (the read only connection on SQLite; on Postgres the SELECT only
  role named by `--ask-dsn`, after trying to undo the session setting the
  fallback reader relies on) and asserts that `INSERT`, `UPDATE`, `DELETE`,
  `CREATE` and `COPY` all fail. On Postgres without `--ask-dsn` it is
  deferred, because the role is a deployment's to create; CI creates one.
- **In the gate itself**: `marker-escape`, second in the run (Wave 4c §6.6, A6).
  The value sampler is asked for the subject code under the gate's scope and
  must answer shapes that contain none of the registry's own codes; the
  identity probe reads a synthetic tree seeded with a placeholder in the tag
  and three codes in the paths, and its answer must name the placeholder by
  shape and hold neither a seeded value nor a path.
- **In the engine's own tests** (`engine/crates/nils/tests/ask_serve.rs`,
  named after the fixture they are): a reader queuing a document that projects
  identifiers is refused and a queued job runs under the roles the door
  recorded; a reader paging an operator's handle is refused and every page read
  is audited; event streams ask for the reader role and are capped. The
  idempotency, ceiling, sampler and schema fixtures join with their slices.
