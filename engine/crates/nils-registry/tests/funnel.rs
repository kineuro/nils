// SPDX-License-Identifier: AGPL-3.0-only

//! Record 58 P4 (Wave 7b spec section 15, gate E4) on both backends: the
//! yield funnel's counts over a small seeded registry. Each row counts the
//! distinct subjects and sessions of its stacks; a session is the cached
//! session where the cache holds the study, else the study; each row's
//! breakdowns by year, base and 2D or 3D add up to the row; the stacks of
//! one row are the row before less what the filter removed; and the
//! error-budget line multiplies the certificate's bound of each axis a
//! filter read (its stack bound where it measured none) by the stacks that
//! entered the filter, rounded up. A graded result may carry its per-axis
//! counts, whose bound the engine works out. Postgres runs when
//! `NILS_TEST_POSTGRES_DSN` is set.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::funnel::{self, Bounds, Read};
use nils_registry::home::{Home, InitOptions};
use nils_registry::schema::{Type, table};
use nils_registry::{Backend, Insert, Param, Registry, Scheme, Store, labels};
use serde_json::{Value, json};

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_funnel_test";

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
    let dir = TempDir::new("funnel-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-funnel-test-key").unwrap();
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

fn filler(ty: Type) -> Param {
    match ty {
        Type::Int | Type::Id => Param::Int(1),
        Type::Double => Param::Double(1.0),
        Type::Bool => Param::Bool(false),
        Type::Date => Param::from("2026-01-01"),
        Type::Time => Param::from("00:00:00"),
        Type::Timestamp => Param::from("2026-01-01T00:00:00Z"),
        Type::Json => Param::from("{}"),
        Type::Text => Param::from("x"),
        Type::Bytes => Param::Bytes(vec![0]),
    }
}

/// One row with the columns given and every other required one filled.
fn row(store: &mut Store, name: &str, given: &[(&str, Param)]) -> i64 {
    let t = table(name);
    let mut cols: Vec<&str> = Vec::new();
    let mut vals: Vec<Param> = Vec::new();
    for c in t.columns.iter().filter(|c| c.ty != Type::Id) {
        if let Some((_, v)) = given.iter().find(|(n, _)| *n == c.name) {
            cols.push(c.name);
            vals.push(v.clone());
        } else if c.not_null {
            cols.push(c.name);
            vals.push(filler(c.ty));
        }
    }
    store
        .insert(&Insert::new(t, &cols).returning(&["id"]), &[vals])
        .unwrap()[0]
        .int(0)
        .unwrap()
}

/// One stack as seeded: its subject, its study, the study's date (none,
/// or filled by the vote), its base and its acquisition type.
struct Seed {
    subject: usize,
    study: usize,
    date: Option<&'static str>,
    filled: Option<&'static str>,
    base: Option<&'static str>,
    dim: Option<&'static str>,
}

const fn s(
    subject: usize,
    study: usize,
    date: Option<&'static str>,
    base: Option<&'static str>,
    dim: Option<&'static str>,
) -> Seed {
    Seed {
        subject,
        study,
        date,
        filled: None,
        base,
        dim,
    }
}

