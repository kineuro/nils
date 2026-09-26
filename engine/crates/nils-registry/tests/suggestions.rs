// SPDX-License-Identifier: AGPL-3.0-only

//! Record 50 on both backends: suggestions from outside the engine carried
//! by a campaign (their author and their confidences, one per author and
//! item, none on a sealed stack), a page of items accepted in one move with
//! each item's own value, and the claims that take the least certain first.

use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use std::collections::BTreeMap;

use nils_registry::campaign::{self, Each, Given, Items, New, Order, Role};
use nils_registry::home::{Home, InitOptions};
use nils_registry::labels;
use nils_registry::schema::{Type, table};
use nils_registry::suggestion::{self, Given as Suggested, Import};
use nils_registry::{Backend, Insert, Param, Registry, Scheme, Store};
use serde_json::json;

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_suggest_test";

fn postgres_dsn() -> Option<String> {
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => Some(dsn),
        _ => {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
            None
        }
    }
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
            let mut store = Store::connect_postgres(&dsn, SCHEMA).expect("connect");
            store
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
                ))
                .expect("drop");
        }
    }
}

fn lab(name: &'static str, backend: Backend, dsn: Option<String>) -> Lab {
    let dir = TempDir::new("suggest-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-suggest-test-key").unwrap();
    let registry = home
        .init(&InitOptions {
            backend,
            dsn,
            schema: (backend == Backend::Postgres).then(|| SCHEMA.to_string()),
            scheme: Scheme::DEFAULT,
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
        let mut store = Store::connect_postgres(&dsn, SCHEMA).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
            ))
            .expect("drop");
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

/// One row of a table with the columns given and every other required
/// column filled.
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

/// One subject with one series of `n` stacks. Answers the stack ids.
fn stacks(reg: &mut Registry, n: usize) -> Vec<i64> {
    let store = reg.store();
    let subject = row(store, "subject", &[("code", Param::from("s0"))]);
    let study = row(
        store,
        "study",
        &[
            ("subject_id", Param::Int(subject)),
            ("study_instance_uid", Param::from("1.2.840.48.0")),
        ],
    );
    let series = row(
        store,
        "series",
        &[
            ("subject_id", Param::Int(subject)),
            ("study_id", Param::Int(study)),
            ("series_instance_uid", Param::from("1.2.840.48")),
        ],
    );
    (0..n)
        .map(|i| {
            row(
                store,
                "stack",
                &[
                    ("series_id", Param::Int(series)),
                    ("stack_index", Param::Int(i as i64)),
                    ("stack_key", Param::from(format!("1.2.840.48#{i}"))),
                ],
            )
        })
        .collect()
}

fn classified(reg: &mut Registry, stack: i64, axis: &str, value: &str, confidence: f64) {
    row(
        reg.store(),
        "classification_axis",
        &[
            ("stack_id", Param::Int(stack)),
            ("axis", Param::from(axis)),
            ("value", Param::from(value)),
            ("confidence", Param::Double(confidence)),
            ("tier", Param::from("keywords")),
        ],
    );
}

fn body_part() -> serde_json::Value {
    json!({"kind": "axis", "axis": "body_part", "values": ["brain", "brain-neck", "spine", "neck", "other"]})
}

fn new<'a>(
    name: &'a str,
    q: &'a serde_json::Value,
    adj: &'a serde_json::Value,
    items: Items,
    raters: i64,
) -> New<'a> {
    New {
        name,
        owner: "cleo@lab",
        question: q,
        source: json!({"test": true}),
        items,
        handle_id: None,
        content_hash: None,
        pack_version: Some("mri@0.3.0"),
        raters_per_item: raters,
        raters: Vec::new(),
        adjudicators: Vec::new(),
        adjudication: adj,
        closes_into: "decision",
        lease_seconds: 600,
        inputs: Default::default(),
        hold_back: None,
    }
}

fn give<'a>(assignment: i64, who: &'a str, value: &'a str) -> Given<'a> {
    Given {
        assignment,
        principal: who,
        author_kind: "person",
        model: None,
        value: Some(value),
        form: None,
        derivative_id: None,
        why: None,
        unsure: false,
    }
}

fn at(minute: u32, second: u32) -> String {
    format!("2026-09-24T10:{minute:02}:{second:02}Z")
}

/// Every name a value goes by, in lower case, to its identity, as the door
/// builds it from the pack.
fn names() -> BTreeMap<String, String> {
    ["brain", "brain-neck", "spine", "neck", "other"]
        .iter()
        .map(|v| (v.to_lowercase(), v.to_string()))
        .collect()
}

