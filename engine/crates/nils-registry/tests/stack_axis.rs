// SPDX-License-Identifier: AGPL-3.0-only

//! One row a stack (`nils_registry::stack_axis`) on both backends.
//!
//! The axes: a refresh makes a stack's row what its axis rows say, an axis
//! of several values, one decided to nothing and a stack the pack ruled
//! out among them; a stack left with no axis row is left with no row;
//! `verify` finds a row that is missing, one too many and one that says
//! something else, and `rebuild` mends all three; a stack that is removed
//! takes its row with it.
//!
//! The stack's place and its fingerprint columns, which the database keeps:
//! every kind of write to `stack` and `stack_fingerprint` (an insert, an
//! upsert as the fingerprint job writes, a bulk insert by each path, an
//! update of each column by itself, by key and from a list, a fingerprint
//! moved to another stack, a delete, a table emptied) leaves every row
//! saying what its sources say, with no call from the writer; a row that
//! says nothing is no row; and where the triggers are gone `verify` says
//! so, and `rebuild` sets them again.
//!
//! Postgres runs when `NILS_TEST_POSTGRES_DSN` is set.

use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::dialect::Conflict;
use nils_registry::home::{Home, InitOptions};
use nils_registry::schema::table;
use nils_registry::store::BulkPath;
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

/// A stack's row, column by column after the axes: its series, whether it
/// has a fingerprint, the acquisition type as filled and as read, the two
/// spacings and the thickness; none where it has no row.
type Place = (
    Option<i64>,
    i64,
    Option<String>,
    Option<String>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
);

fn place(store: &mut Store, stack: i64) -> Option<Place> {
    let sql = format!(
        "SELECT series_id, fingerprinted, acquisition_type_filled, mr_acquisition_type, \
         pixel_spacing_row, pixel_spacing_col, slice_thickness FROM {} WHERE stack_id = {stack}",
        store.qualified("stack_axis")
    );
    store.query_opt(&sql, &[]).unwrap().map(|r| {
        (
            r.opt_int(0).unwrap(),
            r.int(1).unwrap(),
            r.opt_text(2).unwrap().map(str::to_string),
            r.opt_text(3).unwrap().map(str::to_string),
            r.opt_double(4).unwrap(),
            r.opt_double(5).unwrap(),
            r.opt_double(6).unwrap(),
        )
    })
}

fn run(store: &mut Store, sql: &str) {
    let sql = sql
        .replace("{stack}", &store.qualified("stack"))
        .replace("{print}", &store.qualified("stack_fingerprint"))
        .replace("{axis}", &store.qualified("classification_axis"))
        .replace("{row}", &store.qualified("stack_axis"));
    store.execute(&sql, &[]).unwrap();
}

/// A stack of a series, written as SQL by hand.
fn stack(store: &mut Store, id: i64, series: i64) {
    // the key is one the database gives, which a test names itself
    let named = match store {
        Store::Sqlite(_) => "",
        Store::Postgres { .. } => " OVERRIDING SYSTEM VALUE",
    };
    run(
        store,
        &format!(
            "INSERT INTO {{stack}} (id, series_id, stack_index, stack_key, modality, orientation, \
             n_instances, first_batch_id){named} VALUES ({id}, {series}, 0, 'k{id}', 'MR', 'ax', 1, 1)"
        ),
    );
}

const PRINTED: &[&str] = &[
    "stack_id",
    "series_id",
    "study_id",
    "subject_id",
    "modality",
    "orientation",
    "n_instances",
    "stack_index",
    "stacks_in_series",
    "job_id",
    "epoch",
    "acquisition_type_filled",
    "mr_acquisition_type",
    "pixel_spacing_row",
    "pixel_spacing_col",
    "slice_thickness",
];

type Printed<'a> = (
    Option<&'a str>,
    Option<&'a str>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
);

fn printed(stack: i64, series: i64, of: Printed<'_>) -> Vec<Param> {
    let text = |t: Option<&str>| t.map_or(Param::Null, Param::from);
    let number = |d: Option<f64>| d.map_or(Param::Null, Param::Double);
    vec![
        Param::Int(stack),
        Param::Int(series),
        Param::Int(1),
        Param::Int(1),
        Param::from("MR"),
        Param::from("ax"),
        Param::Int(1),
        Param::Int(0),
        Param::Int(1),
        Param::Int(1),
        Param::Int(1),
        text(of.0),
        text(of.1),
        number(of.2),
        number(of.3),
        number(of.4),
    ]
}

