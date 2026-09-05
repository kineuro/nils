// SPDX-License-Identifier: AGPL-3.0-only

//! The dataset, not just the files
//! (`docs/specs/wave3-anonymize-and-bids.md`, §9.4 and §9.5).
//!
//! v0 writes none of these, which is why its tree is **not a dataset** rather
//! than an invalid one: no `dataset_description.json`, no `participants.tsv`,
//! no `README`, no `_scans.tsv`, and the validator is never run.
//!
//! §9.4 is the other half of breaking the coupling of §2.1. The directory is
//! named by the session scheme and **the time is carried in the standard's own
//! slot**, under the release's date policy, so anything joining on a date reads
//! a column instead of parsing a directory name.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// One row of `participants.tsv`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    pub id: String,
    /// Only what the policy allows out. A release that shifted dates has no
    /// age to give unless the age was computed before the birth date went
    /// (§8.3), and one that removed the patient categories has no sex.
    pub extra: BTreeMap<String, String>,
}

/// One row of a `_sessions.tsv`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub label: String,
    /// The session's time, in the standard's own column, under the policy.
    pub acq_time: Option<String>,
}

/// One row of a `_scans.tsv`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// The path, relative to the subject and session directory.
    pub filename: String,
    pub acq_time: Option<String>,
}

/// What made this dataset, for `GeneratedBy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MadeBy {
    pub name: String,
    pub version: String,
    /// The release's version (§8.6), so a tree says which version it is
    /// without a database.
    pub dataset_version: String,
    /// The converter and the version of it that was found, because a tree
    /// should say which converter made it (§9.6).
    pub converter: Option<String>,
    /// The policy the release applied, in one line.
    pub policy: String,
    /// The pack that judged the stacks.
    pub pack: String,
    /// The placements a release chose (§9.3): a tree that does not say where
    /// it put its localizers is a tree whose absence of localizers means
    /// nothing.
    pub placements: BTreeMap<String, String>,
    /// Whose dataset it is. BIDS asks for it, and the official validator warns
    /// without it, because a dataset with no authors cannot be registered for
    /// a DOI. The release's actor is the honest default: whoever ran it.
    pub authors: Vec<String>,
}

/// A TSV cell. Empty is `n/a`, which is what the standard says absent is, and
/// a tab or a newline in a value would silently make a new column.
fn cell(value: Option<&str>) -> String {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => "n/a".to_string(),
        Some(v) => v.replace(['\t', '\n', '\r'], " "),
    }
}

/// `dataset_description.json` (§9.5).
pub fn description(name: &str, made_by: &MadeBy, source: Option<&str>) -> String {
    let mut generated = serde_json::json!({
        "Name": made_by.name,
        "Version": made_by.version,
        "DatasetVersion": made_by.dataset_version,
        "Description": made_by.policy,
        "CodeURL": "https://github.com/kineuro/nils",
    });
    if let Some(c) = &made_by.converter {
        generated["Container"] = serde_json::json!({ "Converter": c });
    }
    if !made_by.placements.is_empty() {
        generated["Placements"] = serde_json::json!(made_by.placements);
    }
    generated["Pack"] = serde_json::json!(made_by.pack);
    let mut doc = serde_json::json!({
        "Name": name,
        "BIDSVersion": super::schema::BIDS_VERSION,
        "DatasetType": "raw",
        "Authors": made_by.authors,
        "GeneratedBy": [generated],
    });
    if let Some(s) = source {
        doc["SourceDatasets"] = serde_json::json!([{ "URL": s }]);
    }
    serde_json::to_string_pretty(&doc).unwrap_or_default() + "\n"
}

/// The `dataset_description.json` of `derivatives/nils/`, which is a dataset
/// in its own right so that the tree stays valid and the data stays present.
pub fn derivative_description(name: &str, made_by: &MadeBy) -> String {
    let doc = serde_json::json!({
        "Name": format!("{name}: what BIDS has no name for"),
        "BIDSVersion": super::schema::BIDS_VERSION,
        "DatasetType": "derivative",
        "GeneratedBy": [{
            "Name": made_by.name,
            "Version": made_by.version,
            "DatasetVersion": made_by.dataset_version,
            "Description": "reformats, projections, susceptibility-weighted images, \
                            maps and subtractions: images the archive holds and the \
                            standard has no suffix for",
        }],
        "SourceDatasets": [{ "URL": "../.." }],
    });
    serde_json::to_string_pretty(&doc).unwrap_or_default() + "\n"
}

