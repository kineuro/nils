// SPDX-License-Identifier: AGPL-3.0-only

//! What the file says, for a person reading a stack (record 48, after the
//! first real read): "Blind hides NILS's answers, never the file."
//!
//! - [`texts_and_physics`]: the header's text in full (series description,
//!   protocol name, sequence name and variant, scan options, image type and
//!   the other text the pack reads) and the physics, as the reader shows
//!   them on every item, blind or not.
//! - [`whole`]: the whole stored header of one representative instance of
//!   the stack, one key away: every column the digest keeps for its study,
//!   its series, the series' detail, the stack and the instance, less what
//!   names a person. Nothing of the subject's row is read; nothing the
//!   pseudonymiser always removes (the patient, trial, provider and
//!   institution groups) nor any direct identifier of the release's `ids`
//!   group is shown; the UIDs that name one file or study are left out; and
//!   below detail quasi every quasi-identifying column is.
//!
//! None of this is anything a system said of the stack: no value in force,
//! no rule, no vote, no matched word, no score. These are the inputs.

use std::collections::{BTreeMap, BTreeSet};

use dicom_core::Tag;
use nils_dicom::catalogue::{Level, Sensitivity, Source, Step, fields_of};
use nils_registry::schema::{Type, table};
use nils_registry::store::{Error as StoreError, Param, Store};
use serde_json::{Map, Value, json};

/// The header's text a reader is shown, key first: (key, table, column,
/// quasi-identifying). Read from the series, its MR detail, the study and
/// the representative instance; the sequence tokens from the stack's own
/// fingerprint, which holds the stack's value where a series held several.
const TEXTS: &[(&str, &str, &str, bool)] = &[
    ("series_description", "series", "series_description", true),
    ("protocol_name", "series", "protocol_name", true),
    ("sequence_name", "series", "sequence_name", true),
    ("sequence_variant", "fingerprint", "sequence_variant", false),
    (
        "scanning_sequence",
        "fingerprint",
        "scanning_sequence",
        false,
    ),
    ("scan_options", "fingerprint", "scan_options", false),
    ("image_type", "fingerprint", "image_type", false),
    (
        "mr_acquisition_type",
        "fingerprint",
        "mr_acquisition_type",
        false,
    ),
    ("body_part_examined", "series", "body_part_examined", false),
    (
        "contrast_bolus_agent",
        "series",
        "contrast_bolus_agent",
        false,
    ),
    (
        "contrast_bolus_route",
        "series",
        "contrast_bolus_route",
        false,
    ),
    ("angio_flag", "series_mr", "angio_flag", false),
    ("image_comments", "instance", "image_comments", true),
    (
        "derivation_description",
        "instance",
        "derivation_description",
        true,
    ),
    ("study_description", "study", "study_description", true),
];

/// The physics a reader is shown, key first, from the stack's fingerprint
/// (the stack's own values) and, where it has none, the MR detail.
const PHYSICS: &[(&str, &str)] = &[
    ("repetition_time", "fingerprint"),
    ("echo_time", "fingerprint"),
    ("inversion_time", "fingerprint"),
    ("flip_angle", "fingerprint"),
    ("echo_train_length", "fingerprint"),
    ("diffusion_b_value", "fingerprint"),
    ("dwi_b_values", "fingerprint"),
    ("pixel_bandwidth", "fingerprint"),
    ("magnetic_field_strength", "fingerprint"),
    ("manufacturer", "fingerprint"),
    ("manufacturer_model_name", "fingerprint"),
    ("slice_thickness", "fingerprint"),
    ("spacing_between_slices", "fingerprint"),
    ("rows", "fingerprint"),
    ("columns", "fingerprint"),
    ("acquisition_matrix", "fingerprint"),
    ("pixel_spacing", "fingerprint"),
    ("orientation", "fingerprint"),
    ("n_slices", "fingerprint"),
    ("n_instances", "fingerprint"),
    ("number_of_averages", "fingerprint"),
    ("imaged_nucleus", "series_mr"),
    ("contrast_bolus_volume", "series"),
];

/// The UIDs a reader may see: which kind of object and syntax the file is,
/// never which file or study.
const UIDS_SHOWN: &[&str] = &[
    "sop_class_uid",
    "transfer_syntax_uid",
    "implementation_class_uid",
];

/// The keys of one stack's rows: series, study, modality and its
/// representative instance (the lowest instance number of the stack).
struct Keys {
    series: i64,
    study: i64,
    instance: Option<i64>,
}