/// A fingerprint written as the fingerprint job writes it: an insert that
/// overwrites every column but the key where the stack has a row.
fn fingerprint(store: &mut Store, rows: &[Vec<Param>]) {
    let overwritten: Vec<&str> = PRINTED[1..].to_vec();
    store
        .insert(
            &Insert::new(table("stack_fingerprint"), PRINTED).on_conflict(Conflict::Update {
                target: &["stack_id"],
                set: &overwritten,
            }),
            rows,
        )
        .unwrap();
}

fn agrees(store: &mut Store, name: &str, after: &str) {
    let standing = stack_axis::verify(store).unwrap();
    assert!(standing.agrees(), "{name}, after {after}: {standing:?}");
}

fn text(t: &str) -> Option<String> {
    Some(t.to_string())
}

#[test]
fn the_database_keeps_a_stack_s_place_and_its_fingerprint_columns() {
    for mut l in labs() {
        let name = l.name;
        let store = l.registry.store();
        assert!(stack_axis::triggers(store).unwrap().set(), "{name}");

        // a stack: its row names its series and says it has no fingerprint
        stack(store, 1, 11);
        assert_eq!(
            place(store, 1),
            Some((Some(11), 0, None, None, None, None, None)),
            "{name}"
        );
        assert_eq!(row(store, 1), Some((json!({}), 0)), "{name}");
        agrees(store, name, "a stack");

        // its fingerprint, as the job writes one
        fingerprint(
            store,
            &[printed(
                1,
                11,
                (Some("3D"), None, Some(0.5), Some(0.6), Some(1.0)),
            )],
        );
        assert_eq!(
            place(store, 1),
            Some((
                Some(11),
                1,
                text("3D"),
                None,
                Some(0.5),
                Some(0.6),
                Some(1.0)
            )),
            "{name}"
        );
        agrees(store, name, "a fingerprint");

        // written again with other values, some of them none
        fingerprint(
            store,
            &[printed(1, 11, (None, Some("2D"), None, Some(0.7), None))],
        );
        assert_eq!(
            place(store, 1),
            Some((Some(11), 1, None, text("2D"), None, Some(0.7), None)),
            "{name}"
        );
        agrees(store, name, "a fingerprint written again");

        // each column by itself, as SQL by hand
        for set in [
            "acquisition_type_filled = '3D'",
            "mr_acquisition_type = NULL",
            "pixel_spacing_row = 0.25",
            "pixel_spacing_col = NULL",
            "slice_thickness = 2.5",
        ] {
            run(
                store,
                &format!("UPDATE {{print}} SET {set} WHERE stack_id = 1"),
            );
            agrees(store, name, set);
        }
        assert_eq!(
            place(store, 1),
            Some((Some(11), 1, text("3D"), None, Some(0.25), None, Some(2.5))),
            "{name}"
        );
        // a column the row does not carry changes nothing of it
        run(
            store,
            "UPDATE {print} SET fingerprint_revision = 0, n_instances = 9 WHERE stack_id = 1",
        );
        assert_eq!(
            place(store, 1),
            Some((Some(11), 1, text("3D"), None, Some(0.25), None, Some(2.5))),
            "{name}"
        );
        agrees(store, name, "another column");

        // by the store's own updates: one row by its key, rows from a list
        store
            .update_by_id(
                table("stack_fingerprint"),
                &[("mr_acquisition_type", Param::from("2D"))],
                "stack_id",
                1,
            )
            .unwrap();
        agrees(store, name, "an update by key");
        store
            .update_from_values(
                table("stack_fingerprint"),
                "slice_thickness = v.val",
                "stack_id",
                &[(1, 4)],
            )
            .unwrap();
        assert_eq!(
            place(store, 1),
            Some((
                Some(11),
                1,
                text("3D"),
                text("2D"),
                Some(0.25),
                None,
                Some(4.0)
            )),
            "{name}"
        );
        agrees(store, name, "an update from a list");

        // the stack is given another series
        store
            .update_from_values(table("stack"), "series_id = v.val", "id", &[(1, 12)])
            .unwrap();
        assert_eq!(place(store, 1).unwrap().0, Some(12), "{name}");
        // and a column of the stack the row does not carry changes nothing
        run(store, "UPDATE {stack} SET n_instances = 7 WHERE id = 1");
        agrees(store, name, "a stack moved to another series");

        // a fingerprint whose stack is not there has a row that names no
        // series, which no set reads; the stack arrives and the row names
        // its series
        fingerprint(
            store,
            &[printed(
                2,
                11,
                (None, Some("3D"), Some(1.0), Some(1.0), Some(1.0)),
            )],
        );
        assert_eq!(
            place(store, 2),
            Some((None, 1, None, text("3D"), Some(1.0), Some(1.0), Some(1.0))),
            "{name}"
        );
        agrees(store, name, "a fingerprint of no stack");
        stack(store, 2, 11);
        assert_eq!(place(store, 2).unwrap().0, Some(11), "{name}");
        agrees(store, name, "its stack");

        // the fingerprint of one stack becomes another's: the one it left
        // has none, the one it reached has it
        stack(store, 3, 13);
        run(store, "UPDATE {print} SET stack_id = 3 WHERE stack_id = 2");
        assert_eq!(
            place(store, 2),
            Some((Some(11), 0, None, None, None, None, None)),
            "{name}"
        );
        assert_eq!(
            place(store, 3),
            Some((
                Some(13),
                1,
                None,
                text("3D"),
                Some(1.0),
                Some(1.0),
                Some(1.0)
            )),
            "{name}"
        );
        agrees(store, name, "a fingerprint moved");

        // the axes are written by a refresh, which keeps the rest
        put(
            store,
            &[
                (3, "base", Some("T1w")),
                (3, "disposition", Some("excluded")),
            ],
        );
        stack_axis::refresh(store, &[3]).unwrap();
        assert_eq!(
            row(store, 3),
            Some((json!({"base": ["T1w"], "disposition": ["excluded"]}), 1)),
            "{name}"
        );
        assert_eq!(
            place(store, 3),
            Some((
                Some(13),
                1,
                None,
                text("3D"),
                Some(1.0),
                Some(1.0),
                Some(1.0)
            )),
            "{name}"
        );
        agrees(store, name, "a refresh");
        // and a fingerprint written after it keeps the axes
        fingerprint(
            store,
            &[printed(
                3,
                13,
                (Some("2D"), Some("3D"), None, None, Some(5.0)),
            )],
        );
        assert_eq!(row(store, 3).unwrap().1, 1, "{name}");
        assert_eq!(
            place(store, 3),
            Some((Some(13), 1, text("2D"), text("3D"), None, None, Some(5.0))),
            "{name}"
        );
        agrees(store, name, "a fingerprint after a refresh");

        // the fingerprint goes: the row stays for the stack and its axes
        run(store, "DELETE FROM {print} WHERE stack_id = 3");
        assert_eq!(
            place(store, 3),
            Some((Some(13), 0, None, None, None, None, None)),
            "{name}"
        );
        agrees(store, name, "a fingerprint deleted");
        // the stack goes: the row stays for its axes, and names no series
        run(store, "DELETE FROM {stack} WHERE id = 3");
        assert_eq!(place(store, 3).unwrap().0, None, "{name}");
        assert_eq!(row(store, 3).unwrap().1, 1, "{name}");
        agrees(store, name, "a stack deleted");
        // its axis rows go, and a refresh leaves it no row
        run(store, "DELETE FROM {axis} WHERE stack_id = 3");
        stack_axis::refresh(store, &[3]).unwrap();
        assert_eq!(place(store, 3), None, "{name}");
        agrees(store, name, "its axis rows deleted");
        // a stack with nothing else goes with its row at once
        run(store, "DELETE FROM {stack} WHERE id = 2");
        assert_eq!(place(store, 2), None, "{name}");
        agrees(store, name, "a bare stack deleted");
        // a stack removed by the sweep takes its fingerprint and its row
        nils_registry::empty::remove_stacks(store, &[1], &[]).unwrap();
        assert_eq!(place(store, 1), None, "{name}");
        agrees(store, name, "a stack swept");
        assert_eq!(stack_axis::verify(store).unwrap().stacks, 0, "{name}");
    }
}

