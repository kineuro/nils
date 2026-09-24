// SPDX-License-Identifier: AGPL-3.0-only

//! Record 42 S5, S6 and S7 on both backends: campaigns with raters, leases
//! and an adjudicator, the close through the one write path, and label sets
//! and v0's labels.

use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::campaign::{self, Close, Given, Items, New, Role};
use nils_registry::home::{Home, InitOptions};
use nils_registry::labels::{self, DecisionQuery, Of};
use nils_registry::schema::{Type, table};
use nils_registry::{Backend, Insert, Param, Registry, Scheme, Store};
use serde_json::json;

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_campaign_test";

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
    let dir = TempDir::new("campaign-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-campaign-test-key").unwrap();
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

/// Two subjects, a series each with the UID given, and `per` stacks in
/// each series. Answers the stack ids.
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

fn new<'a>(
    name: &'a str,
    q: &'a serde_json::Value,
    adj: &'a serde_json::Value,
    items: Items,
    raters: i64,
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
        pack_version: Some("mri@0.3.0"),
        raters_per_item: raters,
        raters: Vec::new(),
        adjudicators: Vec::new(),
        adjudication: adj,
        closes_into,
        lease_seconds: 600,
        inputs: Default::default(),
    }
}

/// A derivative row as the derivative door registers one (record 42 S4),
/// made from one stack; the file itself is the door's and not read here.
fn derivative(reg: &mut Registry, stack: i64, kind: &str) -> i64 {
    let store = reg.store();
    let belongs = nils_registry::derivative::belongs(store, Some(stack), None, None, None).unwrap();
    nils_registry::derivative::insert(
        store,
        &nils_registry::derivative::New {
            kind,
            belongs: &belongs,
            place_id: 1,
            path: "derivatives/mask/ab/ab",
            bytes: 1,
            sha256: "ab",
            media_type: "application/octet-stream",
            registered_by: "anna@lab",
            actor: None,
            model_id: None,
            supersedes_id: None,
            created_at: "2026-09-24T10:00:00Z",
        },
    )
    .unwrap()
}

fn at(minute: u32) -> String {
    format!("2026-09-24T10:{minute:02}:00Z")
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
    }
}

/// Rows of a statement made with the store at hand, for its qualified names.
fn select(reg: &mut Registry, sql: impl Fn(&Store) -> String) -> Vec<nils_registry::Row> {
    let store = reg.store();
    let text = sql(store);
    store.query(&text, &[]).unwrap()
}

fn count(reg: &mut Registry, t: &str, filter: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM {}{filter}", reg.store().qualified(t));
    reg.store().query(&sql, &[]).unwrap()[0].int(0).unwrap()
}

/// S5: two raters on each of two items, one lease that runs out and returns
/// its item to the pool, and no rater given the same item twice.
#[test]
fn two_raters_share_the_items_and_an_expired_lease_returns_one() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 1);
        let q = body_part();
        let adj = json!({"when": "disagree", "metric": "exact"});
        let c = campaign::create(
            reg,
            &new(
                "curate",
                &q,
                &adj,
                Items::Stacks(ids.clone()),
                2,
                "decision",
            ),
        )
        .unwrap();
        assert_eq!(c.grain, "stack", "{name}");
        let items = campaign::items(reg.store(), c.id).unwrap();
        assert_eq!(items.len(), 2, "{name}");
        // every item is backed by a review item of its own, open in the queue
        for it in &items {
            let r = nils_registry::review::item(reg.store(), it.review_item_id)
                .unwrap()
                .unwrap();
            assert_eq!(r.kind, "campaign.axis", "{name}");
            assert_eq!(r.status, "open", "{name}");
            assert_eq!(r.evidence["axis"], "body_part", "{name}");
        }

        // anna takes the first item; asking again hands the same lease back
        let a1 = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(0))
            .unwrap()
            .unwrap();
        assert_eq!(a1.item.position, 0, "{name}");
        let again = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(1))
            .unwrap()
            .unwrap();
        assert!(again.held, "{name}");
        assert_eq!(again.assignment.id, a1.assignment.id, "{name}");
        // bo takes the first item too, since it wants two raters
        let b1 = campaign::claim(reg, c.id, "bo@lab", Role::Rater, &at(1))
            .unwrap()
            .unwrap();
        assert_eq!(b1.item.id, a1.item.id, "{name}");
        // a third rater finds the first full and takes the second
        let c1 = campaign::claim(reg, c.id, "cy@lab", Role::Rater, &at(2))
            .unwrap()
            .unwrap();
        assert_eq!(c1.item.position, 1, "{name}");
        // anna and bo answer the first
        campaign::answer(reg, &give(a1.assignment.id, "anna@lab", "brain"), &at(3)).unwrap();
        let done =
            campaign::answer(reg, &give(b1.assignment.id, "bo@lab", "brain"), &at(4)).unwrap();
        assert_eq!(done.state, "agreed", "{name}");
        // an answer is not a decision
        assert_eq!(count(reg, "decision", ""), 0, "{name}");
        // anna's next claim is the second item; she never gets the first again
        let a2 = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(5))
            .unwrap()
            .unwrap();
        assert_eq!(a2.item.position, 1, "{name}");
        campaign::answer(reg, &give(a2.assignment.id, "anna@lab", "spine"), &at(6)).unwrap();
        // cy's lease ran out (ten minutes): bo may now take the second item
        let b2 = campaign::claim(reg, c.id, "bo@lab", Role::Rater, &at(13))
            .unwrap()
            .unwrap();
        assert_eq!(b2.item.position, 1, "{name}");
        let states: Vec<String> = campaign::assignments(reg.store(), c.id)
            .unwrap()
            .into_iter()
            .filter(|a| a.id == c1.assignment.id)
            .map(|a| a.state)
            .collect();
        assert_eq!(states, ["expired"], "{name}");
        // cy's expired lease cannot be answered, and cy is never given the
        // item again
        let late = campaign::answer(reg, &give(c1.assignment.id, "cy@lab", "spine"), &at(14));
        assert!(late.is_err(), "{name}");
        assert!(
            campaign::claim(reg, c.id, "cy@lab", Role::Rater, &at(14))
                .unwrap()
                .is_none(),
            "{name}"
        );
        // a rater may give an item back unanswered, and does not get it again
        campaign::release(reg, b2.assignment.id, "bo@lab", &at(15)).unwrap();
        assert!(
            campaign::claim(reg, c.id, "bo@lab", Role::Rater, &at(15))
                .unwrap()
                .is_none(),
            "{name}"
        );
        let d2 = campaign::claim(reg, c.id, "dan@lab", Role::Rater, &at(16))
            .unwrap()
            .unwrap();
        assert_eq!(d2.item.position, 1, "{name}");
        // an assignment is its rater's own
        assert!(
            campaign::answer(reg, &give(d2.assignment.id, "anna@lab", "brain"), &at(16)).is_err(),
            "{name}"
        );
        campaign::answer(reg, &give(d2.assignment.id, "dan@lab", "spine"), &at(17)).unwrap();
        // an answer outside the vocabulary is refused before anything is written
        let e2 = campaign::claim(reg, c.id, "eve@lab", Role::Rater, &at(18)).unwrap();
        assert!(e2.is_none(), "{name}: both items are full");
        let counts = campaign::counts(reg.store(), c.id).unwrap();
        assert_eq!(counts["items"]["agreed"], 2, "{name}: {counts}");
        assert_eq!(counts["answers"], 4, "{name}: {counts}");

        let closed = campaign::close(
            reg,
            &Close {
                campaign: c.id,
                who: "cleo@lab",
                author_kind: "person",
                model: None,
                picks: None,
            },
            &at(20),
        )
        .unwrap();
        assert_eq!(closed.decisions.len(), 2, "{name}: {closed:?}");
        assert_eq!(closed.resolved, 2, "{name}");
        assert_eq!(closed.agreement["exact"], 1.0, "{name}: {closed:?}");
        // the decisions are the person's who closed it, and every answer stays
        let who = select(reg, |s| {
            format!(
                "SELECT actor, author_kind, value FROM {} WHERE withdrawn_at IS NULL ORDER BY id",
                s.qualified("decision")
            )
        });
        assert_eq!(who[0].text(0).unwrap(), "cleo@lab", "{name}");
        assert_eq!(who[0].text(1).unwrap(), "person", "{name}");
        assert_eq!(who[0].text(2).unwrap(), "brain", "{name}");
        assert_eq!(who[1].text(2).unwrap(), "spine", "{name}");
        assert_eq!(count(reg, "campaign_answer", ""), 4, "{name}");
        // record 42 S1's columns: the campaign each came from, and who put
        // it in force, its own author, since it was written in force
        let from = c.id;
        let rows = select(reg, |s| {
            format!(
                "SELECT campaign_id, committed_by FROM {} ORDER BY id",
                s.qualified("decision")
            )
        });
        for r in &rows {
            assert_eq!(r.opt_int(0).unwrap(), Some(from), "{name}");
            assert_eq!(r.text(1).unwrap(), "cleo@lab", "{name}");
        }
        // the review items are closed by the decisions
        for it in campaign::items(reg.store(), c.id).unwrap() {
            let r = nils_registry::review::item(reg.store(), it.review_item_id)
                .unwrap()
                .unwrap();
            assert_eq!(r.status, "accepted", "{name}");
            assert_eq!(it.state, "resolved", "{name}");
        }
        // a closed campaign takes no more claims
        assert!(campaign::claim(reg, c.id, "fay@lab", Role::Rater, &at(21)).is_err());
    }
}

