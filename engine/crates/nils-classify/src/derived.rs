// SPDX-License-Identifier: AGPL-3.0-only

//! Fields the fingerprint works out rather than reads
//! (`docs/specs/wave3-anonymize-and-bids.md`, §6).
//!
//! Wave 2 §7.3 found that three of v0's nine post-classification phases only
//! fill fields, and that they belong here rather than in a pass: they are the
//! same for every reader, so they should be computed once, from what was
//! measured, before anything is judged.
//!
//! Every one of them is stored **beside** the measured column and never over
//! it. That is not tidiness. v0's acquisition-type fill writes its inference
//! back into `stack_fingerprint.mr_acquisition_type`, and classification reads
//! that column, so a guess one run made becomes an input the next run treats as
//! a measurement, and the same stack classified twice can come out differently.
//! Its field-strength normaliser does the same to
//! `mri_series_details.magnetic_field_strength`, and there the measured value
//! is gone for good. Keeping both means a reader can always ask what the
//! scanner said.
//!
//! Each function here is a pure function of one stack's own row, which is what
//! keeps the result independent of which stacks happened to be in a batch.
//! The fourth field of §6, the session rescue, is not one of those and is not
//! here.

/// The field strengths a scanner is actually built at.
const STANDARD: &[f64] = &[0.5, 1.0, 1.5, 3.0, 7.0];

/// How far from a standard value a reading may sit and still be that scanner.
///
/// v0's table, kept: wider at 3 T and 7 T because the shim and the reported
/// precision are both looser there. The gaps between the bands are deliberate.
fn tolerance(standard: f64) -> f64 {
    if standard >= 7.0 {
        0.5
    } else if standard >= 3.0 {
        0.3
    } else {
        0.15
    }
}

/// What the reading turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// Tesla, which is what DICOM asks for.
    Tesla,
    /// Gauss. Ten thousand to the tesla, and some scanners write it.
    Gauss,
    /// Millitesla. A thousand to the tesla.
    Milli,
}

impl Unit {
    pub fn name(self) -> &'static str {
        match self {
            Unit::Tesla => "tesla",
            Unit::Gauss => "gauss",
            Unit::Milli => "millitesla",
        }
    }
}

/// A field strength on the standard grid, with what had to be assumed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    /// The nearest standard value, or none when the reading is not near one.
    pub normalized: Option<f64>,
    /// The reading in tesla, whatever unit it arrived in.
    pub tesla: f64,
    pub unit: Unit,
}

/// A magnetic field strength, normalised.
///
/// Two things differ from v0, and both are about not inventing a measurement.
///
/// v0 falls back to the nearest standard value when nothing is within
/// tolerance, so a 0.2 T open scanner is recorded as 0.5 T and a 4.7 T animal
/// scanner as 3 T. Here an out-of-band reading gets no normalised value: the
/// measured column still says what the scanner said, and a reader who wants a
/// grid can bin, but nobody is handed a strength that was never measured.
///
/// v0 treats anything above 100 as gauss. A scanner reporting 1500 for a 1.5 T
/// magnet is writing millitesla, and dividing by ten thousand turns it into
/// 0.15 T, which v0 then rounds up to 0.5 T. Both scales are tried, and the one
/// that lands on the grid wins.
pub fn field_strength(raw: Option<f64>) -> Option<Field> {
    let raw = raw?;
    if !raw.is_finite() || raw <= 0.0 {
        return None;
    }
    // In the order a value is most likely to have been written.
    for (unit, tesla) in [
        (Unit::Tesla, raw),
        (Unit::Gauss, raw / 10_000.0),
        (Unit::Milli, raw / 1_000.0),
    ] {
        if let Some(standard) = on_grid(tesla) {
            return Some(Field {
                normalized: Some(standard),
                tesla,
                unit,
            });
        }
    }
    // Nothing landed. The reading stands as tesla, unnormalised, because that
    // is what DICOM says the units are and guessing another scale for a value
    // that fits no grid would be two guesses stacked.
    Some(Field {
        normalized: None,
        tesla: raw,
        unit: Unit::Tesla,
    })
}

