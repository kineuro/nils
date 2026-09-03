// SPDX-License-Identifier: AGPL-3.0-only

//! The fingerprint (`docs/specs/wave2-fingerprint-and-classify.md`, §4).
//!
//! One row per stack: the five-table join a classifier would otherwise do per
//! stack, materialized and typed. Reading it is what makes a rule read a value
//! instead of a parser, and what makes a verdict reproducible from what is
//! stored.

use nils_registry::schema::{Table, Type, table};
use nils_registry::store::{Error, Param, Row, Store};

use crate::fold;

/// The stack's own columns, in the order the select reads them.
const STACK: &[&str] = &[
    "id",
    "series_id",
    "stack_index",
    "stack_key",
    "modality",
    "orientation",
    "orientation_confidence",
    "n_instances",
    "image_type",
    "image_orientation_patient",
    "echo_time",
    "repetition_time",
    "inversion_time",
    "flip_angle",
    "echo_train_length",
    "echo_numbers",
];

/// The series' columns. `series_comments` is always null (v0 named a keyword
/// that is no DICOM element) and is read anyway, so that the day it holds
/// something the fingerprint already carries it.
const SERIES: &[&str] = &[
    "study_id",
    "subject_id",
    "n_stacks",
    "series_description",
    "protocol_name",
    "sequence_name",
    "body_part_examined",
    "series_comments",
    "image_type",
    "scanning_sequence",
    "sequence_variant",
    "scan_options",
    "image_orientation_patient",
    "slice_thickness",
    "spacing_between_slices",
    "implementation_class_uid",
    "implementation_version_name",
    "contrast_bolus_agent",
    "contrast_bolus_route",
    "contrast_bolus_total_dose",
    "contrast_bolus_start_time",
    "contrast_bolus_volume",
    "contrast_flow_rate",
    "contrast_flow_duration",
];

/// The MR detail columns, absent for a CT or PET stack.
const MR: &[&str] = &[
    "mr_acquisition_type",
    "magnetic_field_strength",
    "number_of_averages",
    "pixel_bandwidth",
    "diffusion_b_value",
    "echo_time",
    "repetition_time",
    "inversion_time",
    "flip_angle",
    "echo_train_length",
    "echo_numbers",
];

const STUDY: &[&str] = &["manufacturer", "manufacturer_model_name", "station_name"];

const S: usize = STACK.len();
/// Where `stacks_in_series` sits in the window's row.
pub const STACKS_IN_SERIES: usize = S + 2;
const E: usize = S + SERIES.len();
const M: usize = E + MR.len();

/// The columns the fingerprint row is written from, in this order.
pub const WRITTEN: &[&str] = &[
    "stack_id",
    "series_id",
    "study_id",
    "subject_id",
    "modality",
    "text_series_description",
    "text_protocol_name",
    "text_sequence_name",
    "text_body_part",
    "text_series_comments",
    "text_image_comments",
    "text_all",
    "text_contrast",
    "image_type",
    "scanning_sequence",
    "sequence_variant",
    "scan_options",
    "image_orientation_patient",
    "echo_time",
    "repetition_time",
    "inversion_time",
    "flip_angle",
    "echo_train_length",
    "echo_numbers",
    "diffusion_b_value",
    "magnetic_field_strength",
    "slice_thickness",
    "spacing_between_slices",
    "number_of_averages",
    "pixel_bandwidth",
    "mr_acquisition_type",
    "orientation",
    "orientation_confidence",
    "n_instances",
    "stack_index",
    "signature",
    "stacks_in_series",
    "split_reason",
    "rows",
    "columns",
    "pixel_spacing",
    "fov_x",
    "fov_y",
    "aspect_ratio",
    "manufacturer",
    "manufacturer_model_name",
    "station_name",
    "implementation_class_uid",
    "implementation_version_name",
    "job_id",
    "epoch",
];

/// The columns an upsert overwrites: everything but the key.
pub fn overwritten() -> Vec<&'static str> {
    WRITTEN
        .iter()
        .copied()
        .filter(|c| *c != "stack_id")
        .collect()
}

/// The geometry of the stack's first instance, by SOP Instance UID.
#[derive(Default, Clone)]
pub struct First {
    pub pixel_spacing: Option<String>,
    pub rows: Option<i64>,
    pub columns: Option<i64>,
    pub image_comments: Option<String>,
}

