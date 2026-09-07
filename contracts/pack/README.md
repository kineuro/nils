<!-- SPDX-License-Identifier: Apache-2.0 -->

# The pack contract

A modality pack is data: a directory with a `pack.yml` manifest and the files
it names, read by the engine's loader and by nothing else. This contract fixes
the **manifest**: which keys it has, what each holds, and which are required.
The shapes of the files the manifest names (parsers, flags, axes, rule sets,
passes, picks, the private lists, the dictionary, the BIDS mapping) are
documented in the engine's pack specification and will be added here as
schemas in their own versions.

`VERSION` is the contract version. A pack declares the contract it was written
for in its manifest's `contract:` field, and an engine that implements a lower
one refuses the pack rather than half-understanding it.

**The version changes only by a pull request that says so**, in its title, and
that bumps `VERSION` and adds a new `vN/` directory beside the old one, which
stays. A change that adds an optional key is still a change to the contract
and gets a version; nothing is amended in place.

| version | schema | since |
|---|---|---|
| 1 | [`v1/pack.schema.json`](v1/pack.schema.json) | Wave 2 (the manifest as the loader reads it); written down in Wave 4a |
| 2 | [`v2/pack.schema.json`](v2/pack.schema.json) | Wave 4a slice 15, 2026-09-06: the optional `fields` key, a visibility (`local`, `federated`, `sensitive`) the pack puts on a catalogue field (C27) |
| 3 | [`v3/pack.schema.json`](v3/pack.schema.json) | Wave 4b slice 4, 2026-09-07: the optional `levels` key, the comparability level files that say what the same acquisition means at each named level (Wave 4b section 6) |

The engine's own copy of the version is `nils_pack::CONTRACT`; a test keeps
the two the same and keeps every manifest key the loader reads on the schema.