fn on_grid(tesla: f64) -> Option<f64> {
    STANDARD
        .iter()
        .copied()
        .find(|s| (tesla - s).abs() <= tolerance(*s))
}

/// How a 2D or 3D acquisition was worked out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// `ImageType` carries `DIS2D` or `DIS3D`, or a bare `2D`/`3D` token.
    ImageType,
    /// The sequence name says so, which is where Siemens puts it.
    SequenceName,
    /// The rest of the stack's text does.
    Text,
}

impl How {
    pub fn name(self) -> &'static str {
        match self {
            How::ImageType => "image_type",
            How::SequenceName => "sequence_name",
            How::Text => "text",
        }
    }
}

/// Text that means a volume was acquired as a slab.
const THREE_D: &[&str] = &[
    "3d",
    "space",
    "mprage",
    "cube",
    "vista",
    "ciss",
    "vibe",
    "isotropic",
    "mp2rage",
    "bravo",
    "spc",
];

/// And text that means it was acquired slice by slice.
const TWO_D: &[&str] = &[
    "2d",
    "haste",
    "blade",
    "propeller",
    "tse2d",
    "flash2d",
    "single shot",
    "single-shot",
    "ss-tse",
    "sstse",
];

/// `2D` or `3D` for a stack whose `MRAcquisitionType` is missing.
///
/// v0 has a third tier that reads the **technique** the classifier assigned,
/// and it is left out on purpose. The technique is a conclusion, the
/// fingerprint records measurements, and v0 writes the result of that tier back
/// into the column classification reads next time: it decides, among other
/// things, whether a magnetisation-prepared gradient echo is MPRAGE, so a stack
/// can be MPRAGE because a previous run guessed it was 3D because a previous
/// run called it MPRAGE. A pack that wants to conclude 3D from a technique can
/// still do it, as a rule, where it is recorded as a conclusion.
///
/// `measured` is `MRAcquisitionType` as read. When it says anything, that is
/// the answer and nothing here runs.
pub fn acquisition_type(
    measured: Option<&str>,
    image_type: Option<&str>,
    sequence_name: Option<&str>,
    text_all: Option<&str>,
) -> Option<(&'static str, How)> {
    if measured.is_some_and(|m| !m.trim().is_empty()) {
        return None;
    }
    // `DIS2D`/`DIS3D` is the distortion-corrected marker and is the most
    // reliable of the three, being a token rather than prose.
    let it = image_type.unwrap_or("").to_ascii_uppercase();
    let has = |t: &str| it.split(['\\', '/', ' ', ',']).any(|p| p.trim() == t);
    if has("DIS3D") || has("3D") {
        return Some(("3D", How::ImageType));
    }
    if has("DIS2D") || has("2D") {
        return Some(("2D", How::ImageType));
    }
    // Siemens writes the dimensionality into the sequence name, which is
    // shorter than the description and so has fewer ways to be wrong.
    let seq = sequence_name.unwrap_or("").to_ascii_lowercase();
    if seq.contains("3d") || seq.contains("spc") || seq.contains("space") {
        return Some(("3D", How::SequenceName));
    }
    if seq.contains("2d") {
        return Some(("2D", How::SequenceName));
    }
    // And last the prose, where 3D is checked first because its words are the
    // more specific of the two.
    let text = text_all.unwrap_or("").to_ascii_lowercase();
    if THREE_D.iter().any(|p| text.contains(p)) {
        return Some(("3D", How::Text));
    }
    if TWO_D.iter().any(|p| text.contains(p)) {
        return Some(("2D", How::Text));
    }
    None
}

