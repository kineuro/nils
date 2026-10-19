// SPDX-License-Identifier: AGPL-3.0-only

//! Record 58 P1 (Wave 7b spec §12), gate E1, on both backends: a verify
//! campaign of 200 items plants a tenth of them from a confusion table,
//! every planted item is recorded and its catch reported; every verify
//! answer is not blind and a label set of the campaign never trains; the
//! planted list is opened by the close; a plant keeps the pack's
//! constraints, and the decisions a verify close writes name the reader
//! and are left out of the training labels. A planted value the reader
//! accepted is known to be wrong and is never written as a decision.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::campaign::{self, Close, Given, Items, New, Role, Suggest, Timing};
use nils_registry::home::{Home, InitOptions};
use nils_registry::schema::{Type, table};
use nils_registry::suggestion::{self, Given as Suggested, Import};
use nils_registry::verify::{self, Planting};
use nils_registry::{
    Backend, Insert, Param, Registry, Scheme, Store, audit, blind, labels, review,
};
use serde_json::json;

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_verify_test";

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
    let dir = TempDir::new("verify-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-verify-test-key").unwrap();
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

/// A value of the column's type for a row made up by the test.
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

/// Two subjects, a series each, and `per` stacks in each series. Answers
/// the stack ids.
fn stacks(reg: &mut Registry, per: usize) -> Vec<i64> {
    let store = reg.store();
    let mut out = Vec::new();
    for (n, uid) in ["1.2.840.9.1", "1.2.840.9.2"].iter().enumerate() {
        let subject = row(store, "subject", &[("code", Param::from(format!("s{n}")))]);
        let study = row(
            store,
            "study",
            &[
                ("subject_id", Param::Int(subject)),
                ("study_instance_uid", Param::from(format!("{uid}.0"))),
            ],
        );
        let series = row(
            store,
            "series",
            &[
                ("subject_id", Param::Int(subject)),
                ("study_id", Param::Int(study)),
                ("series_instance_uid", Param::from(*uid)),
            ],
        );
        for i in 0..per {
            out.push(row(
                store,
                "stack",
                &[
                    ("series_id", Param::Int(series)),
                    ("stack_index", Param::Int(i as i64)),
                    ("stack_key", Param::from(format!("{uid}#{i}"))),
                ],
            ));
        }
    }
    out
}

fn body_part() -> serde_json::Value {
    json!({"kind": "axis", "axis": "body_part", "values": ["brain", "spine", "neck", "brain-neck"]})
}

fn axes_question() -> serde_json::Value {
    json!({
        "kind": "axes",
        "axes": ["base", "technique", "modifier"],
        "constraints": {
            "pack": "test@1",
            "values": {
                "base": ["T1w", "T2w", "PDw"],
                "technique": ["MPRAGE", "TSE", "SE"],
                "modifier": ["FLAIR", "STIR", "FatSat"]
            },
            "multi": ["modifier"],
            "groups": {"modifier": {"IR_CONTRAST": ["FLAIR", "STIR"]}},
            "implications": [{
                "rule": "base/technique:MPRAGE",
                "when": {"axis": "technique", "is": "MPRAGE"},
                "then": [{"axis": "base", "value": "T1w"}]
            }]
        }
    })
}

/// A verify campaign to create: one rater per item, suggestions brought
/// in, as the paper's campaigns are made.
fn verify_new<'a>(
    name: &'a str,
    q: &'a serde_json::Value,
    adj: &'a serde_json::Value,
    items: Items,
    closes_into: &'a str,
) -> New<'a> {
    New {
        name,
        owner: "cleo@lab",
        question: q,
        source: json!({"test": true}),
        items,
        handle_id: None,
        content_hash: None,
        pack_version: Some("mri@1.0.0"),
        raters_per_item: 1,
        raters: Vec::new(),
        adjudicators: Vec::new(),
        adjudication: adj,
        closes_into,
        lease_seconds: 600,
        inputs: Default::default(),
        hold_back: None,
        suggest: Some(Suggest::Imported),
        hide_header: false,
    }
}

fn at(minute: u32) -> String {
    format!("2026-10-18T10:{minute:02}:00Z")
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

fn person_closes(c: i64) -> Close<'static> {
    Close {
        campaign: c,
        who: "cleo@lab",
        author_kind: "person",
        model: None,
        picks: None,
        sealed: None,
    }
}

/// Rows of a statement made with the store at hand, for its qualified names.
fn select(reg: &mut Registry, sql: impl Fn(&Store) -> String) -> Vec<nils_registry::Row> {
    let store = reg.store();
    let text = sql(store);
    store.query(&text, &[]).unwrap()
}

