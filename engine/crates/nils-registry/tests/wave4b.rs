// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 4b slice 1 (`docs/specs/wave4b-the-ask.md`, §15): the schema and
//! store changes every later slice stands on, on both backends.

use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_registry::audit::Action;
use nils_registry::clinical::{self, Vocabulary};
use nils_registry::migrate::{self, Kind};
use nils_registry::schema::{Type, table};
use nils_registry::session::Scheme;
use nils_registry::{Insert, Param, Store};

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_wave4b_test";

fn postgres_dsn() -> Option<String> {
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => Some(dsn),
        _ => {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
            None
        }
    }
}

fn postgres_store() -> Option<(MutexGuard<'static, ()>, Store)> {
    let dsn = postgres_dsn()?;
    let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = Store::connect_postgres(&dsn, SCHEMA).expect("connect");
    store
        .batch(&format!(
            "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; CREATE SCHEMA {SCHEMA}"
        ))
        .expect("fresh schema");
    Some((guard, store))
}

/// Every backend, migrated to the current version.
fn stores() -> Vec<(String, Option<MutexGuard<'static, ()>>, Store)> {
    let mut out = vec![(
        "sqlite".to_string(),
        None,
        Store::sqlite_in_memory().expect("sqlite"),
    )];
    if let Some((guard, store)) = postgres_store() {
        out.push(("postgres".to_string(), Some(guard), store));
    }
    for (_, _, store) in &mut out {
        migrate::migrate(store, Kind::Registry).expect("migrate");
    }
    out
}

fn stamp(day: u32) -> String {
    format!("2026-01-{day:02}T00:00:00Z")
}

