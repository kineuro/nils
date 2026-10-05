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
    // Wave 3 §6: what the fingerprint worked out rather than read. Readable
    // by a pack because that is the point of putting them there: the
    // disposition of §7 is decided from them.
    "field_strength_normalized",
    "dwi_directions",
    // Record 37 S1: what the stack covers, counted from the positions its
    // images sit on rather than from how many images there are.
    "n_slices",
    "slice_span_mm",
    // MRI pack 0.10.0: how many time points the series holds, where the
    // file says (NumberOfTemporalPositions), and the series number, which
    // Siemens moves by 1000 for a single-band reference. Both were in the
    // fingerprint and no rule could read them.
    "temporal_positions",
    "series_number",
    // The 2026-10-03 fields: how fast a dynamic series samples, in
    // milliseconds (TemporalResolution where the series writes one above
    // nought, else the frame interval of its images; which one answered is
    // `temporal_resolution_source`), and how many samples a pixel holds (3
    // for a colour image).
    "temporal_resolution",
    "samples_per_pixel",
    // MRI pack 1.0.1: two numbers the view works out from the fingerprint's
    // own fields, never stored. How many images the stack holds at each slice
    // position (`n_instances` over `n_slices`), and how many distinct b values
    // its images write (`dwi_b_values` counted). A pack compares them with
    // each other: a diffusion stack with more images a slice than b values
    // holds directional images beside its isotropic one. Read from a
    // registry, they are the same arithmetic on the columns
    // ([`DERIVED`]); anywhere else, [`Stack::num`] works them out.
    "images_per_position",
    "dwi_b_value_count",
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
    "image_role",
    "acquisition_type_filled",
    "acquisition_type_source",
    "dwi_b_values",
    "dwi_b_value_source",
    "dwi_pe_direction",
    "dwi_pe_direction_source",
    "dwi_directions_source",
    "field_strength_unit",
    "coverage_source",
    "acquisition_matrix",
    // Record 37 S3: the coil a series was already split on.
    "receive_coil_name",
    // The 2026-09-28 sequence research: PulseSequenceName (0018,9005), which
    // Siemens XA writes where it leaves SequenceName empty.
    "pulse_sequence_name",
    // Record 53 S1: the SOP class the file was written as (an MR
    // spectroscopy object is told by it, the deep research's case 16), and the
    // mechanism attributes of the MR Pulse Sequence module with the MR
    // Modifier group's spoiling and inversion recovery, which an enhanced
    // object writes where it leaves ScanningSequence out (case 22).
    "sop_class_uid",
    "echo_pulse_sequence",
    "multiple_spin_echo",
    "echo_planar_pulse_sequence",
    "steady_state_pulse_sequence",
    "phase_contrast",
    "time_of_flight_contrast",
    "arterial_spin_labeling_contrast",
    "geometry_of_k_space_traversal",
    "segmented_k_space_traversal",
    "spoiling",
    "inversion_recovery",
    // The ImageType Philips writes per frame of an enhanced MR object in
    // (2005,140F), whose third and fourth values name a Dixon part (W, F, IP
    // or OP) that the object's top-level ImageType does not.
    "private_frame_image_type",
    // The 2026-10-03 fields: AngioFlag (0018,0025), Y or N;
    // AcquisitionContrast (0008,9209), the contrast an enhanced object says
    // it was acquired for; where the temporal resolution came from (`header`
    // or `acquisition_times`); every DiffusionDirectionality (0018,9075) the
    // stack's images write, ISOTROPIC for an enhanced trace image; and
    // PhotometricInterpretation (0028,0004), RGB for a colour display
    // composite rather than a map.
    "angio_flag",
    "acquisition_contrast",
    "temporal_resolution_source",
    "diffusion_directionality",
    "photometric_interpretation",
];

/// Where the text half begins.
pub const FIRST_TEXT: usize = 27;

pub fn field_index(name: &str) -> Option<usize> {
    FIELDS.iter().position(|f| *f == name)
}

/// The fields the view works out rather than reads (MRI pack 1.0.1), each
/// with the fields it is worked out from. A value set on the stack wins; an
/// absent one is worked out when it is read.
pub const DERIVED: &[(&str, &[&str])] = &[
    ("images_per_position", &["n_instances", "n_slices"]),
    ("dwi_b_value_count", &["dwi_b_values"]),
];

const IMAGES_PER_POSITION: usize = 25;
const DWI_B_VALUE_COUNT: usize = 26;
const N_INSTANCES: usize = 9;
const N_SLICES: usize = 19;
const DWI_B_VALUES: usize = 56;

/// Whether a field is one the view works out ([`DERIVED`]).
pub fn is_derived(i: usize) -> bool {
    i == IMAGES_PER_POSITION || i == DWI_B_VALUE_COUNT
}