/// Bring one suggestion per item in, by item id, under one author.
fn suggest(reg: &mut Registry, campaign: i64, values: &[(i64, &str)], author: &str) {
    let rows: Vec<Suggested> = values
        .iter()
        .map(|(item, value)| Suggested {
            item: Some(*item),
            stack: None,
            series: None,
            value: Some(value.to_string()),
            confidences: BTreeMap::new(),
            author: None,
        })
        .collect();
    let names = BTreeMap::new();
    suggestion::import(
        reg,
        &Import {
            campaign,
            rows: &rows,
            author: Some(author),
            source: Some("development-read.tsv"),
            who: "cleo@lab",
            names: &names,
            dry_run: false,
        },
        &at(0),
    )
    .unwrap();
}

/// E1: a verify campaign of 200 items, 10 % planted from a confusion table:
/// exactly 20 items are drawn when the campaign is made, each planted
/// item's shown proposal carries the table's commonest wrong value for its
/// true value, the same at every serve; every answer is recorded not blind;
/// the catch counts the planted items the reader did not accept, against
/// the bar; a label set of the campaign never trains; the close opens the
/// seed and the list.
#[test]
fn a_verify_campaign_of_two_hundred_items_plants_a_tenth_and_reports_its_catch() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 100);
        assert_eq!(ids.len(), 200, "{name}");
        let q = body_part();
        let adj = json!({"when": "never"});
        let c = campaign::create(
            reg,
            &verify_new("verify-200", &q, &adj, Items::Stacks(ids.clone()), "none"),
        )
        .unwrap();
        blind::set_purpose(reg.store(), c.id, verify::PURPOSE).unwrap();
        let planting = Planting::parse(&json!({
            "share": 0.1, "seed": "paper-2026",
            "from": [
                {"true": "brain", "wrong": "neck", "count": 5},
                {"true": "brain", "wrong": "spine", "count": 2},
                {"true": "spine", "wrong": "neck", "count": 3},
            ],
        }))
        .unwrap();
        assert_eq!(
            verify::plant(reg.store(), c.id, &planting).unwrap(),
            20,
            "{name}"
        );
        let items = campaign::items(reg.store(), c.id).unwrap();
        assert_eq!(items.len(), 200, "{name}");
        // the development read's suggestions: brain and spine, by turns
        let proposal_of = |position: i64| if position % 2 == 0 { "brain" } else { "spine" };
        let values: Vec<(i64, &str)> = items
            .iter()
            .map(|it| (it.id, proposal_of(it.position)))
            .collect();
        suggest(reg, c.id, &values, "v0-model");

        // drawn, nothing fixed, nothing opened, and no door tells the seed
        let drawn = verify::planted(reg.store(), c.id).unwrap();
        assert_eq!(drawn.len(), 20, "{name}");
        assert!(
            drawn.iter().all(|r| !r.fixed() && r.served_at.is_none()),
            "{name}"
        );
        let summary = verify::summary(reg.store(), c.id).unwrap().unwrap();
        assert_eq!(
            summary,
            json!({"share": 0.1, "count": 20, "opened": false}),
            "{name}"
        );
        let planted_ids: BTreeSet<i64> = drawn.iter().map(|r| r.item_id).collect();

        // each item's proposal as shown: a planted one says the table's
        // commonest wrong value for its true value, twice the same
        let mut shown_of: BTreeMap<i64, (String, String)> = BTreeMap::new();
        for it in &items {
            let proposal = proposal_of(it.position);
            let shown = verify::shown(reg.store(), &c, it.id, proposal, &at(1)).unwrap();
            if planted_ids.contains(&it.id) {
                assert_eq!(shown, "neck", "{name}: item {}", it.id);
            } else {
                assert_eq!(shown, proposal, "{name}: item {}", it.id);
            }
            assert_eq!(
                verify::shown(reg.store(), &c, it.id, proposal, &at(2)).unwrap(),
                shown,
                "{name}"
            );
            shown_of.insert(it.id, (proposal.to_string(), shown));
        }
        let fixed = verify::planted(reg.store(), c.id).unwrap();
        assert!(
            fixed.iter().all(|r| r.fixed()
                && r.axis.as_deref() == Some("body_part")
                && r.wrong_value.as_deref() == Some("neck")
                && r.served_at.as_deref() == Some(at(1).as_str())),
            "{name}: {fixed:?}"
        );
        assert_eq!(verify::list_digest(&fixed).len(), 64, "{name}");

        // the reader accepts the shown value on three planted items and on
        // every unplanted one, and catches the other seventeen
        let mut accepted_planted = 0;
        for _ in 0..200 {
            let cl = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(3))
                .unwrap()
                .unwrap_or_else(|| panic!("{name}: an item to read"));
            let (proposal, shown) = shown_of[&cl.item.id].clone();
            let answer = if planted_ids.contains(&cl.item.id) && accepted_planted < 3 {
                accepted_planted += 1;
                shown.clone()
            } else {
                proposal
            };
            campaign::answer_with(
                reg,
                &give(cl.assignment.id, "anna@lab", &answer),
                &Timing {
                    suggested: Some(&shown),
                    suggested_by: Some("v0-model"),
                    batch: false,
                    derived: None,
                },
                &at(4),
            )
            .unwrap();
        }
        assert!(
            campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(5))
                .unwrap()
                .is_none(),
            "{name}"
        );

        // every verify answer is not blind
        assert_eq!(
            blind::answers_not_blind(reg.store(), c.id).unwrap(),
            200,
            "{name}"
        );
        let answers = campaign::answers(reg.store(), c.id).unwrap();
        assert_eq!(answers.len(), 200, "{name}");
        assert!(answers.iter().all(|a| a.blind == Some(false)), "{name}");
        // and keeps the shown proposal beside it, so a changed answer is a catch
        assert_eq!(
            answers.iter().filter(|a| a.changed == Some(true)).count(),
            17,
            "{name}"
        );

        // the catch: 17 of 20, under the bar of 90 %
        let catch = verify::catch(reg.store(), &c).unwrap();
        assert_eq!(catch["planted"], 20, "{name}: {catch}");
        assert_eq!(catch["caught"], 17, "{name}: {catch}");
        assert_eq!(catch["items"], 20, "{name}");
        assert_eq!(catch["shown"], 20, "{name}");
        assert_eq!(catch["unread"], 0, "{name}");
        assert!(
            (catch["rate"].as_f64().unwrap() - 0.85).abs() < 1e-9,
            "{name}: {catch}"
        );
        assert_eq!(catch["bar"], 0.9, "{name}");
        assert_eq!(catch["below_bar"], true, "{name}");
        let per = catch["per_rater"].as_array().unwrap();
        assert_eq!(per.len(), 1, "{name}");
        assert_eq!(per[0]["rater"], "anna@lab", "{name}");
        assert_eq!(per[0]["planted"], 20, "{name}");
        assert_eq!(per[0]["caught"], 17, "{name}");

        // a label set of the campaign never trains
        let set = labels::record(
            reg,
            &labels::NewSet {
                name: "verify-200",
                version: 1,
                kind: "outcomes",
                what: "body_part",
                source: json!({"campaign": c.id}),
                campaign_id: Some(c.id),
                handle_id: None,
                pack_version: None,
                scheme_digest: None,
                sealed: false,
                rows: 200,
                digest: "abc",
                place_id: None,
                path: None,
                created_by: "cleo@lab",
            },
        )
        .unwrap();
        let refused = labels::usable_for_training(reg.store(), set.id)
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("verify campaign, whose reads never train"),
            "{name}: {refused}"
        );

        // the close opens the seed and the list; the catch reads the same.
        // It closes into nothing, a reference's check, so the three plants
        // the reader kept leave nothing out
        let closed = campaign::close(reg, &person_closes(c.id), &at(6)).unwrap();
        assert!(closed.decisions.is_empty(), "{name}");
        assert_eq!(closed.planted_skipped, Some(0), "{name}");
        let summary = verify::summary(reg.store(), c.id).unwrap().unwrap();
        assert_eq!(summary["opened"], true, "{name}: {summary}");
        assert_eq!(summary["seed"], "paper-2026", "{name}");
        assert_eq!(summary["from"].as_array().unwrap().len(), 3, "{name}");
        let list = summary["list"].as_array().unwrap();
        assert_eq!(list.len(), 20, "{name}");
        assert!(
            list.iter().all(|r| r["axis"] == "body_part"
                && r["wrong"] == "neck"
                && (r["true"] == "brain" || r["true"] == "spine")),
            "{name}: {list:?}"
        );
        assert_eq!(
            verify::catch(reg.store(), &c).unwrap()["caught"],
            17,
            "{name}"
        );
        eprintln!("{name}: 200 items, 20 planted, 17 caught");
    }
}

