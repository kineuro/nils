// SPDX-License-Identifier: AGPL-3.0-only

//! What BIDS requires of a converted image before its name is valid (Wave 7a
//! §8.2, T12).
//!
//! The official validator, run on a release of the development install's test
//! data, found images whose names are right and whose sidecars lack what the
//! standard requires of that kind of image: an MP2RAGE without its preparation
//! timings, an ASL image without its labelling, a field map without its units,
//! a diffusion image without its gradient table, a T1w with a fourth
//! dimension. `dcm2niix` writes what the headers say and nothing more, so such
//! a stack can never be a valid BIDS image from its headers alone. Like a
//! stack the converter will not convert, it is written as DICOM under
//! `sourcedata/` with the reason, and its data stays present.
//!
//! The rules are the raw MRI ones of the BIDS schema the pinned validator
//! reads (schema 1.2.7, `rules.sidecars` and `rules.checks`): the required
//! fields of each selector that names only a datatype, a suffix, an entity or
//! a sidecar value, and the checks on the gradient table, the readout time and
//! a T1w's rank. A rule that needs another file of the session (a field map's
//! magnitude) is not here: it is the whole tree's, and the validator in CI
//! holds it.

use std::path::Path;

use serde_json::Value;

/// A required field and the rule that requires it.
struct Rule {
    /// The datatype, or any.
    datatype: Option<&'static str>,
    /// The suffixes, or any.
    suffixes: &'static [&'static str],
    /// An entity the name must carry, or none.
    entity: Option<&'static str>,
    fields: &'static [&'static str],
}

const RULES: &[Rule] = &[
    // entity_rules
    Rule {
        datatype: None,
        suffixes: &[],
        entity: Some("echo"),
        fields: &["EchoTime"],
    },
    Rule {
        datatype: None,
        suffixes: &[],
        entity: Some("flip"),
        fields: &["FlipAngle"],
    },
    Rule {
        datatype: None,
        suffixes: &[],
        entity: Some("inv"),
        fields: &["InversionTime"],
    },
    Rule {
        datatype: None,
        suffixes: &[],
        entity: Some("mt"),
        fields: &["MTState"],
    },
    // fmap
    Rule {
        datatype: Some("fmap"),
        suffixes: &["phasediff"],
        entity: None,
        fields: &["EchoTime1", "EchoTime2"],
    },
    Rule {
        datatype: Some("fmap"),
        suffixes: &["phase1", "phase2"],
        entity: None,
        fields: &["EchoTime"],
    },
    Rule {
        datatype: Some("fmap"),
        suffixes: &["fieldmap"],
        entity: None,
        fields: &["Units"],
    },
    Rule {
        datatype: Some("fmap"),
        suffixes: &["epi"],
        entity: None,
        fields: &["PhaseEncodingDirection"],
    },
    Rule {
        datatype: Some("fmap"),
        suffixes: &["TB1EPI"],
        entity: None,
        fields: &["EchoTime", "FlipAngle", "TotalReadoutTime", "MixingTime"],
    },
    // asl and mri, perf
    Rule {
        datatype: Some("perf"),
        suffixes: &["asl", "m0scan"],
        entity: None,
        fields: &[
            "RepetitionTimePreparation",
            "MagneticFieldStrength",
            "EchoTime",
        ],
    },
    Rule {
        datatype: Some("perf"),
        suffixes: &["asl"],
        entity: None,
        fields: &[
            "ArterialSpinLabelingType",
            "PostLabelingDelay",
            "BackgroundSuppression",
            "M0Type",
            "TotalAcquiredPairs",
            "MRAcquisitionType",
        ],
    },
    Rule {
        datatype: Some("perf"),
        suffixes: &["m0scan"],
        entity: None,
        fields: &["IntendedFor"],
    },
    // qmri
    Rule {
        datatype: None,
        suffixes: &["VFA"],
        entity: None,
        fields: &["FlipAngle", "PulseSequenceType", "RepetitionTimeExcitation"],
    },
    Rule {
        datatype: None,
        suffixes: &["IRT1"],
        entity: None,
        fields: &["InversionTime"],
    },
    Rule {
        datatype: None,
        suffixes: &["MP2RAGE"],
        entity: None,
        fields: &[
            "FlipAngle",
            "InversionTime",
            "RepetitionTimeExcitation",
            "RepetitionTimePreparation",
            "NumberShots",
            "MagneticFieldStrength",
        ],
    },
    Rule {
        datatype: None,
        suffixes: &["MESE", "MEGRE"],
        entity: None,
        fields: &["EchoTime"],
    },
    Rule {
        datatype: None,
        suffixes: &["MTR"],
        entity: None,
        fields: &["MTState"],
    },
    Rule {
        datatype: None,
        suffixes: &["MTS", "MPM"],
        entity: None,
        fields: &["FlipAngle", "MTState", "RepetitionTimeExcitation"],
    },
    Rule {
        datatype: None,
        suffixes: &["TB1DAM"],
        entity: None,
        fields: &["FlipAngle"],
    },
    Rule {
        datatype: None,
        suffixes: &["TB1AFI"],
        entity: None,
        fields: &["RepetitionTimeExcitation"],
    },
    Rule {
        datatype: None,
        suffixes: &["TB1SRGE"],
        entity: None,
        fields: &[
            "FlipAngle",
            "InversionTime",
            "RepetitionTimeExcitation",
            "RepetitionTimePreparation",
            "NumberShots",
        ],
    },
];