/// One stack, as a pack sees it: numbers by index, text by index, nothing
/// else. Built by whoever has the row.
#[derive(Default, Clone, Debug)]
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
            self.num[i].or_else(|| self.derived(i))
        } else {
            self.text[i - FIRST_TEXT].trim().parse().ok()
        }
    }

    /// A field the view works out ([`DERIVED`]), from the stack's own:
    /// images a slice position where both counts are above nought, and the
    /// distinct b values `dwi_b_values` lists (comma-separated, as the
    /// fingerprint writes them). None for any other field.
    fn derived(&self, i: usize) -> Option<f64> {
        match i {
            IMAGES_PER_POSITION => {
                let n = self.num[N_INSTANCES].filter(|v| *v > 0.0)?;
                let s = self.num[N_SLICES].filter(|v| *v > 0.0)?;
                Some(n / s)
            }
            DWI_B_VALUE_COUNT => {
                let mut seen: Vec<f64> = Vec::new();
                for part in self.text[DWI_B_VALUES - FIRST_TEXT].split([',', '\\']) {
                    if let Ok(v) = part.trim().parse::<f64>()
                        && !seen.contains(&v)
                    {
                        seen.push(v);
                    }
                }
                (!seen.is_empty()).then_some(seen.len() as f64)
            }
            _ => None,
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

    /// The field as the text something else stores, numbers included. A
    /// corpus keeps what a pass reads as text and parses it back, so this is
    /// the one accessor that does not care which half of the stack a field
    /// lives in.
    pub fn as_text(&self, i: usize) -> std::borrow::Cow<'_, str> {
        if i < FIRST_TEXT {
            match self.num(i) {
                None => std::borrow::Cow::Borrowed(""),
                Some(v) => std::borrow::Cow::Owned(format!("{v}")),
            }
        } else {
            std::borrow::Cow::Borrowed(&self.text[i - FIRST_TEXT])
        }
    }

    /// Whether the field carries anything at all.
    pub fn present(&self, i: usize) -> bool {
        if i < FIRST_TEXT {
            self.num(i).is_some()
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
        s.set("text_all", Value::Text(Some("one two three")))
            .unwrap();
        assert_eq!(s.num(field_index("inversion_time").unwrap()), Some(2500.0));
        assert_eq!(s.text(field_index("text_all").unwrap()), "one two three");
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
    fn the_derived_fields_sit_where_their_constants_say() {
        for (name, from) in DERIVED {
            let i = field_index(name).unwrap();
            assert!(is_derived(i) && i < FIRST_TEXT, "{name}");
            for f in *from {
                assert!(field_index(f).is_some(), "{f}");
            }
        }
        assert_eq!(
            field_index("images_per_position"),
            Some(IMAGES_PER_POSITION)
        );
        assert_eq!(field_index("dwi_b_value_count"), Some(DWI_B_VALUE_COUNT));
        assert_eq!(field_index("n_instances"), Some(N_INSTANCES));
        assert_eq!(field_index("n_slices"), Some(N_SLICES));
        assert_eq!(field_index("dwi_b_values"), Some(DWI_B_VALUES));
    }

    #[test]
    fn images_a_position_and_b_values_are_worked_out_from_the_stack() {
        let ipp = field_index("images_per_position").unwrap();
        let nb = field_index("dwi_b_value_count").unwrap();
        let mut s = Stack::new();
        assert_eq!(s.num(ipp), None);
        assert_eq!(s.num(nb), None);
        assert!(!s.present(ipp));
        // a Philips diffusion set: b=0, three directions and the isotropic
        // image at each of 24 slices, two b values
        s.set("n_instances", Value::Text(Some("120"))).unwrap();
        s.set("n_slices", Value::Num(Some(24.0))).unwrap();
        s.set("dwi_b_values", Value::Text(Some("0,1000"))).unwrap();
        assert_eq!(s.num(ipp), Some(5.0));
        assert_eq!(s.num(nb), Some(2.0));
        assert!(s.present(ipp));
        assert_eq!(s.as_text(ipp), "5");
        // a stack with no position counted, or none of its own, says nothing
        s.set("n_slices", Value::Num(Some(0.0))).unwrap();
        assert_eq!(s.num(ipp), None);
        s.set("n_slices", Value::Num(None)).unwrap();
        assert_eq!(s.num(ipp), None);
        // repeated and backslash-separated values count once
        s.set("dwi_b_values", Value::Text(Some("0\\1000, 1000 ,2000")))
            .unwrap();
        assert_eq!(s.num(nb), Some(3.0));
        s.set("dwi_b_values", Value::Text(Some(""))).unwrap();
        assert_eq!(s.num(nb), None);
        // a value set on the stack wins over the working out
        s.set("images_per_position", Value::Num(Some(2.0))).unwrap();
        assert_eq!(s.num(ipp), Some(2.0));
    }

    #[test]
    fn an_absent_field_is_absent_in_both_halves() {
        let s = Stack::new();
        assert!(!s.present(field_index("echo_time").unwrap()));
        assert!(!s.present(field_index("text_all").unwrap()));
    }
}
