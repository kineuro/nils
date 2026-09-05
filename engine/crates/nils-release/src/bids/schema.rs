// SPDX-License-Identifier: AGPL-3.0-only

//! The BIDS schema, as data (`docs/specs/wave3-anonymize-and-bids.md`, §9.2).
//!
//! **Generated. Do not edit by hand**: `python3 tools/bids-schema/extract.py`.
//!
//! The entity grammar, which entities a suffix takes and which it requires,
//! are the standard and not our choice. So the engine carries them and a pack
//! cannot contradict them: a pack says which of our values means `T1w`, and
//! this says what a `T1w` file may be called. v0 approximates none of it,
//! because v0 writes no entity names at all.

/// The specification this was taken from.
pub const BIDS_VERSION: &str = "1.11.1";

/// The schema's own version, which moves faster than the specification's.
pub const SCHEMA_VERSION: &str = "1.2.1";

/// One entity of the grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entity {
    /// What the schema calls it, which is what a rule names.
    pub key: &'static str,
    /// What a filename spells, which is not always the same word.
    pub name: &'static str,
    /// An index is a number and a label is a string, and the difference
    /// decides both the validation and the rendering.
    pub index: bool,
    /// The only values it may take, when the schema fixes them.
    pub values: &'static [&'static str],
}

/// Every entity the MRI datatypes use, **in the order a filename spells
/// them**. The order is the schema's, so a name built by walking this list
/// is in the standard's order by construction rather than by care.
pub const ENTITIES: &[Entity] = &[
    Entity {
        key: "task",
        name: "task",
        index: false,
        values: &[],
    },
    Entity {
        key: "acquisition",
        name: "acq",
        index: false,
        values: &[],
    },
    Entity {
        key: "ceagent",
        name: "ce",
        index: false,
        values: &[],
    },
    Entity {
        key: "reconstruction",
        name: "rec",
        index: false,
        values: &[],
    },
    Entity {
        key: "direction",
        name: "dir",
        index: false,
        values: &[],
    },
    Entity {
        key: "run",
        name: "run",
        index: true,
        values: &[],
    },
    Entity {
        key: "modality",
        name: "mod",
        index: false,
        values: &[],
    },
    Entity {
        key: "echo",
        name: "echo",
        index: true,
        values: &[],
    },
    Entity {
        key: "flip",
        name: "flip",
        index: true,
        values: &[],
    },
    Entity {
        key: "inversion",
        name: "inv",
        index: true,
        values: &[],
    },
    Entity {
        key: "mtransfer",
        name: "mt",
        index: false,
        values: &["on", "off"],
    },
    Entity {
        key: "part",
        name: "part",
        index: false,
        values: &["mag", "phase", "real", "imag"],
    },
    Entity {
        key: "chunk",
        name: "chunk",
        index: true,
        values: &[],
    },
];

/// One row of the schema's file rules: a set of suffixes that share a
/// datatype and an entity set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Group {
    pub datatype: &'static str,
    /// The schema's name for the group, so a refusal can cite it.
    pub name: &'static str,
    pub suffixes: &'static [&'static str],
    /// Entities without which the name is invalid. `MEGRE` requires
    /// `echo`, which turns v0's second export bug from a cosmetic
    /// complaint into a validator error.
    pub required: &'static [&'static str],
    /// Entities it may carry. Anything else is refused rather than
    /// written: `part` on a scanner-derived ADC is not a BIDS name.
    pub allowed: &'static [&'static str],
    /// The extensions the group admits, so a sidecar or a `.bval` is
    /// written only where the standard has one.
    pub extensions: &'static [&'static str],
}