/// Many rows at once, by each of the paths a bulk insert takes: the rows
/// are the sources' as they are after it.
#[test]
fn a_bulk_write_is_kept_row_by_row() {
    for mut l in labs() {
        let name = l.name;
        let store = l.registry.store();
        let paths: &[Option<BulkPath>] = if name == "postgres" {
            &[Some(BulkPath::Copy), Some(BulkPath::Insert)]
        } else {
            &[None]
        };
        for (round, path) in paths.iter().enumerate() {
            if let Some(path) = path {
                store.set_bulk_path(*path);
            }
            let first = 10_000 * (round as i64 + 1);
            let stacks: Vec<Vec<Param>> = (first..first + 1_200)
                .map(|id| {
                    vec![
                        Param::Int(id),
                        Param::Int(id / 3),
                        Param::Int(id % 3),
                        Param::from(format!("k{id}")),
                        Param::from("MR"),
                        Param::from("ax"),
                        Param::Int(1),
                        Param::Int(1),
                    ]
                })
                .collect();
            let columns = [
                "id",
                "series_id",
                "stack_index",
                "stack_key",
                "modality",
                "orientation",
                "n_instances",
                "first_batch_id",
            ];
            let made = match store {
                // a generated key takes no value on Postgres: by hand there
                Store::Postgres { .. } => {
                    for chunk in stacks.chunks(400) {
                        let values: Vec<String> = chunk
                            .iter()
                            .map(|r| match (&r[0], &r[1]) {
                                (Param::Int(id), Param::Int(series)) => {
                                    format!(
                                        "({id}, {series}, {}, 'k{id}', 'MR', 'ax', 1, 1)",
                                        id % 3
                                    )
                                }
                                _ => unreachable!(),
                            })
                            .collect();
                        run(
                            store,
                            &format!(
                                "INSERT INTO {{stack}} ({}) OVERRIDING SYSTEM VALUE VALUES {}",
                                columns.join(", "),
                                values.join(", ")
                            ),
                        );
                    }
                    stacks.len()
                }
                Store::Sqlite(_) => store
                    .insert(&Insert::new(table("stack"), &columns), &stacks)
                    .map(|_| stacks.len())
                    .unwrap(),
            };
            assert_eq!(made, 1_200, "{name}");
            // every third stack has no fingerprint
            let prints: Vec<Vec<Param>> = (first..first + 1_200)
                .filter(|id| id % 3 != 0)
                .map(|id| {
                    printed(
                        id,
                        id / 3,
                        (
                            (id % 2 == 0).then_some("3D"),
                            (id % 5 != 0).then_some("2D"),
                            (id % 7 != 0).then_some(id as f64 / 1000.0),
                            Some(0.5),
                            (id % 4 != 0).then_some(1.5),
                        ),
                    )
                })
                .collect();
            fingerprint(store, &prints);
            agrees(store, name, "a bulk insert");
            // and written again in bulk, every value another
            let again: Vec<Vec<Param>> = (first..first + 1_200)
                .filter(|id| id % 3 != 0)
                .map(|id| {
                    printed(
                        id,
                        id / 3,
                        (Some("2D"), None, None, Some(0.9), Some(id as f64)),
                    )
                })
                .collect();
            fingerprint(store, &again);
            agrees(store, name, "a bulk upsert");
            let with = (first..).find(|id| id % 3 != 0).unwrap();
            let without = (first..).find(|id| id % 3 == 0).unwrap();
            assert_eq!(
                place(store, with),
                Some((
                    Some(with / 3),
                    1,
                    text("2D"),
                    None,
                    None,
                    Some(0.9),
                    Some(with as f64)
                )),
                "{name}"
            );
            assert_eq!(
                place(store, without),
                Some((Some(without / 3), 0, None, None, None, None, None)),
                "{name}"
            );
        }
        let rows = stack_axis::verify(store).unwrap().stacks;
        assert_eq!(rows as usize, 1_200 * paths.len(), "{name}");
        // a table emptied without a row deleted, which Postgres can do
        if let Store::Postgres { .. } = store {
            run(store, "TRUNCATE {print}");
            agrees(store, name, "the fingerprints emptied");
            assert_eq!(place(store, 10_001).unwrap().1, 0, "{name}");
            run(store, "TRUNCATE {stack}");
            agrees(store, name, "the stacks emptied");
            assert_eq!(stack_axis::verify(store).unwrap().stacks, 0, "{name}");
        }
    }
}