/// S6: three raters who disagree give one adjudicator assignment and
/// exactly one decision per item, and every answer is kept.
#[test]
fn three_raters_who_disagree_give_one_adjudicator_and_one_decision() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 1);
        let q = body_part();
        let adj = json!({"when": "disagree", "metric": "kappa"});
        let mut n = new(
            "disagree",
            &q,
            &adj,
            Items::Stacks(ids[..1].to_vec()),
            3,
            "decision",
        );
        n.adjudicators = vec!["judge@lab".into()];
        let c = campaign::create(reg, &n).unwrap();
        let mut opened = Vec::new();
        for (who, value) in [
            ("anna@lab", "brain"),
            ("bo@lab", "brain"),
            ("cy@lab", "neck"),
        ] {
            let a = campaign::claim(reg, c.id, who, Role::Rater, &at(0))
                .unwrap()
                .unwrap();
            let done = campaign::answer(reg, &give(a.assignment.id, who, value), &at(1)).unwrap();
            opened.extend(done.adjudication);
        }
        assert_eq!(opened.len(), 1, "{name}: one adjudicator assignment");
        let adjudicators: Vec<_> = campaign::assignments(reg.store(), c.id)
            .unwrap()
            .into_iter()
            .filter(|a| a.role == "adjudicator")
            .collect();
        assert_eq!(adjudicators.len(), 1, "{name}");
        assert_eq!(
            adjudicators[0].principal.as_deref(),
            Some("judge@lab"),
            "{name}"
        );
        // the queue shows the item waiting for its adjudicator
        let it = &campaign::items(reg.store(), c.id).unwrap()[0];
        assert_eq!(it.state, "needs_adjudication", "{name}");
        let r = nils_registry::review::item(reg.store(), it.review_item_id)
            .unwrap()
            .unwrap();
        assert_eq!(r.evidence["campaign_state"], "needs_adjudication", "{name}");
        // a rater is not the adjudicator, and the policy names who is
        assert!(matches!(
            campaign::claim(reg, c.id, "anna@lab", Role::Adjudicator, &at(2)),
            Err(campaign::Error::Forbidden(_))
        ));
        let j = campaign::claim(reg, c.id, "judge@lab", Role::Adjudicator, &at(2))
            .unwrap()
            .unwrap();
        assert_eq!(j.assignment.round, 2, "{name}");
        let done = campaign::answer(
            reg,
            &give(j.assignment.id, "judge@lab", "brain-neck"),
            &at(3),
        )
        .unwrap();
        assert_eq!(done.state, "adjudicated", "{name}");
        let closed = campaign::close(
            reg,
            &Close {
                campaign: c.id,
                who: "cleo@lab",
                author_kind: "person",
                model: None,
                picks: None,
            },
            &at(4),
        )
        .unwrap();
        assert_eq!(closed.decisions.len(), 1, "{name}");
        let rows = select(reg, |s| {
            format!(
                "SELECT actor, value, why FROM {} WHERE withdrawn_at IS NULL",
                s.qualified("decision")
            )
        });
        assert_eq!(rows.len(), 1, "{name}: exactly one decision");
        assert_eq!(rows[0].text(0).unwrap(), "judge@lab", "{name}");
        assert_eq!(rows[0].text(1).unwrap(), "brain-neck", "{name}");
        assert!(
            rows[0]
                .text(2)
                .unwrap()
                .contains("adjudicated by judge@lab over 3"),
            "{name}"
        );
        assert_eq!(
            count(reg, "campaign_answer", ""),
            4,
            "{name}: every answer kept"
        );
        // no rater agreed with the adjudicator
        let it = &campaign::items(reg.store(), c.id).unwrap()[0];
        assert_eq!(it.agreement, Some(0.0), "{name}");
        assert_eq!(closed.agreement["exact"], 0.0, "{name}");
    }
}