fn by_stack(stack: i64, value: Option<&str>, p: &[(&str, f64)], author: Option<&str>) -> Suggested {
    Suggested {
        stack: Some(stack),
        value: value.map(str::to_string),
        confidences: p.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        author: author.map(str::to_string),
        ..Suggested::default()
    }
}

fn import(
    reg: &mut Registry,
    campaign: i64,
    rows: &[Suggested],
    author: Option<&str>,
) -> suggestion::Imported {
    let names = names();
    suggestion::import(
        reg,
        &Import {
            campaign,
            rows,
            author,
            source: Some("test.tsv"),
            who: "cleo@lab",
            names: &names,
            dry_run: false,
        },
        &at(0, 0),
    )
    .unwrap()
}

/// R3: a campaign carries suggestions from outside, each with its author and
/// its confidences; values read in any case, a value from the highest
/// confidence where none is given, one per author and item, and what the
/// question does not take is refused and counted.
#[test]
fn suggestions_come_in_with_their_author_and_confidences() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 4);
        let (q, adj) = (body_part(), json!({"when": "never"}));
        let c = campaign::create(
            reg,
            &new("bp", &q, &adj, Items::Stacks(ids[..3].to_vec()), 1),
        )
        .unwrap();
        let rows = vec![
            by_stack(
                ids[0],
                Some("Brain"),
                &[("brain", 0.9), ("Brain-Neck", 0.1)],
                None,
            ),
            by_stack(ids[1], None, &[("brain", 0.3), ("brain-neck", 0.7)], None),
            by_stack(ids[2], Some("hand"), &[], None),
            by_stack(ids[2], Some("brain"), &[("elbow", 0.5)], None),
            by_stack(ids[2], Some("brain"), &[("brain", 1.5)], None),
            // the fourth stack is not the campaign's
            by_stack(ids[3], Some("brain"), &[], None),
        ];
        let done = import(reg, c.id, &rows, Some("v0-model"));
        assert_eq!(done.suggestions, 2, "{name}: {done:?}");
        assert_eq!(done.refused_values, 3, "{name}");
        assert_eq!(done.unmatched, 1, "{name}");
        let items = campaign::items(reg.store(), c.id).unwrap();
        let told = suggestion::of_items(reg.store(), c.id, &[items[0].id, items[1].id]).unwrap();
        let first = &told[&items[0].id][0];
        assert_eq!(
            (first.value.as_str(), first.author.as_str()),
            ("brain", "v0-model"),
            "{name}"
        );
        assert_eq!(first.confidence, Some(0.9), "{name}");
        assert_eq!(first.confidences["brain-neck"], 0.1, "{name}");
        let second = &told[&items[1].id][0];
        assert_eq!(second.value, "brain-neck", "{name}");
        assert_eq!(second.confidence, Some(0.7), "{name}");

        // a row with no author, where the import names none, is counted
        let none = import(
            reg,
            c.id,
            &[by_stack(ids[0], Some("brain"), &[], None)],
            None,
        );
        assert_eq!((none.suggestions, none.no_author), (0, 1), "{name}");
        // by series: every stack of it; a second author beside the first,
        // and the same author again replaces its own
        let series = Suggested {
            series: Some("1.2.840.48".into()),
            value: Some("neck".into()),
            author: Some("v0-person".into()),
            ..Suggested::default()
        };
        let people = import(reg, c.id, &[series], None);
        assert_eq!(people.suggestions, 3, "{name}: {people:?}");
        let again = import(
            reg,
            c.id,
            &[by_stack(ids[0], Some("spine"), &[], Some("v0-model"))],
            None,
        );
        assert_eq!(again.suggestions, 1, "{name}: {again:?}");
        assert_eq!(again.replaced, 1, "{name}");
        let all = suggestion::of_campaign(reg.store(), c.id).unwrap();
        assert_eq!(all.len(), 5, "{name}");
        let told = suggestion::of_items(reg.store(), c.id, &[items[0].id]).unwrap();
        let list = &told[&items[0].id];
        assert_eq!(list.len(), 2, "{name}");
        assert!(suggestion::disagree(list), "{name}");
        // the latest import is shown first
        let p = suggestion::primary(list).unwrap();
        assert_eq!(p.value, "spine", "{name}");
        let summary = suggestion::summary(reg.store(), c.id).unwrap();
        assert_eq!(summary["count"], 5, "{name}");
        assert_eq!(summary["items"], 3, "{name}");

        // can't tell is never suggested, and a closed campaign takes none
        let ct = import(
            reg,
            c.id,
            &[by_stack(ids[0], Some("cant_tell"), &[], Some("x"))],
            None,
        );
        assert_eq!(ct.refused_values, 1, "{name}");
        let dry = suggestion::import(
            reg,
            &Import {
                campaign: c.id,
                rows: &[by_stack(ids[1], Some("other"), &[], Some("m@1"))],
                author: None,
                source: None,
                who: "cleo@lab",
                names: &names(),
                dry_run: true,
            },
            &at(0, 1),
        )
        .unwrap();
        assert_eq!(dry.suggestions, 1, "{name}");
        assert_eq!(
            suggestion::of_campaign(reg.store(), c.id).unwrap().len(),
            5,
            "{name}: a dry run writes nothing"
        );
    }
}

