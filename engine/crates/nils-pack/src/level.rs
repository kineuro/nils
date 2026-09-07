// SPDX-License-Identifier: AGPL-3.0-only

//! Comparability levels (`docs/specs/wave4b-the-ask.md`, §6): what "the
//! same acquisition" means at a named level, as pack data. The categorical
//! axes and the acquisition type compare exactly at every level; the numeric
//! physics compare rounded to a step, or not at all.

use std::collections::BTreeMap;

use crate::error::{Error, R};
use crate::yaml::{self, File};

/// The fingerprint's numeric physics a level may round or ignore.
pub const PHYSICS: &[&str] = &[
    "magnetic_field_strength",
    "repetition_time",
    "echo_time",
    "inversion_time",
    "flip_angle",
    "echo_train_length",
    "slice_thickness",
    "spacing_between_slices",
    "pixel_spacing_row",
    "pixel_spacing_col",
    "number_of_averages",
];

/// A field every level may name beside the axes and the physics.
pub const SHAPE: &[&str] = &["acquisition_type", "orientation"];

#[derive(Debug, Clone, PartialEq)]
pub struct Level {
    pub name: String,
    pub description: String,
    /// Compared as they are: axes, the acquisition type, physics to the digit.
    pub exact: Vec<String>,
    /// Compared rounded to a step.
    pub rounded: BTreeMap<String, f64>,
    /// Not compared at this level.
    pub ignored: Vec<String>,
}

impl Level {
    /// Every field the level's signature carries, exact first, then
    /// rounded, in the order declared.
    pub fn members(&self) -> impl Iterator<Item = &str> {
        self.exact
            .iter()
            .map(String::as_str)
            .chain(self.rounded.keys().map(String::as_str))
    }
}

pub fn load(f: &File, axes: &[String]) -> R<Level> {
    let m = f.blame(yaml::obj(&f.value, "level"))?;
    let name = f.blame(yaml::text(yaml::get(m, "level", "level")?, "level"))?;
    let description = match m.get("description") {
        Some(v) => f.blame(yaml::text(v, "description"))?,
        None => String::new(),
    };
    let exact = match m.get("exact") {
        Some(v) => f.blame(yaml::texts(v, "exact"))?,
        None => Vec::new(),
    };
    let mut rounded = BTreeMap::new();
    if let Some(v) = m.get("rounded") {
        for (k, step) in f.blame(yaml::obj(v, "rounded"))? {
            let n = f.blame(yaml::number(step, &format!("rounded.{k}")))?;
            if n <= 0.0 {
                return Err(
                    Error::at(format!("rounded.{k}"), "a step is a positive number")
                        .in_file(&f.path, Some(&f.source)),
                );
            }
            rounded.insert(k.clone(), n);
        }
    }
    let ignored = match m.get("ignored") {
        Some(v) => f.blame(yaml::texts(v, "ignored"))?,
        None => Vec::new(),
    };
    let known = |field: &str| {
        axes.iter().any(|a| a == field) || PHYSICS.contains(&field) || SHAPE.contains(&field)
    };
    for (slot, fields) in [
        ("exact", exact.clone()),
        ("rounded", rounded.keys().cloned().collect()),
        ("ignored", ignored.clone()),
    ] {
        for field in fields {
            if !known(&field) {
                return Err(Error::at(
                    format!("{slot}: {field}"),
                    "is neither an axis of this pack, the acquisition type, the orientation nor a physics field of the fingerprint",
                )
                .in_file(&f.path, Some(&f.source)));
            }
            if slot != "exact" && axes.iter().any(|a| a == &field) {
                return Err(Error::at(
                    format!("{slot}: {field}"),
                    "an axis compares exactly at every level; only the numbers widen",
                )
                .in_file(&f.path, Some(&f.source)));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    for field in exact.iter().chain(rounded.keys()).chain(ignored.iter()) {
        if !seen.insert(field.as_str()) {
            return Err(Error::at(field.clone(), "is named twice in one level")
                .in_file(&f.path, Some(&f.source)));
        }
    }
    Ok(Level {
        name,
        description,
        exact,
        rounded,
        ignored,
    })
}