/// The select that reads one window of stacks, ordered by id.
pub fn select(store: &Store, extra: &str) -> String {
    let cols = |alias: &str, names: &[&str]| -> String {
        names
            .iter()
            .map(|c| format!("{alias}.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "SELECT {}, {}, {}, {} \
         FROM {} st \
         JOIN {} se ON se.id = st.series_id \
         JOIN {} sy ON sy.id = se.study_id \
         LEFT JOIN {} mr ON mr.series_id = se.id \
         WHERE st.id > {}{extra} ORDER BY st.id LIMIT {}",
        cols("st", STACK),
        cols("se", SERIES),
        cols("mr", MR),
        cols("sy", STUDY),
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("study"),
        store.qualified("series_mr"),
        store.dialect().param(1, Type::Int),
        store.dialect().param(2, Type::Int),
    )
}

/// The first instance of every stack in the window `(after, last]`, by SOP
/// Instance UID rather than by row id, so that the fingerprint does not depend
/// on the order the walk happened to insert in (Wave 1 §15's open item).
pub fn select_first_instances(store: &Store) -> String {
    format!(
        "SELECT stack_id, pixel_spacing, rows, columns, image_comments FROM (\
           SELECT stack_id, pixel_spacing, rows, columns, image_comments, \
                  ROW_NUMBER() OVER (PARTITION BY stack_id ORDER BY sop_instance_uid) AS rn \
           FROM {} WHERE stack_id > {} AND stack_id <= {}\
         ) t WHERE rn = 1",
        store.qualified("instance"),
        store.dialect().param(1, Type::Int),
        store.dialect().param(2, Type::Int),
    )
}

/// The stack ids in the window that already have a fingerprint agreeing with
/// the stack's instance count. A stack that gained instances since is stale
/// and is derived again.
pub fn select_fresh(store: &Store) -> String {
    format!(
        "SELECT f.stack_id FROM {} f JOIN {} st ON st.id = f.stack_id \
         WHERE f.stack_id > {} AND f.stack_id <= {} AND f.n_instances = st.n_instances",
        store.qualified("stack_fingerprint"),
        store.qualified("stack"),
        store.dialect().param(1, Type::Int),
        store.dialect().param(2, Type::Int),
    )
}

fn text(r: &Row, i: usize) -> Result<Option<String>, Error> {
    Ok(r.opt_text(i)?.map(str::to_string))
}

/// Prefer the stack's own value, fall back to the series'. A series with
/// several stacks holds the first stack's timing on its own row, which is the
/// stack's only by accident.
fn double_of(r: &Row, stack: usize, series: usize) -> Result<Option<f64>, Error> {
    Ok(match r.get(stack) {
        nils_registry::store::Cell::Null => opt_double(r, series)?,
        _ => opt_double(r, stack)?,
    })
}

fn opt_double(r: &Row, i: usize) -> Result<Option<f64>, Error> {
    Ok(match r.get(i) {
        nils_registry::store::Cell::Null => None,
        _ => Some(r.double(i)?),
    })
}

fn opt_int(r: &Row, i: usize) -> Result<Option<i64>, Error> {
    r.opt_int(i)
}

fn text_of(r: &Row, stack: usize, series: usize) -> Result<Option<String>, Error> {
    Ok(match text(r, stack)? {
        Some(v) => Some(v),
        None => text(r, series)?,
    })
}

/// v0's FOV: the column spacing times the columns, the row spacing times the
/// rows, each rounded to two places, and the ratio of the larger to the
/// smaller rounded to three (`sort/fingerprint.py`).
fn fov(first: &First) -> (Option<f64>, Option<f64>, Option<f64>) {
    let spacing = first.pixel_spacing.as_deref().unwrap_or("");
    let mut it = spacing.split('\\');
    let row_sp: Option<f64> = it.next().and_then(|v| v.trim().parse().ok());
    let col_sp: Option<f64> = it.next().and_then(|v| v.trim().parse().ok());
    let round = |v: f64, places: i32| {
        let f = 10f64.powi(places);
        (v * f).round() / f
    };
    let x = match (col_sp, first.columns) {
        (Some(sp), Some(n)) => Some(round(sp * n as f64, 2)),
        _ => None,
    };
    let y = match (row_sp, first.rows) {
        (Some(sp), Some(n)) => Some(round(sp * n as f64, 2)),
        _ => None,
    };
    let ratio = match (x, y) {
        (Some(a), Some(b)) if a > 0.0 && b > 0.0 => Some(round(a.max(b) / a.min(b), 3)),
        _ => None,
    };
    (x, y, ratio)
}

/// One read row plus its first instance, as the parameters of one write.
pub fn derive(
    r: &Row,
    first: &First,
    split_reason: Option<&str>,
    job_id: i64,
    epoch: i64,
) -> Result<Vec<Param>, Error> {
    let stack_id = r.int(0)?;
    let series_id = r.int(1)?;

    let description = text(r, S + 3)?;
    let protocol = text(r, S + 4)?;
    let sequence_name = text(r, S + 5)?;
    let body_part = text(r, S + 6)?;
    let series_comments = text(r, S + 7)?;
    let image_comments = first.image_comments.clone();

    let f_description = fold::fold(description.as_deref());
    let f_protocol = fold::fold(protocol.as_deref());
    let f_sequence = fold::fold(sequence_name.as_deref());
    let f_body_part = fold::fold(body_part.as_deref());
    let f_series_comments = fold::fold(series_comments.as_deref());
    let f_image_comments = fold::fold(image_comments.as_deref());

    // v0 joins the six raw fields and normalizes the join; folding each first
    // and joining after gives the same string, since folding is idempotent and
    // the join is on a single space.
    let text_all = fold::join(&[
        f_description.as_deref(),
        f_protocol.as_deref(),
        f_sequence.as_deref(),
        f_body_part.as_deref(),
        f_series_comments.as_deref(),
        f_image_comments.as_deref(),
    ]);

    let text_contrast = fold::contrast(
        text(r, S + 17)?.as_deref(),
        text(r, S + 18)?.as_deref(),
        opt_double(r, S + 19)?,
        text(r, S + 20)?.as_deref(),
        opt_double(r, S + 21)?,
        opt_double(r, S + 22)?,
        opt_double(r, S + 23)?,
    );

    let (fov_x, fov_y, aspect) = fov(first);

    Ok(vec![
        Param::Int(stack_id),
        Param::Int(series_id),
        Param::Int(r.int(S)?),     // study_id
        Param::Int(r.int(S + 1)?), // subject_id
        Param::from(r.text(4)?),   // modality
        opt(f_description),
        opt(f_protocol),
        opt(f_sequence),
        opt(f_body_part),
        opt(f_series_comments),
        opt(f_image_comments),
        opt(text_all),
        opt(text_contrast),
        opt(text_of(r, 8, S + 8)?),    // image_type
        opt(text(r, S + 9)?),          // scanning_sequence
        opt(text(r, S + 10)?),         // sequence_variant
        opt(text(r, S + 11)?),         // scan_options
        opt(text_of(r, 9, S + 12)?),   // image_orientation_patient
        num(double_of(r, 10, E + 5)?), // echo_time
        num(double_of(r, 11, E + 6)?), // repetition_time
        num(double_of(r, 12, E + 7)?), // inversion_time
        num(double_of(r, 13, E + 8)?), // flip_angle
        int(match opt_int(r, 14)? {
            Some(v) => Some(v),
            None => opt_int(r, E + 9)?,
        }), // echo_train_length
        opt(text_of(r, 15, E + 10)?),  // echo_numbers
        opt(text(r, E + 4)?),          // diffusion_b_value
        num(opt_double(r, E + 1)?),    // magnetic_field_strength
        num(opt_double(r, S + 13)?),   // slice_thickness
        num(opt_double(r, S + 14)?),   // spacing_between_slices
        num(opt_double(r, E + 2)?),    // number_of_averages
        opt(text(r, E + 3)?),          // pixel_bandwidth
        opt(text(r, E)?),              // mr_acquisition_type
        Param::from(r.text(5)?),       // orientation
        num(opt_double(r, 6)?),        // orientation_confidence
        Param::Int(r.int(7)?),         // n_instances
        Param::Int(r.int(2)?),         // stack_index
        opt(text(r, 3)?),              // signature
        Param::Int(r.int(S + 2)?),     // stacks_in_series
        opt(split_reason.map(str::to_string)),
        int(first.rows),
        int(first.columns),
        opt(first.pixel_spacing.clone()),
        num(fov_x),
        num(fov_y),
        num(aspect),
        opt(text(r, M)?),     // manufacturer
        opt(text(r, M + 1)?), // manufacturer_model_name
        opt(text(r, M + 2)?), // station_name
        opt(text(r, S + 15)?),
        opt(text(r, S + 16)?),
        Param::Int(job_id),
        Param::Int(epoch),
    ])
}

fn opt(v: Option<String>) -> Param {
    match v {
        Some(s) => Param::from(s),
        None => Param::Null,
    }
}

fn num(v: Option<f64>) -> Param {
    match v {
        Some(x) => Param::Double(x),
        None => Param::Null,
    }
}

fn int(v: Option<i64>) -> Param {
    match v {
        Some(x) => Param::Int(x),
        None => Param::Null,
    }
}

/// The table this module writes.
pub fn fingerprint_table() -> &'static Table {
    table("stack_fingerprint")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fov_follows_v0_rounding() {
        let f = First {
            pixel_spacing: Some("0.4297\\0.4297".into()),
            rows: Some(512),
            columns: Some(512),
            image_comments: None,
        };
        let (x, y, r) = fov(&f);
        assert_eq!(x, Some(220.01));
        assert_eq!(y, Some(220.01));
        assert_eq!(r, Some(1.0));
    }

    #[test]
    fn an_aspect_needs_both_sides() {
        let f = First {
            pixel_spacing: Some("0.5".into()),
            rows: Some(256),
            columns: Some(256),
            image_comments: None,
        };
        let (x, y, r) = fov(&f);
        assert_eq!(x, None);
        assert_eq!(y, Some(128.0));
        assert_eq!(r, None);
    }

    #[test]
    fn a_rectangular_field_has_a_ratio_above_one() {
        let f = First {
            pixel_spacing: Some("1\\0.5".into()),
            rows: Some(128),
            columns: Some(256),
            image_comments: None,
        };
        let (x, y, r) = fov(&f);
        assert_eq!((x, y), (Some(128.0), Some(128.0)));
        assert_eq!(r, Some(1.0));
    }

    #[test]
    fn the_written_columns_are_the_declared_ones() {
        let t = fingerprint_table();
        for c in WRITTEN {
            assert!(t.column(c).is_some(), "{c} is not a column of the table");
        }
        let declared: Vec<&str> = t.data_columns().map(|c| c.name).collect();
        assert_eq!(declared.len(), WRITTEN.len(), "{declared:?}");
    }
}

/// Why a series split into several stacks, in v0's order
/// (`sort/stack_key.py`). `None` for a series with one stack.
///
/// v0 computes this, stores it on the stack, and then never selects it into
/// the fingerprint its classifier reads, so its `is_multi_echo`,
/// `is_multi_ti` and `is_multi_fa` flags are false for every stack it has
/// ever classified. The value is a fact about the series, so it belongs here
/// and a pack reads it as `split_reason`.
pub fn split_reason(varying: &std::collections::BTreeSet<&str>) -> Option<&'static str> {
    if varying.is_empty() {
        return Some("multi_stack");
    }
    let any = |names: &[&str]| names.iter().any(|n| varying.contains(n));
    Some(
        if any(&["echo_time", "echo_numbers", "echo_train_length"]) {
            "multi_echo"
        } else if any(&["image_type"]) {
            "image_type_variation"
        } else if any(&["image_orientation_patient"]) {
            "multi_orientation"
        } else if any(&["pet_bed_index"]) {
            "multi_bed"
        } else if any(&["inversion_time"]) {
            "multi_ti"
        } else if any(&["flip_angle"]) {
            "multi_flip_angle"
        } else if any(&["receive_coil_name"]) {
            "multi_coil"
        } else if varying.len() > 1 {
            "multi_parameter"
        } else {
            "multi_stack"
        },
    )
}