/// A plant on an axes question keeps the pack's constraints: a T1w MPRAGE
/// is never planted a wrong base, since MPRAGE forces T1w; without a table
/// the wrong value is a legal other value drawn from the seed. A verify
/// close into decisions writes each decision with the reader as its author
/// and marked not blind, so the training labels leave them out.
#[test]
fn a_plant_keeps_the_constraints_and_a_verify_close_writes_decisions_that_never_train() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 10);
        let q = axes_question();
        let adj = json!({"when": "never"});
        let c = campaign::create(
            reg,
            &verify_new(
                "verify-axes",
                &q,
                &adj,
                Items::Stacks(ids.clone()),
                "decision",
            ),
        )
        .unwrap();
        blind::set_purpose(reg.store(), c.id, verify::PURPOSE).unwrap();
        let planting = Planting::parse(&json!({"share": 0.5, "seed": "axes"})).unwrap();
        assert_eq!(
            verify::plant(reg.store(), c.id, &planting).unwrap(),
            10,
            "{name}"
        );
        let items = campaign::items(reg.store(), c.id).unwrap();
        let proposal = r#"{"base":"T1w","technique":"MPRAGE","modifier":["FatSat"]}"#;
        let values: Vec<(i64, &str)> = items.iter().map(|it| (it.id, proposal)).collect();
        suggest(reg, c.id, &values, "v0-model");
        let planted_ids: BTreeSet<i64> = verify::planted(reg.store(), c.id)
            .unwrap()
            .iter()
            .map(|r| r.item_id)
            .collect();
        let question = c.question().unwrap();
        let campaign::Question::Axes {
            axes, constraints, ..
        } = &question
        else {
            unreachable!()
        };
        let canonical = |text: &str| {
            campaign::canonical_joint(
                constraints,
                &campaign::joint_of(axes, constraints, text).unwrap(),
            )
        };
        let mut shown_of: BTreeMap<i64, String> = BTreeMap::new();
        for it in &items {
            let shown = verify::shown(reg.store(), &c, it.id, proposal, &at(1)).unwrap();
            if planted_ids.contains(&it.id) {
                assert_ne!(shown, canonical(proposal), "{name}: item {}", it.id);
                assert_ne!(shown, proposal, "{name}: item {}", it.id);
            } else {
                // an unplanted proposal is shown as it came
                assert_eq!(shown, proposal, "{name}: item {}", it.id);
            }
            let joint = campaign::joint_of(axes, constraints, &shown).unwrap();
            assert!(
                campaign::legal(constraints, &joint).is_ok(),
                "{name}: {shown} is not legal"
            );
            shown_of.insert(it.id, shown);
        }
        let fixed = verify::planted(reg.store(), c.id).unwrap();
        assert!(fixed.iter().all(|r| r.fixed()), "{name}: {fixed:?}");
        // the base is forced by the technique, so the plant falls on another axis
        assert!(
            fixed
                .iter()
                .all(|r| matches!(r.axis.as_deref(), Some("technique" | "modifier"))),
            "{name}: {fixed:?}"
        );
        // a plant drawn without a table still comes from the seed: twice the same
        for r in &fixed {
            assert_eq!(
                verify::shown(reg.store(), &c, r.item_id, proposal, &at(2)).unwrap(),
                shown_of[&r.item_id],
                "{name}"
            );
        }

        // the reader catches every plant and accepts every other proposal
        for _ in 0..20 {
            let cl = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(3))
                .unwrap()
                .unwrap_or_else(|| panic!("{name}: an item to read"));
            let shown = shown_of[&cl.item.id].clone();
            let answer = if planted_ids.contains(&cl.item.id) {
                proposal.to_string()
            } else {
                shown.clone()
            };
            campaign::answer_with(
                reg,
                &give(cl.assignment.id, "anna@lab", &answer),
                &Timing {
                    suggested: Some(&shown),
                    suggested_by: Some("v0-model"),
                    batch: false,
                    derived: None,
                },
                &at(4),
            )
            .unwrap();
        }
        let catch = verify::catch(reg.store(), &c).unwrap();
        assert_eq!(catch["planted"], 10, "{name}: {catch}");
        assert_eq!(catch["caught"], 10, "{name}: {catch}");
        assert_eq!(catch["rate"], 1.0, "{name}");
        assert_eq!(catch["below_bar"], false, "{name}");

        // the close writes one decision per axis per item, each the
        // reader's and marked not blind, and opens the list
        let closed = campaign::close(reg, &person_closes(c.id), &at(5)).unwrap();
        assert_eq!(closed.decisions.len(), 60, "{name}: {closed:?}");
        assert_eq!(closed.resolved, 20, "{name}");
        // every plant was caught, so every item is written whole
        assert_eq!(closed.planted_skipped, Some(0), "{name}");
        let listed = closed
            .decisions
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let rows = select(reg, |s| {
            format!(
                "SELECT blind, actor, author_kind FROM {} WHERE id IN ({listed})",
                s.qualified("decision")
            )
        });
        assert_eq!(rows.len(), 60, "{name}");
        for r in &rows {
            assert_eq!(r.opt_int(0).unwrap(), Some(0), "{name}");
            assert_eq!(r.text(1).unwrap(), "anna@lab", "{name}");
            assert_eq!(r.text(2).unwrap(), "person", "{name}");
        }
        // the decisions stand, and the training labels leave every one out
        let standing = labels::decision_labels(
            reg.store(),
            &labels::DecisionQuery {
                axis: "technique",
                campaign: Some(c.id),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(standing.len(), 20, "{name}");
        let (training, left_out) =
            labels::training_labels(reg.store(), Some("technique"), None, &[], Some(c.id)).unwrap();
        assert!(training.is_empty(), "{name}: {} train", training.len());
        assert_eq!(left_out, 20, "{name}");
        let summary = verify::summary(reg.store(), c.id).unwrap().unwrap();
        assert_eq!(summary["opened"], true, "{name}");
        assert_eq!(summary["seed"], "axes", "{name}");
        assert!(summary["from"].is_null(), "{name}: {summary}");
        assert_eq!(summary["list"].as_array().unwrap().len(), 10, "{name}");
        eprintln!("{name}: 20 items, 10 planted on a legal axis, 60 decisions not blind");
    }
}