/// What `ImageType`'s first two values say a stack is.
///
/// DICOM's value 1 is `ORIGINAL` or `DERIVED` and value 2 is `PRIMARY` or
/// `SECONDARY`, so the pair is four cases and every archive holds all four.
/// It is worked out here, once, because three separate things read it: the
/// disposition of §7, the exclusion a pack applies, and the session rescue,
/// which asks whether a whole session is missing its primaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// What the scanner reconstructed and meant you to look at.
    OriginalPrimary,
    /// Original pixels, but not the image the scanner considered the output.
    /// Some Philips exports mark every reconstruction this way and never write
    /// a primary at all, which is what the rescue exists for.
    OriginalSecondary,
    /// Made from other images: a reformat, a projection, a map.
    Derived,
    /// A screenshot, a pasted report, an error image. Never a rescue candidate
    /// whatever else is missing, because it is not an acquisition.
    NotAnImage,
    /// `ImageType` was absent or said neither.
    Unknown,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::OriginalPrimary => "original_primary",
            Role::OriginalSecondary => "original_secondary",
            Role::Derived => "derived",
            Role::NotAnImage => "not_an_image",
            Role::Unknown => "unknown",
        }
    }

    /// Whether a session with no primary anywhere may treat this stack as one.
    ///
    /// v0's rule, with the same exclusions: original, secondary, not primary,
    /// not derived, and not one of the three kinds of picture that are not an
    /// acquisition.
    pub fn rescuable(self) -> bool {
        self == Role::OriginalSecondary
    }
}

