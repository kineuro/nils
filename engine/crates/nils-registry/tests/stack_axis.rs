// SPDX-License-Identifier: AGPL-3.0-only

//! The axes of a stack as one row (`nils_registry::stack_axis`) on both
//! backends: a refresh makes a stack's row what its axis rows say, an axis
//! of several values, one decided to nothing and a stack the pack ruled
//! out among them; a stack left with no axis row is left with no row;
//! `verify` finds a row that is missing, one too many and one that says
//! something else, and `rebuild` mends all three; a stack that is removed
//! takes its row with it. Postgres runs when `NILS_TEST_POSTGRES_DSN` is
//! set.

use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::home::{Home, InitOptions};
use nils_registry::schema::table;
use nils_registry::{Backend, Insert, Param, Registry, Scheme, Store, stack_axis};
use serde_json::{Value, json};

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_stack_axis_test";

fn postgres_dsn() -> Option<String> {
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => Some(dsn),
        _ => {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
            None
        }
    }
}

fn drop_schema(dsn: &str) {
    let mut store = Store::connect_postgres(dsn, SCHEMA).expect("connect");
    store
        .batch(&format!(
            "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
        ))
        .expect("drop");
}

struct Lab {
    name: &'static str,
    registry: Registry,
    _dir: TempDir,
    _guard: Option<MutexGuard<'static, ()>>,
}

impl Drop for Lab {
    fn drop(&mut self) {
        if let Some(dsn) = postgres_dsn().filter(|_| self._guard.is_some()) {
            drop_schema(&dsn);
        }
    }
}

fn lab(name: &'static str, backend: Backend, dsn: Option<String>) -> Lab {
    let dir = TempDir::new("stack-axis-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-stack-axis-key").unwrap();
    let registry = home
        .init(&InitOptions {
            backend,
            dsn,
            schema: (backend == Backend::Postgres).then(|| SCHEMA.to_string()),
            scheme: Scheme::Blake2b32,
            key: "k".to_string(),
            display_length: 12,
            session_scheme: None,
        })
        .unwrap();
    Lab {
        name,
        registry,
        _dir: dir,
        _guard: None,
    }
}

fn labs() -> Vec<Lab> {
    let mut out = vec![lab("sqlite", Backend::Sqlite, None)];
    if let Some(dsn) = postgres_dsn() {
        let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
        drop_schema(&dsn);
        let mut l = lab("postgres", Backend::Postgres, Some(dsn));
        l._guard = Some(guard);
        out.push(l);
    }
    out
}

fn put(store: &mut Store, rows: &[(i64, &str, Option<&str>)]) {
    let rows: Vec<Vec<Param>> = rows
        .iter()
        .map(|(stack, axis, value)| {
            vec![
                Param::Int(*stack),
                Param::from(*axis),
                value.map_or(Param::Null, Param::from),
                Param::Double(0.9),
                Param::from("stated"),
            ]
        })
        .collect();
    store
        .insert(
            &Insert::new(
                table("classification_axis"),
                &["stack_id", "axis", "value", "confidence", "tier"],
            ),
            &rows,
        )
        .unwrap();
}

/// A stack's row: its document and whether it is ruled out; none where it
/// has no row.
fn row(store: &mut Store, stack: i64) -> Option<(Value, i64)> {
    let axes = store
        .dialect()
        .text_of(table("stack_axis").column("axes").unwrap());
    let sql = format!(
        "SELECT {axes}, excluded FROM {} WHERE stack_id = {stack}",
        store.qualified("stack_axis")
    );
    store.query_opt(&sql, &[]).unwrap().map(|r| {
        (
            serde_json::from_str(r.text(0).unwrap()).unwrap(),
            r.int(1).unwrap(),
        )
    })
}