/// Three subjects. Subject 0 has two studies a day apart that the cache
/// holds as one session; subject 1 two studies years apart, uncached;
/// subject 2 one study whose date the vote filled. Ten stacks.
fn seed(store: &mut Store) -> Vec<i64> {
    let seeds = [
        s(0, 0, Some("2019-03-01"), Some("T1w"), Some("3D")),
        s(0, 0, Some("2019-03-01"), Some("T2w"), Some("2D")),
        s(0, 1, Some("2019-03-02"), Some("FLAIR"), Some("2D")),
        s(0, 1, Some("2019-03-02"), Some("T1w"), Some("2D")),
        s(1, 2, Some("2015-06-10"), Some("T1w"), Some("3D")),
        s(1, 2, Some("2015-06-10"), None, Some("3D")),
        s(1, 3, Some("2021-01-05"), Some("T1w"), None),
        s(1, 3, Some("2021-01-05"), Some("T2w"), Some("2D")),
        Seed {
            filled: Some("2021-11-30"),
            ..s(2, 4, None, Some("T1w"), Some("3D"))
        },
        s(2, 4, None, Some("T2w"), Some("2D")),
    ];
    let subjects: Vec<i64> = (0..3)
        .map(|n| row(store, "subject", &[("code", Param::from(format!("sub{n}")))]))
        .collect();
    let mut studies: BTreeMap<usize, (i64, i64)> = BTreeMap::new();
    let mut out = Vec::new();
    for (i, sd) in seeds.iter().enumerate() {
        let subject = subjects[sd.subject];
        let (study, series) = *studies.entry(sd.study).or_insert_with(|| {
            let mut given = vec![
                ("subject_id", Param::Int(subject)),
                ("study_instance_uid", Param::from(format!("1.2.9.{}", sd.study))),
            ];
            if let Some(d) = sd.date {
                given.push(("study_date", Param::from(d)));
            }
            if let Some(d) = sd.filled.or(seeds
                .iter()
                .find(|x| x.study == sd.study && x.filled.is_some())
                .and_then(|x| x.filled))
            {
                given.push(("date_filled", Param::from(d)));
            }
            let study = row(store, "study", &given);
            let series = row(
                store,
                "series",
                &[
                    ("subject_id", Param::Int(subject)),
                    ("study_id", Param::Int(study)),
                    ("series_instance_uid", Param::from(format!("1.2.9.{}.1", sd.study))),
                ],
            );
            (study, series)
        });
        let stack = row(
            store,
            "stack",
            &[
                ("series_id", Param::Int(series)),
                ("stack_index", Param::Int(i as i64)),
                ("stack_key", Param::from(format!("k{i}"))),
            ],
        );
        if let Some(b) = sd.base {
            row(
                store,
                "classification_axis",
                &[
                    ("stack_id", Param::Int(stack)),
                    ("axis", Param::from("base")),
                    ("value", Param::from(b)),
                ],
            );
        }
        if let Some(d) = sd.dim {
            row(
                store,
                "stack_fingerprint",
                &[
                    ("stack_id", Param::Int(stack)),
                    ("series_id", Param::Int(series)),
                    ("study_id", Param::Int(study)),
                    ("subject_id", Param::Int(subject)),
                    ("mr_acquisition_type", Param::from(d)),
                ],
            );
        }
        out.push(stack);
    }
    // subject 0's two studies are one cached session under the default
    // scheme's window
    let window = nils_registry::session::Scheme::default().window_days;
    let cached = row(
        store,
        "session_cache",
        &[
            ("subject_id", Param::Int(subjects[0])),
            ("window_days", Param::Int(window)),
            ("first", Param::from("2019-03-01")),
            ("last", Param::from("2019-03-02")),
        ],
    );
    for st in [0, 1] {
        row(
            store,
            "session_cache_study",
            &[
                ("session_id", Param::Int(cached)),
                ("study_id", Param::Int(studies[&st].0)),
                ("window_days", Param::Int(window)),
            ],
        );
    }
    out
}

fn n(v: &Value, k: &str) -> i64 {
    v[k].as_i64().unwrap_or_else(|| panic!("{k} in {v}"))
}

/// Each breakdown adds up to the row.
fn adds_up(name: &str, row: &Value) {
    for by in ["by_year", "by_base", "by_dimension"] {
        let groups = row[by].as_array().unwrap();
        let stacks: i64 = groups.iter().map(|g| n(g, "stacks")).sum();
        assert_eq!(stacks, n(row, "stacks"), "{name} {by}: {row}");
        // a subject or session may sit in several groups, never in none
        for c in ["subjects", "sessions"] {
            let most = groups.iter().map(|g| n(g, c)).max().unwrap_or(0);
            let sum: i64 = groups.iter().map(|g| n(g, c)).sum();
            assert!(most <= n(row, c) && n(row, c) <= sum, "{name} {by} {c}: {row}");
        }
    }
}

