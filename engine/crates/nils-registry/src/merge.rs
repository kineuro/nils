// SPDX-License-Identifier: AGPL-3.0-only

//! A later link merges (record 26, decision 6). Merging an alias subject
//! into a canonical one re-points every row of the alias to the canonical
//! subject in one transaction per store, moves the alias's identities,
//! files the alias's code on the canonical subject as an identifier of
//! type `subject-code`, leaves the alias row marked merged, closes the
//! alias's provisional item, records `subject.merge` in the audit and moves
//! the epoch. Nothing is deleted but the session cache, which is derived
//! and rebuilt, and a membership interval of the alias that is the
//! canonical's own interval twice over.
//!
//! A merge stands alone ([`merge`]) or inside a larger transaction
//! ([`merge_in`]): the identifier map merges as part of its one apply, so
//! that a map whose merge fails leaves nothing behind.
//!
//! Which tables carry a subject is not remembered here by hand: every
//! table with a `subject_id` column is listed in [`SUBJECT_TABLES`] with
//! what the merge does to it, and a test fails when the schema gains one
//! the list does not name.

use std::collections::BTreeMap;

use crate::audit::{self, Action, Entry};
use crate::linkage::{self, NewIdentity, Subject, Subkeys};
use crate::review;
use crate::schema::{SUBJECT_CODE_TYPE, Type, table};
use crate::store::{Error, Param, Store};
use crate::time::now_iso;

/// What the merge does to a table that names a subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handling {
    /// `subject_id` of the alias becomes the canonical's.
    Repoint,
    /// Derived from the subject's studies: the rows of both subjects are
    /// dropped and the next `session rebuild` makes them again.
    Rebuild,
    /// The membership intervals, whose key is `(cohort, subject,
    /// joined_at)`: an interval of the alias the canonical holds too, the
    /// same cohort at the same time, is dropped when it is the same interval
    /// (both open, or both closed at one time) and closed and left on the
    /// alias otherwise; an open one whose cohort the canonical is in already
    /// is closed with `left_by = merge`; the rest are re-pointed. A digest
    /// joins every subject it makes at one time, so two subjects of one
    /// digest that turn out to be one person are the common case.
    Memberships,
}

/// Every table with a `subject_id` column, in either store, and what the
/// merge does to it.
pub const SUBJECT_TABLES: &[(&str, Handling)] = &[
    ("study", Handling::Repoint),
    ("series", Handling::Repoint),
    ("stack_fingerprint", Handling::Repoint),
    ("cohort_member", Handling::Memberships),
    ("subject_disease", Handling::Repoint),
    ("event", Handling::Repoint),
    ("pick", Handling::Repoint),
    // record 42 S4: a derivative names its subject whatever its scope
    ("derivative", Handling::Repoint),
    ("handover_subject", Handling::Repoint),
    ("session_cache", Handling::Rebuild),
    ("handle_member", Handling::Repoint),
    ("values_member", Handling::Repoint),
    // the linkage store
    ("identity", Handling::Repoint),
];

/// The tables that name a subject otherwise, each handled by name in
/// [`merge`]: a decision at subject scope (`ref` is the id as text), a
/// review item at subject scope (its `group_key`), the alias row itself,
/// and the linkage rows, which name the link and stay as history.
pub const BY_REFERENCE: &[&str] = &["decision", "review_item", "subject", "linkage"];

/// What to merge, and who says so.
#[derive(Debug, Clone)]
pub struct Ask<'a> {
    pub canonical: i64,
    pub alias: i64,
    pub why: &'a str,
    pub actor: &'a str,
    pub job_id: Option<i64>,
    /// The dataset the merge was asked from, if one.
    pub place_id: Option<i64>,
}

/// What a merge did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    pub canonical: Subject,
    pub alias: Subject,
    /// Rows re-pointed, by table.
    pub moved: BTreeMap<&'static str, u64>,
    /// Alias memberships closed because the canonical subject was in the
    /// cohort already.
    pub memberships_closed: u64,
    /// Alias memberships dropped because they were the canonical's own
    /// interval twice over: the same cohort, joined at the same time.
    pub memberships_dropped: u64,
    pub provisional_closed: u64,
    pub audit: i64,
}