/// S6: masks are compared by a number something with pixels computed; below
/// the threshold the item goes to an adjudicator, and the campaign closes
/// into nothing, its outcome the adjudicator's file.
#[test]
fn an_external_metric_sends_masks_to_adjudication_and_closes_into_nothing() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 1);
        let q = json!({"kind": "derivative", "derivative_kind": "mask", "form": {
            "properties": {"lesions": {"type": "integer"}}, "required": ["lesions"]}});
        let adj = json!({"when": "disagree", "metric": "external", "threshold": 0.8});
        // a mask question is never compared by the engine
        let exact = json!({"when": "disagree", "metric": "exact"});
        assert!(
            campaign::create(
                reg,
                &new("x", &q, &exact, Items::Stacks(ids.clone()), 2, "none")
            )
            .is_err()
        );
        // nor closed into a decision
        assert!(
            campaign::create(
                reg,
                &new("y", &q, &adj, Items::Stacks(ids.clone()), 2, "decision")
            )
            .is_err()
        );
        let c = campaign::create(
            reg,
            &new("segment", &q, &adj, Items::Stacks(ids.clone()), 2, "none"),
        )
        .unwrap();
        let form = json!({"lesions": 3});
        let mut item = 0;
        let mut files = Vec::new();
        for who in ["anna@lab", "bo@lab"] {
            let a = campaign::claim(reg, c.id, who, Role::Rater, &at(0))
                .unwrap()
                .unwrap();
            item = a.item.id;
            let stack = a.item.stack_id.unwrap();
            let other = *ids.iter().find(|s| **s != stack).unwrap();
            // a mask answer names its file
            assert!(campaign::answer(reg, &give(a.assignment.id, who, "x"), &at(1)).is_err());
            // a file the derivative door registered, of the kind asked, made
            // from the item's stack
            let refused = |reg: &mut Registry, file: i64, says: &str| {
                let e = campaign::answer(
                    reg,
                    &Given {
                        assignment: a.assignment.id,
                        principal: who,
                        author_kind: "person",
                        model: None,
                        value: None,
                        form: Some(&form),
                        derivative_id: Some(file),
                        why: None,
                    },
                    &at(1),
                )
                .unwrap_err()
                .to_string();
                assert!(e.contains(says), "{name}: {e}");
            };
            refused(reg, 9999, "no derivative 9999");
            let embedding = derivative(reg, other, "embedding");
            refused(reg, embedding, "asks for a mask");
            let elsewhere = derivative(reg, other, "mask");
            refused(reg, elsewhere, "was made from stack");
            let file = derivative(reg, stack, "mask");
            files.push(file);
            campaign::answer(
                reg,
                &Given {
                    assignment: a.assignment.id,
                    principal: who,
                    author_kind: "person",
                    model: None,
                    value: None,
                    form: Some(&form),
                    derivative_id: Some(file),
                    why: None,
                },
                &at(1),
            )
            .unwrap();
        }
        assert_eq!(
            campaign::item(reg.store(), item).unwrap().unwrap().state,
            "awaiting_metric"
        );
        let posted = campaign::post_metric(reg, item, "app@lab", "dice", 0.61, &at(2)).unwrap();
        assert_eq!(posted.state, "needs_adjudication", "{name}");
        let j = campaign::claim(reg, c.id, "judge@lab", Role::Adjudicator, &at(3))
            .unwrap()
            .unwrap();
        let union = derivative(reg, j.item.stack_id.unwrap(), "mask");
        files.push(union);
        campaign::answer(
            reg,
            &Given {
                assignment: j.assignment.id,
                principal: "judge@lab",
                author_kind: "person",
                model: None,
                value: None,
                form: Some(&form),
                derivative_id: Some(union),
                why: Some("the union, trimmed"),
            },
            &at(4),
        )
        .unwrap();
        let closed = campaign::close(
            reg,
            &Close {
                campaign: c.id,
                who: "cleo@lab",
                author_kind: "person",
                model: None,
                picks: None,
            },
            &at(5),
        )
        .unwrap();
        assert!(closed.decisions.is_empty(), "{name}");
        // the second stack was never rated and is left open in the queue
        assert_eq!((closed.resolved, closed.unresolved), (1, 1), "{name}");
        assert_eq!(count(reg, "decision", ""), 0, "{name}");
        let outcomes = labels::campaign_labels(reg.store(), c.id, Of::Outcomes).unwrap();
        assert_eq!(outcomes.len(), 1, "{name}");
        assert_eq!(outcomes[0].derivative_id, Some(union), "{name}");
        assert_eq!(outcomes[0].author, "judge@lab", "{name}");
        let answers = labels::campaign_labels(reg.store(), c.id, Of::Answers).unwrap();
        assert_eq!(answers.len(), 3, "{name}");
        let named: Vec<i64> = answers.iter().filter_map(|a| a.derivative_id).collect();
        assert_eq!(named, files, "{name}");
    }
}

/// S5: a campaign over review items adopts them as they are, and one open
/// campaign at a time asks each.
#[test]
fn a_campaign_adopts_review_items_and_one_campaign_asks_each() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 1);
        let now = nils_registry::time::now_iso();
        let store = reg.store();
        for (i, s) in ids.iter().enumerate() {
            row(
                store,
                "review_item",
                &[
                    (
                        "kind",
                        Param::from(if i == 0 {
                            "body_part:low_confidence"
                        } else {
                            "body_part:conflict"
                        }),
                    ),
                    ("scope", Param::from("stack")),
                    ("ref", Param::from(json!({"stack_id": s}).to_string())),
                    (
                        "evidence",
                        Param::from(json!({"axis": "body_part", "confidence": 0.4}).to_string()),
                    ),
                    ("status", Param::from("open")),
                    ("created_at", Param::from(now.as_str())),
                ],
            );
        }
        let q = body_part();
        let adj = json!({"when": "never"});
        let found = campaign::review_items(
            reg.store(),
            &campaign::ReviewQuery {
                kind_prefix: Some("body_part:".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(found.len(), 2, "{name}");
        let c = campaign::create(
            reg,
            &new("adopt", &q, &adj, Items::Review(found.clone()), 1, "stage"),
        )
        .unwrap();
        assert!(
            campaign::create(
                reg,
                &new("twice", &q, &adj, Items::Review(found.clone()), 1, "stage")
            )
            .is_err(),
            "{name}"
        );
        let it = &campaign::items(reg.store(), c.id).unwrap()[0];
        assert_eq!(it.review_item_id, found[0], "{name}");
        assert_eq!(it.stack_id, Some(ids[0]), "{name}");
        let a = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(0))
            .unwrap()
            .unwrap();
        campaign::answer(reg, &give(a.assignment.id, "anna@lab", "spine"), &at(1)).unwrap();
        let closed = campaign::close(
            reg,
            &Close {
                campaign: c.id,
                who: "cleo@lab",
                author_kind: "person",
                model: None,
                picks: None,
            },
            &at(2),
        )
        .unwrap();
        assert!(closed.staged, "{name}");
        assert_eq!(closed.decisions.len(), 1, "{name}");
        // staged: written, not in force, the adopted item staged by it
        let r = nils_registry::review::item(reg.store(), found[0])
            .unwrap()
            .unwrap();
        assert_eq!(r.status, "staged", "{name}");
        assert_eq!(
            count(reg, "decision", " WHERE committed_at IS NULL"),
            1,
            "{name}"
        );
    }
}