/// Where the triggers are gone a write is not seen: `verify` says that
/// they are gone, and then that a row differs, and `rebuild` sets them
/// again and mends the rows.
#[test]
fn a_registry_without_the_triggers_is_found_and_mended() {
    for mut l in labs() {
        let name = l.name;
        let store = l.registry.store();
        stack(store, 1, 11);
        fingerprint(
            store,
            &[printed(
                1,
                11,
                (Some("3D"), None, Some(0.5), Some(0.5), Some(1.0)),
            )],
        );
        agrees(store, name, "a stack and its fingerprint");
        match store {
            Store::Sqlite(_) => store
                .batch("DROP TRIGGER stack_axis_of_fingerprint_changed")
                .unwrap(),
            Store::Postgres { .. } => {
                let print = store.qualified("stack_fingerprint");
                store
                    .batch(&format!(
                        "ALTER TABLE {print} DISABLE TRIGGER stack_axis_of_fingerprint_changed"
                    ))
                    .unwrap();
            }
        }
        assert!(!stack_axis::triggers(store).unwrap().set(), "{name}");
        let found = stack_axis::verify(store).unwrap();
        assert!(!found.agrees() && !found.triggers, "{name}: {found:?}");
        assert_eq!(found.different, 0, "{name}");
        // and which it was: dropped on SQLite, switched off on Postgres
        let lost = vec!["stack_axis_of_fingerprint_changed".to_string()];
        let none: Vec<String> = Vec::new();
        if name == "postgres" {
            assert_eq!(
                (
                    &found.lost.missing,
                    &found.lost.disabled,
                    &found.lost.astray
                ),
                (&none, &lost, &none),
                "{name}"
            );
        } else {
            assert_eq!(
                (
                    &found.lost.missing,
                    &found.lost.disabled,
                    &found.lost.astray
                ),
                (&lost, &none, &none),
                "{name}"
            );
        }
        // a write the row does not hear of
        run(
            store,
            "UPDATE {print} SET slice_thickness = 3.0 WHERE stack_id = 1",
        );
        let found = stack_axis::verify(store).unwrap();
        assert_eq!(
            (found.different, found.sample.clone()),
            (1, vec![1]),
            "{name}"
        );
        assert_eq!(place(store, 1).unwrap().6, Some(1.0), "{name}");
        stack_axis::rebuild(store).unwrap();
        assert!(stack_axis::triggers(store).unwrap().set(), "{name}");
        assert_eq!(place(store, 1).unwrap().6, Some(3.0), "{name}");
        agrees(store, name, "a rebuild");
        assert!(stack_axis::verify(store).unwrap().lost.set(), "{name}");
        // and the trigger set again hears the next write
        run(
            store,
            "UPDATE {print} SET slice_thickness = 3.5 WHERE stack_id = 1",
        );
        assert_eq!(place(store, 1).unwrap().6, Some(3.5), "{name}");
        agrees(store, name, "a write after the rebuild");
    }
}

