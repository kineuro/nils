// SPDX-License-Identifier: AGPL-3.0-only

//! A dataset's subjects, from one place. Nima, 2026-10-11: the chat and the
//! page tell the same story, so every reader that counts or lists the
//! subjects of a dataset asks this module, as every reader of the main
//! scans asks [`crate::pick_truth`].
//!
//! - **A subject with a scan** is of every dataset whose tree holds a file
//!   of one of its scans, whoever read the scan first (record 55's
//!   duplicate policy, [`crate::location`]): [`scanned`].
//! - **A subject with no scan** is of the dataset it was pseudonymised
//!   from, the one whose pseudonymise run gave it its subject code and
//!   wrote its files (`pseudonym_file.subject_id`): the read set every copy
//!   of them aside (record 56: none of them is a scan), so no scan places
//!   the subject and its pseudonymised files do. A page says it is a
//!   subject "with no scans": [`unscanned`].
//! - **A subject with no scan that no pseudonymise run recorded** (one a
//!   registry from before the run kept each file's subject made, or one a
//!   map or an import made) is of no dataset. A reader says its dataset is
//!   not known; it never guesses one.
//!
//! [`subjects`] is the two together, what a reader counts or lists. A
//! reader of scans (the main scans, the scans a viewer shows) reads
//! [`scanned`] alone, since a subject with no scan has none to read.

use std::collections::BTreeSet;

use crate::dialect::qualified;
use crate::store::{Error, Store};

/// The states of a pseudonymised file whose copy stands in the tree the
/// registry reads: written by a run, or the copy from before still there.
const COPIED: &str = "'written', 'unchanged'";

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn text(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The subjects with a scan a dataset's tree holds a file of, as a relation
/// of one column, `subject_id`, over the dataset's `source` rows
/// ([`crate::place::dataset_sources`]); none where it has none.
pub fn scanned(schema: Option<&str>, sources: &[i64]) -> String {
    let filter = if sources.is_empty() {
        "1 = 0".to_string()
    } else {
        format!("hss.source_id IN ({})", list(sources))
    };
    format!(
        "SELECT hse.subject_id FROM {} hss JOIN {} hst ON hst.id = hss.stack_id \
         JOIN {} hse ON hse.id = hst.series_id WHERE {filter}",
        qualified(schema, "source_stack"),
        qualified(schema, "stack"),
        qualified(schema, "series"),
    )
}

/// The subjects with no scan at all that a dataset (its place's id) was
/// pseudonymised for, as a relation of one column, `subject_id`, each
/// once: the subjects its pseudonymised files carry whose copy stands in
/// its tree, none of whom has a stack anywhere.
pub fn unscanned(schema: Option<&str>, place_id: i64) -> String {
    format!(
        "SELECT nu.subject_id FROM (SELECT DISTINCT npf.subject_id FROM {} npf \
         WHERE npf.place_id = {place_id} AND npf.state IN ({COPIED}) \
         AND npf.subject_id IS NOT NULL) nu \
         WHERE NOT EXISTS (SELECT 1 FROM {} nse JOIN {} nst ON nst.series_id = nse.id \
         WHERE nse.subject_id = nu.subject_id)",
        qualified(schema, "pseudonym_file"),
        qualified(schema, "series"),
        qualified(schema, "stack"),
    )
}

/// A dataset's subjects, [`scanned`] and [`unscanned`] together, as a
/// relation of one column, `subject_id`, each once.
pub fn subjects(schema: Option<&str>, place_id: i64, sources: &[i64]) -> String {
    format!(
        "{} UNION {}",
        scanned(schema, sources),
        unscanned(schema, place_id)
    )
}

/// The names of the datasets a subject with no scan is of, as one list
/// joined by `, ` in the order `datasets` are given, or null where it is of
/// none: for a reader of one subject at a time (the ask's `dataset` of a
/// subject), `subject` the SQL of its id and `datasets` each `(place id,
/// name)`. Every [`unscanned`] relation is read once by a statement, not
/// once a subject.
pub fn unscanned_names(schema: Option<&str>, subject: &str, datasets: &[(i64, String)]) -> String {
    if datasets.is_empty() {
        return "CAST(NULL AS TEXT)".to_string();
    }
    let each: Vec<String> = datasets
        .iter()
        .map(|(id, name)| {
            format!(
                "CASE WHEN {subject} IN ({}) THEN {} ELSE '' END",
                unscanned(schema, *id),
                text(&format!(", {name}"))
            )
        })
        .collect();
    format!("NULLIF(SUBSTR({}, 3), '')", each.join(" || "))
}

/// How many subjects a dataset has, and how many of them have no scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Count {
    pub subjects: i64,
    pub no_scans: i64,
}