pub fn role(image_type: Option<&str>) -> Role {
    let Some(text) = image_type else {
        return Role::Unknown;
    };
    let it = text.to_ascii_uppercase();
    let tokens: Vec<&str> = it.split('\\').map(str::trim).collect();
    let has = |t: &str| tokens.contains(&t);
    // Checked before anything else: a screenshot that says ORIGINAL and
    // SECONDARY would otherwise be rescuable, and it is a photograph of a
    // screen.
    if has("SCREENSHOT") || has("PASTED") || has("ERROR") {
        return Role::NotAnImage;
    }
    if has("DERIVED") {
        return Role::Derived;
    }
    if has("ORIGINAL") {
        if has("PRIMARY") {
            return Role::OriginalPrimary;
        }
        if has("SECONDARY") {
            return Role::OriginalSecondary;
        }
    }
    Role::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(raw: f64) -> Option<f64> {
        field_strength(Some(raw)).and_then(|f| f.normalized)
    }

    #[test]
    fn a_reading_near_a_real_magnet_is_that_magnet() {
        assert_eq!(t(1.5), Some(1.5));
        assert_eq!(t(1.493806), Some(1.5), "a Siemens 1.5 T");
        assert_eq!(t(2.89362), Some(3.0), "a Siemens 3 T");
        assert_eq!(t(0.95), Some(1.0));
        assert_eq!(t(6.98), Some(7.0));
    }

    #[test]
    fn a_reading_in_another_unit_is_converted_not_rounded() {
        let g = field_strength(Some(15000.0)).unwrap();
        assert_eq!(g.normalized, Some(1.5));
        assert_eq!(g.unit, Unit::Gauss);

        // v0 divides anything over 100 by ten thousand, so 1500 millitesla
        // becomes 0.15 T and then rounds up to 0.5 T: a 1.5 T scanner recorded
        // as a third of its strength.
        let m = field_strength(Some(1500.0)).unwrap();
        assert_eq!(m.normalized, Some(1.5));
        assert_eq!(m.unit, Unit::Milli);
        assert!((m.tesla - 1.5).abs() < 1e-9);
    }

    #[test]
    fn a_magnet_that_is_not_on_the_grid_is_not_moved_onto_it() {
        // v0 returns the nearest standard whatever the distance, so these come
        // back as 0.5, 3.0 and 7.0 and the real value is overwritten.
        for raw in [0.2, 4.7, 9.4, 11.7] {
            let f = field_strength(Some(raw)).unwrap();
            assert_eq!(f.normalized, None, "{raw} T");
            assert_eq!(f.tesla, raw, "and the reading is kept");
        }
    }

    #[test]
    fn nothing_is_not_a_field_strength() {
        assert!(field_strength(None).is_none());
        assert!(field_strength(Some(0.0)).is_none());
        assert!(field_strength(Some(-1.5)).is_none());
        assert!(field_strength(Some(f64::NAN)).is_none());
    }

    #[test]
    fn a_measured_acquisition_type_is_never_second_guessed() {
        assert_eq!(
            acquisition_type(Some("2D"), Some("ORIGINAL\\PRIMARY\\M\\DIS3D"), None, None),
            None,
            "the scanner said 2D and that is the answer"
        );
        // `text_all` is the join of the six text fields, so the sequence name
        // is inside it; a caller that passes one passes both.
        assert_eq!(
            acquisition_type(
                Some("  "),
                None,
                Some("tfl3d1"),
                Some("t1 mprage sag tfl3d1")
            ),
            Some(("3D", How::SequenceName)),
            "but whitespace is not an answer"
        );
    }

    #[test]
    fn image_type_is_read_before_the_prose() {
        assert_eq!(
            acquisition_type(
                None,
                Some("ORIGINAL\\PRIMARY\\M\\DIS3D"),
                Some("tse2d1_9"),
                None
            ),
            Some(("3D", How::ImageType))
        );
        assert_eq!(
            acquisition_type(None, Some("DERIVED\\SECONDARY\\2D"), None, None),
            Some(("2D", How::ImageType))
        );
        // and a token, not a substring: MOSAIC3DSOMETHING is not a 3D marker
        assert_eq!(
            acquisition_type(None, Some("ORIGINAL\\PRIMARY\\M3D"), None, None),
            None
        );
    }

    #[test]
    fn the_sequence_name_is_read_before_the_description() {
        assert_eq!(
            acquisition_type(None, None, Some("*spc_314ns"), Some("t2 haste cor")),
            Some(("3D", How::SequenceName)),
            "spc is SPACE, and it beats the word haste in the description"
        );
        assert_eq!(
            acquisition_type(None, None, None, Some("t2 haste cor")),
            Some(("2D", How::Text))
        );
    }

    #[test]
    fn three_d_words_are_checked_before_two_d_words() {
        // "3D TSE isotropic" holds no 2D word, but "t1 mprage 2d ref" holds
        // both, and the more specific one wins.
        assert_eq!(
            acquisition_type(None, None, None, Some("t1 mprage sag 2d ref")),
            Some(("3D", How::Text))
        );
    }

    #[test]
    fn a_stack_with_nothing_to_go_on_stays_unknown() {
        assert_eq!(acquisition_type(None, None, None, None), None);
        assert_eq!(
            acquisition_type(
                None,
                Some("ORIGINAL\\PRIMARY\\M\\ND"),
                Some("tfl"),
                Some("localizer")
            ),
            None
        );
    }

    #[test]
    fn image_type_says_what_a_stack_is() {
        assert_eq!(
            role(Some("ORIGINAL\\PRIMARY\\M\\ND")),
            Role::OriginalPrimary
        );
        assert_eq!(
            role(Some("ORIGINAL\\SECONDARY\\M\\ND")),
            Role::OriginalSecondary
        );
        assert_eq!(role(Some("DERIVED\\SECONDARY\\MPR")), Role::Derived);
        assert_eq!(role(Some("DERIVED\\PRIMARY\\ADC")), Role::Derived);
        assert_eq!(role(None), Role::Unknown);
        assert_eq!(role(Some("")), Role::Unknown);
        assert_eq!(role(Some("ORIGINAL")), Role::Unknown, "value 2 is missing");
    }

    #[test]
    fn a_screenshot_is_not_an_acquisition_however_it_is_labelled() {
        // The exclusions matter because they are what a rescue must not pick
        // up: a session with no primaries is exactly the session whose only
        // ORIGINAL\SECONDARY images might be screen captures.
        for it in [
            "ORIGINAL\\SECONDARY\\SCREENSHOT",
            "ORIGINAL\\SECONDARY\\PASTED",
            "ORIGINAL\\SECONDARY\\ERROR",
        ] {
            assert_eq!(role(Some(it)), Role::NotAnImage, "{it}");
            assert!(!role(Some(it)).rescuable());
        }
        assert!(role(Some("ORIGINAL\\SECONDARY\\M\\ND")).rescuable());
        assert!(!role(Some("ORIGINAL\\PRIMARY\\M\\ND")).rescuable());
        assert!(!role(Some("DERIVED\\SECONDARY\\MPR")).rescuable());
    }
}