fn keys(store: &mut Store, stack: i64) -> Result<Option<Keys>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT k.series_id, r.study_id FROM {} k JOIN {} r ON r.id = k.series_id WHERE k.id = {}",
        store.qualified("stack"),
        store.qualified("series"),
        d.param(1, Type::Int)
    );
    let Some(r) = store.query_opt(&sql, &[Param::Int(stack)])? else {
        return Ok(None);
    };
    let (series, study) = (r.int(0)?, r.int(1)?);
    let sql = format!(
        "SELECT id FROM {} WHERE stack_id = {} ORDER BY instance_number, id LIMIT 1",
        store.qualified("instance"),
        d.param(1, Type::Int)
    );
    let mut instance = store
        .query_opt(&sql, &[Param::Int(stack)])?
        .map(|r| r.int(0))
        .transpose()?;
    if instance.is_none() {
        // a multi-frame file whose frames are in several stacks
        let sql = format!(
            "SELECT instance_id FROM {} WHERE stack_id = {} ORDER BY instance_id LIMIT 1",
            store.qualified("instance_frame"),
            d.param(1, Type::Int)
        );
        instance = store
            .query_opt(&sql, &[Param::Int(stack)])?
            .map(|r| r.int(0))
            .transpose()?;
    }
    Ok(Some(Keys {
        series,
        study,
        instance,
    }))
}

/// One row's columns, by name, where they hold something.
fn row(
    store: &mut Store,
    name: &str,
    key: &str,
    id: i64,
    columns: &[&str],
) -> Result<BTreeMap<String, Value>, StoreError> {
    let t = table(name);
    let cols: Vec<&str> = columns
        .iter()
        .copied()
        .filter(|c| t.column(c).is_some())
        .collect();
    if cols.is_empty() {
        return Ok(BTreeMap::new());
    }
    let d = store.dialect();
    let list = cols
        .iter()
        .map(|c| d.text_of(t.column(c).expect("a column")))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {list} FROM {} WHERE {key} = {}",
        store.qualified(name),
        d.param(1, Type::Int)
    );
    let mut out = BTreeMap::new();
    if let Some(r) = store.query_opt(&sql, &[Param::Int(id)])? {
        for (i, c) in cols.iter().enumerate() {
            let v = crate::reader::cell_json(r.get(i));
            if !v.is_null() {
                out.insert(c.to_string(), v);
            }
        }
    }
    Ok(out)
}

/// A value read as text by the store, back as a number where it is one.
fn typed(v: Value) -> Value {
    match &v {
        Value::String(s) => {
            if let Ok(i) = s.parse::<i64>() {
                json!(i)
            } else if let Ok(f) = s.parse::<f64>()
                && f.is_finite()
                && s.contains('.')
            {
                json!(f)
            } else {
                v
            }
        }
        _ => v,
    }
}

/// Two maps of `{key: value}`: the text, then the physics.
type TextsAndPhysics = (Map<String, Value>, Map<String, Value>);

/// The header's text and physics of a stack, each as `{key: value}` with
/// only what the file has, in the order a reader reads them. Below detail
/// quasi the quasi-identifying text is left out.
pub(crate) fn texts_and_physics(
    store: &mut Store,
    stack: i64,
    quasi: bool,
) -> Result<TextsAndPhysics, StoreError> {
    let Some(k) = keys(store, stack)? else {
        return Ok((Map::new(), Map::new()));
    };
    let want = |from: &str| -> Vec<&str> {
        TEXTS
            .iter()
            .filter(|(_, t, _, q)| *t == from && (quasi || !q))
            .map(|(_, _, c, _)| *c)
            .chain(PHYSICS.iter().filter(|(_, t)| *t == from).map(|(c, _)| *c))
            .collect()
    };
    let mut got: BTreeMap<&str, BTreeMap<String, Value>> = BTreeMap::new();
    got.insert(
        "fingerprint",
        row(
            store,
            "stack_fingerprint",
            "stack_id",
            stack,
            &want("fingerprint"),
        )?,
    );
    got.insert(
        "series",
        row(store, "series", "id", k.series, &want("series"))?,
    );
    got.insert(
        "series_mr",
        row(
            store,
            "series_mr",
            "series_id",
            k.series,
            &want("series_mr"),
        )?,
    );
    got.insert("study", row(store, "study", "id", k.study, &want("study"))?);
    if let Some(i) = k.instance {
        got.insert(
            "instance",
            row(store, "instance", "id", i, &want("instance"))?,
        );
    }
    let mut texts = Map::new();
    for (key, from, column, q) in TEXTS {
        if *q && !quasi {
            continue;
        }
        if let Some(v) = got.get(from).and_then(|r| r.get(*column)) {
            texts.insert(key.to_string(), v.clone());
        }
    }
    let mut physics = Map::new();
    for (column, from) in PHYSICS {
        if let Some(v) = got.get(from).and_then(|r| r.get(*column)) {
            physics.insert(column.to_string(), typed(v.clone()));
        }
    }
    Ok((texts, physics))
}