/// The decisions a campaign's close wrote, as (stack, axis, value, actor,
/// author kind, blind), in stack and axis order.
type Written = (String, String, Option<String>, String, String, Option<i64>);

fn decisions_of(reg: &mut Registry, campaign: i64) -> Vec<Written> {
    select(reg, |s| {
        format!(
            "SELECT ref, axis, value, actor, author_kind, blind FROM {} \
             WHERE campaign_id = {campaign} ORDER BY ref, axis",
            s.qualified("decision")
        )
    })
    .iter()
    .map(|r| {
        (
            r.text(0).unwrap().to_string(),
            r.text(1).unwrap().to_string(),
            r.opt_text(2).unwrap().map(str::to_string),
            r.text(3).unwrap().to_string(),
            r.text(4).unwrap().to_string(),
            r.opt_int(5).unwrap(),
        )
    })
    .collect()
}

/// A verify campaign over these stacks that plants every item, with one
/// proposal brought in for all of them and the plants fixed as a reader is
/// shown them.
struct Read {
    c: campaign::Campaign,
    items: Vec<campaign::Item>,
    /// What each item shows its reader.
    shown: BTreeMap<i64, String>,
    /// The plant fixed on each item.
    plants: BTreeMap<i64, verify::Planted>,
}

impl Read {
    fn stack_of(&self, item: i64) -> String {
        self.items
            .iter()
            .find(|i| i.id == item)
            .and_then(|i| i.stack_id)
            .unwrap()
            .to_string()
    }
}