impl Merged {
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "canonical": { "id": self.canonical.id, "code": self.canonical.code },
            "alias": { "id": self.alias.id, "code": self.alias.code },
            "moved": self.moved,
            "memberships_closed": self.memberships_closed,
            "memberships_dropped": self.memberships_dropped,
            "provisional_closed": self.provisional_closed,
            "audit": self.audit,
        })
    }
}

/// What the membership intervals of the alias became.
struct Memberships {
    moved: u64,
    closed: u64,
    dropped: u64,
}

/// The alias's membership intervals under the canonical subject, keeping
/// the table's one key `(cohort, subject, joined_at)` whole: see
/// [`Handling::Memberships`].
fn memberships(
    registry: &mut Store,
    canonical: i64,
    alias: i64,
    now: &str,
) -> Result<Memberships, Error> {
    let d = registry.dialect();
    let member = registry.qualified("cohort_member");
    // the same interval twice over: dropped
    let sql = format!(
        "DELETE FROM {member} WHERE subject_id = {} AND id IN (\
           SELECT a.id FROM {member} a JOIN {member} c \
             ON c.cohort_id = a.cohort_id AND c.joined_at = a.joined_at AND c.subject_id = {} \
           WHERE a.subject_id = {} \
             AND ((a.left_at IS NULL AND c.left_at IS NULL) OR a.left_at = c.left_at))",
        d.param(1, Type::Int),
        d.param(2, Type::Int),
        d.param(3, Type::Int),
    );
    let dropped = registry.execute(
        &sql,
        &[Param::Int(alias), Param::Int(canonical), Param::Int(alias)],
    )?;
    // an open interval the canonical's cohort covers already, or one that
    // shares the canonical's key: closed; the second kind is then left on
    // the alias
    let sql = format!(
        "UPDATE {member} SET left_at = {}, left_by = 'merge' \
         WHERE subject_id = {} AND left_at IS NULL AND (cohort_id IN \
         (SELECT cohort_id FROM {member} WHERE subject_id = {} AND left_at IS NULL) \
         OR EXISTS (SELECT 1 FROM {member} c WHERE c.subject_id = {} \
             AND c.cohort_id = {member}.cohort_id AND c.joined_at = {member}.joined_at))",
        d.param(1, Type::Timestamp),
        d.param(2, Type::Int),
        d.param(3, Type::Int),
        d.param(4, Type::Int),
    );
    let closed = registry.execute(
        &sql,
        &[
            Param::from(now),
            Param::Int(alias),
            Param::Int(canonical),
            Param::Int(canonical),
        ],
    )?;
    let sql = format!(
        "UPDATE {member} SET subject_id = {} WHERE subject_id = {} AND NOT EXISTS (\
           SELECT 1 FROM {member} c WHERE c.subject_id = {} \
             AND c.cohort_id = {member}.cohort_id AND c.joined_at = {member}.joined_at)",
        d.param(1, Type::Int),
        d.param(2, Type::Int),
        d.param(3, Type::Int),
    );
    let moved = registry.execute(
        &sql,
        &[
            Param::Int(canonical),
            Param::Int(alias),
            Param::Int(canonical),
        ],
    )?;
    Ok(Memberships {
        moved,
        closed,
        dropped,
    })
}

fn subject(registry: &mut Store, id: i64) -> Result<Subject, Error> {
    linkage::subjects_by_id(registry, &[id])?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Message(format!("no subject with id {id}")))
}

/// Merge `alias` into `canonical` on its own: one transaction on the
/// registry and one on the linkage store (§9.3), both written before
/// either commits, both rolled back when either fails. Refused when the
/// two are one, or either was merged already.
pub fn merge(
    registry: &mut Store,
    linkage: &mut Store,
    keys: &Subkeys,
    ask: &Ask<'_>,
) -> Result<Merged, Error> {
    registry.begin()?;
    if let Err(e) = linkage.begin() {
        let _ = registry.rollback();
        return Err(e);
    }
    match merge_in(registry, linkage, keys, ask) {
        Ok(merged) => {
            registry.commit()?;
            linkage.commit()?;
            Ok(merged)
        }
        Err(e) => {
            let _ = registry.rollback();
            let _ = linkage.rollback();
            Err(e)
        }
    }
}