/// A fold clears what the sort and the fingerprint said of the stack that
/// stays: its row stays, the row of a stack nothing was said of yet. A
/// list of tables deleted it once, with the stack's other rows.
#[test]
fn a_stack_a_fold_clears_keeps_the_row_of_a_bare_stack() {
    for mut l in labs() {
        let name = l.name;
        let store = l.registry.store();
        stack(store, 1, 11);
        stack(store, 2, 12);
        fingerprint(
            store,
            &[
                printed(1, 11, (Some("3D"), None, Some(0.5), Some(0.5), Some(1.0))),
                printed(2, 12, (Some("2D"), None, Some(0.5), Some(0.5), Some(4.0))),
            ],
        );
        put(store, &[(1, "base", Some("T1w")), (2, "base", Some("T2w"))]);
        stack_axis::refresh(store, &[1, 2]).unwrap();
        agrees(store, name, "two stacks");
        nils_registry::fold::clear(store, &[1]).unwrap();
        assert_eq!(
            place(store, 1),
            Some((Some(11), 0, None, None, None, None, None)),
            "{name}"
        );
        assert_eq!(row(store, 1), Some((json!({}), 0)), "{name}");
        assert_eq!(row(store, 2).unwrap().0, json!({"base": ["T2w"]}), "{name}");
        agrees(store, name, "a fold's clearing");
    }
}