fn verify_read(
    reg: &mut Registry,
    name: &str,
    ids: Vec<i64>,
    q: &serde_json::Value,
    planting: serde_json::Value,
    proposal: &str,
) -> Read {
    let adj = json!({"when": "never"});
    let c = campaign::create(
        reg,
        &verify_new(name, q, &adj, Items::Stacks(ids), "decision"),
    )
    .unwrap();
    blind::set_purpose(reg.store(), c.id, verify::PURPOSE).unwrap();
    verify::plant(reg.store(), c.id, &Planting::parse(&planting).unwrap()).unwrap();
    let items = campaign::items(reg.store(), c.id).unwrap();
    let values: Vec<(i64, &str)> = items.iter().map(|it| (it.id, proposal)).collect();
    suggest(reg, c.id, &values, "v0-model");
    let mut shown = BTreeMap::new();
    for it in &items {
        shown.insert(
            it.id,
            verify::shown(reg.store(), &c, it.id, proposal, &at(1)).unwrap(),
        );
    }
    let plants = verify::planted(reg.store(), c.id)
        .unwrap()
        .into_iter()
        .map(|r| (r.item_id, r))
        .collect();
    Read {
        c,
        items,
        shown,
        plants,
    }
}

/// The reader answers every item of the campaign with `answer(item)`, the
/// proposal shown beside it kept as the doors keep it.
fn read_every_item(reg: &mut Registry, r: &Read, answer: impl Fn(i64) -> String) {
    for _ in 0..r.items.len() {
        let cl = campaign::claim(reg, r.c.id, "anna@lab", Role::Rater, &at(3))
            .unwrap()
            .expect("an item to read");
        campaign::answer_with(
            reg,
            &give(cl.assignment.id, "anna@lab", &answer(cl.item.id)),
            &Timing {
                suggested: Some(&r.shown[&cl.item.id]),
                suggested_by: Some("v0-model"),
                batch: false,
                derived: None,
            },
            &at(4),
        )
        .unwrap();
    }
}

