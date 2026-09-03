// SPDX-License-Identifier: AGPL-3.0-only

//! The view a pack is evaluated against
//! (`docs/specs/wave2-fingerprint-and-classify.md`, §4.2).
//!
//! This crate knows nothing about a registry. A pack plus one of these yields
//! a verdict, which is what makes a pack testable from a fixture and
//! shippable by someone who has never seen our schema.

/// Every field a pack may name, in the order the fingerprint declares them.
/// The first ten are numbers, the rest are text; a pack names them, never
/// their position.
pub const FIELDS: &[&str] = &[
    // numbers
    "echo_time",
    "repetition_time",
    "inversion_time",
    "flip_angle",
    "echo_train_length",
    "magnetic_field_strength",
    "slice_thickness",
    "spacing_between_slices",
    "number_of_averages",
    "n_instances",
    "stacks_in_series",
    "orientation_confidence",
    "rows",
    "columns",
    "fov_x",
    "fov_y",
    "aspect_ratio",
    // text
    "modality",
    "manufacturer",
    "manufacturer_model_name",
    "station_name",
    "implementation_class_uid",
    "implementation_version_name",
    "mr_acquisition_type",
    "orientation",
    "split_reason",
    "echo_numbers",
    "diffusion_b_value",
    "pixel_bandwidth",
    "pixel_spacing",
    "image_type",
    "scanning_sequence",
    "sequence_variant",
    "scan_options",
    "image_orientation_patient",
    "text_series_description",
    "text_protocol_name",
    "text_sequence_name",
    "text_body_part",
    "text_series_comments",
    "text_image_comments",
    "text_all",
    "text_contrast",
];

/// Where the text half begins.
pub const FIRST_TEXT: usize = 17;

pub fn field_index(name: &str) -> Option<usize> {
    FIELDS.iter().position(|f| *f == name)
}

/// One stack, as a pack sees it: numbers by index, text by index, nothing
/// else. Built by whoever has the row.
#[derive(Default, Clone)]
pub struct Stack {
    num: Vec<Option<f64>>,
    text: Vec<String>,
}

impl Stack {
    pub fn new() -> Stack {
        Stack {
            num: vec![None; FIRST_TEXT],
            text: vec![String::new(); FIELDS.len() - FIRST_TEXT],
        }
    }

    /// Set a field by name. An unknown name is a caller's mistake and is
    /// reported rather than ignored, since a silent miss is a wrong verdict.
    pub fn set(&mut self, name: &str, value: Value<'_>) -> Result<(), String> {
        let i = field_index(name).ok_or_else(|| format!("no field named {name}"))?;
        match (i < FIRST_TEXT, value) {
            (true, Value::Num(v)) => self.num[i] = v,
            (true, Value::Text(t)) => self.num[i] = t.and_then(|t| t.trim().parse().ok()),
            (false, Value::Text(t)) => self.text[i - FIRST_TEXT] = t.unwrap_or("").to_string(),
            (false, Value::Num(v)) => {
                self.text[i - FIRST_TEXT] = v.map(|x| x.to_string()).unwrap_or_default()
            }
        }
        Ok(())
    }

    /// The field as a number, when it reads as one. A text field that holds
    /// digits does, which is how v0's b value (stored as text) is compared.
    pub fn num(&self, i: usize) -> Option<f64> {
        if i < FIRST_TEXT {
            self.num[i]
        } else {
            self.text[i - FIRST_TEXT].trim().parse().ok()
        }
    }

    /// The field as text, empty when absent.
    pub fn text(&self, i: usize) -> &str {
        if i < FIRST_TEXT {
            ""
        } else {
            &self.text[i - FIRST_TEXT]
        }
    }

    /// Whether the field carries anything at all.
    pub fn present(&self, i: usize) -> bool {
        if i < FIRST_TEXT {
            self.num[i].is_some()
        } else {
            !self.text[i - FIRST_TEXT].is_empty()
        }
    }
}

/// What [`Stack::set`] takes, so a caller need not know which half a field is
/// in.
pub enum Value<'a> {
    Num(Option<f64>),
    Text(Option<&'a str>),
}

impl<'a> From<Option<&'a str>> for Value<'a> {
    fn from(v: Option<&'a str>) -> Value<'a> {
        Value::Text(v)
    }
}

impl From<Option<f64>> for Value<'_> {
    fn from(v: Option<f64>) -> Value<'static> {
        Value::Num(v)
    }
}

impl From<Option<i64>> for Value<'_> {
    fn from(v: Option<i64>) -> Value<'static> {
        Value::Num(v.map(|x| x as f64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_is_named_never_positioned() {
        let mut s = Stack::new();
        s.set("inversion_time", Value::Num(Some(2500.0))).unwrap();
        s.set("text_all", Value::Text(Some("ax t2 flair"))).unwrap();
        assert_eq!(s.num(field_index("inversion_time").unwrap()), Some(2500.0));
        assert_eq!(s.text(field_index("text_all").unwrap()), "ax t2 flair");
        assert_eq!(
            s.set("no_such_field", Value::Num(None)).unwrap_err(),
            "no field named no_such_field"
        );
    }

    #[test]
    fn a_text_field_of_digits_reads_as_a_number() {
        let mut s = Stack::new();
        s.set("diffusion_b_value", Value::Text(Some(" 1000 ")))
            .unwrap();
        assert_eq!(
            s.num(field_index("diffusion_b_value").unwrap()),
            Some(1000.0)
        );
        s.set("diffusion_b_value", Value::Text(Some("['0','1000']")))
            .unwrap();
        assert_eq!(s.num(field_index("diffusion_b_value").unwrap()), None);
    }

    #[test]
    fn an_absent_field_is_absent_in_both_halves() {
        let s = Stack::new();
        assert!(!s.present(field_index("echo_time").unwrap()));
        assert!(!s.present(field_index("text_all").unwrap()));
    }
}
