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
| 2 | [`v2/review-item.schema.json`](v2/review-item.schema.json) | Wave 4a slice 13, 2026-09-06: `scope` gains `group`, `status` gains `accepted`, `rejected`, `superseded` and `staged`, and the item gains `members`, `group_key` and `accepted_by` |
| 3 | [`v3/review-item.schema.json`](v3/review-item.schema.json) | Wave 4c slice A2, 2026-09-09: `decision` gains `actor_detail`, the actor object of the suite contract (who acted for the principal: kind, name, model, version, conversation, ceiling; `absent` as its own value) |
| 4 | [`v4/review-item.schema.json`](v4/review-item.schema.json) | Wave 4c slice A6, 2026-09-09: `scope` gains `overlay`, the review item beside a proposed overlay (Wave 4c §6.6); the four classifier diagnostics are `diagnostic` rows by kind, not items, so no kind is added |

## The kinds so far

Two grammars of `kind`: `<area>.<what>` for the engine's own stages, and
`<axis>:<reason>` for the classifier, where the axis is one the pack
declares.

| kind | scope | who raises it |
|---|---|---|
| `identity.collision` | subject | the digest, when two identifiers of one batch derive one code, or a code is another's |
| `identity.unmapped` | batch | the pseudonymiser (record 26, decision 4), one per dataset and shape of identifier the linkage store did not know, so the files were held; `ref` is `{place_id, place, shape, id_type}` and `evidence` is `{files, batch_id, shape, id_type}`, counts and the shape (digits as 9, letters as A), never a value; `group_key` is `place:<id>|shape:<shape>` and a later run brings the open item up to date |
| `identity.provisional` | subject | the pseudonymiser, for a subject coded from an unmapped identifier because the dataset said `code`; `ref` is `{subject_id, code}`, `evidence` `{id_type, shape, place_id, place, batch_id}`, `group_key` `subject:<id>`, one open per subject; a merge of the subject closes it as `superseded` with the merge as its decision |
| `ingest.quarantine` | batch | the digest, one per batch and class of refused file |
| `<axis>:low_confidence` | stack | the classifier, below the pack's threshold for that axis |
| `<axis>:missing` | stack | the classifier, for an axis the pack says is always expected |
| `<axis>:decision` | stack | the classifier, when a person's decision disagrees with the rule |
| `<axis>:vote` | stack | a pass, when its answer is weak or the pack asks for every touched stack |
| `release.burned_in`, `release.unjudged` | stack | the release, for a stack it held back |
| `release.no_task` | study | the release, for a functional series with no task to name |

A test keeps the item the CLI prints on this schema.