/// Merge `alias` into `canonical` inside transactions the caller holds on
/// both stores and commits or rolls back itself: how the identifier map
/// merges as part of its one apply. Refused as [`merge`] refuses.
pub fn merge_in(
    registry: &mut Store,
    linkage: &mut Store,
    keys: &Subkeys,
    ask: &Ask<'_>,
) -> Result<Merged, Error> {
    if ask.canonical == ask.alias {
        return Err(Error::Message(
            "a subject cannot be merged into itself".to_string(),
        ));
    }
    let canonical = subject(registry, ask.canonical)?;
    let alias = subject(registry, ask.alias)?;
    for s in [&canonical, &alias] {
        if let Some(into) = s.merged_into {
            let into = subject(registry, into)?;
            return Err(Error::Message(format!(
                "subject {} was merged into {} already",
                s.code, into.code
            )));
        }
    }
    let now = now_iso();
    let d = registry.dialect();
    let mut moved = BTreeMap::new();
    let mut closed = 0u64;
    let mut dropped = 0u64;
    for (name, handling) in SUBJECT_TABLES {
        if crate::schema::linkage_tables()
            .iter()
            .any(|t| t.name == *name)
        {
            continue;
        }
        let n = match handling {
            Handling::Repoint => repoint(registry, name, canonical.id, alias.id)?,
            Handling::Memberships => {
                let m = memberships(registry, canonical.id, alias.id, &now)?;
                closed = m.closed;
                dropped = m.dropped;
                m.moved
            }
            Handling::Rebuild => {
                let cache = registry.qualified("session_cache");
                let ids = format!(
                    "SELECT id FROM {cache} WHERE subject_id IN ({}, {})",
                    d.param(1, Type::Int),
                    d.param(2, Type::Int)
                );
                let both = [Param::Int(canonical.id), Param::Int(alias.id)];
                for t in ["session_label", "session_cache_study"] {
                    registry.execute(
                        &format!(
                            "DELETE FROM {} WHERE session_id IN ({ids})",
                            registry.qualified(t)
                        ),
                        &both,
                    )?;
                }
                registry.execute(
                    &format!(
                        "DELETE FROM {cache} WHERE subject_id IN ({}, {})",
                        d.param(1, Type::Int),
                        d.param(2, Type::Int)
                    ),
                    &both,
                )?
            }
        };
        moved.insert(*name, n);
    }
    // a decision at subject scope names the subject by its id as text
    let sql = format!(
        "UPDATE {} SET ref = {} WHERE scope = 'subject' AND ref = {}",
        registry.qualified("decision"),
        d.param(1, Type::Text),
        d.param(2, Type::Text)
    );
    let decisions = registry.execute(
        &sql,
        &[
            Param::from(canonical.id.to_string()),
            Param::from(alias.id.to_string()),
        ],
    )?;
    moved.insert("decision", decisions);
    let provisional = review::close_provisional(
        registry,
        alias.id,
        ask.actor,
        &serde_json::json!({ "merged_into": canonical.code, "why": ask.why }),
    )?;
    registry.update_by_id(
        table("subject"),
        &[
            ("merged_into", Param::Int(canonical.id)),
            ("merged_at", Param::from(now.as_str())),
        ],
        "id",
        alias.id,
    )?;

    // the linkage store: the identities move, and the alias's code is filed
    // on the canonical
    let n = repoint(linkage, "identity", canonical.id, alias.id)?;
    moved.insert("identity", n);
    let type_id = match linkage::id_type_id(linkage, SUBJECT_CODE_TYPE)? {
        Some(id) => id,
        None => linkage::add_id_type(linkage, SUBJECT_CODE_TYPE, None)?.id,
    };
    let lookup = keys.lookup(SUBJECT_CODE_TYPE, &alias.code);
    if linkage::identities_by_lookup(linkage, std::slice::from_ref(&lookup))?.is_empty() {
        linkage::insert_identities(
            linkage,
            &[NewIdentity {
                subject_id: canonical.id,
                id_type_id: type_id,
                lookup,
                ciphertext: keys.seal(&alias.code),
                source: "merge",
                first_batch_id: None,
            }],
        )?;
    }

    // the audit row last, with everything counted
    let audit = audit::record_in(
        registry,
        &Entry {
            principal: ask.actor,
            action: Action::SubjectMerge,
            scope: serde_json::json!({
                "canonical": { "id": canonical.id, "code": canonical.code },
                "alias": { "id": alias.id, "code": alias.code },
            }),
            policy: None,
            job_id: ask.job_id,
            details: Some(serde_json::json!({
                "why": ask.why,
                "moved": moved,
                "memberships_closed": closed,
                "memberships_dropped": dropped,
                "provisional_closed": provisional,
                "place": ask.place_id,
            })),
        },
    )?;
    Ok(Merged {
        canonical,
        alias,
        moved,
        memberships_closed: closed,
        memberships_dropped: dropped,
        provisional_closed: provisional,
        audit,
    })
}

