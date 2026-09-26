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
| 4 | [`v4/pack.schema.json`](v4/pack.schema.json) | Wave 4b slice 11, 2026-09-07: the optional `mcp` key, what the MCP door tells a model and which doors it opts in as tools (Wave 4b section 12.3) |
| 5 | [`v5/pack.schema.json`](v5/pack.schema.json), [`v5/overlay.schema.json`](v5/overlay.schema.json) | 2026-09-16: no manifest key changes; every axis value's `keywords` list is a site's to amend through an overlay, named `lists.<axis>.<value>` beside the `buckets`, and the overlay document has a schema of its own. The flags, the physics, the thresholds and the order values are tried in stay the pack's. An engine at 5 loads a contract-4 pack unchanged |
| 6 | [`v6/pack.schema.json`](v6/pack.schema.json), [`v6/overlay.schema.json`](v6/overlay.schema.json) | 2026-09-26, record 48: the optional `excludes` and `hints` keys, files of constraints between axes over axis values alone. An exclusion rules values of one axis out where its condition holds, and an answer holding one is refused; a hint names the value usually found on an axis, with its reason, and is shown and never enforced. Neither decides an axis of a stack. The overlay schema is unchanged. An engine at 6 loads a contract-5 pack unchanged |

The engine's own copy of the version is `nils_pack::CONTRACT`; a test keeps
the two the same and keeps every manifest key the loader reads on the schema.

**The overlay** (`v6/overlay.schema.json`, unchanged since 5) is the one document a site writes
against a pack: `overlay`, `version`, `pack`, an origin `scope`, the
`buckets` and `lists` it amends (each an `add` and a `remove`), and its
`cases`. The same document is what `POST /api/classify/try` rehearses and
`POST /api/overlays` proposes. An engine refuses an overlay that names
anything the schema does not, and says what stays the pack's.
