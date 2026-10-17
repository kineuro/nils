// SPDX-License-Identifier: AGPL-3.0-only

//! The subjects counted in a dataset, through its stacks, from one place.
//! Nima, 2026-10-12: "subject only belong to cohorts. its the stack that
//! belong to dataset", "dataset is like a 'folder' containing data. Cohort
//! is membership. so when you say N number of subject are in that dataset it
//! just means they have some of their stacks in that path."
//!
//! - **A dataset holds stacks and files, never subjects.** A subject is
//!   counted in a dataset when the dataset's tree holds a file of one of its
//!   stacks (record 55's duplicate policy, [`crate::location`]): [`scanned`].
//!   A stack with files in two datasets is one stack with a path in each:
//!   the registry keeps it once, each dataset with a path of it counts it
//!   and its subject once, and nothing ties it or its subject to either
//!   beyond its paths ([`holders`]).
//! - **Only a cohort has members** ([`crate::cohort`]); nothing joins one on
//!   its own because a dataset holds a file of the subject.
//! - **A subject whose files in a dataset were all set aside** by the read
//!   (record 56: none of them a scan) has no stack there and is not counted
//!   in it. Where the dataset's pseudonymise run recorded whose those files
//!   are, the set-aside files say how many subjects with no stack there they
//!   are of ([`set_aside_without_stacks`]): a fact about the folder's
//!   files, never a member.
//!
//! Every reader that counts or lists a dataset's subjects reads [`scanned`]
//! (the dataset summary, the sources door, the picks summary and a pick run
//! for the dataset, the Grid and its visits, the main scans), and every
//! reader of the datasets of a stack, a subject or a session reads
//! [`holders`] (the ask's `dataset`).

use std::collections::BTreeSet;

use crate::dialect::qualified;
use crate::store::{Error, Store};

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn text(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The join that reads the `source` rows holding a file of a stack: the
/// stack aliased `stack`, the holder aliased `holder`, whose `source_id`
/// is a source whose tree has a file of the stack. A stack is held by
/// every such source, whoever read it first.
pub fn holders(schema: Option<&str>, stack: &str, holder: &str) -> String {
    format!(
        "JOIN {} {holder} ON {holder}.stack_id = {stack}.id",
        qualified(schema, "source_stack")
    )
}

/// The subjects counted in a dataset, those with a stack its tree holds a
/// file of, as a relation of one column, `subject_id`, over the dataset's
/// `source` rows ([`crate::place::dataset_sources`]); none where it has
/// none.
pub fn scanned(schema: Option<&str>, sources: &[i64]) -> String {
    let filter = if sources.is_empty() {
        "1 = 0".to_string()
    } else {
        format!("hss.source_id IN ({})", list(sources))
    };
    format!(
        "SELECT hse.subject_id FROM {} hst {} JOIN {} hse ON hse.id = hst.series_id WHERE {filter}",
        qualified(schema, "stack"),
        holders(schema, "hst", "hss"),
        qualified(schema, "series"),
    )
}

fn ids(store: &mut Store, relation: &str) -> Result<BTreeSet<i64>, Error> {
    let sql = format!(
        "SELECT DISTINCT m.subject_id FROM ({relation}) m JOIN {} su ON su.id = m.subject_id \
         WHERE su.merged_into IS NULL",
        store.qualified("subject")
    );
    store.query(&sql, &[])?.iter().map(|r| r.int(0)).collect()
}

/// How many subjects are counted in a dataset (its `source` rows): the
/// subjects with a stack its tree holds a file of, each once. A subject
/// merged into another is not counted (record 26 §6): the merge moved its
/// stacks to the other.
pub fn count(store: &mut Store, sources: &[i64]) -> Result<i64, Error> {
    if sources.is_empty() {
        return Ok(0);
    }
    let sql = format!(
        "SELECT COUNT(DISTINCT m.subject_id) FROM ({}) m JOIN {} su ON su.id = m.subject_id \
         WHERE su.merged_into IS NULL",
        scanned(store.schema(), sources),
        store.qualified("subject")
    );
    store.query(&sql, &[])?[0].int(0)
}

/// The subjects counted in a dataset, by id: what a pick run for the
/// dataset decides and what its main scans count.
pub fn of(store: &mut Store, sources: &[i64]) -> Result<BTreeSet<i64>, Error> {
    if sources.is_empty() {
        return Ok(BTreeSet::new());
    }
    let relation = scanned(store.schema(), sources);
    ids(store, &relation)
}

/// What a dataset's set-aside files say of subjects with no stack in it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SetAside {
    /// The subjects the set-aside files are of that have no stack the
    /// dataset's tree holds a file of.
    pub subjects: i64,
    /// How many of the set-aside files are theirs.
    pub files: i64,
}

