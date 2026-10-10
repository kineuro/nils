// SPDX-License-Identifier: AGPL-3.0-only

//! The `NILS` object in every BIDS sidecar (record 55 C4, ruled 2026-10-08:
//! "side card can have all axes info and details we need like FOV and
//! quality and so on").
//!
//! A name spells some of what NILS knows about a stack, and the minimal
//! naming style spells almost none of it, so the sidecar carries all of it:
//! every classification axis with the tier that decided it, the descriptive
//! name of v0's grammar, and the acquisition as the registry measured it.
//!
//! **Only technical facts.** Everything here is a decided axis or a column
//! of the fingerprint that describes the acquisition. The station name, the
//! receive coil's name and every free text are left out, because they can
//! name a place; dates and times are the release's to write in their own
//! slots under its policy, never here.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::repeat::Acquisition;

/// How each axis was decided: the tier and the confidence, by axis.
pub type Tiers = BTreeMap<String, (String, f64)>;

/// The `NILS` object of one stack.
pub fn card(
    descriptive: &str,
    said: Option<&BTreeMap<String, Vec<String>>>,
    tiers: Option<&Tiers>,
    acquisition: Option<&Acquisition>,
) -> Value {
    let mut axes = Map::new();
    for (axis, values) in said.into_iter().flatten() {
        let mut one = Map::new();
        one.insert("values".into(), json!(values));
        if let Some((tier, confidence)) = tiers.and_then(|t| t.get(axis)) {
            one.insert("tier".into(), json!(tier));
            one.insert("confidence".into(), json!(confidence));
        }
        axes.insert(axis.clone(), Value::Object(one));
    }
    let mut out = Map::new();
    out.insert("DescriptiveName".into(), json!(descriptive));
    out.insert("Axes".into(), Value::Object(axes));
    if let Some(a) = acquisition {
        out.insert("Acquisition".into(), Value::Object(details(a)));
    }
    Value::Object(out)
}

/// The acquisition, as the fingerprint holds it, in DICOM's units: lengths
/// in millimetres, times in milliseconds, angles in degrees, the field in
/// tesla. A value the file did not state is left out rather than guessed.
fn details(a: &Acquisition) -> Map<String, Value> {
    let mut out = Map::new();
    let mut put = |key: &str, v: Option<Value>| {
        if let Some(v) = v {
            out.insert(key.to_string(), v);
        }
    };
    put("Manufacturer", a.manufacturer.as_ref().map(|v| json!(v)));
    put("ManufacturersModelName", a.model.as_ref().map(|v| json!(v)));
    put("MagneticFieldStrengthT", a.field_strength.map(|v| json!(v)));
    put(
        "AcquisitionType",
        a.acquisition_type.as_ref().map(|v| json!(v)),
    );
    put("RepetitionTimeMs", a.repetition_time.map(|v| json!(v)));
    put("EchoTimeMs", a.echo_time.map(|v| json!(v)));
    put("InversionTimeMs", a.inversion_time.map(|v| json!(v)));
    put("FlipAngleDeg", a.flip_angle.map(|v| json!(v)));
    put("EchoNumbers", a.echo_numbers.as_ref().map(|v| json!(v)));
    put("EchoTrainLength", a.echo_train_length.map(|v| json!(v)));
    put("NumberOfAverages", a.averages.map(|v| json!(v)));
    put("PixelBandwidthHz", a.pixel_bandwidth.map(|v| json!(v)));
    put("SliceThicknessMm", a.slice_thickness.map(|v| json!(v)));
    put("SpacingBetweenSlicesMm", a.slice_spacing.map(|v| json!(v)));
    put(
        "PixelSpacingMm",
        match (a.pixel_spacing_row, a.pixel_spacing_col) {
            (Some(r), Some(c)) => Some(json!([r, c])),
            _ => None,
        },
    );
    put(
        "VoxelSizeMm",
        match (a.pixel_spacing_row, a.pixel_spacing_col, a.slice_thickness) {
            (Some(r), Some(c), Some(t)) => Some(json!([r, c, t])),
            _ => None,
        },
    );
    put(
        "Matrix",
        match (a.rows, a.columns) {
            (Some(r), Some(c)) => Some(json!([r, c])),
            _ => None,
        },
    );
    put("AcquisitionMatrix", a.matrix.as_ref().map(|v| json!(v)));
    put(
        "FieldOfViewMm",
        match (a.rows, a.columns, a.pixel_spacing_row, a.pixel_spacing_col) {
            (Some(r), Some(c), Some(sr), Some(sc)) => Some(json!([
                (r as f64 * sr * 100.0).round() / 100.0,
                (c as f64 * sc * 100.0).round() / 100.0
            ])),
            _ => None,
        },
    );
    put("NumberOfSlices", a.coverage.n_slices.map(|v| json!(v)));
    put("SliceCoverageMm", a.coverage.span_mm.map(|v| json!(v)));
    put("NumberOfImages", a.images.map(|v| json!(v)));
    put("SliceOrientation", a.orientation.as_ref().map(|v| json!(v)));
    put("BValue", a.b_value.map(|v| json!(v)));
    put("BValues", a.b_values.as_ref().map(|v| json!(v)));
    put("DiffusionDirections", a.directions.map(|v| json!(v)));
    put(
        "PhaseEncodingDirection",
        a.pe_direction.as_ref().map(|v| json!(v)),
    );
    put("TemporalPositions", a.temporal_positions.map(|v| json!(v)));
    put(
        "ScanningSequence",
        a.scanning_sequence.as_ref().map(|v| json!(v)),
    );
    put(
        "SequenceVariant",
        a.sequence_variant.as_ref().map(|v| json!(v)),
    );
    put("ScanOptions", a.scan_options.as_ref().map(|v| json!(v)));
    put("ImageType", a.image_type.as_ref().map(|v| json!(v)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_carries_the_axes_the_name_and_the_acquisition_and_no_place() {
        let said: BTreeMap<String, Vec<String>> = [
            ("base".to_string(), vec!["T1w".to_string()]),
            ("post_contrast".to_string(), vec!["given".to_string()]),
        ]
        .into();
        let tiers: Tiers = [("post_contrast".to_string(), ("rule".to_string(), 0.9))].into();
        let a = Acquisition {
            rows: Some(256),
            columns: Some(240),
            pixel_spacing_row: Some(1.0),
            pixel_spacing_col: Some(0.9),
            slice_thickness: Some(1.0),
            repetition_time: Some(2300.0),
            station: Some("a station".into()),
            coil: Some("a coil".into()),
            protocol: Some("a protocol".into()),
            ..Acquisition::default()
        };
        let c = card("Sag_T1w_3D_MPRAGE_CE", Some(&said), Some(&tiers), Some(&a));
        assert_eq!(c["DescriptiveName"], "Sag_T1w_3D_MPRAGE_CE");
        assert_eq!(c["Axes"]["post_contrast"]["values"], json!(["given"]));
        assert_eq!(c["Axes"]["post_contrast"]["tier"], "rule");
        assert_eq!(c["Acquisition"]["FieldOfViewMm"], json!([256.0, 216.0]));
        assert_eq!(c["Acquisition"]["VoxelSizeMm"], json!([1.0, 0.9, 1.0]));
        assert_eq!(c["Acquisition"]["Matrix"], json!([256, 240]));
        let text = c.to_string();
        for never in ["a station", "a coil", "a protocol"] {
            assert!(!text.contains(never), "{text}");
        }
    }
}
