// SPDX-License-Identifier: AGPL-3.0-only

//! Where a pseudonymised file goes (record 26 §3): a place made from what
//! the file says about itself and never from the path it came in on, since
//! that path is the sender's and carries whatever the sender put in it.
//! `<code>/<StudyDate>-<8 hex of the study UID>/<series>/<InstanceNumber>.dcm`,
//! the numbers padded so that a listing sorts; a clash takes a counter.
//!
//! The series folder (record 55 K9, ruling 18 of 2026-10-14) is
//! `<SeriesNumber>-<SeriesDescription>` in a tree begun under it: the
//! description as the copy keeps it, reduced to letters, digits and hyphens
//! and cut to about 40 characters, and the number alone where nothing is
//! left. A tree begun before keeps `<SeriesNumber>` alone: existing trees
//! are never renamed, and a resumed run writes in its tree's scheme
//! ([`SeriesFolders`]).

use blake2::Digest;
use blake2::digest::consts::U32;

/// What the layout is made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// `YYYYMMDD` as written, or empty.
    pub study_date: String,
    pub study_uid: String,
    pub series_number: Option<i64>,
    pub instance_number: Option<i64>,
    /// SeriesDescription as the copy keeps it (cleaned of the file's own
    /// identifiers), or none where it is empty or the dataset removes it.
    pub series_description: Option<String>,
}

/// How a tree names its series folders. A tree is begun under one scheme
/// and keeps it: a run on a tree that holds files already writes in the
/// scheme its earlier runs recorded, `Number` for a tree begun before the
/// scheme was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeriesFolders {
    /// `<SeriesNumber>`, as every tree was written up to record 55 K9.
    Number,
    /// `<SeriesNumber>-<description>`, the number alone with no description.
    Description,
}

impl SeriesFolders {
    pub fn name(self) -> &'static str {
        match self {
            SeriesFolders::Number => "number",
            SeriesFolders::Description => "description",
        }
    }

    pub fn of(name: &str) -> Option<SeriesFolders> {
        match name {
            "number" => Some(SeriesFolders::Number),
            "description" => Some(SeriesFolders::Description),
            _ => None,
        }
    }
}

/// How many characters of the reduced description a folder takes.
pub const DESCRIPTION_CHARS: usize = 40;

/// A Latin letter with a diacritic as its base letters, so a Swedish or
/// German description stays readable (`Före` is `Fore`); none for any other
/// character.
fn folded(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => "a",
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' => "A",
        'è' | 'é' | 'ê' | 'ë' | 'ē' => "e",
        'È' | 'É' | 'Ê' | 'Ë' | 'Ē' => "E",
        'ì' | 'í' | 'î' | 'ï' => "i",
        'Ì' | 'Í' | 'Î' | 'Ï' => "I",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => "o",
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'Ō' => "O",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' => "u",
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' => "U",
        'ý' | 'ÿ' => "y",
        'Ý' => "Y",
        'ñ' => "n",
        'Ñ' => "N",
        'ç' => "c",
        'Ç' => "C",
        'ß' => "ss",
        'æ' => "ae",
        'Æ' => "AE",
        'œ' => "oe",
        'Œ' => "OE",
        _ => return None,
    })
}

/// A description reduced to ASCII letters, digits and single hyphens, with
/// no hyphen at either end, cut to [`DESCRIPTION_CHARS`]: every other
/// character (a space, `_`, `*`, `/`, a letter of another script) is a
/// hyphen. Empty where nothing is left.
pub fn reduced(description: &str) -> String {
    let mut out = String::new();
    let mut push = |c: char, out: &mut String| {
        if c == '-' {
            if !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
        } else {
            out.push(c);
        }
    };
    for c in description.chars() {
        if c.is_ascii_alphanumeric() {
            push(c, &mut out);
        } else if let Some(f) = folded(c) {
            for d in f.chars() {
                push(d, &mut out);
            }
        } else {
            push('-', &mut out);
        }
    }
    let mut cut: String = out.chars().take(DESCRIPTION_CHARS).collect();
    while cut.ends_with('-') {
        cut.pop();
    }
    cut
}

/// The series folder of a file, in a tree's scheme.
pub fn series_folder(scheme: SeriesFolders, facts: &Facts) -> String {
    let number = format!("{:03}", facts.series_number.unwrap_or(0).max(0));
    match scheme {
        SeriesFolders::Number => number,
        SeriesFolders::Description => {
            let words = facts
                .series_description
                .as_deref()
                .map(reduced)
                .unwrap_or_default();
            if words.is_empty() {
                number
            } else {
                format!("{number}-{words}")
            }
        }
    }
}

/// The eight hex characters that stand for a study UID: the head of its
/// unkeyed BLAKE2b-256, enough to tell two studies of one day apart and
/// nothing a reader could turn back into the UID.
pub fn study_hash(uid: &str) -> String {
    let digest = blake2::Blake2b::<U32>::digest(uid.trim().as_bytes());
    hex::encode(&digest[..4])
}