/// Spec §12.4: where a verify campaign closes into decisions the answer is a
/// decision with the reader as its author, but a planted value is known to be
/// wrong and is never one. Of four planted items she keeps the plant on two
/// (the seed plants a single-valued axis on one, a multi-valued axis on the
/// other) and corrects it on two: each kept item has no decision on its
/// planted axis and a decision on each other axis; each corrected item is
/// written whole; every decision is hers and not blind, and the training
/// labels leave all of them out.
#[test]
fn a_planted_value_the_reader_accepted_is_never_a_decision_and_a_caught_one_is_written_whole() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 2);
        assert_eq!(ids.len(), 4, "{name}");
        let proposal = r#"{"base":"T1w","technique":"MPRAGE","modifier":["FatSat"]}"#;
        let r = verify_read(
            reg,
            "verify-accepted",
            ids,
            &axes_question(),
            json!({"share": 1.0, "seed": "accepted"}),
            proposal,
        );
        assert_eq!(r.plants.len(), 4, "{name}");
        assert!(r.plants.values().all(verify::Planted::fixed), "{name}");
        let kept: Vec<i64> = r.items[..2].iter().map(|i| i.id).collect();
        let caught: Vec<i64> = r.items[2..].iter().map(|i| i.id).collect();
        let axis_of = |item: i64| r.plants[&item].axis.clone().unwrap();
        let wrong_of = |item: i64| r.plants[&item].wrong_value.clone().unwrap();
        // the seed's draw: the plants she keeps fall on two different axes
        let planted_axes: BTreeSet<String> = kept.iter().map(|k| axis_of(*k)).collect();
        assert_eq!(
            planted_axes,
            BTreeSet::from(["technique", "modifier"].map(String::from)),
            "{name}"
        );
        for k in &kept {
            // a plant is shown, differing from the proposal
            assert_ne!(r.shown[k], proposal, "{name}");
        }

        // she keeps the first two plants shown and corrects the other two
        read_every_item(reg, &r, |item| {
            if kept.contains(&item) {
                r.shown[&item].clone()
            } else {
                proposal.to_string()
            }
        });
        let catch = verify::catch(reg.store(), &r.c).unwrap();
        assert_eq!(catch["planted"], 4, "{name}: {catch}");
        assert_eq!(catch["caught"], 2, "{name}: {catch}");
        assert_eq!(catch["below_bar"], true, "{name}: {catch}");

        let closed = campaign::close(reg, &person_closes(r.c.id), &at(5)).unwrap();
        assert_eq!(closed.resolved, 4, "{name}: {closed:?}");
        assert!(closed.refused.is_empty(), "{name}: {closed:?}");
        // two axes of each kept item and all three of each corrected one
        assert_eq!(closed.decisions.len(), 2 * 2 + 2 * 3, "{name}: {closed:?}");
        assert_eq!(closed.planted_skipped, Some(2), "{name}: {closed:?}");
        assert_eq!(closed.as_json()["planted_skipped"], 2, "{name}");
        let entry = audit::list(
            reg.store(),
            &audit::Filter {
                action: Some("campaign.close".into()),
                limit: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            entry[0].details.as_ref().unwrap()["planted_skipped"],
            2,
            "{name}"
        );

        let all: BTreeSet<String> = ["base", "technique", "modifier"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let rows = decisions_of(reg, r.c.id);
        assert_eq!(rows.len(), 10, "{name}: {rows:?}");
        let axes_of = |item: i64| -> BTreeSet<String> {
            rows.iter()
                .filter(|d| d.0 == r.stack_of(item))
                .map(|d| d.1.clone())
                .collect()
        };
        let value_of = |item: i64, axis: &str| {
            rows.iter()
                .find(|d| d.0 == r.stack_of(item) && d.1 == axis)
                .and_then(|d| d.2.clone())
        };
        let truth = [
            ("base", "T1w"),
            ("technique", "MPRAGE"),
            ("modifier", "FatSat"),
        ];
        for k in &kept {
            // no decision on the planted axis, the reader's on the others
            let mut others = all.clone();
            others.remove(&axis_of(*k));
            assert_eq!(axes_of(*k), others, "{name}: {rows:?}");
            assert!(
                !rows.iter().any(|d| d.0 == r.stack_of(*k)
                    && d.1 == axis_of(*k)
                    && d.2.as_deref() == Some(wrong_of(*k).as_str())),
                "{name}: {rows:?}"
            );
            for (axis, value) in truth {
                if axis != axis_of(*k) {
                    assert_eq!(value_of(*k, axis).as_deref(), Some(value), "{name}");
                }
            }
        }
        for c in &caught {
            // the corrected plant is written whole
            assert_eq!(axes_of(*c), all, "{name}: {rows:?}");
            for (axis, value) in truth {
                assert_eq!(value_of(*c, axis).as_deref(), Some(value), "{name}");
            }
        }
        // each is the reader's own, a person's, and not blind
        for d in &rows {
            assert_eq!(d.3, "anna@lab", "{name}: {d:?}");
            assert_eq!(d.4, "person", "{name}: {d:?}");
            assert_eq!(d.5, Some(0), "{name}: {d:?}");
        }

        // the item says what the close left out, and a corrected one says nothing
        let items = campaign::items(reg.store(), r.c.id).unwrap();
        let of = |id: i64| items.iter().find(|i| i.id == id).unwrap();
        for k in &kept {
            assert_eq!(of(*k).state, "resolved", "{name}");
            assert_eq!(
                of(*k).outcome["planted_skipped"],
                json!([axis_of(*k)]),
                "{name}"
            );
            assert_eq!(
                of(*k).outcome["decisions"].as_object().unwrap().len(),
                2,
                "{name}"
            );
            assert!(
                of(*k).outcome["decisions"].get(axis_of(*k)).is_none(),
                "{name}"
            );
            // the item's review item says what was accepted: the planted
            // value is named beside it, never told as the answer
            let review_item = of(*k).review_item_id;
            let own = select(reg, |s| {
                format!(
                    "SELECT status, {} FROM {} WHERE id = {review_item}",
                    s.dialect()
                        .text_of(table("review_item").column("decision").expect("decision")),
                    s.qualified("review_item")
                )
            });
            assert_eq!(own[0].text(0).unwrap(), "accepted", "{name}");
            let told: serde_json::Value =
                serde_json::from_str(own[0].opt_text(1).unwrap().unwrap()).unwrap();
            assert_eq!(
                told["planted_skipped"],
                json!([axis_of(*k)]),
                "{name}: {told}"
            );
            let told_value = told["value"].as_object().unwrap();
            assert_eq!(told_value.len(), 2, "{name}: {told}");
            assert!(!told_value.contains_key(&axis_of(*k)), "{name}: {told}");
        }
        for c in &caught {
            assert_eq!(of(*c).state, "resolved", "{name}");
            assert!(of(*c).outcome.get("planted_skipped").is_none(), "{name}");
            assert_eq!(
                of(*c).outcome["decisions"].as_object().unwrap().len(),
                3,
                "{name}"
            );
        }

        // the training labels leave every decision out; an axis loses one
        // decision for each kept plant that fell on it
        for axis in ["base", "technique", "modifier"] {
            let want = 4 - kept.iter().filter(|k| axis_of(**k) == axis).count();
            let standing = labels::decision_labels(
                reg.store(),
                &labels::DecisionQuery {
                    axis,
                    campaign: Some(r.c.id),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(standing.len(), want, "{name}: {axis}");
            let (training, left_out) =
                labels::training_labels(reg.store(), Some(axis), None, &[], Some(r.c.id)).unwrap();
            assert!(training.is_empty(), "{name}: {axis} trains");
            assert_eq!(left_out, want, "{name}: {axis}");
        }

        // the opened list still names all four plants, the kept ones among them
        let summary = verify::summary(reg.store(), r.c.id).unwrap().unwrap();
        let list = summary["list"].as_array().unwrap();
        assert_eq!(list.len(), 4, "{name}: {summary}");
        for k in &kept {
            assert!(
                list.iter().any(|p| p["item"] == *k
                    && p["axis"] == axis_of(*k).as_str()
                    && p["wrong"] == wrong_of(*k).as_str()),
                "{name}: {list:?}"
            );
        }
        eprintln!(
            "{name}: two accepted plants left out ({}), two caught written whole",
            planted_axes.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
}

/// Where the settled answer keeps the planted value and nothing else is
/// left to write, the close writes nothing for the item: it is refused, with
/// why, and its question stays open for a person. That is a question of one
/// axis, or an axes question whose reader could not tell on the others.
#[test]
fn an_accepted_plant_with_nothing_else_to_write_is_refused_and_its_question_left_open() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 2);
        assert_eq!(ids.len(), 4, "{name}");

        // one axis asked: the reader keeps the plant on the first item
        let one = verify_read(
            reg,
            "verify-one-axis",
            ids[..2].to_vec(),
            &body_part(),
            json!({"share": 1.0, "seed": "one", "from": [{"true": "brain", "wrong": "neck", "count": 3}]}),
            "brain",
        );
        assert!(one.shown.values().all(|s| s == "neck"), "{name}");
        let (kept, caught) = (one.items[0].id, one.items[1].id);
        read_every_item(reg, &one, |item| {
            if item == kept { "neck" } else { "brain" }.to_string()
        });
        let closed = campaign::close(reg, &person_closes(one.c.id), &at(5)).unwrap();
        assert_eq!(closed.planted_skipped, Some(1), "{name}: {closed:?}");
        assert_eq!((closed.resolved, closed.unresolved), (1, 1), "{name}");
        assert_eq!(closed.refused.len(), 1, "{name}: {closed:?}");
        assert_eq!(closed.refused[0].0, kept, "{name}");
        assert!(
            closed.refused[0].1.contains(
                "holds the planted wrong value on body_part, which is never written as a decision"
            ),
            "{name}: {}",
            closed.refused[0].1
        );
        assert_eq!(closed.decisions.len(), 1, "{name}: {closed:?}");
        let rows = decisions_of(reg, one.c.id);
        assert_eq!(rows.len(), 1, "{name}: {rows:?}");
        assert_eq!(rows[0].0, one.stack_of(caught), "{name}");
        assert_eq!(rows[0].2.as_deref(), Some("brain"), "{name}");
        assert_eq!(
            (rows[0].3.as_str(), rows[0].4.as_str(), rows[0].5),
            ("anna@lab", "person", Some(0)),
            "{name}"
        );
        let items = campaign::items(reg.store(), one.c.id).unwrap();
        let refused = items.iter().find(|i| i.id == kept).unwrap();
        assert_eq!(refused.state, "refused", "{name}");
        assert!(
            refused
                .refused_why
                .as_deref()
                .is_some_and(|w| w.contains("never written as a decision")),
            "{name}: {refused:?}"
        );
        let waiting = review::item(reg.store(), refused.review_item_id)
            .unwrap()
            .unwrap();
        assert!(
            matches!(waiting.status.as_str(), "open" | "staged"),
            "{name}: {}",
            waiting.status
        );
        assert_eq!(
            items.iter().find(|i| i.id == caught).unwrap().state,
            "resolved",
            "{name}"
        );

        // three axes asked: the reader keeps the plant on the first item and
        // cannot tell on the other two
        let proposal = r#"{"base":"T1w","technique":"MPRAGE","modifier":["FatSat"]}"#;
        let cant = verify_read(
            reg,
            "verify-cant-tell",
            ids[2..].to_vec(),
            &axes_question(),
            json!({"share": 1.0, "seed": "cant"}),
            proposal,
        );
        let (kept, caught) = (cant.items[0].id, cant.items[1].id);
        let planted_axis = cant.plants[&kept].axis.clone().unwrap();
        read_every_item(reg, &cant, |item| {
            if item != kept {
                return proposal.to_string();
            }
            let shown: serde_json::Value = serde_json::from_str(&cant.shown[&item]).unwrap();
            let mut answer = serde_json::Map::new();
            for axis in ["base", "technique", "modifier"] {
                answer.insert(
                    axis.to_string(),
                    if axis == planted_axis {
                        shown[axis].clone()
                    } else {
                        json!("cant_tell")
                    },
                );
            }
            serde_json::Value::Object(answer).to_string()
        });
        let closed = campaign::close(reg, &person_closes(cant.c.id), &at(6)).unwrap();
        assert_eq!(closed.planted_skipped, Some(1), "{name}: {closed:?}");
        assert_eq!(closed.refused.len(), 1, "{name}: {closed:?}");
        assert_eq!(closed.refused[0].0, kept, "{name}");
        assert_eq!(closed.decisions.len(), 3, "{name}: {closed:?}");
        let rows = decisions_of(reg, cant.c.id);
        assert!(
            rows.iter().all(|d| d.0 == cant.stack_of(caught)),
            "{name}: {rows:?}"
        );
        let items = campaign::items(reg.store(), cant.c.id).unwrap();
        assert_eq!(
            items.iter().find(|i| i.id == kept).unwrap().state,
            "refused",
            "{name}"
        );
        eprintln!("{name}: an accepted plant with nothing else to write is refused");
    }
}

/// The four purposes of Wave 7a keep their close exactly: a decide campaign's
/// close says nothing of planted values, in its answer or its audit entry.
#[test]
fn another_purpose_s_close_says_nothing_of_planted_values() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 1);
        let q = body_part();
        let adj = json!({"when": "never"});
        let c = campaign::create(
            reg,
            &verify_new("decide", &q, &adj, Items::Stacks(ids), "decision"),
        )
        .unwrap();
        assert_eq!(
            blind::purpose(reg.store(), c.id).unwrap(),
            "decide",
            "{name}"
        );
        for _ in 0..2 {
            let cl = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(3))
                .unwrap()
                .expect("an item to read");
            campaign::answer_with(
                reg,
                &give(cl.assignment.id, "anna@lab", "brain"),
                &Timing {
                    suggested: None,
                    suggested_by: None,
                    batch: false,
                    derived: None,
                },
                &at(4),
            )
            .unwrap();
        }
        let closed = campaign::close(reg, &person_closes(c.id), &at(5)).unwrap();
        assert_eq!(closed.decisions.len(), 2, "{name}: {closed:?}");
        assert_eq!(closed.planted_skipped, None, "{name}");
        assert!(closed.as_json().get("planted_skipped").is_none(), "{name}");
        let entry = audit::list(
            reg.store(),
            &audit::Filter {
                action: Some("campaign.close".into()),
                limit: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            entry[0]
                .details
                .as_ref()
                .unwrap()
                .get("planted_skipped")
                .is_none(),
            "{name}"
        );
    }
}