/// S7: the same state gives the same digest, one new decision changes it,
/// and every row resolves to a decision and its author.
#[test]
fn a_label_set_is_the_decisions_in_force_and_its_bytes_follow_them() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 2);
        let q = body_part();
        let adj = json!({"when": "never"});
        let c = campaign::create(
            reg,
            &new(
                "labels",
                &q,
                &adj,
                Items::Stacks(ids[..3].to_vec()),
                1,
                "decision",
            ),
        )
        .unwrap();
        for (i, v) in ["brain", "brain", "spine"].iter().enumerate() {
            let a = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(i as u32)).unwrap();
            let a = match a {
                Some(a) => a,
                None => panic!("{name}: nothing to claim"),
            };
            campaign::answer(reg, &give(a.assignment.id, "anna@lab", v), &at(i as u32)).unwrap();
        }
        campaign::close(
            reg,
            &Close {
                campaign: c.id,
                who: "cleo@lab",
                author_kind: "person",
                model: None,
                picks: None,
            },
            &at(9),
        )
        .unwrap();
        let q = DecisionQuery {
            axis: "body_part",
            ..Default::default()
        };
        let first = labels::decision_labels(reg.store(), &q).unwrap();
        assert_eq!(first.len(), 3, "{name}");
        for l in &first {
            assert!(l.decision_id.is_some(), "{name}");
            assert_eq!(l.author, "cleo@lab", "{name}");
            assert_eq!(l.campaign_id, Some(c.id), "{name}");
            assert!(l.subject_id.is_some(), "{name}");
        }
        let one = labels::tsv(&first);
        let again = labels::tsv(&labels::decision_labels(reg.store(), &q).unwrap());
        assert_eq!(one, again, "{name}: the same state, the same bytes");
        // a person's decision on the fourth stack changes the set
        let now = nils_registry::time::now_iso();
        let item = row(
            reg.store(),
            "review_item",
            &[
                ("kind", Param::from("body_part:missing")),
                ("scope", Param::from("stack")),
                ("ref", Param::from(json!({"stack_id": ids[3]}).to_string())),
                (
                    "evidence",
                    Param::from(json!({"axis": "body_part"}).to_string()),
                ),
                ("status", Param::from("open")),
                ("created_at", Param::from(now.as_str())),
            ],
        );
        nils_registry::review::apply(
            reg,
            &nils_registry::review::Apply {
                item,
                member: None,
                scope: "stack",
                value: Some("neck"),
                author: nils_registry::review::Author {
                    who: "dan@lab",
                    kind: "person",
                    version: None,
                    model: None,
                },
                stage: false,
                why: None,
                campaign: None,
            },
        )
        .unwrap();
        let later = labels::tsv(&labels::decision_labels(reg.store(), &q).unwrap());
        assert_ne!(one, later, "{name}");
        // held to a campaign, to a frozen list, to an author kind
        let only = labels::decision_labels(
            reg.store(),
            &DecisionQuery {
                axis: "body_part",
                campaign: Some(c.id),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(only.len(), 3, "{name}");
        let few = [ids[0], ids[3]];
        let listed = labels::decision_labels(
            reg.store(),
            &DecisionQuery {
                axis: "body_part",
                stacks: Some(&few),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(listed.len(), 2, "{name}");
        let models = ["model".to_string()];
        let none = labels::decision_labels(
            reg.store(),
            &DecisionQuery {
                axis: "body_part",
                authors: &models,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(none.is_empty(), "{name}");
        // a sealed set is recorded, and refused for training
        let set = labels::record(
            reg,
            &labels::NewSet {
                name: "sealed-draw",
                version: 1,
                kind: "decisions",
                what: "body_part",
                source: json!({"axis": "body_part"}),
                campaign_id: None,
                handle_id: None,
                pack_version: None,
                scheme_digest: None,
                sealed: true,
                rows: 4,
                digest: "abc",
                place_id: None,
                path: None,
                created_by: "cleo@lab",
            },
        )
        .unwrap();
        assert!(
            labels::usable_for_training(reg.store(), set.id).is_err(),
            "{name}"
        );
        assert_eq!(set.as_json()["sealed"], true, "{name}");
    }
}

/// R5: v0's labels, by SeriesInstanceUID, become person decisions marked
/// imported from v0 and dated as v0 dated them; a person's decision that
/// already stands keeps its place; an import run twice writes nothing new.
#[test]
fn v0_labels_become_dated_person_decisions_marked_as_imported() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 2);
        // a person already decided the first stack of the second series
        let now = nils_registry::time::now_iso();
        let item = row(
            reg.store(),
            "review_item",
            &[
                ("kind", Param::from("body_part:missing")),
                ("scope", Param::from("stack")),
                ("ref", Param::from(json!({"stack_id": ids[2]}).to_string())),
                (
                    "evidence",
                    Param::from(json!({"axis": "body_part"}).to_string()),
                ),
                ("status", Param::from("open")),
                ("created_at", Param::from(now.as_str())),
            ],
        );
        nils_registry::review::apply(
            reg,
            &nils_registry::review::Apply {
                item,
                member: None,
                scope: "stack",
                value: Some("neck"),
                author: nils_registry::review::Author {
                    who: "dan@lab",
                    kind: "person",
                    version: None,
                    model: None,
                },
                stage: false,
                why: None,
                campaign: None,
            },
        )
        .unwrap();
        let (v0, bad) = labels::parse_v0(
            "SeriesInstanceUID\tbody_part\tdate\n\
             1.2.840.9.1\tBrain\t2024-05-06\n\
             1.2.840.9.2\tSpine\t2024-05-07T08:30:00Z\n\
             1.2.840.9.3\tBrain\t2024-05-07\n\
             1.2.840.9.1\tChest\t2024-05-07\n\
             1.2.840.9.2\tSpine\tnot a date\n",
        );
        assert_eq!((v0.len(), bad), (5, 0), "{name}");
        let allowed: Vec<String> = ["brain", "spine", "neck", "brain-neck"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let dry = labels::import_v0(reg, &v0, "body_part", &allowed, "cleo@lab", true).unwrap();
        assert!(dry.decisions.is_empty(), "{name}");
        assert_eq!(
            count(reg, "decision", ""),
            1,
            "{name}: a dry run writes nothing"
        );
        let done = labels::import_v0(reg, &v0, "body_part", &allowed, "cleo@lab", false).unwrap();
        assert_eq!(done.series_matched, 2, "{name}: {done:?}");
        assert_eq!(done.series_unmatched, 1, "{name}");
        assert_eq!(done.refused_values, 1, "{name}");
        assert_eq!(done.bad_dates, 1, "{name}");
        assert_eq!(done.held, 1, "{name}: the person's decision stands");
        assert_eq!(done.decisions.len(), 3, "{name}");
        let rows = select(reg, |s| {
            format!(
                "SELECT ref, value, actor, author_kind, author_version, {} FROM {} \
                 WHERE author_version = 'imported:v0' ORDER BY id",
                s.dialect()
                    .text_of(table("decision").column("decided_at").unwrap()),
                s.qualified("decision")
            )
        });
        assert_eq!(rows.len(), 3, "{name}");
        assert_eq!(rows[0].text(1).unwrap(), "brain", "{name}");
        assert_eq!(rows[0].text(3).unwrap(), "person", "{name}");
        assert_eq!(rows[0].text(5).unwrap(), "2024-05-06T00:00:00Z", "{name}");
        assert_eq!(rows[2].text(0).unwrap(), ids[3].to_string(), "{name}");
        assert_eq!(rows[2].text(5).unwrap(), "2024-05-07T08:30:00Z", "{name}");
        let again = labels::import_v0(reg, &v0, "body_part", &allowed, "cleo@lab", false).unwrap();
        assert!(again.decisions.is_empty(), "{name}");
        assert_eq!(again.already, 3, "{name}");
        let set = labels::imported_labels(reg.store(), "body_part").unwrap();
        assert_eq!(set.len(), 3, "{name}");
        assert!(
            set.iter()
                .all(|l| l.author == "imported:v0" && l.decision_id.is_some())
        );
    }
}

/// S6: a close into staged decisions, then a commit by a minimum confidence
/// that takes only its part and leaves the rest staged; a commit by filter
/// that names no filter is refused.
#[test]
fn a_commit_by_minimum_confidence_commits_only_its_part() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 2);
        let q = body_part();
        let adj = json!({"when": "disagree", "metric": "exact"});
        let c = campaign::create(
            reg,
            &new("staged", &q, &adj, Items::Stacks(ids.clone()), 2, "stage"),
        )
        .unwrap();
        for i in 0..ids.len() {
            let other = if i == 0 { "spine" } else { "brain" };
            for (who, value) in [("anna@lab", "brain"), ("bo@lab", other)] {
                let a = campaign::claim(reg, c.id, who, Role::Rater, &at(i as u32))
                    .unwrap()
                    .unwrap();
                campaign::answer(reg, &give(a.assignment.id, who, value), &at(i as u32)).unwrap();
            }
        }
        let j = campaign::claim(reg, c.id, "judge@lab", Role::Adjudicator, &at(8))
            .unwrap()
            .unwrap();
        campaign::answer(reg, &give(j.assignment.id, "judge@lab", "brain"), &at(8)).unwrap();
        let closed = campaign::close(
            reg,
            &Close {
                campaign: c.id,
                who: "cleo@lab",
                author_kind: "person",
                model: None,
                picks: None,
            },
            &at(9),
        )
        .unwrap();
        assert_eq!(closed.decisions.len(), ids.len(), "{name}");
        assert!(
            nils_registry::review::commit_where(
                reg,
                &nils_registry::review::CommitFilter::default(),
                false,
                "cleo@lab",
                "person",
            )
            .is_err(),
            "{name}"
        );
        let done = nils_registry::review::commit_where(
            reg,
            &nils_registry::review::CommitFilter {
                min_confidence: Some(0.9),
                campaign: Some(c.id),
            },
            false,
            "cleo@lab",
            "person",
        )
        .unwrap();
        assert_eq!(done.decisions.len(), ids.len() - 1, "{name}: {done:?}");
        assert_eq!(done.left, 1, "{name}");
        assert_eq!(
            count(reg, "decision", " WHERE committed_at IS NULL"),
            1,
            "{name}: the adjudicated one, half its raters behind it, stays staged"
        );
        let by = select(reg, |s| {
            format!(
                "SELECT DISTINCT committed_by FROM {} WHERE committed_at IS NOT NULL",
                s.qualified("decision")
            )
        });
        assert_eq!(by.len(), 1, "{name}");
        assert_eq!(by[0].text(0).unwrap(), "cleo@lab", "{name}");
        // the confidence is the item's agreement: one rater of two
        let it = &campaign::items(reg.store(), c.id).unwrap()[0];
        assert_eq!(it.agreement, Some(0.5), "{name}");
    }
}