fn group<'a>(row: &'a Value, by: &str, value: &str) -> &'a Value {
    row[by]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["value"] == value)
        .unwrap_or_else(|| panic!("no {value} in {by}: {row}"))
}

#[test]
fn rows_count_subjects_sessions_and_stacks_and_add_up() {
    for mut l in labs() {
        let name = l.name;
        let store = l.registry.store();
        let ids = seed(store);
        let of = funnel::stacks_of(store, &ids).unwrap();
        assert_eq!(of.len(), 10, "{name}");
        // row 0: everything; row 1: a base filter keeps T1w; row 2: a
        // second filter keeps 3D of those
        let all: BTreeSet<i64> = ids.iter().copied().collect();
        let t1: BTreeSet<i64> = all
            .iter()
            .copied()
            .filter(|s| of[s].base.as_deref() == Some("T1w"))
            .collect();
        let t1_3d: BTreeSet<i64> = t1
            .iter()
            .copied()
            .filter(|s| of[s].dimension.as_deref() == Some("3D"))
            .collect();
        let rows: Vec<Value> = [&all, &t1, &t1_3d]
            .iter()
            .map(|set| funnel::row_counts(&of, set))
            .collect();
        // subject 0's four stacks are one session, subject 1's two studies
        // two, subject 2's one: four sessions
        assert_eq!(
            (n(&rows[0], "subjects"), n(&rows[0], "sessions"), n(&rows[0], "stacks")),
            (3, 4, 10),
            "{name}: {}",
            rows[0]
        );
        assert_eq!(
            (n(&rows[1], "subjects"), n(&rows[1], "sessions"), n(&rows[1], "stacks")),
            (3, 4, 5),
            "{name}: {}",
            rows[1]
        );
        assert_eq!(
            (n(&rows[2], "subjects"), n(&rows[2], "sessions"), n(&rows[2], "stacks")),
            (3, 3, 3),
            "{name}: {}",
            rows[2]
        );
        for r in &rows {
            adds_up(name, r);
        }
        // a row is the row before less what its filter removed
        assert_eq!(n(&rows[0], "stacks") - (all.len() - t1.len()) as i64, n(&rows[1], "stacks"));
        assert_eq!(n(&rows[1], "stacks") - (t1.len() - t1_3d.len()) as i64, n(&rows[2], "stacks"));
        // the year is the study's date, else the filled day; the base and the
        // acquisition type the registry's, else unknown
        assert_eq!(n(group(&rows[0], "by_year", "2019"), "stacks"), 4, "{name}");
        assert_eq!(n(group(&rows[0], "by_year", "2019"), "sessions"), 1, "{name}");
        assert_eq!(n(group(&rows[0], "by_year", "2021"), "stacks"), 4, "{name}");
        assert_eq!(n(group(&rows[0], "by_year", "2021"), "subjects"), 2, "{name}");
        assert_eq!(n(group(&rows[0], "by_year", "2015"), "stacks"), 2, "{name}");
        assert_eq!(n(group(&rows[0], "by_base", "unknown"), "stacks"), 1, "{name}");
        assert_eq!(n(group(&rows[0], "by_base", "T1w"), "stacks"), 5, "{name}");
        assert_eq!(n(group(&rows[0], "by_dimension", "unknown"), "stacks"), 1, "{name}");
        assert_eq!(n(group(&rows[0], "by_dimension", "2D"), "stacks"), 5, "{name}");
        assert_eq!(n(group(&rows[0], "by_dimension", "3D"), "stacks"), 4, "{name}");
        // counts only: nothing of a subject's code
        let text = serde_json::to_string(&rows).unwrap();
        assert!(!text.contains("sub0") && !text.contains("1.2.9"), "{name}: {text}");
        // a stack the registry does not hold is not counted
        let mut with_ghost = all.clone();
        with_ghost.insert(999_999);
        assert_eq!(n(&funnel::row_counts(&of, &with_ghost), "stacks"), 10, "{name}");
    }
}

