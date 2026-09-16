// SPDX-License-Identifier: AGPL-3.0-only

//! Where a pseudonymised file goes (record 26 §3): a place made from what
//! the file says about itself and never from the path it came in on, since
//! that path is the sender's and carries whatever the sender put in it.
//! `<code>/<StudyDate>-<8 hex of the study UID>/<SeriesNumber>/<InstanceNumber>.dcm`,
//! the numbers padded so that a listing sorts; a clash takes a counter.

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
}

/// The eight hex characters that stand for a study UID: the head of its
/// unkeyed BLAKE2b-256, enough to tell two studies of one day apart and
/// nothing a reader could turn back into the UID.
pub fn study_hash(uid: &str) -> String {
    let digest = blake2::Blake2b::<U32>::digest(uid.trim().as_bytes());
    hex::encode(&digest[..4])
}

/// The path under the pseudonymised tree, forward slashes.
pub fn relative(code: &str, facts: &Facts) -> String {
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
        "{code}/{date}-{}/{:03}/{:05}.dcm",
        study_hash(&facts.study_uid),
        facts.series_number.unwrap_or(0).max(0),
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
        };
        let rel = relative("abc123", &facts);
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
        };
        assert!(relative("c", &bare).starts_with("c/00000000-"));
        assert!(relative("c", &bare).ends_with("/000/00000.dcm"));
        assert_eq!(
            with_counter(&rel, 2),
            format!("abc123/20240131-{hash}/007/00042-2.dcm")
        );
        assert_eq!(with_counter("x", 3), "x-3");
    }
}