/// R3 under record 48's blind rule: a stack of a sample sealed now takes no
/// suggestion; the import counts it and keeps nothing for it.
#[test]
fn a_sealed_stack_takes_no_suggestion() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 3);
        let (q, adj) = (body_part(), json!({"when": "never"}));
        let c =
            campaign::create(reg, &new("sealed", &q, &adj, Items::Stacks(ids.clone()), 1)).unwrap();
        labels::seal(reg, "selection:cert@1", None, &ids[..1], "op@lab").unwrap();
        let series = Suggested {
            series: Some("1.2.840.48".into()),
            value: Some("brain".into()),
            ..Suggested::default()
        };
        let done = import(reg, c.id, &[series], Some("v0-model"));
        assert_eq!((done.suggestions, done.sealed), (2, 1), "{name}: {done:?}");
        let all = suggestion::of_campaign(reg.store(), c.id).unwrap();
        assert!(all.iter().all(|s| s.stack_id != Some(ids[0])), "{name}");
    }
}

/// R3: a page accepted in one move answers each item with its own value,
/// the suggestion shown and who suggested it kept beside, `changed` where
/// the person corrected it, and the page's time shared among the answers;
/// one value the question does not take refuses the whole move.
#[test]
fn a_page_accepted_answers_each_item_with_its_own_value() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 3);
        let (q, adj) = (body_part(), json!({"when": "never"}));
        let c =
            campaign::create(reg, &new("page", &q, &adj, Items::Stacks(ids.clone()), 1)).unwrap();
        let items = campaign::items(reg.store(), c.id).unwrap();
        let g = give(0, "anna@lab", "unused");
        // a value the question does not take: nothing is written
        let bad = [
            Each {
                item: items[0].id,
                value: "brain",
                suggested: Some("brain"),
                suggested_by: Some("v0-model"),
                seconds: Some(2.0),
            },
            Each {
                item: items[1].id,
                value: "hand",
                suggested: Some("brain"),
                suggested_by: Some("v0-model"),
                seconds: Some(2.0),
            },
        ];
        assert!(
            campaign::accept_each(reg, c.id, &bad, &g, &at(0, 0)).is_err(),
            "{name}"
        );
        assert!(
            campaign::answers(reg.store(), c.id).unwrap().is_empty(),
            "{name}"
        );
        let rows = [
            Each {
                item: items[0].id,
                value: "brain",
                suggested: Some("brain"),
                suggested_by: Some("v0-model"),
                seconds: Some(2.0),
            },
            Each {
                item: items[1].id,
                value: "neck",
                suggested: Some("brain"),
                suggested_by: Some("v0-model"),
                seconds: Some(2.0),
            },
            Each {
                item: items[2].id,
                value: "other",
                suggested: None,
                suggested_by: Some("v0-model"),
                seconds: None,
            },
        ];
        let done = campaign::accept_each(reg, c.id, &rows, &g, &at(0, 1)).unwrap();
        assert_eq!(done.accepted.len(), 3, "{name}");
        assert!(done.accepted.iter().all(|a| a.state == "agreed"), "{name}");
        let all = campaign::answers(reg.store(), c.id).unwrap();
        let of = |item: i64| all.iter().find(|a| a.item_id == item).unwrap();
        let a0 = of(items[0].id);
        assert_eq!(a0.via.as_deref(), Some("batch"), "{name}");
        assert_eq!(a0.principal, "anna@lab", "{name}");
        assert_eq!(a0.changed, Some(false), "{name}");
        assert_eq!(a0.suggested_by.as_deref(), Some("v0-model"), "{name}");
        assert_eq!(a0.seconds, Some(2.0), "{name}");
        let a1 = of(items[1].id);
        assert_eq!(
            (a1.value.as_deref(), a1.changed),
            (Some("neck"), Some(true)),
            "{name}"
        );
        assert_eq!(a1.suggested.as_deref(), Some("brain"), "{name}");
        // no suggestion: nothing changed, and no suggester
        let a2 = of(items[2].id);
        assert_eq!(
            (a2.changed, a2.suggested_by.as_deref()),
            (None, None),
            "{name}"
        );
        // each is its own answer: accepted again, each is refused alone
        let again =
            campaign::accept_each(reg, c.id, &rows[..1], &give(0, "anna@lab", "x"), &at(0, 2))
                .unwrap();
        assert_eq!(
            (again.accepted.len(), again.refused.len()),
            (0, 1),
            "{name}"
        );
        let stats = campaign::stats(reg.store(), c.id).unwrap();
        assert_eq!(stats["all"]["batched"], 3, "{name}");
    }
}