#[test]
fn the_last_line_multiplies_the_certified_bound_by_the_count_filtered() {
    for mut l in labs() {
        let name = l.name;
        let store = l.registry.store();
        // no certificate: no number, and the line says why
        let none = Bounds::read(store, None).unwrap();
        let reads = [Read {
            row: 1,
            axes: vec!["base".into()],
            entered: 10,
        }];
        let line = funnel::budget(&none, &reads);
        assert!(line["at_most"].is_null(), "{name}: {line}");
        assert!(line["words"].as_str().unwrap().contains("no certificate"), "{name}: {line}");
        // a certificate whose graded result measured base on its own
        let result = labels::graded(&json!({
            "bar": 0.02, "n": 5000, "errors": 6,
            "axes": {"base": {"n": 5000, "errors": 2}}
        }))
        .unwrap();
        let stack_bound = result["bound"].as_f64().unwrap();
        let base_bound = result["axes"]["base"]["bound"].as_f64().unwrap();
        assert!(base_bound < stack_bound, "{result}");
        let id = row(
            store,
            "certificate",
            &[
                ("sample", Param::from("evidence:run-1")),
                ("model_ids", Param::from("[]")),
                ("result", Param::from(result.to_string())),
                ("created_by", Param::from("cleo@lab")),
                ("created_at", Param::from("2026-10-30T00:00:00Z")),
            ],
        );
        let b = Bounds::read(store, None).unwrap();
        assert_eq!(b.certificate, Some(id), "{name}");
        assert_eq!(b.axes.get("base"), Some(&base_bound), "{name}");
        // a filter on base read 5000 stacks, one on post_contrast 800: base
        // takes its own bound, post_contrast the stack bound
        let reads = [
            Read {
                row: 1,
                axes: vec!["base".into()],
                entered: 5000,
            },
            Read {
                row: 2,
                axes: vec!["post_contrast".into()],
                entered: 800,
            },
        ];
        let line = funnel::budget(&b, &reads);
        let want = base_bound * 5000.0 + stack_bound * 800.0;
        assert!((line["sum"].as_f64().unwrap() - want).abs() < 1e-9, "{name}: {line}");
        assert_eq!(line["at_most"], want.ceil() as u64, "{name}: {line}");
        assert_eq!(line["terms"][0]["bound_from"], "axis", "{name}: {line}");
        assert_eq!(line["terms"][1]["bound_from"], "stack", "{name}: {line}");
        assert_eq!(
            line["words"],
            format!("at most {} stacks misplaced by label error", want.ceil() as u64),
            "{name}"
        );
        // a certificate asked for by id, and one that is not there
        assert_eq!(Bounds::read(store, Some(id)).unwrap().certificate, Some(id));
        assert!(Bounds::read(store, Some(id + 7)).is_err(), "{name}");
    }
}

#[test]
fn a_graded_result_checks_its_axes() {
    let bad = labels::graded(&json!({"bar": 0.02, "n": 5000, "errors": 6, "axes": {"base": {"n": 10, "errors": 11}}}));
    assert!(bad.is_err());
    let stated = labels::graded(&json!({"bar": 0.02, "n": 5000, "errors": 6, "axes": {"base": {"n": 10, "errors": 0, "bound": 0.5}}}));
    assert!(stated.unwrap_err().to_string().contains("worked out from the counts"));
    let none = labels::graded(&json!({"bar": 0.02, "n": 5000, "errors": 6})).unwrap();
    assert!(none.get("axes").is_none(), "{none}");
}