/// A cell as text, for the only thing the split reason needs: whether two
/// stacks of one series agree on a signature column.
pub fn same(a: &nils_registry::store::Cell, b: &nils_registry::store::Cell) -> bool {
    a == b
}

#[cfg(test)]
mod split_tests {
    use super::split_reason;
    use std::collections::BTreeSet;

    fn of(names: &[&str]) -> Option<&'static str> {
        let set: BTreeSet<&str> = names.iter().copied().collect();
        split_reason(&set)
    }

    #[test]
    fn the_echo_family_wins_first() {
        assert_eq!(of(&["echo_time"]), Some("multi_echo"));
        assert_eq!(of(&["echo_train_length"]), Some("multi_echo"));
        // even against a later reason
        assert_eq!(of(&["echo_numbers", "flip_angle"]), Some("multi_echo"));
    }

    #[test]
    fn the_order_after_it_is_v0s() {
        assert_eq!(of(&["image_type"]), Some("image_type_variation"));
        assert_eq!(
            of(&["image_orientation_patient", "inversion_time"]),
            Some("multi_orientation")
        );
        assert_eq!(of(&["inversion_time"]), Some("multi_ti"));
        assert_eq!(of(&["flip_angle"]), Some("multi_flip_angle"));
        assert_eq!(of(&["receive_coil_name"]), Some("multi_coil"));
    }

    #[test]
    fn several_unnamed_reasons_are_multi_parameter_and_one_is_not() {
        assert_eq!(of(&["kvp", "tube_current"]), Some("multi_parameter"));
        assert_eq!(of(&["repetition_time"]), Some("multi_stack"));
        assert_eq!(of(&[]), Some("multi_stack"));
    }
}