/// The path under the pseudonymised tree, forward slashes, its series
/// folder in the tree's scheme.
pub fn relative(code: &str, facts: &Facts, scheme: SeriesFolders) -> String {
    let date = {
        let d: String = facts
            .study_date
            .chars()
            .filter(char::is_ascii_digit)
            .take(8)
            .collect();
        if d.is_empty() {
            "00000000".to_string()
        } else {
            d
        }
    };
    format!(
        "{code}/{date}-{}/{}/{:05}.dcm",
        study_hash(&facts.study_uid),
        series_folder(scheme, facts),
        facts.instance_number.unwrap_or(0).max(0),
    )
}

/// The same path with a counter before the extension, for a clash:
/// `00001-2.dcm`.
pub fn with_counter(rel: &str, n: u32) -> String {
    match rel.strip_suffix(".dcm") {
        Some(stem) => format!("{stem}-{n}.dcm"),
        None => format!("{rel}-{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_is_made_from_facts_and_pads_its_numbers() {
        let facts = Facts {
            study_date: "20240131".into(),
            study_uid: "1.2.826.0.1.3680043.8.498.1".into(),
            series_number: Some(7),
            instance_number: Some(42),
            series_description: None,
        };
        let rel = relative("abc123", &facts, SeriesFolders::Number);
        let hash = study_hash("1.2.826.0.1.3680043.8.498.1");
        assert_eq!(hash.len(), 8);
        assert_eq!(rel, format!("abc123/20240131-{hash}/007/00042.dcm"));
        assert_eq!(study_hash(" 1.2.826.0.1.3680043.8.498.1 "), hash);
        assert_ne!(study_hash("1.2.826.0.1.3680043.8.498.2"), hash);
        let bare = Facts {
            study_date: String::new(),
            study_uid: "1.2".into(),
            series_number: None,
            instance_number: Some(-3),
            series_description: None,
        };
        assert!(relative("c", &bare, SeriesFolders::Number).starts_with("c/00000000-"));
        assert!(relative("c", &bare, SeriesFolders::Number).ends_with("/000/00000.dcm"));
        assert_eq!(
            with_counter(&rel, 2),
            format!("abc123/20240131-{hash}/007/00042-2.dcm")
        );
        assert_eq!(with_counter("x", 3), "x-3");
    }

    fn series(number: Option<i64>, description: Option<&str>) -> Facts {
        Facts {
            study_date: "20240131".into(),
            study_uid: "1.2.3".into(),
            series_number: number,
            instance_number: Some(1),
            series_description: description.map(str::to_string),
        }
    }

    #[test]
    fn a_new_tree_names_its_series_folders_by_number_and_description() {
        let f = series(Some(7), Some("t1_mprage_sag 1mm"));
        assert_eq!(
            series_folder(SeriesFolders::Description, &f),
            "007-t1-mprage-sag-1mm"
        );
        let rel = relative("abc", &f, SeriesFolders::Description);
        assert!(rel.ends_with("/007-t1-mprage-sag-1mm/00001.dcm"), "{rel}");
        // punctuation runs become one hyphen, none at either end
        assert_eq!(reduced("  Ax T2* FLAIR (+C) / ok_  "), "Ax-T2-FLAIR-C-ok");
        assert_eq!(reduced("MPR__sag--3"), "MPR-sag-3");
    }

    #[test]
    fn an_old_tree_keeps_the_number_alone() {
        let f = series(Some(7), Some("t1_mprage_sag"));
        assert_eq!(series_folder(SeriesFolders::Number, &f), "007");
        assert!(relative("abc", &f, SeriesFolders::Number).ends_with("/007/00001.dcm"));
        assert_eq!(SeriesFolders::of("number"), Some(SeriesFolders::Number));
        assert_eq!(
            SeriesFolders::of("description"),
            Some(SeriesFolders::Description)
        );
        assert_eq!(
            SeriesFolders::of(SeriesFolders::Description.name()),
            Some(SeriesFolders::Description)
        );
        assert_eq!(SeriesFolders::of("other"), None);
    }

    #[test]
    fn an_empty_description_leaves_the_number_alone() {
        for d in [None, Some(""), Some("   "), Some("***"), Some("磁気共鳴")] {
            assert_eq!(
                series_folder(SeriesFolders::Description, &series(Some(12), d)),
                "012",
                "{d:?}"
            );
        }
        assert_eq!(
            series_folder(SeriesFolders::Description, &series(None, Some("x"))),
            "000-x"
        );
    }

    #[test]
    fn unicode_is_folded_or_reduced_to_ascii() {
        assert_eq!(reduced("Före kontrast ÅÄÖ"), "Fore-kontrast-AAO");
        assert_eq!(reduced("Straße Æble"), "Strasse-AEble");
        assert_eq!(reduced("T2 тест ax"), "T2-ax");
        assert!(
            reduced("ηλ ü ☃ é")
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        );
    }

    #[test]
    fn an_over_long_description_is_cut_to_forty_with_no_trailing_hyphen() {
        let long = "a".repeat(39) + " bcdefgh";
        let r = reduced(&long);
        assert_eq!(r, "a".repeat(39), "the cut fell on a hyphen, which goes");
        let r = reduced(&"x".repeat(100));
        assert_eq!(r.chars().count(), DESCRIPTION_CHARS);
        let folder = series_folder(
            SeriesFolders::Description,
            &series(Some(3), Some(&"ö".repeat(60))),
        );
        assert_eq!(folder, format!("003-{}", "o".repeat(40)));
    }
}