/// Every MRI file rule of the schema.
pub const GROUPS: &[Group] = &[
    Group {
        datatype: "anat",
        name: "nonparametric",
        suffixes: &[
            "T1w",
            "T2w",
            "PDw",
            "T2starw",
            "FLAIR",
            "inplaneT1",
            "inplaneT2",
            "PDT2",
            "angio",
            "T2star",
            "FLASH",
            "PD",
        ],
        required: &[],
        allowed: &[
            "task",
            "acquisition",
            "ceagent",
            "reconstruction",
            "run",
            "echo",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "anat",
        name: "parametric",
        suffixes: &[
            "T1map",
            "T2map",
            "T2starmap",
            "R1map",
            "R2map",
            "R2starmap",
            "PDmap",
            "MTRmap",
            "MTsat",
            "UNIT1",
            "T1rho",
            "MWFmap",
            "MTVmap",
            "Chimap",
            "S0map",
            "M0map",
        ],
        required: &[],
        allowed: &[
            "task",
            "acquisition",
            "ceagent",
            "reconstruction",
            "run",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "anat",
        name: "defacemask",
        suffixes: &["defacemask"],
        required: &[],
        allowed: &[
            "task",
            "acquisition",
            "ceagent",
            "reconstruction",
            "run",
            "modality",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "anat",
        name: "megre",
        suffixes: &["MEGRE"],
        required: &["echo"],
        allowed: &[
            "task",
            "acquisition",
            "ceagent",
            "reconstruction",
            "run",
            "echo",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "anat",
        name: "mese",
        suffixes: &["MESE"],
        required: &["echo"],
        allowed: &[
            "task",
            "acquisition",
            "ceagent",
            "reconstruction",
            "direction",
            "run",
            "echo",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "anat",
        name: "multiflip",
        suffixes: &["VFA"],
        required: &["flip"],
        allowed: &[
            "task",
            "acquisition",
            "ceagent",
            "reconstruction",
            "run",
            "echo",
            "flip",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "anat",
        name: "multiinversion",
        suffixes: &["IRT1"],
        required: &["inversion"],
        allowed: &[
            "task",
            "acquisition",
            "ceagent",
            "reconstruction",
            "run",
            "inversion",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "anat",
        name: "mp2rage",
        suffixes: &["MP2RAGE"],
        required: &["inversion"],
        allowed: &[
            "task",
            "acquisition",
            "ceagent",
            "reconstruction",
            "run",
            "echo",
            "flip",
            "inversion",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "anat",
        name: "vfamt",
        suffixes: &["MPM", "MTS"],
        required: &["flip", "mtransfer"],
        allowed: &[
            "task",
            "acquisition",
            "ceagent",
            "reconstruction",
            "run",
            "echo",
            "flip",
            "mtransfer",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "anat",
        name: "mtr",
        suffixes: &["MTR"],
        required: &["mtransfer"],
        allowed: &[
            "task",
            "acquisition",
            "ceagent",
            "reconstruction",
            "run",
            "mtransfer",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "dwi",
        name: "dwi",
        suffixes: &["dwi"],
        required: &[],
        allowed: &[
            "acquisition",
            "reconstruction",
            "direction",
            "run",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json", ".bvec", ".bval"],
    },
    Group {
        datatype: "dwi",
        name: "sbref",
        suffixes: &["sbref"],
        required: &[],
        allowed: &[
            "acquisition",
            "reconstruction",
            "direction",
            "run",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "dwi",
        name: "ScannerDerivatives",
        suffixes: &["ADC", "FA", "S0map", "colFA", "expADC", "trace"],
        required: &[],
        allowed: &["acquisition", "reconstruction", "direction", "run", "chunk"],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "fmap",
        name: "fieldmaps",
        suffixes: &[
            "phasediff",
            "phase1",
            "phase2",
            "magnitude1",
            "magnitude2",
            "magnitude",
            "fieldmap",
        ],
        required: &[],
        allowed: &["acquisition", "run", "chunk"],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "fmap",
        name: "pepolar",
        suffixes: &["epi"],
        required: &[],
        allowed: &[
            "acquisition",
            "ceagent",
            "reconstruction",
            "direction",
            "run",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json", ".bval", ".bvec"],
    },
    Group {
        datatype: "fmap",
        name: "pepolar_m0scan",
        suffixes: &["m0scan"],
        required: &[],
        allowed: &[
            "acquisition",
            "ceagent",
            "reconstruction",
            "direction",
            "run",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "fmap",
        name: "TB1DAM",
        suffixes: &["TB1DAM"],
        required: &["flip"],
        allowed: &[
            "acquisition",
            "ceagent",
            "reconstruction",
            "run",
            "flip",
            "inversion",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "fmap",
        name: "TB1EPI",
        suffixes: &["TB1EPI"],
        required: &["echo", "flip"],
        allowed: &[
            "acquisition",
            "ceagent",
            "reconstruction",
            "run",
            "echo",
            "flip",
            "inversion",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "fmap",
        name: "RFFieldMaps",
        suffixes: &["TB1AFI", "TB1TFL", "TB1RFM", "RB1COR"],
        required: &[],
        allowed: &[
            "acquisition",
            "ceagent",
            "reconstruction",
            "run",
            "echo",
            "flip",
            "inversion",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "fmap",
        name: "TB1SRGE",
        suffixes: &["TB1SRGE"],
        required: &["flip", "inversion"],
        allowed: &[
            "acquisition",
            "ceagent",
            "reconstruction",
            "run",
            "echo",
            "flip",
            "inversion",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "fmap",
        name: "parametric",
        suffixes: &["TB1map", "RB1map"],
        required: &[],
        allowed: &["acquisition", "ceagent", "reconstruction", "run", "chunk"],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "func",
        name: "func",
        suffixes: &["bold", "cbv", "sbref"],
        required: &["task"],
        allowed: &[
            "task",
            "acquisition",
            "ceagent",
            "reconstruction",
            "direction",
            "run",
            "echo",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "func",
        name: "norf",
        suffixes: &["noRF"],
        required: &["task"],
        allowed: &[
            "task",
            "acquisition",
            "ceagent",
            "reconstruction",
            "direction",
            "run",
            "modality",
            "echo",
            "part",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "func",
        name: "phase",
        suffixes: &["phase"],
        required: &["task"],
        allowed: &[
            "task",
            "acquisition",
            "ceagent",
            "reconstruction",
            "direction",
            "run",
            "echo",
            "chunk",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "perf",
        name: "asl",
        suffixes: &["asl", "m0scan"],
        required: &[],
        allowed: &[
            "acquisition",
            "reconstruction",
            "direction",
            "run",
            "echo",
            "part",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
    Group {
        datatype: "perf",
        name: "aslcontext",
        suffixes: &["aslcontext"],
        required: &[],
        allowed: &["acquisition", "reconstruction", "direction", "run"],
        extensions: &[".tsv"],
    },
    Group {
        datatype: "perf",
        name: "asllabeling",
        suffixes: &["asllabeling"],
        required: &[],
        allowed: &["acquisition", "reconstruction", "run"],
        extensions: &[".jpg", ".png", ".tif"],
    },
    Group {
        datatype: "perf",
        name: "norf",
        suffixes: &["noRF"],
        required: &[],
        allowed: &[
            "acquisition",
            "reconstruction",
            "direction",
            "run",
            "modality",
            "echo",
            "part",
        ],
        extensions: &[".nii.gz", ".nii", ".json"],
    },
];

/// The entity of a key, if the MRI datatypes use it.
pub fn entity(key: &str) -> Option<&'static Entity> {
    ENTITIES.iter().find(|e| e.key == key)
}

/// The file rule a datatype and suffix fall under.
///
/// The pair and not the suffix alone: `m0scan` is a `perf` suffix and an
/// `fmap` one, with different entities, and `noRF` is both `func` and `perf`.
pub fn group_of(datatype: &str, suffix: &str) -> Option<&'static Group> {
    GROUPS
        .iter()
        .find(|g| g.datatype == datatype && g.suffixes.contains(&suffix))
}

/// The datatype, as a word the engine owns rather than one a pack supplied.
pub fn datatype(name: &str) -> Option<&'static str> {
    GROUPS
        .iter()
        .find(|g| g.datatype == name)
        .map(|g| g.datatype)
}

/// Whether a value is one the entity may take.
///
/// Three rules, all the schema's: an index is a number, a label is
/// `[0-9a-zA-Z+]+`, and where the schema fixes the values there are no others.
pub fn admits(key: &str, value: &str) -> bool {
    let Some(e) = entity(key) else { return false };
    if value.is_empty() {
        return false;
    }
    if e.index {
        return value.chars().all(|c| c.is_ascii_digit());
    }
    if !e.values.is_empty() && !e.values.contains(&value) {
        return false;
    }
    value.chars().all(|c| c.is_ascii_alphanumeric() || c == '+')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pair_and_not_the_suffix_alone_finds_the_rule() {
        // `m0scan` is a `perf` suffix and an `fmap` one, with different
        // entities; a lookup by suffix alone would take whichever came first.
        assert_eq!(group_of("perf", "m0scan").unwrap().datatype, "perf");
        assert_eq!(group_of("fmap", "m0scan").unwrap().datatype, "fmap");
        assert!(group_of("anat", "m0scan").is_none());
    }

    #[test]
    fn the_suffixes_that_require_an_entity_are_the_standards() {
        for (datatype, suffix, entity) in [
            ("anat", "MEGRE", "echo"),
            ("anat", "MESE", "echo"),
            ("anat", "VFA", "flip"),
            ("anat", "MP2RAGE", "inversion"),
            ("anat", "MTR", "mtransfer"),
            ("func", "bold", "task"),
        ] {
            let g = group_of(datatype, suffix).unwrap_or_else(|| panic!("{suffix}"));
            assert!(g.required.contains(&entity), "{suffix} requires {entity}");
        }
        assert!(group_of("anat", "T1w").unwrap().required.is_empty());
    }

    #[test]
    fn a_scanner_derived_map_takes_no_part() {
        // Which is why an entity the group does not admit is dropped rather
        // than written: a magnitude ADC is an ADC.
        let g = group_of("dwi", "ADC").unwrap();
        assert!(!g.allowed.contains(&"part"));
        assert!(group_of("dwi", "dwi").unwrap().allowed.contains(&"part"));
    }

    #[test]
    fn an_index_is_a_number_and_a_label_is_alphanumeric() {
        assert!(admits("echo", "2"));
        assert!(!admits("echo", "two"));
        assert!(!admits("echo", ""));
        assert!(admits("acquisition", "SagMPRAGE"));
        assert!(
            !admits("acquisition", "Sag-MPRAGE"),
            "a hyphen is not a label"
        );
        assert!(!admits("acquisition", "Sag_MPRAGE"), "nor an underscore");
        assert!(!admits("acquisition", "T2*w"), "nor a star");
    }

    #[test]
    fn an_entity_the_schema_fixes_the_values_of_takes_no_others() {
        assert!(admits("part", "mag"));
        assert!(!admits("part", "magnitude"));
        assert!(admits("mtransfer", "on"));
        assert!(!admits("mtransfer", "yes"));
    }

    #[test]
    fn the_generated_table_says_which_standard_it_is() {
        // A tree has to be able to say which version of BIDS it claims to be,
        // and a name built against one schema and validated against another is
        // the failure this is here to prevent.
        assert!(BIDS_VERSION.starts_with("1."), "{BIDS_VERSION}");
        assert!(!SCHEMA_VERSION.is_empty());
        assert!(GROUPS.len() > 20, "{}", GROUPS.len());
        assert!(
            ENTITIES
                .iter()
                .any(|e| e.key == "part" && !e.values.is_empty())
        );
    }
}