/// A refresh names its stacks to Postgres as one list, so the planner
/// expects a row a stack named and ties each to its sources by their keys.
/// Asked for the union of what each source has of them, it expected the
/// sum of three guesses and read the whole of `stack` for every 500
/// stacks, which on a registry of half a million made a sort's refresh
/// four times as long. What the planner expects does not depend on the
/// registry's size, which is why it is held here and not the plan.
#[test]
fn a_refresh_is_planned_for_the_stacks_it_names() {
    for mut l in labs() {
        let name = l.name;
        let store = l.registry.store();
        let ids: Vec<i64> = (1..=40).collect();
        for id in &ids {
            stack(store, *id, 100 + *id);
        }
        fingerprint(
            store,
            &ids.iter()
                .map(|id| printed(*id, 100 + *id, (Some("3D"), None, None, None, Some(1.0))))
                .collect::<Vec<_>>(),
        );
        let axes: Vec<(i64, &str, Option<&str>)> = ids
            .iter()
            .flat_map(|id| {
                [
                    (*id, "base", Some("T1w")),
                    (*id, "modifier", Some("FLAIR")),
                    (*id, "modifier", Some("FatSat")),
                    (*id, "construct", None),
                    (*id, "disposition", Some("acquisition")),
                ]
            })
            .collect();
        put(store, &axes);
        // a stack named twice is one row
        let mut named = ids.clone();
        named.extend([3, 7, 7]);
        stack_axis::refresh(store, &named).unwrap();
        agrees(store, name, "a refresh that names a stack twice");
        if let Store::Postgres { .. } = store {
            store.batch("ANALYZE").unwrap();
            let rows = stack_axis::fresh_rows(store, &ids);
            let plan = store
                .query(&format!("EXPLAIN (FORMAT JSON) {rows}"), &[])
                .unwrap();
            let plan: Value = serde_json::from_str(plan[0].text(0).unwrap()).unwrap();
            let expected = plan[0]["Plan"]["Plan Rows"].as_f64().unwrap();
            assert!(
                expected <= ids.len() as f64,
                "{name}: the planner expects {expected} rows for {} stacks: {plan}",
                ids.len()
            );
        }
    }
}

/// How many repairs the registry's audit holds.
fn repairs(store: &mut Store) -> i64 {
    let sql = format!(
        "SELECT COUNT(*) FROM {} WHERE action = 'registry.repair'",
        store.qualified("audit")
    );
    store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
}

