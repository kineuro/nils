// SPDX-License-Identifier: AGPL-3.0-only

//! One spelling of a value in a row (`nils_registry::spelling`) on both
//! backends.
//!
//! A decision is written in the stored form whichever name of the value
//! its author gave, and a text that names no value as it was given; a
//! registry is checked for rows and decisions in another spelling; a
//! repair moves them, merges a row whose stack holds the stored form
//! already, leaves what is on a sealed stack and reports it apart, and
//! brings the read model in line.
//!
//! Postgres runs when `NILS_TEST_POSTGRES_DSN` is set.

use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::home::{Home, InitOptions};
use nils_registry::review::{self, Apply, Author};
use nils_registry::schema::table;
use nils_registry::spelling::{self, Spellings};
use nils_registry::{Backend, Insert, Param, Registry, Scheme, Store, stack_axis};
use serde_json::{Value, json};

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_spelling_test";

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
    let dir = TempDir::new("spelling-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-spelling-key").unwrap();
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

/// A made-up pack's names: `contrast` stores the word and reads `1` and `0`
/// as it stored before; `kind` stores a label, `KA` for the identity `k-a`;
/// `extras` is multi-valued and stores `Fat` for `fat` and `Long` for
/// `long`.
fn spellings() -> Spellings {
    let n = |axis: &str, text: &str, stored: &str, id: &str| {
        (
            axis.to_string(),
            text.to_string(),
            stored.to_string(),
            id.to_string(),
        )
    };
    Spellings::new([
        n("contrast", "given", "given", "given"),
        n("contrast", "1", "given", "given"),
        n("contrast", "not_given", "not_given", "not_given"),
        n("contrast", "0", "not_given", "not_given"),
        n("kind", "k-a", "KA", "k-a"),
        n("kind", "KA", "KA", "k-a"),
        n("extras", "fat", "Fat", "fat"),
        n("extras", "Fat", "Fat", "fat"),
        n("extras", "long", "Long", "long"),
        n("extras", "Long", "Long", "long"),
    ])
}