/// `participants.tsv`, carrying what the policy allows.
pub fn participants(rows: &[Participant]) -> String {
    let mut columns: Vec<String> = Vec::new();
    for r in rows {
        for k in r.extra.keys() {
            if !columns.contains(k) {
                columns.push(k.clone());
            }
        }
    }
    let mut out = String::from("participant_id");
    for c in &columns {
        let _ = write!(out, "\t{c}");
    }
    out.push('\n');
    for r in rows {
        let _ = write!(out, "sub-{}", r.id);
        for c in &columns {
            let _ = write!(out, "\t{}", cell(r.extra.get(c).map(String::as_str)));
        }
        out.push('\n');
    }
    out
}

/// `sub-<label>_sessions.tsv` (§9.4).
pub fn sessions(rows: &[Session]) -> String {
    let mut out = String::from("session_id\tacq_time\n");
    for r in rows {
        let _ = writeln!(out, "ses-{}\t{}", r.label, cell(r.acq_time.as_deref()));
    }
    out
}

/// `sub-<label>[_ses-<label>]_scans.tsv` (§9.4).
pub fn scans(rows: &[Scan]) -> String {
    let mut sorted = rows.to_vec();
    sorted.sort_by(|a, b| a.filename.cmp(&b.filename));
    let mut out = String::from("filename\tacq_time\n");
    for r in &sorted {
        let _ = writeln!(out, "{}\t{}", r.filename, cell(r.acq_time.as_deref()));
    }
    out
}

/// `.bidsignore`, naming what we put in the tree that the standard does not
/// know (§9.3).
///
/// Only the lines the release's own choices need. A `.bidsignore` that lists
/// what is not there tells a reader the tree holds things it does not.
pub fn bidsignore(lines: &[String]) -> String {
    let mut out = String::new();
    for l in lines {
        let _ = writeln!(out, "{l}");
    }
    out
}