/// An open looks for the triggers. Where they are set it does nothing and
/// says nothing. Where one is dropped, switched off or runs a function
/// that writes another schema's rows, it sets them all again and makes
/// every row again before the registry is used, says what it found, and
/// writes an audit row as the repair does; the open after it finds
/// nothing.
#[test]
fn an_open_finds_lost_triggers_and_mends_the_rows() {
    for mut l in labs() {
        let name = l.name;
        let home = Home::new(l._dir.path());
        let postgres = name == "postgres";
        let astray_schema = format!("{SCHEMA}_astray");
        {
            let store = l.registry.store();
            stack(store, 1, 11);
            fingerprint(
                store,
                &[printed(
                    1,
                    11,
                    (Some("3D"), None, Some(0.5), Some(0.5), Some(1.0)),
                )],
            );
        }
        // the ordinary open: nothing found, nothing done, nothing audited
        let mut reg = home.open().unwrap();
        assert_eq!(reg.mended(), None, "{name}");
        assert_eq!(repairs(reg.store()), 0, "{name}");
        drop(reg);

        let changed = "stack_axis_of_fingerprint_changed";
        let of_fingerprint = [
            "stack_axis_of_fingerprint_written",
            changed,
            "stack_axis_of_fingerprint_removed",
        ];
        // each way a trigger is lost: what breaks it, and what an open
        // then finds missing, switched off and astray
        let mut ways: Vec<(&str, String, Vec<&str>, Vec<&str>, Vec<&str>)> = vec![(
            "dropped",
            if postgres {
                format!("DROP TRIGGER {changed} ON {{print}}")
            } else {
                format!("DROP TRIGGER {changed}")
            },
            vec![changed],
            vec![],
            vec![],
        )];
        if postgres {
            ways.push((
                "switched off",
                format!("ALTER TABLE {{print}} DISABLE TRIGGER {changed}"),
                vec![],
                vec![changed],
                vec![],
            ));
            // the function of a registry copied under another name: its
            // body as it is, naming the tables of the other schema
            ways.push((
                "astray",
                format!(
                    "DO $$ DECLARE body text; BEGIN \
                     SELECT replace(p.prosrc, '{SCHEMA}.', '{astray_schema}.') INTO body \
                     FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
                     WHERE n.nspname = '{SCHEMA}' AND p.proname = 'stack_axis_of_fingerprint'; \
                     EXECUTE format('CREATE OR REPLACE FUNCTION {SCHEMA}.stack_axis_of_fingerprint() \
                     RETURNS trigger LANGUAGE plpgsql AS %L', body); END $$"
                ),
                vec![],
                vec![],
                of_fingerprint.to_vec(),
            ));
            let store = l.registry.store();
            store
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {astray_schema} CASCADE; CREATE SCHEMA {astray_schema}; \
                     CREATE TABLE {astray_schema}.stack_axis (LIKE {SCHEMA}.stack_axis INCLUDING ALL); \
                     CREATE TABLE {astray_schema}.stack (LIKE {SCHEMA}.stack INCLUDING ALL)"
                ))
                .unwrap();
        }
        let mut thickness = 1.0;
        for (round, (what, breaking, missing, disabled, astray)) in ways.iter().enumerate() {
            let store = l.registry.store();
            run(store, breaking);
            // a write the stack's row does not hear of
            thickness += 1.0;
            run(
                store,
                &format!("UPDATE {{print}} SET slice_thickness = {thickness} WHERE stack_id = 1"),
            );
            assert_eq!(
                place(store, 1).unwrap().6,
                Some(thickness - 1.0),
                "{name}: {what}"
            );
            let names = |v: &Vec<&str>| v.iter().map(|n| n.to_string()).collect::<Vec<_>>();
            let found = stack_axis::triggers(store).unwrap();
            assert_eq!(
                (
                    found.missing.clone(),
                    found.disabled.clone(),
                    found.astray.clone()
                ),
                (names(missing), names(disabled), names(astray)),
                "{name}: {what}"
            );
            assert!(!found.in_words().is_empty(), "{name}: {what}");

            let mut reg = home.open().unwrap();
            let mended = reg
                .mended()
                .cloned()
                .unwrap_or_else(|| panic!("{name}: {what}"));
            assert_eq!(mended.found, found, "{name}: {what}");
            assert_eq!(
                (mended.rows, mended.error.clone()),
                (Some(1), None),
                "{name}: {what}"
            );
            let store = reg.store();
            assert_eq!(
                place(store, 1).unwrap().6,
                Some(thickness),
                "{name}: {what}"
            );
            agrees(store, name, what);
            assert_eq!(repairs(store), round as i64 + 1, "{name}: {what}");
            // and the triggers hear the next write
            run(
                store,
                "UPDATE {print} SET pixel_spacing_row = 0.75 WHERE stack_id = 1",
            );
            assert_eq!(place(store, 1).unwrap().4, Some(0.75), "{name}: {what}");
            run(
                store,
                "UPDATE {print} SET pixel_spacing_row = 0.5 WHERE stack_id = 1",
            );
            drop(reg);
            // the open after it finds nothing and writes nothing
            let mut reg = home.open().unwrap();
            assert_eq!(reg.mended(), None, "{name}: {what}");
            assert_eq!(repairs(reg.store()), round as i64 + 1, "{name}: {what}");
        }
        if postgres {
            l.registry
                .store()
                .batch(&format!("DROP SCHEMA IF EXISTS {astray_schema} CASCADE"))
                .unwrap();
        }
    }
}