/// How many subjects a dataset (its place's id and its `source` rows) has,
/// those with no scan among them. A subject merged into another is not
/// counted (record 26 §6): the merge moved its rows to the other.
pub fn count(store: &mut Store, place_id: i64, sources: &[i64]) -> Result<Count, Error> {
    let schema = store.schema().map(str::to_string);
    let subject = store.qualified("subject");
    let mut n = |relation: String| -> Result<i64, Error> {
        store.query(
            &format!(
                "SELECT COUNT(DISTINCT m.subject_id) FROM ({relation}) m \
                 JOIN {subject} su ON su.id = m.subject_id WHERE su.merged_into IS NULL"
            ),
            &[],
        )?[0]
            .int(0)
    };
    let with_scans = if sources.is_empty() {
        0
    } else {
        n(scanned(schema.as_deref(), sources))?
    };
    let no_scans = n(unscanned(schema.as_deref(), place_id))?;
    Ok(Count {
        subjects: with_scans + no_scans,
        no_scans,
    })
}

fn ids(store: &mut Store, relation: &str) -> Result<BTreeSet<i64>, Error> {
    let sql = format!(
        "SELECT DISTINCT m.subject_id FROM ({relation}) m JOIN {} su ON su.id = m.subject_id \
         WHERE su.merged_into IS NULL",
        store.qualified("subject")
    );
    store.query(&sql, &[])?.iter().map(|r| r.int(0)).collect()
}

/// A dataset's subjects, by id: what a pick run for the dataset decides
/// and what its main scans count.
pub fn of(store: &mut Store, place_id: i64, sources: &[i64]) -> Result<BTreeSet<i64>, Error> {
    let relation = subjects(store.schema(), place_id, sources);
    ids(store, &relation)
}

/// A dataset's subjects with no scan, by id.
pub fn with_no_scans(store: &mut Store, place_id: i64) -> Result<BTreeSet<i64>, Error> {
    let relation = unscanned(store.schema(), place_id);
    ids(store, &relation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dataset_s_subjects_are_its_scans_and_those_it_was_pseudonymised_for() {
        assert_eq!(
            scanned(None, &[3, 4]),
            "SELECT hse.subject_id FROM source_stack hss JOIN stack hst ON hst.id = hss.stack_id \
             JOIN series hse ON hse.id = hst.series_id WHERE hss.source_id IN (3, 4)"
        );
        // a dataset nothing has read holds no scan
        assert!(scanned(Some("s"), &[]).ends_with(
            "FROM s.source_stack hss JOIN s.stack hst \
             ON hst.id = hss.stack_id JOIN s.series hse ON hse.id = hst.series_id WHERE 1 = 0"
        ));
        let none = unscanned(Some("s"), 7);
        assert!(none.contains("FROM s.pseudonym_file npf WHERE npf.place_id = 7"));
        assert!(none.contains("npf.state IN ('written', 'unchanged')"));
        assert!(none.contains("NOT EXISTS (SELECT 1 FROM s.series nse JOIN s.stack nst"));
        assert_eq!(
            subjects(None, 7, &[3]),
            format!("{} UNION {}", scanned(None, &[3]), unscanned(None, 7))
        );
    }

    #[test]
    fn a_subject_with_no_scan_names_the_datasets_it_was_pseudonymised_for() {
        assert_eq!(unscanned_names(None, "su.id", &[]), "CAST(NULL AS TEXT)");
        let names = unscanned_names(None, "su.id", &[(2, "a".into()), (5, "o'b".into())]);
        assert!(names.starts_with("NULLIF(SUBSTR(CASE WHEN su.id IN (SELECT nu.subject_id"));
        assert!(names.contains("THEN ', a' ELSE '' END || CASE WHEN su.id IN ("));
        assert!(names.contains("npf.place_id = 5"));
        assert!(names.ends_with("THEN ', o''b' ELSE '' END, 3), '')"));
    }
}