/// S6: a pick question is asked of sessions and closes into a person's
/// pick of the role through the writer the engine passes in, which is
/// record 42 S3's (`nils_classify::picking::set_person`, proved in the
/// engine's own tests): the close hands it the role, the scheme, the
/// item's occasion, the stacks, the person and the campaign, and keeps the
/// pick it answers on the item. A close without a writer, or an answer
/// that is not a person's, leaves the item unresolved and says why.
#[test]
fn a_pick_campaign_closes_into_a_persons_pick() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 2);
        let subject = select(reg, |s| {
            format!(
                "SELECT r.subject_id FROM {} k JOIN {} r ON r.id = k.series_id WHERE k.id = {}",
                s.qualified("stack"),
                s.qualified("series"),
                ids[0]
            )
        })[0]
            .int(0)
            .unwrap();
        let q = json!({"kind": "pick", "role": "t1w"});
        let adj = json!({"when": "never"});
        // a pick is asked of sessions, not stacks
        assert!(
            campaign::create(
                reg,
                &new("wrong", &q, &adj, Items::Stacks(ids.clone()), 1, "pick")
            )
            .is_err(),
            "{name}"
        );
        let ask = |reg: &mut Registry, c: &str| {
            let made = campaign::create(
                reg,
                &new(
                    c,
                    &q,
                    &adj,
                    Items::Sessions(vec![(subject, "2026-01-01".into())]),
                    1,
                    "pick",
                ),
            )
            .unwrap();
            assert_eq!(made.grain, "session", "{name}");
            let a = campaign::claim(reg, made.id, "anna@lab", Role::Rater, &at(0))
                .unwrap()
                .unwrap();
            assert_eq!(a.item.subject_id, Some(subject), "{name}");
            let pick = format!("{},{}", ids[1], ids[0]);
            campaign::answer(reg, &give(a.assignment.id, "anna@lab", &pick), &at(1)).unwrap();
            made
        };

        // without a writer the item stays unresolved, and says why
        let c = ask(reg, "first");
        let closed = campaign::close(
            reg,
            &Close {
                campaign: c.id,
                who: "cleo@lab",
                author_kind: "person",
                model: None,
                picks: None,
            },
            &at(2),
        )
        .unwrap();
        assert!(closed.picks.is_empty(), "{name}");
        assert_eq!(closed.unresolved, 1, "{name}");
        assert!(closed.refused[0].1.contains("pick writer"), "{name}");

        // an agent closing is refused a pick: an agent's answer is evidence
        let c = ask(reg, "second");
        let never = |_: &mut Registry, _: &campaign::PickAsk<'_>| -> Result<i64, String> {
            panic!("an agent's close never reaches the writer")
        };
        let closed = campaign::close(
            reg,
            &Close {
                campaign: c.id,
                who: "bot@lab",
                author_kind: "agent",
                model: None,
                picks: Some(&never),
            },
            &at(2),
        )
        .unwrap();
        assert!(closed.picks.is_empty(), "{name}");
        assert!(
            closed.refused[0].1.contains("a pick is a person's"),
            "{name}"
        );

        // with the writer, the pick it wrote is the item's
        let c = ask(reg, "third");
        let asked = std::cell::RefCell::new(Vec::new());
        let writer = |_: &mut Registry, a: &campaign::PickAsk<'_>| -> Result<i64, String> {
            asked.borrow_mut().push(json!({
                "role": a.role, "scheme": a.scheme, "subject": a.subject_id,
                "day": a.session_day, "stacks": a.stacks, "who": a.who,
                "why": a.why, "campaign": a.campaign,
            }));
            Ok(4242)
        };
        let closed = campaign::close(
            reg,
            &Close {
                campaign: c.id,
                who: "cleo@lab",
                author_kind: "person",
                model: None,
                picks: Some(&writer),
            },
            &at(2),
        )
        .unwrap();
        assert_eq!(closed.picks, [4242], "{name}: {closed:?}");
        assert_eq!(closed.resolved, 1, "{name}");
        let asked = asked.into_inner();
        assert_eq!(asked.len(), 1, "{name}");
        let a = &asked[0];
        assert_eq!(a["role"], "t1w", "{name}");
        assert_eq!(a["scheme"], "default", "{name}");
        assert_eq!(a["subject"], subject, "{name}");
        assert_eq!(a["day"], "2026-01-01", "{name}");
        let mut want = vec![ids[0], ids[1]];
        want.sort_unstable();
        assert_eq!(a["stacks"], json!(want), "{name}");
        assert_eq!(a["who"], "cleo@lab", "{name}");
        assert_eq!(a["campaign"], c.id, "{name}");
        assert!(a["why"].as_str().unwrap().contains("third"), "{name}: {a}");
        let it = &campaign::items(reg.store(), c.id).unwrap()[0];
        assert_eq!(it.pick_id, Some(4242), "{name}");
        assert_eq!(it.state, "resolved", "{name}");
        assert_eq!(
            count(reg, "decision", ""),
            0,
            "{name}: a pick is not a decision"
        );
    }
}

/// Record 42 S2 with S6: a model that answers in a campaign names its
/// registered model, and the close carries it onto the decision, which is
/// staged, as a model's answer always is (R6). A model's answer without its
/// model, or a person's naming one, is refused before it is written.
#[test]
fn a_model_s_answer_keeps_its_model_on_the_decision() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 1);
        let digest = format!("sha256:{}", "b".repeat(64));
        let model = nils_registry::model::register(
            reg,
            &json!({"name": "bp", "version": "1", "kind": "pass", "digest": digest, "task": "axis:body_part"}),
            "anna@lab",
        )
        .unwrap();
        nils_registry::model::admit(
            reg,
            model.id,
            &json!({"suite": "heldout", "passed": true, "checks": [{"name": "ece", "passed": true}]}),
            "anna@lab",
        )
        .unwrap();
        let q = json!({"kind": "axis", "axis": "body_part", "values": ["brain", "spine"]});
        let adj = json!({"when": "always"});
        let c = campaign::create(
            reg,
            &new("bp", &q, &adj, Items::Stacks(vec![ids[0]]), 1, "decision"),
        )
        .unwrap();
        let a = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(0))
            .unwrap()
            .unwrap();
        campaign::answer(reg, &give(a.assignment.id, "anna@lab", "brain"), &at(1)).unwrap();
        let j = campaign::claim(reg, c.id, "bp@lab", Role::Adjudicator, &at(2))
            .unwrap()
            .unwrap();
        let by = |kind: &'static str, model: Option<i64>| Given {
            assignment: j.assignment.id,
            principal: "bp@lab",
            author_kind: kind,
            model,
            value: Some("brain"),
            form: None,
            derivative_id: None,
            why: None,
        };
        let e = campaign::answer(reg, &by("model", None), &at(3)).unwrap_err();
        assert!(
            e.to_string().contains("names the registered model"),
            "{name}: {e}"
        );
        let e = campaign::answer(reg, &by("person", Some(model.id)), &at(3)).unwrap_err();
        assert!(e.to_string().contains("not a model"), "{name}: {e}");
        let e = campaign::answer(reg, &by("model", Some(9999)), &at(3)).unwrap_err();
        assert!(
            e.to_string().contains("no registered model 9999"),
            "{name}: {e}"
        );
        campaign::answer(reg, &by("model", Some(model.id)), &at(3)).unwrap();
        let kept = campaign::answers(reg.store(), c.id).unwrap();
        assert_eq!(kept.last().unwrap().model_id, Some(model.id), "{name}");
        let closed = campaign::close(
            reg,
            &Close {
                campaign: c.id,
                who: "cleo@lab",
                author_kind: "person",
                model: None,
                picks: None,
            },
            &at(4),
        )
        .unwrap();
        assert_eq!(closed.decisions.len(), 1, "{name}: {closed:?}");
        let rows = select(reg, |s| {
            format!(
                "SELECT author_kind, model_id, campaign_id, staged_at IS NOT NULL, committed_at IS NULL FROM {}",
                s.qualified("decision")
            )
        });
        assert_eq!(rows[0].text(0).unwrap(), "model", "{name}");
        assert_eq!(rows[0].opt_int(1).unwrap(), Some(model.id), "{name}");
        assert_eq!(rows[0].opt_int(2).unwrap(), Some(c.id), "{name}");
        assert_eq!(rows[0].int(3).unwrap(), 1, "{name}: staged (R6)");
        assert_eq!(rows[0].int(4).unwrap(), 1, "{name}: not in force");
        // R6 by filter as by id: an agent or a model does not put a model's
        // answer in force, and nothing is committed when it is refused
        let filter = nils_registry::review::CommitFilter {
            min_confidence: None,
            campaign: Some(c.id),
        };
        for kind in ["agent", "model"] {
            let e = nils_registry::review::commit_where(reg, &filter, true, "bot@lab", kind)
                .unwrap_err();
            assert!(e.to_string().contains("R6"), "{name}: {e}");
        }
        assert_eq!(
            count(reg, "decision", " WHERE committed_at IS NOT NULL"),
            0,
            "{name}"
        );
        let done =
            nils_registry::review::commit_where(reg, &filter, true, "cleo@lab", "person").unwrap();
        assert_eq!(done.decisions.len(), 1, "{name}");
    }
}