/// The entities of a stem, as (key, label).
fn entities(stem: &str) -> Vec<(&str, &str)> {
    stem.split('_').filter_map(|t| t.split_once('-')).collect()
}

/// What a converted image of `stem` in `datatype` lacks before BIDS admits
/// it: each required field its sidecar has not got, a gradient table not
/// written, a readout time not stated, a T1w of more than three dimensions.
/// `files` are what the conversion wrote, in `dir`. Empty when nothing lacks.
pub fn gaps(datatype: &str, stem: &str, dir: &Path, files: &[String]) -> Vec<String> {
    gaps_in(datatype, stem, &read(dir, stem), dir, files)
}

/// The sidecar of `stem` in `dir`, parsed; null where there is none.
pub fn read(dir: &Path, stem: &str) -> Value {
    std::fs::read(dir.join(format!("{stem}.json")))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null)
}

/// The fields the release states where the converter wrote none, for the
/// name `stem` and the sidecar as read: `Units` of a phase image
/// (`part-phase`) is `arbitrary`, since the pixels are written as stored.
/// A field the sidecar states already is kept.
pub fn defaults(stem: &str, sidecar: &Value) -> Vec<(&'static str, Value)> {
    let has = |field: &str| sidecar.get(field).is_some_and(|v| !v.is_null());
    let mut fills = Vec::new();
    if entities(stem)
        .iter()
        .any(|(k, v)| *k == "part" && *v == "phase")
        && !has("Units")
    {
        fills.push(("Units", Value::from("arbitrary")));
    }
    fills
}

