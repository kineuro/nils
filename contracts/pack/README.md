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
| 7 | [`v7/pack.schema.json`](v7/pack.schema.json), [`v7/overlay.schema.json`](v7/overlay.schema.json) | 2026-09-30, record 51: no manifest key changes. A pick file (`picks:`) gains v0's six other border reasons under `borders` (`retake`, `unknown_dim`, `slice_outlier`, `pre_post_twin`, `fallback`, `dixon_vs_plain`), and `family` may be a list of families, each with a `name`, what a family holding none of its canonical outputs becomes (`without_canonical: drop` or `apart`) and how many kept stacks make a retake (`retake_above`). A role that a component with per-role tables scores must have a table of its own there, and a border the engine does not know is refused. An engine at 7 loads a contract-6 pack unchanged and refuses these keys in a pack that declares less than 7, so an engine at 6, which would ignore them, refuses the pack instead. The overlay schema is unchanged |
| 8 | [`v8/pack.schema.json`](v8/pack.schema.json), [`v8/overlay.schema.json`](v8/overlay.schema.json) | 2026-09-30, record 53: no manifest key changes. A passes file may declare a pass of kind `session_context`: its target and rules read fields, flags, parsers, ingested private elements and decided axes, as a rule does, and its rules alone may read the other stacks of the session, each seen by its header through the fields the pass names (`session.sibling_fields`) and never by what was decided of it. It never replaces a value a header tier (a flag or a number the scanner wrote), a person or a decision set; a value read from the name, inferred, defaulted or voted it replaces only at or below `session.replaces_at_most`, and it writes tier `session`. A pick's `fallback` border may name a list of values (`is`). A private file may list the ingested elements a reader is shown (`shown`, each a `name` and a `why`; the `quasi` of 1.0.0-alpha.66 and .67 is withdrawn and ignored); the loader refuses one that is not ingested, not of a technical kind, or at an identification block. An engine at 8 loads a contract-7 pack unchanged, and refuses these keys in a pack that declares less than 8. The overlay schema is unchanged |
| 9 | [`v9/pack.schema.json`](v9/pack.schema.json), [`v9/overlay.schema.json`](v9/overlay.schema.json) | 2026-10-10, the study of the pick borders. The manifest gains one key under `review`, amended in place the same day: `by_model`, the axes an operation of their own answers, about which sorting raises no question (record 56 section 2); the engine read it since 2026-10-09 and the schema did not say so, so the shipped pack failed its own schema. An engine reads it whatever contract a pack declares, since one that does not know it only asks more; the MRI pack declares 9 from 1.0.2. A pick file (`picks:`) may say which stacks holding a role compete for it, `candidates` per role, a `when` and an `unless` list of conditions over the names a pick reads (`{of, any: [values]}`, a value of an axis by its identity, label or alias, or `{of, lt \| le \| gt \| ge: number}`); a stack that is no candidate for a role is no part of the population the role is scored against either. It may say how a near tie is decided, `near_tie`, a list of steps, each `{of, prefer: [values]}`, `{of, avoid: [values]}`, `{of, lowest: true}` or `{of, highest: true}`, for every role or the `roles` it names: among the candidates within `runner_up_within` of the best, the first step that tells the first two apart decides, and a near tie it decides raises no `too_close`. An engine at 9 loads a contract-8 pack unchanged and refuses these keys in a pack that declares less than 9, so an engine at 8 refuses the pack instead of picking without them. The overlay schema is unchanged |
| 10 | [`v10/pack.schema.json`](v10/pack.schema.json), [`v10/overlay.schema.json`](v10/overlay.schema.json) | 2026-10-10, record 55 B5 (rules releases): the optional `engine` key, the range of engine versions the pack works with, bounds separated by commas, each an operator (`>=`, `>`, `<=`, `<`, `=`) and a version (`">=1.0.0-alpha.80, <2.0.0"`), ordered as releases are: the numbers, then a release after its pre-releases, and a development build after the release it was built from. An engine outside the range refuses the pack, and the update path never installs a pack, from the engine's release or from the pack's own, that its engine would refuse. The contract stays the key every engine checks; the range says what a contract cannot, such as an engine fix a rules release relies on. An engine at 10 loads a contract-9 pack unchanged and refuses the key in a pack that declares less than 10, so an engine at 9, which would load the pack without reading the range, refuses it by its contract. The overlay schema is unchanged |

Added in place to version 6, additively, as the files it names grew keys a
contract-6 engine reads and an older pack never writes: the BIDS mapping's
`when_technique` on a suffix, a rule set's `redecides`
(1.0.0-alpha.55), a normalized text's `unless_manufacturer` and the fields
`temporal_positions` and `series_number` (1.0.0-alpha.56), and `aslcontext` on
a suffix mapping, allowed only with `suffix: asl` and one of the BIDS volume
types `control`, `label`, `m0scan`, `deltam`, `cbf` and `noRF`, which a BIDS
release writes as the `aslcontext.tsv` beside the image, one row per volume
(MRI pack 0.11.0). The manifest is unchanged by all of them.

Added in place to version 8, and withdrawn: 1.0.0-alpha.66 let a
`private.shown` entry carry `quasi: true`, which held a vendor's sequence
name back below detail quasi. From 1.0.0-alpha.68 sequence names are shown
everywhere, at every detail, and every element `shown` lists is shown at
every detail. The engine no longer reads `quasi`, so a pack that still
carries it (MRI pack 0.20.0 and 0.20.1) loads and shows the element at every
detail, and `nils pack show --json` no longer names it. MRI pack 0.20.2
drops the marks. Only 1.0.0-alpha.66 and 1.0.0-alpha.67 read the key. The
manifest is unchanged.

The engine's own copy of the version is `nils_pack::CONTRACT`; a test keeps
the two the same and keeps every manifest key the loader reads on the schema.

**The overlay** (`v10/overlay.schema.json`, unchanged since 5) is the one document a site writes
against a pack: `overlay`, `version`, `pack`, an origin `scope`, the
`buckets` and `lists` it amends (each an `add` and a `remove`), and its
`cases`. The same document is what `POST /api/classify/try` rehearses and
`POST /api/overlays` proposes. An engine refuses an overlay that names
anything the schema does not, and says what stays the pack's.