/// Record 42 R6 at the close: an item a model's answer settled is written
/// staged, whoever closes, and only a person commits it. Two answers of
/// one model that agree keep the model as the author; a person and a model
/// who agree keep the person closing as the author, and the model's answer
/// on the item still holds the commit to a person. A close by an agent is
/// staged whoever rated, and two persons closed by a person stay in force.
#[test]
fn a_close_stages_what_a_model_answered_and_what_an_agent_closed() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 2);
        let digest = format!("sha256:{}", "c".repeat(64));
        let model = nils_registry::model::register(
            reg,
            &json!({"name": "bp", "version": "1", "kind": "pass", "digest": digest, "task": "axis:body_part"}),
            "anna@lab",
        )
        .unwrap();
        nils_registry::model::admit(
            reg,
            model.id,
            &json!({"suite": "heldout", "passed": true, "checks": [{"name": "ece", "passed": true}]}),
            "anna@lab",
        )
        .unwrap();
        let q = body_part();
        let adj = json!({"when": "disagree", "metric": "exact"});
        let as_model = |assignment: i64, who: &'static str| Given {
            assignment,
            principal: who,
            author_kind: "model",
            model: Some(model.id),
            value: Some("brain"),
            form: None,
            derivative_id: None,
            why: None,
        };
        let close = |reg: &mut Registry, id: i64, who: &str, kind: &str, minute: u32| {
            campaign::close(
                reg,
                &Close {
                    campaign: id,
                    who,
                    author_kind: kind,
                    model: None,
                    picks: None,
                },
                &at(minute),
            )
            .unwrap()
        };
        let decision = |reg: &mut Registry, id: i64| {
            let sql = format!(
                "SELECT author_kind, actor, model_id, staged_at IS NOT NULL, committed_at IS NULL FROM {} WHERE id = {id}",
                reg.store().qualified("decision")
            );
            let r = reg.store().query(&sql, &[]).unwrap().remove(0);
            (
                r.text(0).unwrap().to_string(),
                r.text(1).unwrap().to_string(),
                r.opt_int(2).unwrap(),
                r.int(3).unwrap() == 1,
                r.int(4).unwrap() == 1,
            )
        };

        // one model twice, agreeing, closed by a person: the model's, staged
        let c = campaign::create(
            reg,
            &new(
                "models",
                &q,
                &adj,
                Items::Stacks(vec![ids[0]]),
                2,
                "decision",
            ),
        )
        .unwrap();
        for who in ["bp-1@lab", "bp-2@lab"] {
            let a = campaign::claim(reg, c.id, who, Role::Rater, &at(0))
                .unwrap()
                .unwrap();
            campaign::answer(reg, &as_model(a.assignment.id, who), &at(1)).unwrap();
        }
        let closed = close(reg, c.id, "cleo@lab", "person", 2);
        assert_eq!(closed.decisions.len(), 1, "{name}: {closed:?}");
        assert!(closed.staged, "{name}");
        let (kind, _, m, staged, open) = decision(reg, closed.decisions[0]);
        assert_eq!(kind, "model", "{name}");
        assert_eq!(m, Some(model.id), "{name}");
        assert!(staged && open, "{name}: staged, not in force (R6)");

        // a person and the model, agreeing, closed by a person: the
        // person's, staged, and an agent may not commit it
        let c = campaign::create(
            reg,
            &new(
                "mixed",
                &q,
                &adj,
                Items::Stacks(vec![ids[1]]),
                2,
                "decision",
            ),
        )
        .unwrap();
        let a = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(3))
            .unwrap()
            .unwrap();
        campaign::answer(reg, &give(a.assignment.id, "anna@lab", "brain"), &at(3)).unwrap();
        let a = campaign::claim(reg, c.id, "bp-1@lab", Role::Rater, &at(3))
            .unwrap()
            .unwrap();
        campaign::answer(reg, &as_model(a.assignment.id, "bp-1@lab"), &at(3)).unwrap();
        let closed = close(reg, c.id, "cleo@lab", "person", 4);
        let mixed = closed.decisions[0];
        let (kind, actor, m, staged, open) = decision(reg, mixed);
        assert_eq!(
            (kind.as_str(), actor.as_str()),
            ("person", "cleo@lab"),
            "{name}"
        );
        assert_eq!(m, None, "{name}");
        assert!(staged && open, "{name}: a model answered, so it is staged");
        let e = nils_registry::review::commit_as(reg, Some(mixed), true, "bot@lab", "agent")
            .unwrap_err();
        assert!(e.to_string().contains("R6"), "{name}: {e}");
        nils_registry::review::commit_as(reg, Some(mixed), true, "cleo@lab", "person").unwrap();

        // a person and an agent, agreeing, closed by a person: staged as a
        // model's would be (R6 holds for agents), and only a person commits
        let c = campaign::create(
            reg,
            &new(
                "with-agent",
                &q,
                &adj,
                Items::Stacks(vec![ids[0]]),
                2,
                "decision",
            ),
        )
        .unwrap();
        let a = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(4))
            .unwrap()
            .unwrap();
        campaign::answer(reg, &give(a.assignment.id, "anna@lab", "brain"), &at(4)).unwrap();
        let a = campaign::claim(reg, c.id, "helper@lab", Role::Rater, &at(4))
            .unwrap()
            .unwrap();
        campaign::answer(
            reg,
            &Given {
                author_kind: "agent",
                ..give(a.assignment.id, "helper@lab", "brain")
            },
            &at(4),
        )
        .unwrap();
        let closed = close(reg, c.id, "cleo@lab", "person", 5);
        let with_agent = closed.decisions[0];
        let (kind, _, _, staged, open) = decision(reg, with_agent);
        assert_eq!(kind, "person", "{name}");
        assert!(staged && open, "{name}: an agent answered, so it is staged");
        let filter = nils_registry::review::CommitFilter {
            min_confidence: None,
            campaign: Some(c.id),
        };
        let e = nils_registry::review::commit_where(reg, &filter, true, "bot@lab", "agent")
            .unwrap_err();
        assert!(e.to_string().contains("R6"), "{name}: {e}");
        let e = nils_registry::review::commit_as(reg, Some(with_agent), true, "bot@lab", "agent")
            .unwrap_err();
        assert!(e.to_string().contains("R6"), "{name}: {e}");
        // the rule does not hang on the item's link to its decision alone:
        // a close that died before it wrote the link still holds it
        let unlink = format!(
            "UPDATE {} SET decision_id = NULL WHERE campaign_id = {}",
            reg.store().qualified("campaign_item"),
            c.id
        );
        reg.store().batch(&unlink).unwrap();
        let e = nils_registry::review::commit_as(reg, Some(with_agent), true, "bot@lab", "agent")
            .unwrap_err();
        assert!(e.to_string().contains("R6"), "{name}: unlinked: {e}");
        nils_registry::review::commit_where(reg, &filter, true, "cleo@lab", "person").unwrap();

        // two persons, closed by an agent: staged
        let c = campaign::create(
            reg,
            &new(
                "by-agent",
                &q,
                &adj,
                Items::Stacks(vec![ids[2]]),
                2,
                "decision",
            ),
        )
        .unwrap();
        for who in ["anna@lab", "bo@lab"] {
            let a = campaign::claim(reg, c.id, who, Role::Rater, &at(5))
                .unwrap()
                .unwrap();
            campaign::answer(reg, &give(a.assignment.id, who, "brain"), &at(5)).unwrap();
        }
        let closed = close(reg, c.id, "helper@lab", "agent", 6);
        let by_agent = closed.decisions[0];
        let (kind, _, _, staged, open) = decision(reg, by_agent);
        assert_eq!(kind, "agent", "{name}");
        assert!(staged && open, "{name}: an agent's close is staged");
        // and the agent does not put its own close in force
        let e = nils_registry::review::commit_as(reg, Some(by_agent), true, "helper@lab", "agent")
            .unwrap_err();
        assert!(e.to_string().contains("R6"), "{name}: {e}");

        // two persons, closed by a person: in force, as before
        let c = campaign::create(
            reg,
            &new(
                "people",
                &q,
                &adj,
                Items::Stacks(vec![ids[3]]),
                2,
                "decision",
            ),
        )
        .unwrap();
        for who in ["anna@lab", "bo@lab"] {
            let a = campaign::claim(reg, c.id, who, Role::Rater, &at(7))
                .unwrap()
                .unwrap();
            campaign::answer(reg, &give(a.assignment.id, who, "brain"), &at(7)).unwrap();
        }
        let closed = close(reg, c.id, "cleo@lab", "person", 8);
        assert!(!closed.staged, "{name}");
        let (kind, _, _, staged, _) = decision(reg, closed.decisions[0]);
        assert_eq!(kind, "person", "{name}");
        assert!(!staged, "{name}: in force");
    }
}