/// The subjects a dataset's set-aside files are of that have no stack in
/// its tree, and how many of those files are theirs: a file the read set
/// aside for one of `reasons` (the policy classes) is a subject's where the
/// dataset's pseudonymise run wrote it under that subject. Its place's id
/// and its `source` rows name the dataset. A file read in place, which no
/// pseudonymise run wrote, names no subject, and none is counted for it.
///
/// Read from the subjects (2026-10-17), where every copy of the dataset was
/// joined to every file by its path: on a dataset of 30 million files that
/// took 4 to 13 seconds. The subjects its copies carry are read from the
/// index a subject at a time ([`copied`]), those with a stack in its tree
/// are left out, and only the copies of the few left are matched to the
/// files by path.
pub fn set_aside_without_stacks(
    store: &mut Store,
    place_id: i64,
    sources: &[i64],
    reasons: &[&str],
) -> Result<SetAside, Error> {
    if sources.is_empty() || reasons.is_empty() {
        return Ok(SetAside::default());
    }
    let carried = copied(store, place_id)?;
    if carried.is_empty() {
        return Ok(SetAside::default());
    }
    let sql = format!(
        "SELECT DISTINCT ns.subject_id FROM ({}) ns WHERE ns.subject_id IS NOT NULL",
        scanned(store.schema(), sources)
    );
    let mut with: BTreeSet<i64> = BTreeSet::new();
    for r in store.query(&sql, &[])? {
        with.insert(r.int(0)?);
    }
    let without: Vec<i64> = carried.difference(&with).copied().collect();
    let reasons = reasons
        .iter()
        .map(|r| text(r))
        .collect::<Vec<_>>()
        .join(", ");
    let (mut subjects, mut files) = (BTreeSet::new(), BTreeSet::new());
    for chunk in without.chunks(500) {
        let sql = format!(
            "SELECT spf.subject_id, ssf.id FROM {} spf JOIN {} ssf ON ssf.path = spf.out_path \
             WHERE spf.place_id = {place_id} AND spf.state IN ('written', 'unchanged') \
             AND spf.subject_id IN ({}) AND ssf.source_id IN ({}) AND ssf.status = 'quarantined' \
             AND ssf.reason IN ({reasons})",
            store.qualified("pseudonym_file"),
            store.qualified("source_file"),
            list(chunk),
            list(sources),
        );
        for r in store.query(&sql, &[])? {
            subjects.insert(r.int(0)?);
            files.insert(r.int(1)?);
        }
    }
    Ok(SetAside {
        subjects: subjects.len() as i64,
        files: files.len() as i64,
    })
}

/// The subjects a place's pseudonymised files carry: those of its files
/// written, or whose copy from before stands (`unchanged`). Read from the
/// index on the place, the state and the subject one subject at a time, as
/// many reads as there are subjects, however many files each has
/// (2026-10-17: a DISTINCT over every copy of a dataset of 30 million files
/// took 13 seconds).
pub fn copied(store: &mut Store, place_id: i64) -> Result<BTreeSet<i64>, Error> {
    let files = store.qualified("pseudonym_file");
    let mut out = BTreeSet::new();
    for state in ["written", "unchanged"] {
        let next = |after: &str| {
            format!(
                "(SELECT f.subject_id FROM {files} f WHERE f.place_id = {place_id} \
                 AND f.state = '{state}' AND f.subject_id {after} ORDER BY f.subject_id LIMIT 1)"
            )
        };
        let sql = format!(
            "WITH RECURSIVE carried(subject_id) AS (SELECT {} \
             UNION ALL SELECT {} FROM carried c WHERE c.subject_id IS NOT NULL) \
             SELECT subject_id FROM carried WHERE subject_id IS NOT NULL",
            next("IS NOT NULL"),
            next("> c.subject_id"),
        );
        for r in store.query(&sql, &[])? {
            out.insert(r.int(0)?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dataset_counts_the_subjects_of_the_stacks_its_tree_holds() {
        assert_eq!(
            holders(Some("s"), "x", "h"),
            "JOIN s.source_stack h ON h.stack_id = x.id"
        );
        assert_eq!(
            scanned(None, &[3, 4]),
            "SELECT hse.subject_id FROM stack hst JOIN source_stack hss ON hss.stack_id = hst.id \
             JOIN series hse ON hse.id = hst.series_id WHERE hss.source_id IN (3, 4)"
        );
        // a dataset nothing has read holds no stack
        assert!(scanned(Some("s"), &[]).ends_with(
            "FROM s.stack hst JOIN s.source_stack hss ON hss.stack_id = hst.id \
             JOIN s.series hse ON hse.id = hst.series_id WHERE 1 = 0"
        ));
    }
}