#[test]
fn a_stack_s_row_says_what_its_axis_rows_say() {
    for mut l in labs() {
        let name = l.name;
        let store = l.registry.store();
        put(
            store,
            &[
                (1, "base", Some("T2w")),
                (1, "modifier", Some("FatSat")),
                (1, "modifier", Some("FLAIR")),
                (1, "disposition", Some("acquisition")),
                (2, "base", Some("it's \"quoted\", with a comma")),
                (2, "construct", None),
                (2, "disposition", Some("excluded")),
                (3, "disposition", Some("excluded")),
                (3, "modifier", Some("")),
            ],
        );
        // nothing refreshed yet: every stack is missing
        let before = stack_axis::verify(store).unwrap();
        assert_eq!((before.stacks, before.missing), (0, 3), "{name}");
        assert_eq!(before.sample, [1, 2, 3], "{name}");
        stack_axis::refresh(store, &[1, 2, 3, 4]).unwrap();
        assert!(stack_axis::verify(store).unwrap().agrees(), "{name}");

        let (one, excluded) = row(store, 1).unwrap();
        assert_eq!(excluded, 0, "{name}");
        assert_eq!(one["base"], json!(["T2w"]), "{name}");
        assert_eq!(one["disposition"], json!(["acquisition"]), "{name}");
        // an axis of two values holds both, one element a row
        let mut modifiers: Vec<String> = one["modifier"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        modifiers.sort();
        assert_eq!(modifiers, ["FLAIR", "FatSat"], "{name}");
        // a value is kept as it is written, and an axis decided to nothing
        // is an array of one null
        let (two, excluded) = row(store, 2).unwrap();
        assert_eq!(excluded, 1, "{name}");
        assert_eq!(
            two["base"],
            json!(["it's \"quoted\", with a comma"]),
            "{name}"
        );
        assert_eq!(two["construct"], json!([null]), "{name}");
        // an empty text is a value
        assert_eq!(row(store, 3).unwrap().0["modifier"], json!([""]), "{name}");
        // a stack with no axis row has no row
        assert!(row(store, 4).is_none(), "{name}");

        // an axis row changes without a refresh: the row says something else
        let axis = store.qualified("classification_axis");
        store
            .execute(
                &format!("UPDATE {axis} SET value = 'acquisition' WHERE stack_id = 3 AND axis = 'disposition'"),
                &[],
            )
            .unwrap();
        // one is deleted whole: its row is one too many
        store
            .execute(&format!("DELETE FROM {axis} WHERE stack_id = 2"), &[])
            .unwrap();
        // and one arrives: its row is missing
        put(store, &[(9, "base", Some("T1w"))]);
        let stale = stack_axis::verify(store).unwrap();
        assert!(!stale.agrees(), "{name}");
        assert_eq!(
            (stale.missing, stale.extra, stale.different),
            (1, 1, 1),
            "{name}: {stale:?}"
        );
        assert_eq!(stale.sample, [2, 3, 9], "{name}");

        // a refresh of the stacks written mends them, the one left with no
        // axis row by leaving it none
        stack_axis::refresh(store, &[2, 3, 9]).unwrap();
        assert!(stack_axis::verify(store).unwrap().agrees(), "{name}");
        assert!(row(store, 2).is_none(), "{name}");
        assert_eq!(row(store, 3).unwrap().1, 0, "{name}");
        assert_eq!(row(store, 9).unwrap().0, json!({"base": ["T1w"]}), "{name}");

        // a rebuild makes every row again from nothing
        store
            .execute(
                &format!(
                    "DELETE FROM {} WHERE stack_id IN (1, 9)",
                    store.qualified("stack_axis")
                ),
                &[],
            )
            .unwrap();
        assert_eq!(stack_axis::verify(store).unwrap().missing, 2, "{name}");
        assert_eq!(stack_axis::rebuild(store).unwrap(), 3, "{name}");
        let after = stack_axis::verify(store).unwrap();
        assert!(after.agrees(), "{name}: {after:?}");
        assert_eq!(after.stacks, 3, "{name}");
    }
}

#[test]
fn a_stack_that_is_removed_takes_its_row_with_it() {
    for mut l in labs() {
        let name = l.name;
        let store = l.registry.store();
        put(
            store,
            &[
                (5, "base", Some("T1w")),
                (6, "base", Some("T2w")),
                (6, "disposition", Some("excluded")),
            ],
        );
        stack_axis::refresh(store, &[5, 6]).unwrap();
        nils_registry::empty::remove_stacks(store, &[6], &[]).unwrap();
        assert!(row(store, 6).is_none(), "{name}");
        assert!(row(store, 5).is_some(), "{name}");
        assert!(stack_axis::verify(store).unwrap().agrees(), "{name}");
    }
}