/// The tags a field of the catalogue reads.
fn tags_of(source: &Source) -> Vec<Tag> {
    match source {
        Source::Tag(t) | Source::TagOrMeta(t, _) => vec![*t],
        Source::Chain(steps) => steps
            .iter()
            .flat_map(|s| match s {
                Step::Top(t) | Step::Fg(_, t) | Step::Private(t) => vec![*t],
                Step::Item(seq, t) => vec![*seq, *t],
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The elements never shown: every group the pseudonymiser always removes,
/// and the direct identifiers of the release's `ids` group.
fn never_shown() -> BTreeSet<Tag> {
    let mut cats: Vec<nils_release::tags::Category> =
        nils_pseudonymize::rewrite::CATEGORIES.to_vec();
    cats.push(nils_release::tags::Category::Ids);
    let mut out: BTreeSet<Tag> = nils_release::tags::tags_of(&cats).into_iter().collect();
    // the direct identifiers the pseudonymiser replaces rather than removes
    out.extend([
        Tag(0x0010, 0x0010), // PatientName
        Tag(0x0010, 0x0020), // PatientID
        Tag(0x0010, 0x0021), // IssuerOfPatientID
        Tag(0x0010, 0x0030), // PatientBirthDate
        Tag(0x0010, 0x1000), // OtherPatientIDs
        Tag(0x0010, 0x1001), // OtherPatientNames
        Tag(0x0010, 0x1002), // OtherPatientIDsSequence
        Tag(0x0010, 0x1040), // PatientAddress
    ]);
    out
}

/// The whole stored header of one representative instance of a stack, less
/// what names a person (see the module's note). `None` where the stack is
/// not in the registry.
pub(crate) fn whole(
    store: &mut Store,
    stack: i64,
    quasi: bool,
) -> Result<Option<Value>, StoreError> {
    let Some(k) = keys(store, stack)? else {
        return Ok(None);
    };
    let never = never_shown();
    let mut fields = Vec::new();
    let (mut identifying, mut below) = (0usize, 0usize);
    for level in Level::ALL {
        let (name, key, id) = match level {
            Level::Subject => {
                identifying += fields_of(level).count();
                continue;
            }
            Level::Study => ("study", "id", k.study),
            Level::Series => ("series", "id", k.series),
            Level::SeriesMr => ("series_mr", "series_id", k.series),
            Level::SeriesCt => ("series_ct", "series_id", k.series),
            Level::SeriesPet => ("series_pet", "series_id", k.series),
            Level::Stack => ("stack", "id", stack),
            Level::Instance => match k.instance {
                Some(i) => ("instance", "id", i),
                None => continue,
            },
        };
        let mut shown: Vec<(&str, String)> = Vec::new();
        for (_, f) in fields_of(level) {
            if matches!(f.source, Source::None) {
                continue;
            }
            let removed = tags_of(&f.source).iter().any(|t| never.contains(t))
                || (f.column.ends_with("_uid") && !UIDS_SHOWN.contains(&f.column));
            if removed || f.class == Sensitivity::Identifying {
                identifying += 1;
                continue;
            }
            if !quasi && f.class == Sensitivity::QuasiIdentifying {
                below += 1;
                continue;
            }
            shown.push((f.column, f.source.text()));
        }
        let columns: Vec<&str> = shown.iter().map(|(c, _)| *c).collect();
        let got = row(store, name, key, id, &columns)?;
        for (column, source) in shown {
            if let Some(v) = got.get(column) {
                // the element's keyword and tag, as the source names them first
                let keyword = source
                    .split([' ', ',', '[', '.'])
                    .next()
                    .unwrap_or_default();
                let tag = source
                    .find('(')
                    .and_then(|i| source.get(i..i + 11))
                    .filter(|t| t.ends_with(')'));
                fields.push(json!({
                    "level": level.name(), "column": column, "source": source,
                    "keyword": keyword, "tag": tag, "value": v,
                }));
            }
        }
    }
    let instance_number = match k.instance {
        Some(i) => row(store, "instance", "id", i, &["instance_number"])?
            .remove("instance_number")
            .map(typed),
        None => None,
    };
    Ok(Some(json!({
        "stack": stack,
        "detail": if quasi { "quasi" } else { "plain" },
        "instance": {"instance_number": instance_number},
        "fields": fields,
        "left_out": {"identifying": identifying, "below_detail": below},
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The groups the pseudonymiser always removes are never shown: the
    /// patient's name, id and birth date, the institution, the station.
    #[test]
    fn what_names_a_person_is_never_shown() {
        let never = never_shown();
        for t in [
            dicom_dictionary_std::tags::PATIENT_NAME,
            dicom_dictionary_std::tags::PATIENT_ID,
            dicom_dictionary_std::tags::PATIENT_BIRTH_DATE,
            dicom_core::Tag(0x0010, 0x1000), // OtherPatientIDs, retired
            dicom_dictionary_std::tags::PATIENT_ADDRESS,
            dicom_dictionary_std::tags::INSTITUTION_NAME,
            dicom_dictionary_std::tags::ACCESSION_NUMBER,
            dicom_dictionary_std::tags::STATION_NAME,
        ] {
            assert!(never.contains(&t), "{t:?}");
        }
        // and the header a reader reads is not among them
        for t in [
            dicom_dictionary_std::tags::SERIES_DESCRIPTION,
            dicom_dictionary_std::tags::PROTOCOL_NAME,
            dicom_dictionary_std::tags::SEQUENCE_NAME,
            dicom_dictionary_std::tags::IMAGE_TYPE,
        ] {
            assert!(!never.contains(&t), "{t:?}");
        }
    }
}
