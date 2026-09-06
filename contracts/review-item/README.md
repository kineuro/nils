<!-- SPDX-License-Identifier: Apache-2.0 -->

# The review-item contract

A review item is the one primitive behind every decision a person, an agent or
a rule makes about the engine's work (decision record 05 §3, D7): what was
found, in what scope, with what evidence, and what was decided. Every
emitter writes this shape and every consumer reads it: `nils review` in the
CLI, the review queues of the apps, and agents through MCP.

This contract fixes the **item** as the engine hands it out, which is what
`nils review list --json` prints and what the HTTP API will return. It does
not fix the evidence of each kind, which is the emitting stage's and is
described with the kind.

`VERSION` is the contract version. **It changes only by a pull request that
says so**, in its title, and that bumps `VERSION` and adds a new `vN/`
directory beside the old one, which stays. Adding a kind is not a change to
the contract; adding or removing a property of the item is.

| version | schema | since |
|---|---|---|
| 1 | [`v1/review-item.schema.json`](v1/review-item.schema.json) | Wave 1 (the table); written down in Wave 4a |

## The kinds so far

Two grammars of `kind`: `<area>.<what>` for the engine's own stages, and
`<axis>:<reason>` for the classifier, where the axis is one the pack
declares.

| kind | scope | who raises it |
|---|---|---|
| `identity.collision` | subject | the digest, when two identifiers of one batch derive one code, or a code is another's |
| `ingest.quarantine` | batch | the digest, one per batch and class of refused file |
| `<axis>:low_confidence` | stack | the classifier, below the pack's threshold for that axis |
| `<axis>:missing` | stack | the classifier, for an axis the pack says is always expected |
| `<axis>:decision` | stack | the classifier, when a person's decision disagrees with the rule |
| `<axis>:vote` | stack | a pass, when its answer is weak or the pack asks for every touched stack |
| `release.burned_in`, `release.unjudged` | stack | the release, for a stack it held back |
| `release.no_task` | study | the release, for a functional series with no task to name |

A test keeps the item the CLI prints on this schema.
