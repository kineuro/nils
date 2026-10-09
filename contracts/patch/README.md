<!-- SPDX-License-Identifier: Apache-2.0 -->

# The patch contract

A pack patch is rule edits written as typed operations (record 56,
section 5.5): the handful of kinds most changes to a pack's rules are,
each with its scope, its reason and its evidence. A form, a person or the
assistant writes one; the engine checks it against the pack before
anything runs, and it is a reviewable diff of the pack.

| version | schema | since |
|---|---|---|
| 1 | [`v1/patch.schema.json`](v1/patch.schema.json) | 2026-10-09, record 56: `add_words`, `remove_words`, `move_rule`, `move_set`, `set_priority`, `add_value`, `add_rule`, `silence`, `by_model` and `map_name`, each scoped to the whole pack (a pack edit, shipped as a rules release) or to a site, a dataset or a scanner (an overlay) |

**What the engine does with one.** It applies the operations in order to the
pack's own documents (the manifest, an axis file, a rule set, the BIDS
mapping, a pass) and builds the result with the pack's own loader. An
operation that cannot apply is refused with a plain why: a value the axis
does not have, a word already there, a move to where a set already runs.
A result the loader refuses is refused with the loader's why. The pack's
corpus, and the patch's own `cases`, judge the result as they judge an
overlay, and their failures are part of the answer.

- `nils pack rehearse --ops FILE [--scope ...]` and `POST
  /api/packs/{name}/rehearse` (OpenAPI version 7, added in place on
  2026-10-09) answer what the patch does to the sorting: the registry in
  scope is sorted both ways, and nothing is written.
- `nils pack apply DIR --ops FILE --out DIR` writes a patch of pack edits as
  the pack's next version, each changed file rewritten where it changed so
  its comments stay.
- An overlay document (pack contract 5) reads as the word edits it is:
  `add_words` and `remove_words` on its buckets and its `axis.value` lists,
  scoped to its origin.

**The version changes only by a pull request that says so**, in its title,
and that bumps `VERSION` and adds a new `vN/` directory beside the old one,
which stays. The engine's own copy is `nils_pack::patch::FORMAT`; a test
keeps the two the same and every operation and key the engine reads on the
schema.

Contributions here are covered by the Developer Certificate of Origin, not
the CLA: sign off your commits (`git commit -s`).