fn repoint(store: &mut Store, name: &str, canonical: i64, alias: i64) -> Result<u64, Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET subject_id = {} WHERE subject_id = {}",
        store.qualified(name),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    store.execute(&sql, &[Param::Int(canonical), Param::Int(alias)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::{self, Kind};
    use crate::schema::{linkage_tables, registry_tables};

    const KEY: &[u8] = b"nils-fixture-key";

    /// The list and the schema agree: every table with a `subject_id`
    /// column is handled, and nothing handled is missing from the schema.
    /// A new table that carries a subject fails here until the merge says
    /// what it does to it.
    #[test]
    fn every_table_with_a_subject_id_column_is_handled() {
        let with_subject: Vec<&str> = registry_tables()
            .iter()
            .chain(linkage_tables())
            .filter(|t| t.column("subject_id").is_some())
            .map(|t| t.name)
            .collect();
        for name in &with_subject {
            assert!(
                SUBJECT_TABLES.iter().any(|(n, _)| n == name),
                "{name} has a subject_id column and the merge does not say what it does to it"
            );
        }
        for (name, _) in SUBJECT_TABLES {
            assert!(
                with_subject.contains(name),
                "{name} is handled but has no subject_id column"
            );
        }
        for name in BY_REFERENCE {
            assert!(
                registry_tables()
                    .iter()
                    .chain(linkage_tables())
                    .any(|t| t.name == *name),
                "{name} is not a table"
            );
        }
        // the alias row keeps its columns
        assert!(table("subject").column("merged_into").is_some());
        assert!(table("subject").column("merged_at").is_some());
    }

    fn stores() -> (Store, Store, Subkeys) {
        let mut registry = Store::sqlite_in_memory().unwrap();
        migrate::migrate(&mut registry, Kind::Registry).unwrap();
        let mut linkage = Store::sqlite_in_memory().unwrap();
        migrate::migrate(&mut linkage, Kind::Linkage).unwrap();
        (registry, linkage, Subkeys::derive(KEY))
    }

    fn one(store: &mut Store, sql: &str) -> i64 {
        store.query(sql, &[]).unwrap()[0].int(0).unwrap()
    }

    fn exec(store: &mut Store, sql: &str) {
        store.execute(sql, &[]).unwrap();
    }

    #[test]
    fn a_merge_repoints_every_row_of_the_alias_and_files_its_code() {
        let (mut registry, mut linkage, keys) = stores();
        exec(
            &mut registry,
            "INSERT INTO subject (id, code, created_at) VALUES (1, 'canon', 't'), (2, 'alias', 't'), (3, 'other', 't')",
        );
        // one row per subject table for the alias, and a few for the canonical
        exec(
            &mut registry,
            "INSERT INTO study (id, study_instance_uid, subject_id, first_batch_id) VALUES (10, 'S.1', 2, 1), (11, 'S.2', 1, 1)",
        );
        exec(
            &mut registry,
            "INSERT INTO series (id, series_instance_uid, study_id, subject_id, n_instances, n_stacks, first_batch_id) VALUES (20, 'R.1', 10, 2, 1, 1, 1)",
        );
        exec(
            &mut registry,
            "INSERT INTO stack_fingerprint (stack_id, series_id, study_id, subject_id, modality, orientation, n_instances, stack_index, stacks_in_series, job_id, epoch) VALUES (30, 20, 10, 2, 'MR', 'ax', 1, 0, 1, 1, 1)",
        );
        exec(
            &mut registry,
            "INSERT INTO cohort (id, name, owner, created_at) VALUES (1, 'a', 'o', 't'), (2, 'b', 'o', 't')",
        );
        exec(
            &mut registry,
            "INSERT INTO cohort_member (cohort_id, subject_id, joined_at, source) VALUES (1, 1, 't1', 'import'), (1, 2, 't2', 'import'), (2, 2, 't3', 'import')",
        );
        exec(
            &mut registry,
            "INSERT INTO disease (id, name) VALUES (1, 'd')",
        );
        exec(
            &mut registry,
            "INSERT INTO subject_disease (subject_id, disease_id, created_at) VALUES (2, 1, 't')",
        );
        exec(
            &mut registry,
            "INSERT INTO observation_type (id, name, category, is_primary) VALUES (1, 'k', 'c', 0)",
        );
        exec(
            &mut registry,
            "INSERT INTO event (subject_id, observation_type_id, event_date, created_at) VALUES (2, 1, '2020-01-01', 't')",
        );
        exec(
            &mut registry,
            "INSERT INTO pick (model, role, subject_id, session_day, scheme, reference, pack, pack_version, actor, author_kind, decided_at) VALUES ('m', 'r', 2, '2020-01-01', 's', 'ref', 'mri', '1', 'a', 'agent', 't')",
        );
        exec(
            &mut registry,
            "INSERT INTO handover_subject (archive_id, subject_id, code, files, bytes) VALUES (1, 2, 'alias', 1, 1)",
        );
        exec(
            &mut registry,
            "INSERT INTO session_cache (id, subject_id, window_days, timeline_digest, first, last, n_studies, epoch, built_at) VALUES (40, 2, 30, 'x', '2020-01-01', '2020-01-01', 1, 1, 't'), (41, 1, 30, 'y', '2020-02-01', '2020-02-01', 1, 1, 't'), (42, 3, 30, 'z', '2020-02-01', '2020-02-01', 1, 1, 't')",
        );
        exec(
            &mut registry,
            "INSERT INTO session_cache_study (session_id, study_id, window_days) VALUES (40, 10, 30), (41, 11, 30)",
        );
        exec(
            &mut registry,
            "INSERT INTO session_label (session_id, scheme_digest, flagged) VALUES (40, 'd', 0)",
        );
        exec(
            &mut registry,
            "INSERT INTO handle_member (handle_id, position, key, subject_id) VALUES (1, 0, 2, 2)",
        );
        exec(
            &mut registry,
            "INSERT INTO values_member (source_id, position, subject_id) VALUES (1, 0, 2)",
        );
        exec(
            &mut registry,
            "INSERT INTO decision (scope, ref, axis, actor, author_kind, decided_at) VALUES ('subject', '2', 'sex', 'a', 'person', 't'), ('stack', '2', 'x', 'a', 'person', 't')",
        );
        review::raise_provisional(
            &mut registry,
            &review::Provisional {
                subject_id: 2,
                code: "alias",
                id_type: "patient-id",
                shape: "AA",
                place_id: 1,
                place: "p",
                files: 1,
                batch_id: None,
                job_id: None,
            },
            "t",
        )
        .unwrap();
        // the linkage store: an identity each
        linkage::insert_identities(
            &mut linkage,
            &[
                NewIdentity {
                    subject_id: 1,
                    id_type_id: 1,
                    lookup: keys.lookup("patient-id", "P1"),
                    ciphertext: keys.seal("P1"),
                    source: "dicom",
                    first_batch_id: None,
                },
                NewIdentity {
                    subject_id: 2,
                    id_type_id: 1,
                    lookup: keys.lookup("patient-id", "P2"),
                    ciphertext: keys.seal("P2"),
                    source: "dicom",
                    first_batch_id: None,
                },
            ],
        )
        .unwrap();
        // a store migrated in memory has no epoch row yet: it reads as zero
        let epoch_before = registry
            .query_opt("SELECT value FROM registry_meta WHERE key = 'epoch'", &[])
            .unwrap()
            .map(|r| r.text(0).unwrap().parse::<i64>().unwrap())
            .unwrap_or(0);

        let merged = merge(
            &mut registry,
            &mut linkage,
            &keys,
            &Ask {
                canonical: 1,
                alias: 2,
                why: "the clinic renamed P2 to P1",
                actor: "anna@lab",
                job_id: Some(9),
                place_id: None,
            },
        )
        .unwrap();
        assert_eq!(merged.canonical.code, "canon");
        assert_eq!(merged.alias.code, "alias");
        assert_eq!(merged.memberships_closed, 1);
        assert_eq!(merged.memberships_dropped, 0);
        assert_eq!(merged.provisional_closed, 1);
        let moved = &merged.moved;
        for (t, n) in [
            ("study", 1),
            ("series", 1),
            ("stack_fingerprint", 1),
            ("cohort_member", 2),
            ("subject_disease", 1),
            ("event", 1),
            ("pick", 1),
            ("handover_subject", 1),
            ("session_cache", 2),
            ("handle_member", 1),
            ("values_member", 1),
            ("decision", 1),
            ("identity", 1),
        ] {
            assert_eq!(moved.get(t).copied(), Some(n), "{t}");
        }
        // every registry table with a subject_id column names the alias nowhere
        for (t, _) in SUBJECT_TABLES {
            if linkage_tables().iter().any(|l| l.name == *t) {
                continue;
            }
            assert_eq!(
                one(
                    &mut registry,
                    &format!("SELECT COUNT(*) FROM {t} WHERE subject_id = 2")
                ),
                0,
                "{t}"
            );
        }
        assert_eq!(
            one(
                &mut registry,
                "SELECT COUNT(*) FROM study WHERE subject_id = 1"
            ),
            2
        );
        // the duplicate membership closed by the merge, the other moved open
        let rows = registry
            .query(
                "SELECT cohort_id, subject_id, left_at IS NOT NULL, left_by FROM cohort_member ORDER BY id",
                &[],
            )
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            (
                rows[1].int(0).unwrap(),
                rows[1].int(1).unwrap(),
                rows[1].int(2).unwrap()
            ),
            (1, 1, 1)
        );
        assert_eq!(rows[1].text(3).unwrap(), "merge");
        assert_eq!(
            (
                rows[2].int(0).unwrap(),
                rows[2].int(1).unwrap(),
                rows[2].int(2).unwrap()
            ),
            (2, 1, 0)
        );
        // the session cache of both is gone, the other subject's stays
        assert_eq!(one(&mut registry, "SELECT COUNT(*) FROM session_cache"), 1);
        assert_eq!(
            one(&mut registry, "SELECT COUNT(*) FROM session_cache_study"),
            0
        );
        assert_eq!(one(&mut registry, "SELECT COUNT(*) FROM session_label"), 0);
        // the subject-scope decision moved, the stack one did not
        assert_eq!(
            one(
                &mut registry,
                "SELECT COUNT(*) FROM decision WHERE scope = 'subject' AND ref = '1'"
            ),
            1
        );
        assert_eq!(
            one(
                &mut registry,
                "SELECT COUNT(*) FROM decision WHERE scope = 'stack' AND ref = '2'"
            ),
            1
        );
        // the alias row stays, marked
        let row = registry
            .query_opt(
                "SELECT merged_into, merged_at FROM subject WHERE id = 2",
                &[],
            )
            .unwrap()
            .unwrap();
        assert_eq!(row.int(0).unwrap(), 1);
        assert!(row.text(1).unwrap().ends_with('Z'));
        assert_eq!(
            one(
                &mut registry,
                "SELECT COUNT(*) FROM subject WHERE merged_into IS NULL"
            ),
            2
        );
        // the provisional item closed with the merge as its decision
        let item = registry
            .query_opt("SELECT status, decision FROM review_item", &[])
            .unwrap()
            .unwrap();
        assert_eq!(item.text(0).unwrap(), "superseded");
        assert!(item.text(1).unwrap().contains("canon"));
        // the audit names both codes and the why, and the epoch moved
        let audit = registry
            .query_opt(
                "SELECT action, principal, scope, details, job_id, epoch FROM audit",
                &[],
            )
            .unwrap()
            .unwrap();
        assert_eq!(audit.text(0).unwrap(), "subject.merge");
        assert_eq!(audit.text(1).unwrap(), "anna@lab");
        assert!(audit.text(2).unwrap().contains("\"code\":\"alias\""));
        assert!(audit.text(3).unwrap().contains("renamed P2 to P1"));
        assert_eq!(audit.int(4).unwrap(), 9);
        assert_eq!(audit.int(5).unwrap(), epoch_before + 1);
        // the linkage store: identities on the canonical, the alias's code
        // filed as subject-code, from the merge
        let shown = linkage::reveal(&mut linkage, &keys, 1, "tester", None).unwrap();
        let mut values: Vec<(String, String, String)> = shown
            .into_iter()
            .map(|r| (r.id_type, r.value, r.source))
            .collect();
        values.sort();
        assert_eq!(
            values,
            [
                (
                    "patient-id".to_string(),
                    "P1".to_string(),
                    "dicom".to_string()
                ),
                (
                    "patient-id".to_string(),
                    "P2".to_string(),
                    "dicom".to_string()
                ),
                (
                    "subject-code".to_string(),
                    "alias".to_string(),
                    "merge".to_string()
                ),
            ]
        );

        // merged once: the alias is refused as either side
        let err = merge(
            &mut registry,
            &mut linkage,
            &keys,
            &Ask {
                canonical: 2,
                alias: 3,
                why: "x",
                actor: "anna@lab",
                job_id: None,
                place_id: None,
            },
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("merged into canon already"),
            "{err}"
        );
        assert!(
            merge(
                &mut registry,
                &mut linkage,
                &keys,
                &Ask {
                    canonical: 1,
                    alias: 1,
                    why: "x",
                    actor: "anna@lab",
                    job_id: None,
                    place_id: None,
                }
            )
            .is_err()
        );
    }

    /// Lab 26, defect 1: a digest joins every subject it makes to the
    /// dataset's cohort at one time, so two of its subjects that turn out
    /// to be one person hold the same interval, and re-pointing the
    /// alias's row would break the table's key. The interval the canonical
    /// holds already is dropped; one the canonical holds closed is closed
    /// and left on the alias; the rest move.
    #[test]
    fn two_subjects_joined_by_one_digest_merge_without_breaking_the_membership_key() {
        let (mut registry, mut linkage, keys) = stores();
        exec(
            &mut registry,
            "INSERT INTO subject (id, code, created_at) VALUES (1, 'canon', 't'), (2, 'alias', 't'), (3, 'third', 't')",
        );
        exec(
            &mut registry,
            "INSERT INTO cohort (id, name, owner, created_at) VALUES (1, 'north', 'o', 't'), (2, 'south', 'o', 't'), (3, 'east', 'o', 't')",
        );
        // north: both joined by the one digest at t1, both open
        // south: the canonical joined at t2 and left at t3; the alias joined
        //        at t2 and is still in
        // east: the alias alone, at t4
        exec(
            &mut registry,
            "INSERT INTO cohort_member (cohort_id, subject_id, joined_at, left_at, source, batch_id) VALUES \
             (1, 1, '2026-09-16T10:00:00Z', NULL, 'digest', 7), \
             (1, 2, '2026-09-16T10:00:00Z', NULL, 'digest', 7), \
             (2, 1, '2026-09-16T11:00:00Z', '2026-09-16T12:00:00Z', 'manual', NULL), \
             (2, 2, '2026-09-16T11:00:00Z', NULL, 'manual', NULL), \
             (3, 2, '2026-09-16T13:00:00Z', NULL, 'manual', NULL)",
        );
        let merged = merge(
            &mut registry,
            &mut linkage,
            &keys,
            &Ask {
                canonical: 1,
                alias: 2,
                why: "the map named both numbers as one person",
                actor: "anna@lab",
                job_id: None,
                place_id: Some(1),
            },
        )
        .unwrap();
        assert_eq!(merged.memberships_dropped, 1, "north, the same interval");
        assert_eq!(merged.memberships_closed, 1, "south, the alias's open one");
        assert_eq!(merged.moved.get("cohort_member"), Some(&1), "east");
        let rows = registry
            .query(
                "SELECT cohort_id, subject_id, left_at IS NOT NULL, left_by FROM cohort_member ORDER BY cohort_id, subject_id",
                &[],
            )
            .unwrap();
        let got: Vec<(i64, i64, i64, Option<String>)> = rows
            .iter()
            .map(|r| {
                (
                    r.int(0).unwrap(),
                    r.int(1).unwrap(),
                    r.int(2).unwrap(),
                    r.opt_text(3).unwrap().map(str::to_string),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (1, 1, 0, None),
                (2, 1, 1, None),
                (2, 2, 1, Some("merge".to_string())),
                (3, 1, 0, None),
            ],
            "{got:?}"
        );
        // the canonical is in north once, open; the alias holds only the
        // closed south interval, as history
        assert_eq!(
            one(
                &mut registry,
                "SELECT COUNT(*) FROM cohort_member WHERE subject_id = 1 AND cohort_id = 1"
            ),
            1
        );
        let details = registry
            .query_opt("SELECT details FROM audit", &[])
            .unwrap()
            .unwrap();
        assert!(
            details
                .text(0)
                .unwrap()
                .contains("\"memberships_dropped\":1")
        );
        // the third subject joined north at the same time as well: merging
        // it too drops its interval the same way
        exec(
            &mut registry,
            "INSERT INTO cohort_member (cohort_id, subject_id, joined_at, source, batch_id) VALUES (1, 3, '2026-09-16T10:00:00Z', 'digest', 7)",
        );
        let third = merge(
            &mut registry,
            &mut linkage,
            &keys,
            &Ask {
                canonical: 1,
                alias: 3,
                why: "one person",
                actor: "anna@lab",
                job_id: None,
                place_id: None,
            },
        )
        .unwrap();
        assert_eq!(third.memberships_dropped, 1);
        assert_eq!(one(&mut registry, "SELECT COUNT(*) FROM cohort_member"), 4);
    }

    /// A merge that fails leaves nothing behind in either store.
    #[test]
    fn a_merge_that_fails_writes_nothing_in_either_store() {
        let (mut registry, mut linkage, keys) = stores();
        exec(
            &mut registry,
            "INSERT INTO subject (id, code, created_at) VALUES (1, 'canon', 't'), (2, 'alias', 't')",
        );
        exec(
            &mut registry,
            "INSERT INTO study (id, study_instance_uid, subject_id, first_batch_id) VALUES (10, 'S.1', 2, 1)",
        );
        linkage::insert_identities(
            &mut linkage,
            &[NewIdentity {
                subject_id: 2,
                id_type_id: 1,
                lookup: keys.lookup("patient-id", "P2"),
                ciphertext: keys.seal("P2"),
                source: "dicom",
                first_batch_id: None,
            }],
        )
        .unwrap();
        // the linkage store refuses the alias's code being filed
        exec(
            &mut linkage,
            "CREATE TRIGGER refuse BEFORE INSERT ON identity BEGIN SELECT RAISE(ABORT, 'the store refused'); END",
        );
        let err = merge(
            &mut registry,
            &mut linkage,
            &keys,
            &Ask {
                canonical: 1,
                alias: 2,
                why: "x",
                actor: "anna@lab",
                job_id: None,
                place_id: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("refused"), "{err}");
        assert_eq!(
            one(
                &mut registry,
                "SELECT COUNT(*) FROM study WHERE subject_id = 2"
            ),
            1,
            "the study stayed on the alias"
        );
        assert_eq!(
            one(
                &mut registry,
                "SELECT COUNT(*) FROM subject WHERE merged_into IS NOT NULL"
            ),
            0
        );
        assert_eq!(one(&mut registry, "SELECT COUNT(*) FROM audit"), 0);
        assert_eq!(
            one(
                &mut linkage,
                "SELECT COUNT(*) FROM identity WHERE subject_id = 2"
            ),
            1,
            "the identity stayed on the alias"
        );
    }
}