/// [`gaps`] over a sidecar already read (and filled by [`defaults`]).
pub fn gaps_in(
    datatype: &str,
    stem: &str,
    sidecar: &Value,
    dir: &Path,
    files: &[String],
) -> Vec<String> {
    let suffix = stem.rsplit('_').next().unwrap_or(stem);
    let entities = entities(stem);
    let has = |field: &str| sidecar.get(field).is_some_and(|v| !v.is_null());
    let mut lacks: Vec<String> = Vec::new();
    let mut need = |field: &str| {
        if !has(field) && !lacks.iter().any(|f| f == field) {
            lacks.push(field.to_string());
        }
    };
    for rule in RULES {
        let applies = rule.datatype.is_none_or(|d| d == datatype)
            && (rule.suffixes.is_empty() || rule.suffixes.contains(&suffix))
            && rule
                .entity
                .is_none_or(|e| entities.iter().any(|(k, _)| *k == e));
        if applies {
            rule.fields.iter().for_each(|f| need(f));
        }
    }
    // the rules that read a sidecar's own value
    if entities.iter().any(|(k, v)| *k == "part" && *v == "phase") {
        need("Units");
    }
    if sidecar.get("LookLocker") == Some(&Value::Bool(true)) {
        need("FlipAngle");
    }
    if datatype == "perf" {
        if matches!(suffix, "asl" | "m0scan")
            && sidecar.get("MRAcquisitionType").and_then(Value::as_str) == Some("2D")
        {
            need("SliceTiming");
        }
        if suffix == "asl" {
            match sidecar
                .get("ArterialSpinLabelingType")
                .and_then(Value::as_str)
            {
                Some("CASL" | "PCASL") => need("LabelingDuration"),
                Some("PASL") => {
                    need("BolusCutOffFlag");
                    if sidecar.get("BolusCutOffFlag") == Some(&Value::Bool(true)) {
                        need("BolusCutOffDelayTime");
                        need("BolusCutOffTechnique");
                    }
                }
                _ => {}
            }
            if sidecar.get("M0Type").and_then(Value::as_str) == Some("Estimate") {
                need("M0Estimate");
            }
        }
    }
    // rules.checks
    if datatype == "fmap"
        && suffix == "epi"
        && !has("TotalReadoutTime")
        && !has("EffectiveEchoSpacing")
    {
        lacks.push("TotalReadoutTime or EffectiveEchoSpacing".to_string());
    }
    if suffix == "dwi" {
        for table in ["bval", "bvec"] {
            if !files.iter().any(|f| *f == format!("{stem}.{table}")) {
                lacks.push(format!("a .{table} file"));
            }
        }
    }
    if suffix == "T1w"
        && let Some(image) = files.iter().find(|f| f.contains(".nii"))
        && let Ok(rank) = super::aslcontext::rank(&dir.join(image))
        && rank != 3
    {
        lacks.push(format!("three dimensions, not {rank}"));
    }
    lacks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(
        stem: &str,
        sidecar: Value,
        more: &[&str],
    ) -> (nils_dicom::synth::TempDir, Vec<String>) {
        let dir = nils_dicom::synth::TempDir::new("sidecar-gaps");
        std::fs::write(dir.path().join(format!("{stem}.json")), sidecar.to_string()).unwrap();
        let mut files = vec![format!("{stem}.json")];
        for ext in more {
            std::fs::write(dir.path().join(format!("{stem}.{ext}")), b"0").unwrap();
            files.push(format!("{stem}.{ext}"));
        }
        (dir, files)
    }

    #[test]
    fn an_mp2rage_without_its_timings_lacks_them_by_name() {
        let stem = "sub-01_ses-01_inv-1_MP2RAGE";
        let (dir, files) = tree(
            stem,
            serde_json::json!({"FlipAngle": 4, "InversionTime": 0.7, "MagneticFieldStrength": 3}),
            &[],
        );
        assert_eq!(
            gaps("anat", stem, dir.path(), &files),
            [
                "RepetitionTimeExcitation",
                "RepetitionTimePreparation",
                "NumberShots"
            ]
        );
    }

    #[test]
    fn an_asl_image_from_its_headers_lacks_its_labelling_and_a_2d_one_its_slice_timing() {
        let stem = "sub-01_ses-01_acq-Ax_asl";
        let (dir, files) = tree(
            stem,
            serde_json::json!({"MagneticFieldStrength": 3, "EchoTime": 0.01, "MRAcquisitionType": "2D"}),
            &[],
        );
        let lacks = gaps("perf", stem, dir.path(), &files);
        for field in [
            "RepetitionTimePreparation",
            "ArterialSpinLabelingType",
            "PostLabelingDelay",
            "BackgroundSuppression",
            "M0Type",
            "TotalAcquiredPairs",
            "SliceTiming",
        ] {
            assert!(lacks.iter().any(|f| f == field), "{field}: {lacks:?}");
        }
    }

    #[test]
    fn an_echo_entity_needs_its_echo_time_and_a_diffusion_image_its_table() {
        let stem = "sub-01_ses-01_echo-1_angio";
        let (dir, files) = tree(stem, serde_json::json!({}), &["nii.gz"]);
        assert_eq!(gaps("anat", stem, dir.path(), &files), ["EchoTime"]);
        let stem = "sub-01_ses-01_dwi";
        let (dir, files) = tree(stem, serde_json::json!({}), &["nii.gz", "bval"]);
        assert_eq!(gaps("dwi", stem, dir.path(), &files), ["a .bvec file"]);
        let (dir, files) = tree(stem, serde_json::json!({}), &["nii.gz", "bval", "bvec"]);
        assert!(gaps("dwi", stem, dir.path(), &files).is_empty());
    }

    #[test]
    fn field_maps_need_their_units_direction_and_readout() {
        let stem = "sub-01_ses-01_fieldmap";
        let (dir, files) = tree(stem, serde_json::json!({}), &[]);
        assert_eq!(gaps("fmap", stem, dir.path(), &files), ["Units"]);
        let stem = "sub-01_ses-01_dir-PA_epi";
        let (dir, files) = tree(stem, serde_json::json!({}), &[]);
        assert_eq!(
            gaps("fmap", stem, dir.path(), &files),
            [
                "PhaseEncodingDirection",
                "TotalReadoutTime or EffectiveEchoSpacing"
            ]
        );
        let (dir, files) = tree(
            stem,
            serde_json::json!({"PhaseEncodingDirection": "j", "TotalReadoutTime": 0.05}),
            &[],
        );
        assert!(gaps("fmap", stem, dir.path(), &files).is_empty());
    }

    #[test]
    fn a_plain_t2w_with_what_dcm2niix_writes_lacks_nothing() {
        let stem = "sub-01_ses-01_acq-Ax_T2w";
        let (dir, files) = tree(
            stem,
            serde_json::json!({"EchoTime": 0.1, "RepetitionTime": 3}),
            &[],
        );
        assert!(gaps("anat", stem, dir.path(), &files).is_empty());
    }
}