/// Hold the campaign's writers the way a second process would: a
/// transaction on another connection that holds the campaign's row (on
/// SQLite the immediate transaction holds the database), changes what it
/// is given, and lets go after the call on this side has read the state it
/// would act on.
fn while_held<T: Send + 'static>(
    home: &std::path::Path,
    campaign: i64,
    change: &str,
    call: impl FnOnce(&mut Registry) -> T + Send + 'static,
) -> T {
    let mut holder = Home::new(home).open().unwrap();
    let mut caller = Home::new(home).open().unwrap();
    let store = holder.store();
    store.begin().unwrap();
    if matches!(store, Store::Postgres { .. }) {
        let sql = format!(
            "SELECT id FROM {} WHERE id = {campaign} FOR UPDATE",
            store.qualified("campaign")
        );
        store.query(&sql, &[]).unwrap();
    }
    let t = std::thread::spawn(move || call(&mut caller));
    std::thread::sleep(std::time::Duration::from_millis(400));
    let sql = change
        .replace("{campaign}", &holder.store().qualified("campaign"))
        .replace(
            "{assignment}",
            &holder.store().qualified("campaign_assignment"),
        )
        .replace("{item}", &holder.store().qualified("campaign_item"));
    holder.store().batch(&sql).unwrap();
    holder.store().commit().unwrap();
    t.join().unwrap()
}

/// Record 42 S5 and S6 under a second writer: an answer and a metric read
/// the lease and the state again inside their transaction, so a lease that
/// ended or an item that moved on while they waited refuses them; an answer
/// is one per item, rater and round, and a repeat of the same answer on the
/// same assignment is answered again rather than written twice; and two
/// closes cannot both write, since a close takes the campaign from open to
/// closing before it writes anything.
#[test]
fn a_second_writer_never_gets_a_stale_answer_or_a_second_close_in() {
    for mut l in labs() {
        let name = l.name;
        let home = l._dir.path().to_path_buf();
        let reg = &mut l.registry;
        let ids = stacks(reg, 1);
        let q = body_part();
        let adj = json!({"when": "never", "metric": "exact"});
        let c = campaign::create(
            reg,
            &new("race", &q, &adj, Items::Stacks(ids.clone()), 1, "decision"),
        )
        .unwrap();
        let first = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(0))
            .unwrap()
            .unwrap();
        // the lease ends while the answer waits for the campaign
        let a = first.assignment.id;
        let e = while_held(
            &home,
            c.id,
            &format!("UPDATE {{assignment}} SET state = 'expired' WHERE id = {a}"),
            move |r| campaign::answer(r, &give(a, "anna@lab", "brain"), &at(1)).map(|x| x.answer),
        )
        .unwrap_err();
        assert!(e.to_string().contains("expired"), "{name}: {e}");
        assert_eq!(count(reg, "campaign_answer", ""), 0, "{name}");

        // a repeat of one answer on one assignment is the same answer
        let again = campaign::claim(reg, c.id, "bo@lab", Role::Rater, &at(2))
            .unwrap()
            .unwrap();
        let one =
            campaign::answer(reg, &give(again.assignment.id, "bo@lab", "brain"), &at(3)).unwrap();
        let two =
            campaign::answer(reg, &give(again.assignment.id, "bo@lab", "brain"), &at(3)).unwrap();
        assert_eq!(one.answer, two.answer, "{name}");
        let e = campaign::answer(reg, &give(again.assignment.id, "bo@lab", "spine"), &at(3))
            .unwrap_err();
        assert!(e.to_string().contains("answered"), "{name}: {e}");
        assert_eq!(count(reg, "campaign_answer", ""), 1, "{name}");
        // and the table holds one answer per item, rater and round
        let dup = format!(
            "INSERT INTO {} (campaign_id, item_id, assignment_id, principal, role, round, author_kind, answered_at) \
             SELECT campaign_id, item_id, assignment_id, principal, role, round, author_kind, answered_at FROM {}",
            reg.store().qualified("campaign_answer"),
            reg.store().qualified("campaign_answer")
        );
        assert!(reg.store().batch(&dup).is_err(), "{name}");

        // a close that waited behind another close writes nothing
        let id = c.id;
        let e = while_held(
            &home,
            c.id,
            "UPDATE {campaign} SET status = 'closed'",
            move |r| {
                campaign::close(
                    r,
                    &Close {
                        campaign: id,
                        who: "cleo@lab",
                        author_kind: "person",
                        model: None,
                        picks: None,
                    },
                    &at(4),
                )
                .map(|x| x.decisions.len())
            },
        );
        assert!(e.is_err(), "{name}: {e:?}");
        assert_eq!(count(reg, "decision", ""), 0, "{name}");
    }
}