/// The `README`, which BIDS requires and which is the one file in the tree
/// written for a person.
pub fn readme(name: &str, made_by: &MadeBy, counts: &BTreeMap<String, i64>) -> String {
    let mut out = format!("# {name}\n\n");
    let _ = writeln!(
        out,
        "Version {} of this dataset, written by {} {} under BIDS {}.\n",
        made_by.dataset_version,
        made_by.name,
        made_by.version,
        super::schema::BIDS_VERSION
    );
    let _ = writeln!(out, "De-identification: {}.\n", made_by.policy);
    let _ = writeln!(out, "Classified by pack {}.\n", made_by.pack);
    if let Some(c) = &made_by.converter {
        let _ = writeln!(out, "Converted with {c}.\n");
    }
    if !made_by.placements.is_empty() {
        out.push_str("## Where things were put\n\n");
        for (what, where_) in &made_by.placements {
            let _ = writeln!(out, "- {what}: {where_}");
        }
        out.push('\n');
    }
    if !counts.is_empty() {
        out.push_str("## What is here\n\n");
        out.push_str("| route | stacks |\n|---|---|\n");
        for (route, n) in counts {
            let _ = writeln!(out, "| {route} | {n} |");
        }
        out.push('\n');
    }
    out.push_str(
        "Less than half of a clinical archive has a BIDS name, and that is not a defect in\n\
         BIDS: a localizer, a reformat, a projection and a synthetic contrast are not\n\
         acquisitions and the standard has no word for them. What is here is what the\n\
         standard admits; `sourcedata/` and `derivatives/nils/` hold the rest, and the\n\
         run reports anything it could place nowhere.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made_by() -> MadeBy {
        MadeBy {
            name: "nils".into(),
            version: "1.0.0".into(),
            dataset_version: "2026.09.05.1".into(),
            converter: Some("dcm2niix v1.0.20260724".into()),
            policy: "dates shift, uids remap under 2.25".into(),
            pack: "mri@0.1.0".into(),
            placements: [("localizers".to_string(), "sourcedata".to_string())].into(),
            authors: vec!["a person".to_string()],
        }
    }

    #[test]
    fn the_description_says_which_standard_and_which_version_of_the_dataset() {
        // A tree has to say which version it is without a database (§8.6), and
        // which converter made it (§9.6).
        let d: serde_json::Value =
            serde_json::from_str(&description("a cohort", &made_by(), None)).unwrap();
        assert_eq!(d["BIDSVersion"], super::super::schema::BIDS_VERSION);
        assert_eq!(d["GeneratedBy"][0]["DatasetVersion"], "2026.09.05.1");
        assert_eq!(
            d["GeneratedBy"][0]["Container"]["Converter"],
            "dcm2niix v1.0.20260724"
        );
        assert_eq!(
            d["GeneratedBy"][0]["Placements"]["localizers"],
            "sourcedata"
        );
    }

    #[test]
    fn a_tsv_says_absent_the_way_the_standard_says_it() {
        let rows = vec![
            Participant {
                id: "abc".into(),
                extra: [("age".to_string(), "43".to_string())].into(),
            },
            Participant {
                id: "def".into(),
                extra: BTreeMap::new(),
            },
        ];
        assert_eq!(
            participants(&rows),
            "participant_id\tage\nsub-abc\t43\nsub-def\tn/a\n"
        );
    }

    #[test]
    fn a_tab_in_a_value_does_not_make_a_new_column() {
        let rows = vec![Participant {
            id: "abc".into(),
            extra: [("note".to_string(), "one\ttwo".to_string())].into(),
        }];
        assert_eq!(participants(&rows).lines().count(), 2);
        assert_eq!(
            participants(&rows)
                .lines()
                .nth(1)
                .unwrap()
                .split('\t')
                .count(),
            2
        );
    }

    #[test]
    fn the_date_is_in_the_standards_own_column() {
        // §9.4, which is the coupling of §2.1 broken: the directory is named by
        // the scheme and anything joining on a date reads this rather than
        // parsing a directory name.
        let s = sessions(&[
            Session {
                label: "M00".into(),
                acq_time: Some("2022-01-15T03:14:15".into()),
            },
            Session {
                label: "M06".into(),
                acq_time: None,
            },
        ]);
        assert_eq!(
            s,
            "session_id\tacq_time\nses-M00\t2022-01-15T03:14:15\nses-M06\tn/a\n"
        );
    }

    #[test]
    fn the_scans_are_listed_in_a_fixed_order() {
        // Two runs of one version must write the same bytes, or a handover
        // sees a change that is not one.
        let rows = vec![
            Scan {
                filename: "anat/b.nii.gz".into(),
                acq_time: None,
            },
            Scan {
                filename: "anat/a.nii.gz".into(),
                acq_time: Some("t".into()),
            },
        ];
        let a = scans(&rows);
        let mut reversed = rows.clone();
        reversed.reverse();
        assert_eq!(a, scans(&reversed));
        assert!(
            a.starts_with("filename\tacq_time\nanat/a.nii.gz\tt\n"),
            "{a}"
        );
    }

    #[test]
    fn the_readme_says_where_things_were_put() {
        let counts = [("raw".to_string(), 12i64), ("nowhere".to_string(), 3)].into();
        let text = readme("a cohort", &made_by(), &counts);
        assert!(text.contains("2026.09.05.1"), "{text}");
        assert!(text.contains("localizers: sourcedata"), "{text}");
        assert!(text.contains("| nowhere | 3 |"), "{text}");
    }

    #[test]
    fn the_derivative_tree_is_a_dataset_of_its_own() {
        let d: serde_json::Value =
            serde_json::from_str(&derivative_description("a cohort", &made_by())).unwrap();
        assert_eq!(d["DatasetType"], "derivative");
        assert_eq!(d["SourceDatasets"][0]["URL"], "../..");
    }
}