fn put(store: &mut Store, rows: &[(i64, &str, &str)]) {
    let rows: Vec<Vec<Param>> = rows
        .iter()
        .map(|(stack, axis, value)| {
            vec![
                Param::Int(*stack),
                Param::from(*axis),
                Param::from(*value),
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

/// The values a stack holds on an axis, in order.
fn held(store: &mut Store, stack: i64, axis: &str) -> Vec<String> {
    let sql = format!(
        "SELECT value FROM {} WHERE stack_id = {stack} AND axis = '{axis}' ORDER BY value",
        store.qualified("classification_axis")
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect()
}

/// The stack's row of the read model.
fn read_model(store: &mut Store, stack: i64) -> Value {
    let axes = store
        .dialect()
        .text_of(table("stack_axis").column("axes").unwrap());
    let sql = format!(
        "SELECT {axes} FROM {} WHERE stack_id = {stack}",
        store.qualified("stack_axis")
    );
    let row = store.query_opt(&sql, &[]).unwrap().unwrap();
    serde_json::from_str(row.text(0).unwrap()).unwrap()
}

#[allow(clippy::too_many_arguments)]
fn decide(
    store: &mut Store,
    stack: i64,
    axis: &str,
    value: &str,
    in_view: Option<&str>,
    staged: bool,
    withdrawn: bool,
) -> i64 {
    let at = "2026-01-01T00:00:00Z";
    store
        .insert(
            &Insert::new(
                table("decision"),
                &[
                    "scope",
                    "ref",
                    "axis",
                    "value",
                    "actor",
                    "author_kind",
                    "decided_at",
                    "staged_at",
                    "committed_at",
                    "withdrawn_at",
                    "rules_in_view",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from("stack"),
                Param::from(stack.to_string()),
                Param::from(axis),
                Param::from(value),
                Param::from("a-reader"),
                Param::from("person"),
                Param::from(at),
                if staged { Param::from(at) } else { Param::Null },
                if staged { Param::Null } else { Param::from(at) },
                if withdrawn {
                    Param::from(at)
                } else {
                    Param::Null
                },
                in_view.map_or(Param::Null, Param::from),
            ]],
        )
        .unwrap()[0]
        .int(0)
        .unwrap()
}

fn decision(store: &mut Store, id: i64) -> (String, Option<String>) {
    let sql = format!(
        "SELECT value, rules_in_view FROM {} WHERE id = {id}",
        store.qualified("decision")
    );
    let r = store.query_opt(&sql, &[]).unwrap().unwrap();
    (
        r.text(0).unwrap().to_string(),
        r.opt_text(1).unwrap().map(str::to_string),
    )
}

fn seal(store: &mut Store, stack: i64) {
    store
        .insert(
            &Insert::new(
                table("sealed_stack"),
                &["sample", "stack_id", "subject_id", "sealed_by", "sealed_at"],
            ),
            &[vec![
                Param::from("a-sample"),
                Param::Int(stack),
                Param::Int(1),
                Param::from("an-operator"),
                Param::from("2026-01-01T00:00:00Z"),
            ]],
        )
        .unwrap();
}

#[test]
fn a_text_reads_as_its_stored_form_and_its_identity() {
    let s = spellings();
    assert_eq!(s.stored("contrast", "1"), "given");
    assert_eq!(s.stored("contrast", "given"), "given");
    assert_eq!(s.stored("kind", "k-a"), "KA");
    // each comma-joined value, a value named twice once, an unknown kept
    assert_eq!(s.stored("extras", "long, fat,Fat"), "Long,Fat");
    assert_eq!(s.stored("extras", "other,fat"), "other,Fat");
    // an axis the pack has not, and a text that is empty, as they are
    assert_eq!(s.stored("elsewhere", "1"), "1");
    assert_eq!(s.stored("contrast", ""), "");
    assert_eq!(s.id("contrast", "1"), "given");
    assert_eq!(s.id("kind", "KA"), "k-a");
    assert_eq!(s.id("kind", "other"), "other");
    assert!(s.same("contrast", "1", "given"));
    assert!(s.same("extras", "fat,long", "Long, Fat"));
    assert!(!s.same("contrast", "1", "0"));
    assert!(!s.same("contrast", "1", ""));
}

#[test]
fn a_decision_is_written_in_the_stored_form_whoever_spelled_it() {
    for mut l in labs() {
        let name = l.name;
        let ask = |reg: &mut Registry, stack: i64, axis: &str| -> i64 {
            reg.store()
                .insert(
                    &Insert::new(
                        table("review_item"),
                        &["kind", "scope", "ref", "evidence", "status", "created_at"],
                    )
                    .returning(&["id"]),
                    &[vec![
                        Param::from(format!("{axis}:unsure")),
                        Param::from("stack"),
                        Param::from(json!({"stack_id": stack}).to_string()),
                        Param::from(json!({"axis": axis}).to_string()),
                        Param::from("open"),
                        Param::from("2026-01-01T00:00:00Z"),
                    ]],
                )
                .unwrap()[0]
                .int(0)
                .unwrap()
        };
        let answer = |reg: &mut Registry, item: i64, value: &str| -> i64 {
            review::apply(
                reg,
                &Apply {
                    item,
                    member: None,
                    scope: "stack",
                    value: Some(value),
                    author: Author {
                        who: "a-reader",
                        kind: "person",
                        version: None,
                        model: None,
                    },
                    stage: false,
                    why: None,
                    campaign: None,
                },
            )
            .unwrap()
            .decision
        };
        let reg = &mut l.registry;
        // no pack's names given: a text is written as it was given
        let item = ask(reg, 1, "contrast");
        let plain = answer(reg, item, "1");
        assert_eq!(decision(reg.store(), plain).0, "1", "{name}");

        reg.spell(spellings());
        for (stack, axis, given, stored) in [
            (2, "contrast", "1", "given"),
            (3, "contrast", "given", "given"),
            (4, "kind", "k-a", "KA"),
            (5, "extras", "long,fat", "Long,Fat"),
            // a text that names no value is taken at this door, as given
            (6, "contrast", "perhaps", "perhaps"),
            // an axis the pack has not
            (7, "elsewhere", "1", "1"),
        ] {
            let item = ask(reg, stack, axis);
            let id = answer(reg, item, given);
            assert_eq!(
                decision(reg.store(), id).0,
                stored,
                "{name}: {axis} {given}"
            );
        }
    }
}

#[test]
fn a_repair_moves_every_other_spelling_and_leaves_what_is_sealed() {
    for mut l in labs() {
        let name = l.name;
        let s = spellings();
        let store = l.registry.store();
        put(
            store,
            &[
                // written by an older pack, and by decisions as given
                (1, "contrast", "1"),
                (2, "contrast", "0"),
                (3, "contrast", "given"),
                (4, "kind", "k-a"),
                (4, "kind_of_no_pack", "k-a"),
                // a stack that holds the value in both spellings, and one
                // more value in another
                (5, "extras", "fat"),
                (5, "extras", "Fat"),
                (5, "extras", "long"),
                // of a sample sealed now
                (6, "contrast", "1"),
                // a text that names nothing
                (7, "contrast", "perhaps"),
            ],
        );
        stack_axis::refresh(store, &[1, 2, 3, 4, 5, 6, 7]).unwrap();
        seal(store, 6);
        let moved = decide(store, 1, "contrast", "1", Some("0"), false, false);
        let right = decide(store, 3, "contrast", "given", Some("given"), false, false);
        let several = decide(store, 5, "extras", "long, fat", None, false, false);
        let staged = decide(store, 2, "contrast", "0", None, true, false);
        let withdrawn = decide(store, 4, "kind", "k-a", None, false, true);
        let sealed = decide(store, 6, "contrast", "1", Some("1"), false, false);
        let unknown = decide(store, 7, "contrast", "perhaps", None, false, false);

        let found = spelling::find(store, &s).unwrap();
        assert!(!found.clean(), "{name}");
        let rows: Vec<(&str, &str, &str, i64, i64)> = found
            .rows
            .iter()
            .map(|r| {
                (
                    r.axis.as_str(),
                    r.text.as_str(),
                    r.stored.as_str(),
                    r.open,
                    r.sealed,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("contrast", "0", "not_given", 1, 0),
                ("contrast", "1", "given", 1, 1),
                ("extras", "fat", "Fat", 1, 0),
                ("extras", "long", "Long", 1, 0),
                ("kind", "k-a", "KA", 1, 0),
            ],
            "{name}"
        );
        let by_axis = found.by_axis();
        assert_eq!(by_axis["contrast"], (3, 5), "{name}");
        assert_eq!(by_axis["extras"], (2, 1), "{name}");
        assert_eq!(by_axis["kind"], (1, 1), "{name}");
        assert_eq!(found.staged, 1, "{name}");
        let counted = spelling::count_rows(store, &s).unwrap();
        assert_eq!(counted["contrast"], 3, "{name}");
        // reading wrote nothing
        assert_eq!(held(store, 1, "contrast"), ["1"], "{name}");

        store.begin().unwrap();
        let (before, done) = spelling::repair(store, &s).unwrap();
        store.commit().unwrap();
        assert_eq!(before, found, "{name}");
        assert_eq!((done.rows, done.rows_merged), (4, 1), "{name}");
        assert_eq!((done.decisions, done.in_view), (4, 1), "{name}");
        assert_eq!((done.stacks, done.rebuilt), (4, false), "{name}");

        assert_eq!(held(store, 1, "contrast"), ["given"], "{name}");
        assert_eq!(held(store, 2, "contrast"), ["not_given"], "{name}");
        assert_eq!(held(store, 3, "contrast"), ["given"], "{name}");
        assert_eq!(held(store, 4, "kind"), ["KA"], "{name}");
        // an axis the pack has not is not read
        assert_eq!(held(store, 4, "kind_of_no_pack"), ["k-a"], "{name}");
        assert_eq!(held(store, 5, "extras"), ["Fat", "Long"], "{name}");
        // the sealed stack's row and the text that names nothing stay
        assert_eq!(held(store, 6, "contrast"), ["1"], "{name}");
        assert_eq!(held(store, 7, "contrast"), ["perhaps"], "{name}");

        assert_eq!(
            decision(store, moved),
            ("given".to_string(), Some("not_given".to_string())),
            "{name}"
        );
        assert_eq!(decision(store, right).0, "given", "{name}");
        assert_eq!(decision(store, several).0, "Long,Fat", "{name}");
        assert_eq!(decision(store, staged).0, "not_given", "{name}");
        assert_eq!(decision(store, withdrawn).0, "KA", "{name}");
        assert_eq!(
            decision(store, sealed),
            ("1".to_string(), Some("1".to_string())),
            "{name}"
        );
        assert_eq!(decision(store, unknown).0, "perhaps", "{name}");

        // the read model says what the rows say
        assert!(stack_axis::verify(store).unwrap().agrees(), "{name}");
        assert_eq!(read_model(store, 1)["contrast"], json!(["given"]), "{name}");
        let mut extras: Vec<String> =
            serde_json::from_value(read_model(store, 5)["extras"].clone()).unwrap();
        extras.sort();
        assert_eq!(extras, ["Fat", "Long"], "{name}");

        // what is left is what is sealed, and a second repair writes nothing
        let after = spelling::find(store, &s).unwrap();
        assert_eq!(after.rows.len(), 1, "{name}");
        assert_eq!((after.rows[0].open, after.rows[0].sealed), (0, 1), "{name}");
        assert_eq!(after.decisions.len(), 1, "{name}");
        assert_eq!(after.decisions[0].sealed, 1, "{name}");
        store.begin().unwrap();
        let (_, again) = spelling::repair(store, &s).unwrap();
        store.commit().unwrap();
        assert_eq!(again, spelling::Repaired::default(), "{name}");
    }
}

#[test]
fn a_repair_writes_saved_main_scan_rules_in_the_stored_words() {
    use nils_registry::pick_rules::{self, Scope, ScopeKind};
    let tech = |text: &str, stored: &str| {
        (
            "technique".to_string(),
            text.to_string(),
            stored.to_string(),
            "ir-tse".to_string(),
        )
    };
    let s = Spellings::new([tech("TIRM", "IRTSE"), tech("IRTSE", "IRTSE")]);
    assert_eq!(s.stored_kind_key("2D TIRM +C"), "2D IRTSE +C");
    assert_eq!(s.stored_kind_key("3D MPRAGE"), "3D MPRAGE");
    assert_eq!(s.stored_kind_key("TIRM"), "IRTSE");
    for mut l in labs() {
        let name = l.name;
        let store = l.registry.store();
        let scope = Scope {
            kind: ScopeKind::Cohort,
            id: 7,
            name: "c".to_string(),
        };
        let rules = json!({"roles": {"t1w": {
            "kinds_in_order": ["2D TIRM", "3D MPRAGE", "2D IRTSE"],
            "not_used": ["2D TIRM +C"],
            "contrast": "either",
        }}});
        pick_rules::save(
            store,
            &scope,
            &rules,
            "saved before the words moved",
            "t",
            1,
        )
        .unwrap();

        let found = spelling::find(store, &s).unwrap();
        assert_eq!(found.pick_rules, 1, "{name}");
        assert!(!found.clean(), "{name}");
        let (_, done) = spelling::repair(store, &s).unwrap();
        assert_eq!(done.pick_rules, 1, "{name}");
        let saved = pick_rules::latest(store, &scope).unwrap().unwrap();
        let t1 = &saved.rules["roles"]["t1w"];
        assert_eq!(
            t1["kinds_in_order"],
            json!(["2D IRTSE", "3D MPRAGE"]),
            "{name}"
        );
        assert_eq!(t1["not_used"], json!(["2D IRTSE +C"]), "{name}");
        assert_eq!(t1["contrast"], "either", "{name}");
        assert_eq!(saved.digest, pick_rules::digest(&saved.rules), "{name}");
        // a second run finds nothing
        let found = spelling::find(store, &s).unwrap();
        assert_eq!(found.pick_rules, 0, "{name}");
        let (_, again) = spelling::repair(store, &s).unwrap();
        assert_eq!(again.pick_rules, 0, "{name}");
    }
}