/// R3 on an axes question that asks one axis: the value of the axis reads
/// as the whole answer and back.
#[test]
fn a_single_axis_of_an_axes_question_reads_both_ways() {
    let q = campaign::Question::Axes {
        axes: vec!["body_part".into()],
        constraints: json!({"values": {"body_part": ["brain", "neck"]}}),
        derive: Vec::new(),
    };
    assert_eq!(campaign::single_axis(&q).as_deref(), Some("body_part"));
    let answer = campaign::single_answer(&q, "brain").unwrap();
    assert_eq!(answer, r#"{"body_part":"brain"}"#);
    assert_eq!(
        campaign::single_value(&q, &answer).as_deref(),
        Some("brain")
    );
    let two = campaign::Question::Axes {
        axes: vec!["body_part".into(), "base".into()],
        constraints: json!({}),
        derive: Vec::new(),
    };
    assert!(campaign::single_axis(&two).is_none());
    let one = campaign::Question::Axis {
        axis: "body_part".into(),
        values: Vec::new(),
    };
    assert_eq!(
        campaign::single_answer(&one, " neck ").as_deref(),
        Some("neck")
    );
}

/// R7: claims in order of value take what the suggestions are least sure
/// of first, and authors that disagree before that.
#[test]
fn claims_by_value_take_the_least_certain_suggestion_first() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 3);
        for s in &ids {
            classified(reg, *s, "body_part", "brain", 0.95);
        }
        let (q, adj) = (body_part(), json!({"when": "never"}));
        let c =
            campaign::create(reg, &new("order", &q, &adj, Items::Stacks(ids.clone()), 1)).unwrap();
        let items = campaign::items(reg.store(), c.id).unwrap();
        import(
            reg,
            c.id,
            &[
                by_stack(ids[0], None, &[("brain", 0.9), ("neck", 0.1)], None),
                by_stack(ids[1], None, &[("brain", 0.55), ("neck", 0.45)], None),
                by_stack(ids[2], None, &[("brain", 0.8), ("neck", 0.2)], None),
            ],
            Some("bodypart@2"),
        );
        let first = campaign::claim_in(reg, c.id, "anna@lab", Role::Rater, Order::Value, &at(0, 0))
            .unwrap()
            .unwrap();
        assert_eq!(
            first.item.id, items[1].id,
            "{name}: the least certain first"
        );
        // a second author that disagrees puts its item before any doubt
        import(
            reg,
            c.id,
            &[by_stack(ids[0], Some("neck"), &[], None)],
            Some("v0-person"),
        );
        let next = campaign::claim_in(reg, c.id, "bo@lab", Role::Rater, Order::Value, &at(0, 1))
            .unwrap()
            .unwrap();
        assert_eq!(next.item.id, items[0].id, "{name}: a disagreement first");
    }
}

/// R3: a campaign's hold-back draws items one by one, the same for one
/// seed, about the share it holds back.
#[test]
fn the_seed_holds_back_about_its_share_item_by_item() {
    let held = (0..2000)
        .filter(|i| campaign::drawn_back("seed", *i, 0.1))
        .count();
    assert!((150..250).contains(&held), "{held}");
    assert_eq!(
        (0..50)
            .map(|i| campaign::drawn_back("s", i, 0.3))
            .collect::<Vec<_>>(),
        (0..50)
            .map(|i| campaign::drawn_back("s", i, 0.3))
            .collect::<Vec<_>>()
    );
    assert!((0..100).all(|i| !campaign::drawn_back("s", i, 0.0)));
    assert!((0..100).all(|i| campaign::drawn_back("s", i, 1.0)));
}
