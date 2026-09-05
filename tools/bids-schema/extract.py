#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Regenerate the engine's copy of the BIDS schema (Wave 3 §9.2).

The entity grammar, which entities each suffix takes and which it requires, are
the standard and not our choice, so the engine carries them as data and a pack
cannot contradict them. This writes that data from the published schema, so the
copy is a transcription rather than a reading of the prose.

    python3 tools/bids-schema/extract.py [--url URL | --file schema.json]

It prints the module to stdout; write it over
`engine/crates/nils-release/src/bids/schema.rs` and run the tests.
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.request

URL = "https://bids-specification.readthedocs.io/en/stable/schema.json"

# The MRI datatypes, from `rules.modalities.mri.datatypes`. Every other
# modality is somebody else's wave.
MRI = ["anat", "dwi", "fmap", "func", "perf"]

# Entities the engine never writes: the release decides the subject and the
# session itself, and the rest belong to modalities or derivatives we do not
# produce. Kept out of the generated table so an unsupported entity is a
# compile error rather than a silently ignored field.
SKIP = {"subject", "session"}


def level(v) -> str:
    return v if isinstance(v, str) else v.get("level", "optional")


def rust_list(items) -> str:
    if not items:
        return "&[]"
    return "&[" + ", ".join(f'"{i}"' for i in items) + "]"


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--url", default=URL)
    p.add_argument("--file")
    args = p.parse_args()

    if args.file:
        schema = json.load(open(args.file, encoding="utf-8"))
    else:
        with urllib.request.urlopen(args.url, timeout=60) as r:
            schema = json.load(r)

    bids = schema["bids_version"]
    version = schema["schema_version"]
    entities = schema["objects"]["entities"]
    order = schema["rules"]["entities"]
    raw = schema["rules"]["files"]["raw"]

    out: list[str] = []
    w = out.append
    w("// SPDX-License-Identifier: AGPL-3.0-only")
    w("")
    w("//! The BIDS schema, as data (`docs/specs/wave3-anonymize-and-bids.md`, §9.2).")
    w("//!")
    w("//! **Generated. Do not edit by hand**: `python3 tools/bids-schema/extract.py`.")
    w("//!")
    w("//! The entity grammar, which entities a suffix takes and which it requires,")
    w("//! are the standard and not our choice. So the engine carries them and a pack")
    w("//! cannot contradict them: a pack says which of our values means `T1w`, and")
    w("//! this says what a `T1w` file may be called. v0 approximates none of it,")
    w("//! because v0 writes no entity names at all.")
    w("")
    w(f'/// The specification this was taken from.')
    w(f'pub const BIDS_VERSION: &str = "{bids}";')
    w("")
    w("/// The schema's own version, which moves faster than the specification's.")
    w(f'pub const SCHEMA_VERSION: &str = "{version}";')
    w("")
    w("/// One entity of the grammar.")
    w("#[derive(Debug, Clone, Copy, PartialEq, Eq)]")
    w("pub struct Entity {")
    w("    /// What the schema calls it, which is what a rule names.")
    w("    pub key: &'static str,")
    w("    /// What a filename spells, which is not always the same word.")
    w("    pub name: &'static str,")
    w("    /// An index is a number and a label is a string, and the difference")
    w("    /// decides both the validation and the rendering.")
    w("    pub index: bool,")
    w("    /// The only values it may take, when the schema fixes them.")
    w("    pub values: &'static [&'static str],")
    w("}")
    w("")
    w("/// Every entity the MRI datatypes use, **in the order a filename spells")
    w("/// them**. The order is the schema's, so a name built by walking this list")
    w("/// is in the standard's order by construction rather than by care.")
    w("pub const ENTITIES: &[Entity] = &[")
    for key in order:
        if key in SKIP:
            continue
        # Only the entities some MRI group actually admits.
        used = any(
            key in group.get("entities", {})
            for dt in MRI
            for group in raw[dt].values()
        )
        if not used:
            continue
        e = entities[key]
        values = e.get("enum", [])
        values = [v["name"] if isinstance(v, dict) else v for v in values]
        index = "true" if e.get("format") == "index" else "false"
        w(f'    Entity {{ key: "{key}", name: "{e["name"]}", index: {index}, '
          f"values: {rust_list(values)} }},")
    w("];")
    w("")
    w("/// One row of the schema's file rules: a set of suffixes that share a")
    w("/// datatype and an entity set.")
    w("#[derive(Debug, Clone, Copy, PartialEq, Eq)]")
    w("pub struct Group {")
    w("    pub datatype: &'static str,")
    w("    /// The schema's name for the group, so a refusal can cite it.")
    w("    pub name: &'static str,")
    w("    pub suffixes: &'static [&'static str],")
    w("    /// Entities without which the name is invalid. `MEGRE` requires")
    w("    /// `echo`, which turns v0's second export bug from a cosmetic")
    w("    /// complaint into a validator error.")
    w("    pub required: &'static [&'static str],")
    w("    /// Entities it may carry. Anything else is refused rather than")
    w("    /// written: `part` on a scanner-derived ADC is not a BIDS name.")
    w("    pub allowed: &'static [&'static str],")
    w("    /// The extensions the group admits, so a sidecar or a `.bval` is")
    w("    /// written only where the standard has one.")
    w("    pub extensions: &'static [&'static str],")
    w("}")
    w("")
    w("/// Every MRI file rule of the schema.")
    w("pub const GROUPS: &[Group] = &[")
    for dt in MRI:
        for name, g in raw[dt].items():
            if g.get("datatypes") and dt not in g["datatypes"]:
                continue
            ents = g.get("entities", {})
            required = [k for k in order if k in ents and level(ents[k]) == "required" and k not in SKIP]
            allowed = [k for k in order if k in ents and k not in SKIP]
            w(f'    Group {{ datatype: "{dt}", name: "{name}",')
            w(f'        suffixes: {rust_list(g.get("suffixes", []))},')
            w(f"        required: {rust_list(required)},")
            w(f"        allowed: {rust_list(allowed)},")
            w(f'        extensions: {rust_list(g.get("extensions", []))} }},')
    w("];")
    w("")
    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