#[test]
fn a_membership_is_an_interval_that_can_close_and_open_again() {
    for (name, _guard, mut store) in stores() {
        store
            .insert(
                &Insert::new(table("cohort"), &["name", "owner", "created_at"]),
                &[vec![
                    Param::from("study"),
                    Param::from("someone"),
                    Param::from(stamp(1)),
                ]],
            )
            .unwrap();
        let member = Insert::new(
            table("cohort_member"),
            &["cohort_id", "subject_id", "joined_at", "source"],
        );
        let join = |day: u32| {
            vec![
                Param::Int(1),
                Param::Int(7),
                Param::from(stamp(day)),
                Param::from("manual"),
            ]
        };
        store.insert(&member, &[join(1)]).unwrap();
        // the same pair on the same day is one interval, not two
        assert!(
            store.insert(&member, &[join(1)]).is_err(),
            "{name}: the log keyed on (cohort, subject, joined_at) took a duplicate"
        );
        let d = store.dialect();
        store
            .execute(
                &format!(
                    "UPDATE {} SET left_at = {}, left_by = {} WHERE subject_id = 7 AND left_at IS NULL",
                    store.qualified("cohort_member"),
                    d.param(1, Type::Timestamp),
                    d.param(2, Type::Text)
                ),
                &[Param::from(stamp(5)), Param::from("someone")],
            )
            .unwrap();
        // the pair joins again after leaving: the old constraint refused this
        store.insert(&member, &[join(9)]).unwrap();
        let open = store
            .query(
                &format!(
                    "SELECT COUNT(*) FROM {} WHERE subject_id = 7 AND left_at IS NULL",
                    store.qualified("cohort_member")
                ),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        let all = store
            .query(
                &format!(
                    "SELECT COUNT(*) FROM {} WHERE subject_id = 7",
                    store.qualified("cohort_member")
                ),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!((open, all), (1, 2), "{name}");
    }
}

#[test]
fn a_date_carries_its_precision_and_the_vocabulary_marks_the_year() {
    let yaml = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../packs/clinical/vocabulary.yml"
    ))
    .unwrap();
    let vocabulary = Vocabulary::parse(&yaml).unwrap();
    for (name, _guard, mut store) in stores() {
        clinical::load(&mut store, &vocabulary).unwrap();
        let kinds = clinical::observation_types(&mut store).unwrap();
        let transition = kinds.iter().find(|k| k.name == "SP Transition").unwrap();
        let edss = kinds.iter().find(|k| k.name == "EDSS").unwrap();
        assert_eq!(
            (transition.precision.as_str(), edss.precision.as_str()),
            ("year", "day"),
            "{name}"
        );
        // a row written before the kind declared its precision, as a
        // placeholder day
        let event = Insert::new(
            table("event"),
            &[
                "subject_id",
                "observation_type_id",
                "event_date",
                "event_date_precision",
                "created_at",
            ],
        );
        store
            .insert(
                &event,
                &[
                    vec![
                        Param::Int(1),
                        Param::Int(transition.id),
                        Param::from("2012-01-01"),
                        Param::from("day"),
                        Param::from(stamp(1)),
                    ],
                    vec![
                        Param::Int(1),
                        Param::Int(transition.id),
                        Param::from("2013-03-04"),
                        Param::from("day"),
                        Param::from(stamp(1)),
                    ],
                    vec![
                        Param::Int(1),
                        Param::Int(edss.id),
                        Param::from("2012-01-01"),
                        Param::from("day"),
                        Param::from(stamp(1)),
                    ],
                ],
            )
            .unwrap();
        // the next load applies the year rule to the placeholder and to
        // nothing else
        let loaded = clinical::load(&mut store, &vocabulary).unwrap();
        assert_eq!(loaded.events_reprecised, 1, "{name}");
        let d = store.dialect();
        let date = d.text_of(table("event").column("event_date").unwrap());
        let rows = store
            .query(
                &format!(
                    "SELECT {date}, event_date_precision, observation_type_id FROM {} ORDER BY id",
                    store.qualified("event")
                ),
                &[],
            )
            .unwrap();
        let got: Vec<(String, String)> = rows
            .iter()
            .map(|r| {
                (
                    r.text(0).unwrap().to_string(),
                    r.text(1).unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("2012-01-01".to_string(), "year".to_string()),
                ("2013-03-04".to_string(), "day".to_string()),
                ("2012-01-01".to_string(), "day".to_string()),
            ],
            "{name}"
        );
        // and a third load changes nothing
        let again = clinical::load(&mut store, &vocabulary).unwrap();
        assert_eq!(again.events_reprecised, 0, "{name}");
    }
}

#[test]
fn a_query_carries_its_header_and_a_stream_stops_when_told() {
    for (name, _guard, mut store) in stores() {
        let meta = Insert::new(table("registry_meta"), &["key", "value"]);
        let rows: Vec<Vec<Param>> = (1..=5)
            .map(|i| vec![Param::from(format!("k{i}")), Param::from(format!("v{i}"))])
            .collect();
        store.insert(&meta, &rows).unwrap();
        let sql = format!(
            "SELECT key, value FROM {} WHERE key LIKE 'k%' ORDER BY key",
            store.qualified("registry_meta")
        );
        let (header, rows) = store.query_with_header(&sql, &[]).unwrap();
        assert_eq!(
            header,
            vec!["key".to_string(), "value".to_string()],
            "{name}"
        );
        assert_eq!(rows.len(), 5, "{name}");
        let mut seen = Vec::new();
        let handed = store
            .query_stream(&sql, &[], |row| {
                seen.push(row.text(1).unwrap().to_string());
                Ok(seen.len() < 2)
            })
            .unwrap();
        assert_eq!(
            (handed, seen),
            (2, vec!["v1".to_string(), "v2".to_string()]),
            "{name}"
        );
        // the connection is usable after an early stop
        assert_eq!(store.query(&sql, &[]).unwrap().len(), 5, "{name}");
    }
}

#[test]
fn a_read_transaction_refuses_a_write_and_ends_cleanly() {
    for (name, _guard, mut store) in stores() {
        store.begin_read().unwrap();
        let sql = format!("SELECT COUNT(*) FROM {}", store.qualified("registry_meta"));
        store.query(&sql, &[]).unwrap();
        let write = store.insert(
            &Insert::new(table("registry_meta"), &["key", "value"]),
            &[vec![Param::from("x"), Param::from("y")]],
        );
        assert!(write.is_err(), "{name}: the read door wrote");
        // a refused statement aborts a Postgres transaction; ending it is
        // still the caller's job, and the store is whole afterwards
        if store.end_read().is_err() {
            store.rollback().unwrap();
            if name == "sqlite" {
                store.batch("PRAGMA query_only = 0").unwrap();
            }
        }
        store
            .insert(
                &Insert::new(table("registry_meta"), &["key", "value"]),
                &[vec![Param::from("x"), Param::from("y")]],
            )
            .unwrap();
        let cancel = store.cancel_handle();
        // nothing is running: cancelling is harmless on both backends
        let _ = cancel.cancel();
    }
}

#[test]
fn the_cohort_acts_and_the_saved_ask_change_a_judgement() {
    let acts = [
        (Action::CohortCreate, "cohort.create"),
        (Action::CohortMemberAdd, "cohort.member.add"),
        (Action::CohortMemberRemove, "cohort.member.remove"),
        (Action::CohortPromote, "cohort.promote"),
        (Action::SelectionSave, "selection.save"),
    ];
    for (act, name) in acts {
        assert_eq!(act.name(), name);
        assert!(act.changes_judgement(), "{name} must advance the epoch");
    }
}

#[test]
fn a_scheme_digest_follows_its_definition_and_not_its_name() {
    let a = Scheme::default();
    let b = Scheme::default();
    assert_eq!(a.digest(), b.digest());
    assert_eq!(a.digest().len(), 32);
    let wider = Scheme {
        window_days: 30,
        ..Scheme::default()
    };
    assert_ne!(a.digest(), wider.digest());
}

#[test]
fn the_fingerprint_declares_its_folded_companions_and_the_spacing_split() {
    let t = table("stack_fingerprint");
    for c in [
        "text_all_ci",
        "text_series_description_ci",
        "text_contrast_ci",
        "pixel_spacing_row",
        "pixel_spacing_col",
    ] {
        assert!(t.column(c).is_some(), "{c}");
    }
    assert!(t.indexes.contains(&vec!["study_id"]));
    assert!(t.indexes.contains(&vec!["subject_id"]));
    assert!(table("pick").column("scheme_digest").is_some());
    assert!(table("session_scheme").column("digest").is_some());
    assert!(
        table("subject_disease_type")
            .column("assigned_on_precision")
            .is_some()
    );
}