/// Record 40 R3: a sample an operator sealed makes every label set holding
/// any of its items sealed, the registry's own finding and never a flag a
/// caller sets, and a model registered as trained on a sealed set, or on a
/// digest no set has, is refused.
#[test]
fn a_sealed_sample_seals_the_sets_that_hold_it_and_trains_nothing() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 2);
        let label = |stack: Option<i64>, subject: Option<i64>| labels::Label {
            stack_id: stack,
            subject_id: subject,
            what: "body_part".into(),
            value: Some("brain".into()),
            author_kind: "person".into(),
            author: "anna@lab".into(),
            ..labels::Label::default()
        };
        let drawn = vec![label(Some(ids[0]), None)];
        assert!(
            !labels::sealed_among(reg.store(), &drawn).unwrap(),
            "{name}"
        );
        let sealed = labels::seal(reg, "selection:draw@1", None, &ids[..2], "op@lab").unwrap();
        assert_eq!((sealed.stacks, sealed.already), (2, 0), "{name}");
        let again = labels::seal(reg, "selection:draw@1", None, &ids[..2], "op@lab").unwrap();
        assert_eq!((again.stacks, again.already), (0, 2), "{name}");
        assert!(labels::seal(reg, "selection:draw@1", None, &[999_999], "op@lab").is_err());
        assert!(labels::sealed_among(reg.store(), &drawn).unwrap(), "{name}");
        let other = vec![label(Some(ids[2]), None)];
        assert!(
            !labels::sealed_among(reg.store(), &other).unwrap(),
            "{name}"
        );
        // a session's label is sealed when its subject has a sealed stack
        let subject = select(reg, |s| {
            format!(
                "SELECT r.subject_id FROM {} k JOIN {} r ON r.id = k.series_id WHERE k.id = {}",
                s.qualified("stack"),
                s.qualified("series"),
                ids[0]
            )
        })[0]
            .int(0)
            .unwrap();
        let session = vec![label(None, Some(subject))];
        assert!(
            labels::sealed_among(reg.store(), &session).unwrap(),
            "{name}"
        );
        let set = |reg: &mut Registry, digest: &str, sealed: bool| {
            let version = labels::next_version(reg.store(), "bp").unwrap();
            labels::record(
                reg,
                &labels::NewSet {
                    name: "bp",
                    version,
                    kind: "decisions",
                    what: "body_part",
                    source: json!({}),
                    campaign_id: None,
                    handle_id: None,
                    pack_version: None,
                    scheme_digest: None,
                    sealed,
                    rows: 1,
                    digest,
                    place_id: None,
                    path: None,
                    created_by: "cleo@lab",
                },
            )
            .unwrap()
        };
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let sealed_set = labels::sealed_among(reg.store(), &drawn).unwrap();
        set(reg, &a, sealed_set);
        let open_set = labels::sealed_among(reg.store(), &other).unwrap();
        let second = set(reg, &b, open_set);
        assert_eq!(second.version, 2, "{name}: a name's next version");
        // a name and a version are one set
        let taken = labels::record(
            reg,
            &labels::NewSet {
                name: "bp",
                version: 1,
                kind: "decisions",
                what: "body_part",
                source: json!({}),
                campaign_id: None,
                handle_id: None,
                pack_version: None,
                scheme_digest: None,
                sealed: false,
                rows: 1,
                digest: &b,
                place_id: None,
                path: None,
                created_by: "cleo@lab",
            },
        );
        assert!(taken.is_err(), "{name}");
        let card = |digest: &str, trained: &str| {
            json!({"name": format!("h-{}", &digest[7..9]), "version": "1", "kind": "pass",
                   "digest": digest, "task": "axis:body_part",
                   "trained_on": {"label_set": format!("sha256:{trained}")}})
        };
        let e = nils_registry::model::register(
            reg,
            &card(&format!("sha256:{}", "1".repeat(64)), &"c".repeat(64)),
            "op@lab",
        )
        .unwrap_err();
        assert!(e.to_string().contains("no label set"), "{name}: {e}");
        let e = nils_registry::model::register(
            reg,
            &card(&format!("sha256:{}", "2".repeat(64)), &a),
            "op@lab",
        )
        .unwrap_err();
        assert!(e.to_string().contains("R3"), "{name}: {e}");
        nils_registry::model::register(
            reg,
            &card(&format!("sha256:{}", "3".repeat(64)), &b),
            "op@lab",
        )
        .unwrap();
        assert_eq!(count(reg, "audit", " WHERE action = 'labels.seal'"), 2);
    }
}

/// R5 with C15: a person's decision holds a stack at every scope that
/// covers it, not only its own: a series' or a subject's decision keeps
/// v0's label out as a stack's does, and an import that holds everything
/// writes nothing, not even the review items it would have answered.
#[test]
fn v0_labels_keep_out_of_a_person_s_decision_at_any_scope() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 2);
        let now = nils_registry::time::now_iso();
        for (stack, scope) in [(ids[0], "series"), (ids[2], "subject")] {
            let item = row(
                reg.store(),
                "review_item",
                &[
                    ("kind", Param::from("body_part:missing")),
                    ("scope", Param::from("stack")),
                    ("ref", Param::from(json!({"stack_id": stack}).to_string())),
                    (
                        "evidence",
                        Param::from(json!({"axis": "body_part"}).to_string()),
                    ),
                    ("status", Param::from("open")),
                    ("created_at", Param::from(now.as_str())),
                ],
            );
            nils_registry::review::apply(
                reg,
                &nils_registry::review::Apply {
                    item,
                    member: None,
                    scope,
                    value: Some("neck"),
                    author: nils_registry::review::Author {
                        who: "dan@lab",
                        kind: "person",
                        version: None,
                        model: None,
                    },
                    stage: false,
                    why: None,
                    campaign: None,
                },
            )
            .unwrap();
        }
        let items = count(reg, "review_item", "");
        let (v0, _) = labels::parse_v0(
            "SeriesInstanceUID\tbody_part\tdate\n\
             1.2.840.9.1\tBrain\t2024-05-06\n\
             1.2.840.9.2\tSpine\t2024-05-07\n",
        );
        let allowed: Vec<String> = vec!["brain".into(), "spine".into(), "neck".into()];
        let done = labels::import_v0(reg, &v0, "body_part", &allowed, "cleo@lab", false).unwrap();
        assert_eq!(done.stacks, 4, "{name}: {done:?}");
        assert_eq!(done.held, 4, "{name}: {done:?}");
        assert!(done.decisions.is_empty(), "{name}");
        assert_eq!(count(reg, "decision", ""), 2, "{name}");
        assert_eq!(count(reg, "review_item", ""), items, "{name}");
    }
}

/// A close that died part way leaves its campaign closing; a person runs
/// the close again, which writes the items not yet resolved and counts the
/// ones that were, and an agent or a model may not.
#[test]
fn a_close_that_died_is_run_again_by_a_person() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 1);
        let q = body_part();
        let adj = json!({"when": "never", "metric": "exact"});
        let c = campaign::create(
            reg,
            &new("again", &q, &adj, Items::Stacks(ids.clone()), 1, "decision"),
        )
        .unwrap();
        for _ in 0..ids.len() {
            let a = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(0))
                .unwrap()
                .unwrap();
            campaign::answer(reg, &give(a.assignment.id, "anna@lab", "brain"), &at(1)).unwrap();
        }
        let first = campaign::items(reg.store(), c.id).unwrap()[0].id;
        let died = format!(
            "UPDATE {} SET status = 'closing' WHERE id = {}; UPDATE {} SET state = 'resolved' WHERE id = {first}",
            reg.store().qualified("campaign"),
            c.id,
            reg.store().qualified("campaign_item"),
        );
        reg.store().batch(&died).unwrap();
        let by = |kind: &'static str| Close {
            campaign: c.id,
            who: "cleo@lab",
            author_kind: kind,
            model: None,
            picks: None,
        };
        let e = campaign::close(reg, &by("agent"), &at(2)).unwrap_err();
        assert!(e.to_string().contains("closing"), "{name}: {e}");
        let closed = campaign::close(reg, &by("person"), &at(3)).unwrap();
        assert_eq!(closed.decisions.len(), ids.len() - 1, "{name}: {closed:?}");
        assert_eq!(closed.resolved, ids.len() as i64, "{name}");
        assert_eq!(closed.unresolved, 0, "{name}");
        let status = campaign::get(reg.store(), c.id).unwrap().unwrap().status;
        assert_eq!(status, "closed", "{name}");
    }
}
