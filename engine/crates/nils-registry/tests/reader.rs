// SPDX-License-Identifier: AGPL-3.0-only

//! Record 48 on both backends: the reader's half in the registry (claims in
//! order of value, the time and the suggestion kept on an answer, an answer
//! given to a batch, a campaign's speed) and labels back into training (a
//! certificate recorded for a sealed sample, the sample unsealed by it, and
//! only then its labels usable for training).

use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::campaign::{self, Given, Items, New, Order, Role, Timing};
use nils_registry::home::{Home, InitOptions};
use nils_registry::labels;
use nils_registry::schema::{Type, table};
use nils_registry::{Backend, Insert, Param, Registry, Scheme, Store};
use serde_json::json;

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_reader_test";

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
    let dir = TempDir::new("reader-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-reader-test-key").unwrap();
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

fn voter(reg: &mut Registry, rule: &str, axis: &str) -> i64 {
    row(
        reg.store(),
        "classification_voter",
        &[
            ("pack", Param::from("mri")),
            ("pack_version", Param::from("0.3.0")),
            ("rule_set", Param::from("body_part")),
            ("rule", Param::from(rule)),
            ("clause", Param::Int(0)),
            ("axis", Param::from(axis)),
            ("tier", Param::from("keywords")),
            ("restates", Param::Int(0)),
        ],
    )
}

fn votes(reg: &mut Registry, stack: i64, pairs: &[(i64, &str)]) {
    let list: Vec<serde_json::Value> = pairs.iter().map(|(v, s)| json!([v, s])).collect();
    row(
        reg.store(),
        "classification_vote",
        &[
            ("stack_id", Param::Int(stack)),
            ("phase", Param::from("class")),
            ("votes", Param::from(json!(list).to_string())),
        ],
    );
}

fn sha256(text: &str) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, text.as_bytes()).as_ref())
}

fn body_part() -> serde_json::Value {
    json!({"kind": "axis", "axis": "body_part", "values": ["brain", "spine", "neck"]})
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

/// R1, order by value: the rules that disagree come first, then the least
/// confident, then the position; System 1's question, where it asked,
/// speaks for the stack instead of the rules.
#[test]
fn claims_in_order_of_value_take_disagreement_then_doubt_first() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 4);
        // 0: sure; 1: doubtful; 2: rules disagree; 3: System 1 asked, unsure
        classified(reg, ids[0], "body_part", "brain", 0.95);
        classified(reg, ids[1], "body_part", "brain", 0.55);
        classified(reg, ids[2], "body_part", "brain", 0.9);
        classified(reg, ids[3], "body_part", "brain", 0.99);
        let (a, b) = (
            voter(reg, "brain_words", "body_part"),
            voter(reg, "spine_words", "body_part"),
        );
        votes(reg, ids[0], &[(a, "brain")]);
        votes(reg, ids[2], &[(a, "brain"), (b, "spine")]);
        let evidence = json!({
            "axes": ["body_part"],
            "candidates": [
                {"values": {"body_part": "brain"}, "p": 0.6},
                {"values": {"body_part": "neck"}, "p": 0.3}
            ],
            "systems": {
                "rules": {"pack": "mri@0.3.0", "axes": {"body_part": {"value": "brain"}}},
                "model": {"model_id": 1, "p": {"body_part": {"brain": 0.6, "neck": 0.3}}}
            },
            "agree": ["body_part"],
            "certificate": {"risk_level": 0.05, "group": "all", "auto_decided": false},
            "confidence": 0.6
        });
        nils_registry::asked::raise(reg.store(), ids[3], &evidence, None, None).unwrap();

        let worth = campaign::worth(reg.store(), &ids, &["body_part".to_string()]).unwrap();
        assert!(worth[&ids[2]].disagree, "{name}");
        assert!(!worth[&ids[1]].disagree, "{name}");
        assert!(worth[&ids[3]].asked, "{name}");
        assert_eq!(worth[&ids[3]].confidence, 0.6, "{name}");
        assert_eq!(worth[&ids[1]].confidence, 0.55, "{name}");

        let q = body_part();
        let adj = json!({"when": "never"});
        let c =
            campaign::create(reg, &new("value", &q, &adj, Items::Stacks(ids.clone()), 1)).unwrap();
        let mut order = Vec::new();
        for i in 0..4 {
            let who = format!("r{i}@lab");
            // one rater per item: each claim takes the most valuable left
            let cl = campaign::claim_in(reg, c.id, &who, Role::Rater, Order::Value, &at(0, i))
                .unwrap()
                .unwrap();
            order.push(cl.item.stack_id.unwrap());
            campaign::answer(reg, &give(cl.assignment.id, &who, "brain"), &at(1, i)).unwrap();
        }
        assert_eq!(order, vec![ids[2], ids[1], ids[3], ids[0]], "{name}");
        // the order by position is unchanged
        let c2 = campaign::create(
            reg,
            &new("position", &q, &adj, Items::Stacks(ids.clone()), 1),
        )
        .unwrap();
        let first = campaign::claim(reg, c2.id, "p@lab", Role::Rater, &at(2, 0))
            .unwrap()
            .unwrap();
        assert_eq!(first.item.stack_id, Some(ids[0]), "{name}");
        assert!(Order::parse("sideways").is_err());
        // record 48 R2: a sealed stack is never ranked by what the systems
        // said of it: once stack 2 is sealed its disagreement no longer
        // puts it first, and stack 0, whose rules now disagree, comes first
        labels::seal(reg, "selection:s@1", None, &ids[2..3], "op@lab").unwrap();
        row(
            reg.store(),
            "classification_vote",
            &[
                ("stack_id", Param::Int(ids[0])),
                ("phase", Param::from("disposition")),
                ("votes", Param::from(json!([[b, "spine"]]).to_string())),
            ],
        );
        let c3 =
            campaign::create(reg, &new("sealed", &q, &adj, Items::Stacks(ids.clone()), 1)).unwrap();
        let first = campaign::claim_in(reg, c3.id, "z@lab", Role::Rater, Order::Value, &at(3, 0))
            .unwrap()
            .unwrap();
        assert_eq!(first.item.stack_id, Some(ids[0]), "{name}");
    }
}

/// R1, timing: an answer keeps the seconds from its claim, the suggestion
/// and whether it changed it; a campaign's stats give the median seconds
/// and the share changed per rater, and never a value.
#[test]
fn an_answer_is_timed_and_a_campaign_says_how_fast_it_is_read() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 3);
        let q = body_part();
        let adj = json!({"when": "never"});
        let c =
            campaign::create(reg, &new("timed", &q, &adj, Items::Stacks(ids.clone()), 1)).unwrap();
        // anna takes 90 seconds and keeps the suggestion
        let a = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(0, 0))
            .unwrap()
            .unwrap();
        campaign::answer_with(
            reg,
            &give(a.assignment.id, "anna@lab", "brain"),
            &Timing {
                suggested: Some(" brain "),
                suggested_by: None,
                batch: false,
                derived: None,
            },
            &at(1, 30),
        )
        .unwrap();
        // then 30 seconds, and changes it
        let a = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(2, 0))
            .unwrap()
            .unwrap();
        campaign::answer_with(
            reg,
            &give(a.assignment.id, "anna@lab", "spine"),
            &Timing {
                suggested: Some("brain"),
                suggested_by: None,
                batch: false,
                derived: None,
            },
            &at(2, 30),
        )
        .unwrap();
        // bo answers without a suggestion, after 10 seconds
        let b = campaign::claim(reg, c.id, "bo@lab", Role::Rater, &at(3, 0))
            .unwrap()
            .unwrap();
        campaign::answer(reg, &give(b.assignment.id, "bo@lab", "neck"), &at(3, 10)).unwrap();

        let all = campaign::answers(reg.store(), c.id).unwrap();
        assert_eq!(all[0].seconds, Some(90.0), "{name}");
        assert_eq!(all[0].suggested.as_deref(), Some("brain"), "{name}");
        assert_eq!(all[0].changed, Some(false), "{name}");
        assert_eq!(all[1].changed, Some(true), "{name}");
        assert_eq!(all[2].changed, None, "{name}");
        assert_eq!(all[2].via.as_deref(), Some("claim"), "{name}");

        let stats = campaign::stats(reg.store(), c.id).unwrap();
        let anna = stats["raters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["principal"] == "anna@lab")
            .unwrap();
        assert_eq!(anna["answers"], 2, "{name}");
        assert_eq!(anna["median_seconds"], 60.0, "{name}");
        assert_eq!(anna["p90_seconds"], 90.0, "{name}");
        assert_eq!(anna["suggested"], 2, "{name}");
        assert_eq!(anna["changed"], 1, "{name}");
        assert_eq!(anna["share_changed"], 0.5, "{name}");
        assert_eq!(stats["all"]["answers"], 3, "{name}");
        assert_eq!(stats["all"]["median_seconds"], 30.0, "{name}");
        // counts and times only
        for word in ["brain", "spine", "neck"] {
            assert!(!stats.to_string().contains(word), "{name}: {stats}");
        }
    }
}

/// R1, batches: an item of a batch is leased and answered in one move,
/// marked as given to a batch and not timed; it stays its own item, and a
/// rater never answers one item twice or one that has its raters.
#[test]
fn an_accepted_batch_answers_each_item_as_its_own() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 3);
        let q = body_part();
        let adj = json!({"when": "never"});
        let c =
            campaign::create(reg, &new("batch", &q, &adj, Items::Stacks(ids.clone()), 1)).unwrap();
        let items = campaign::items(reg.store(), c.id).unwrap();
        let g = give(0, "anna@lab", "brain");
        for it in &items[..2] {
            let done = campaign::accept(reg, c.id, it.id, &g, Some("brain"), &at(0, 0)).unwrap();
            assert_eq!(done.state, "agreed", "{name}");
        }
        let all = campaign::answers(reg.store(), c.id).unwrap();
        assert_eq!(all.len(), 2, "{name}");
        assert!(
            all.iter().all(|a| a.via.as_deref() == Some("batch")),
            "{name}"
        );
        assert!(all.iter().all(|a| a.seconds.is_none()), "{name}");
        assert!(all.iter().all(|a| a.changed == Some(false)), "{name}");
        // the item has its rater: nobody accepts it again
        assert!(
            campaign::accept(
                reg,
                c.id,
                items[0].id,
                &give(0, "bo@lab", "brain"),
                None,
                &at(0, 1)
            )
            .is_err()
        );
        // a value the question does not ask for leases nothing
        assert!(
            campaign::accept(
                reg,
                c.id,
                items[2].id,
                &give(0, "bo@lab", "hand"),
                None,
                &at(0, 2)
            )
            .is_err()
        );
        let leased = campaign::assignments(reg.store(), c.id)
            .unwrap()
            .into_iter()
            .filter(|a| a.item_id == items[2].id)
            .count();
        assert_eq!(leased, 0, "{name}");
        // an item of another campaign is not this one's
        let c2 =
            campaign::create(reg, &new("other", &q, &adj, Items::Stacks(ids.clone()), 1)).unwrap();
        assert!(campaign::accept(reg, c2.id, items[2].id, &g, None, &at(0, 3)).is_err());
        let stats = campaign::stats(reg.store(), c.id).unwrap();
        assert_eq!(stats["all"]["batched"], 2, "{name}");
        assert_eq!(stats["all"]["read"], 0, "{name}");
    }
}

/// R2: a sealed sample's labels train nothing until a certificate is
/// recorded for that sample and the sample unsealed by it; the rows stay
/// as history, and a set written while sealed is then read again.
#[test]
fn a_certificate_unseals_its_sample_and_only_then_its_labels_train() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 3);
        let model = row(
            reg.store(),
            "model",
            &[("digest", Param::from(format!("sha256:{}", "c".repeat(64))))],
        );
        labels::seal(reg, "selection:cert@1", None, &ids[..2], "op@lab").unwrap();
        let label = |stack: i64| labels::Label {
            stack_id: Some(stack),
            what: "body_part".into(),
            value: Some("brain".into()),
            author_kind: "person".into(),
            author: "anna@lab".into(),
            ..labels::Label::default()
        };
        let drawn: Vec<labels::Label> = ids.iter().map(|s| label(*s)).collect();
        // the set is written while its items are sealed
        let dir = TempDir::new("reader-set");
        let text = labels::tsv(&drawn);
        std::fs::write(dir.path().join("labels.tsv"), &text).unwrap();
        let digest = sha256(&text);
        let path = dir.path().display().to_string();
        let was_sealed = labels::sealed_among(reg.store(), &drawn).unwrap();
        let set = labels::record(
            reg,
            &labels::NewSet {
                name: "cert-draw",
                version: 1,
                kind: "decisions",
                what: "body_part",
                source: json!({}),
                campaign_id: None,
                handle_id: None,
                pack_version: None,
                scheme_digest: None,
                sealed: was_sealed,
                rows: 3,
                digest: &digest,
                place_id: None,
                path: Some(&path),
                created_by: "cleo@lab",
            },
        )
        .unwrap();
        assert!(set.sealed, "{name}");
        assert!(
            labels::usable_for_training(reg.store(), set.id).is_err(),
            "{name}"
        );
        let (kept, dropped) = labels::drop_sealed(reg.store(), drawn.clone()).unwrap();
        assert_eq!((kept.len(), dropped), (1, 2), "{name}");
        assert_eq!(
            labels::samples_of_set(reg.store(), &set).unwrap(),
            vec!["selection:cert@1".to_string()],
            "{name}"
        );

        // no certificate, no unseal
        assert!(
            labels::unseal(reg, "selection:cert@1", 999, "bo@lab", "person").is_err(),
            "{name}"
        );
        // a certificate names the sample it measured, its digest, the risk,
        // the size read and the errors found
        let digest = labels::seal(reg, "selection:cert@1", None, &ids[..2], "op@lab")
            .unwrap()
            .digest;
        assert!(digest.starts_with("sha256:"), "{name}");
        let result = |sample: &str, digest: &str| json!({"sample": sample, "sample_digest": digest, "risk": 0.05, "n": 2, "errors": 0});
        let good = result("selection:cert@1", &digest);
        let record = |reg: &mut Registry,
                      sample: &str,
                      models: &[i64],
                      r: &serde_json::Value,
                      who: &str,
                      kind: &str| {
            labels::record_certificate(reg, sample, models, r, who, kind)
        };
        assert!(
            record(
                reg,
                "selection:never@1",
                &[model],
                &good,
                "op@lab",
                "person"
            )
            .is_err()
        );
        assert!(
            record(
                reg,
                "selection:cert@1",
                &[model + 99],
                &good,
                "op@lab",
                "person"
            )
            .is_err()
        );
        assert!(record(reg, "selection:cert@1", &[], &good, "op@lab", "person").is_err());
        // an empty result, a wrong digest, another sample's name, no risk,
        // more stacks than the sample holds or more errors than read: refused
        for bad in [
            json!({}),
            result("selection:cert@1", "sha256:00"),
            result("selection:other@1", &digest),
            json!({"sample": "selection:cert@1", "sample_digest": digest, "n": 2, "errors": 0}),
            json!({"sample": "selection:cert@1", "sample_digest": digest, "risk": 0.05, "n": 9, "errors": 0}),
            json!({"sample": "selection:cert@1", "sample_digest": digest, "risk": 0.05, "n": 2, "errors": 3}),
        ] {
            assert!(
                record(reg, "selection:cert@1", &[model], &bad, "op@lab", "person").is_err(),
                "{name}: {bad}"
            );
        }
        // an agent's or a model's act is refused
        for kind in ["agent", "model"] {
            assert!(record(reg, "selection:cert@1", &[model], &good, "op@lab", kind).is_err());
        }
        let cert = record(reg, "selection:cert@1", &[model], &good, "op@lab", "person").unwrap();
        assert_eq!(cert.model_ids, vec![model], "{name}");
        assert_eq!(cert.result["risk"], 0.05, "{name}");
        // a certificate of another sample unseals nothing here
        let other_digest = labels::seal(reg, "selection:other@1", None, &ids[2..], "op@lab")
            .unwrap()
            .digest;
        let other = record(
            reg,
            "selection:other@1",
            &[model],
            &json!({"sample": "selection:other@1", "sample_digest": other_digest, "risk": 0.05, "n": 1, "errors": 0}),
            "op@lab",
            "person",
        )
        .unwrap();
        assert!(
            labels::unseal(reg, "selection:cert@1", other.id, "bo@lab", "person").is_err(),
            "{name}"
        );
        // who recorded it does not unseal, nor does an agent
        assert!(
            labels::unseal(reg, "selection:cert@1", cert.id, "op@lab", "person").is_err(),
            "{name}"
        );
        assert!(
            labels::unseal(reg, "selection:cert@1", cert.id, "bo@lab", "agent").is_err(),
            "{name}"
        );
        assert!(
            labels::sealed_among(reg.store(), &drawn[..1]).unwrap(),
            "{name}"
        );

        let done = labels::unseal(reg, "selection:cert@1", cert.id, "bo@lab", "person").unwrap();
        assert_eq!((done.stacks, done.already), (2, 0), "{name}");
        assert!(
            !labels::sealed_among(reg.store(), &drawn[..2]).unwrap(),
            "{name}"
        );
        // stack 3 is still sealed by the other sample
        assert!(
            labels::usable_for_training(reg.store(), set.id).is_err(),
            "{name}"
        );
        labels::unseal(reg, "selection:other@1", other.id, "bo@lab", "person").unwrap();
        let usable = labels::usable_for_training(reg.store(), set.id).unwrap();
        // a file that is not the one its digest names is not trusted
        std::fs::write(
            dir.path().join("labels.tsv"),
            text.replace("brain", "spine"),
        )
        .unwrap();
        assert!(
            labels::usable_for_training(reg.store(), set.id).is_err(),
            "{name}"
        );
        // nor is a set whose file is gone and which was written sealed
        std::fs::remove_file(dir.path().join("labels.tsv")).unwrap();
        assert!(
            labels::usable_for_training(reg.store(), set.id).is_err(),
            "{name}"
        );
        // the set keeps the flag it was written under, as history
        assert!(usable.sealed, "{name}");
        // the rows stay, naming the certificate
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE certificate_id = {} AND unsealed_by = 'bo@lab'",
            reg.store().qualified("sealed_stack"),
            cert.id
        );
        assert_eq!(
            reg.store().query(&sql, &[]).unwrap()[0].int(0).unwrap(),
            2,
            "{name}"
        );
        // unsealing again changes nothing
        let again = labels::unseal(reg, "selection:cert@1", cert.id, "bo@lab", "person").unwrap();
        assert_eq!((again.stacks, again.already), (0, 2), "{name}");
        assert_eq!(
            labels::certificates(reg.store()).unwrap().len(),
            2,
            "{name}"
        );
        let (kept, dropped) = labels::drop_sealed(reg.store(), drawn).unwrap();
        assert_eq!((kept.len(), dropped), (3, 0), "{name}");
    }
}

/// R2, the development labels: every decision in force by a person, on one
/// axis or every axis, leaving out the items of a sample sealed now and
/// taking them in once a certificate unseals the sample; a model's answer
/// and a staged decision are not a person's label.
#[test]
fn the_development_labels_are_every_person_decision_not_sealed_now() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 4);
        let decide = |reg: &mut Registry, stack: i64, axis: &str, kind: &str, staged: bool| {
            let mut given = vec![
                ("scope", Param::from("stack")),
                ("ref", Param::from(stack.to_string())),
                ("axis", Param::from(axis)),
                ("value", Param::from("brain")),
                ("actor", Param::from("anna@lab")),
                ("author_kind", Param::from(kind)),
                ("decided_at", Param::from("2026-09-24T10:00:00Z")),
            ];
            if staged {
                given.push(("staged_at", Param::from("2026-09-24T10:00:00Z")));
            }
            row(reg.store(), "decision", &given);
        };
        decide(reg, ids[0], "body_part", "person", false);
        decide(reg, ids[1], "body_part", "person", false);
        decide(reg, ids[2], "body_part", "model", false);
        decide(reg, ids[3], "body_part", "person", true);
        decide(reg, ids[0], "base", "person", false);
        labels::seal(reg, "selection:cert@1", None, &ids[1..2], "op@lab").unwrap();

        let (rows, left) = labels::training_labels(reg.store(), None, None, &[], None).unwrap();
        assert_eq!(left, 1, "{name}");
        let got: Vec<(i64, String)> = rows
            .iter()
            .map(|l| (l.stack_id.unwrap(), l.what.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                (ids[0], "base".to_string()),
                (ids[0], "body_part".to_string())
            ],
            "{name}"
        );
        let (one, _) =
            labels::training_labels(reg.store(), Some("body_part"), None, &[], None).unwrap();
        assert_eq!(one.len(), 1, "{name}");
        let models = ["model".to_string()];
        let (theirs, _) =
            labels::training_labels(reg.store(), Some("body_part"), None, &models, None).unwrap();
        assert_eq!(theirs.len(), 1, "{name}");
        assert_eq!(theirs[0].author_kind, "model", "{name}");

        let model = row(
            reg.store(),
            "model",
            &[("digest", Param::from(format!("sha256:{}", "d".repeat(64))))],
        );
        let digest = labels::sample_digest(reg.store(), "selection:cert@1").unwrap();
        let cert = labels::record_certificate(
            reg,
            "selection:cert@1",
            &[model],
            &json!({"sample": "selection:cert@1", "sample_digest": digest, "risk": 0.1, "n": 1, "errors": 0}),
            "op@lab",
            "person",
        )
        .unwrap();
        labels::unseal(reg, "selection:cert@1", cert.id, "bo@lab", "person").unwrap();
        let (rows, left) = labels::training_labels(reg.store(), None, None, &[], None).unwrap();
        assert_eq!((rows.len(), left), (3, 0), "{name}");
    }
}

/// R1: a lease taken now is timed to the millisecond, so an answer given
/// within the second it was claimed in still reads as a fraction, and the
/// stats keep a decimal.
#[test]
fn an_answer_given_now_is_timed_finer_than_the_second() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 1);
        let q = body_part();
        let adj = json!({"when": "never"});
        let c = campaign::create(reg, &new("now", &q, &adj, Items::Stacks(ids), 1)).unwrap();
        let now = nils_registry::time::now_iso();
        let a = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &now)
            .unwrap()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(150));
        let later = nils_registry::time::now_iso();
        campaign::answer(reg, &give(a.assignment.id, "anna@lab", "brain"), &later).unwrap();
        let seconds = campaign::answers(reg.store(), c.id).unwrap()[0]
            .seconds
            .unwrap();
        assert!((0.1..5.0).contains(&seconds), "{name}: {seconds}");
        assert!(seconds.fract() != 0.0, "{name}: {seconds}");
    }
}

/// R2: a set whose labels.tsv is gone is never usable by default: one that
/// pins no list says nothing, one that pins a list of stacks is held to the
/// seals on those stacks.
#[test]
fn a_set_whose_file_is_gone_is_held_to_what_the_registry_knows() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 2);
        let set = |reg: &mut Registry, handle: Option<i64>, n: i64| {
            labels::record(
                reg,
                &labels::NewSet {
                    name: "gone",
                    version: n,
                    kind: "decisions",
                    what: "body_part",
                    source: json!({}),
                    campaign_id: None,
                    handle_id: handle,
                    pack_version: None,
                    scheme_digest: None,
                    sealed: false,
                    rows: 2,
                    digest: "0",
                    place_id: None,
                    path: Some("/nonexistent/labels/gone"),
                    created_by: "cleo@lab",
                },
            )
            .unwrap()
        };
        let bare = set(reg, None, 1);
        assert!(
            labels::usable_for_training(reg.store(), bare.id).is_err(),
            "{name}"
        );
        let handle = row(
            reg.store(),
            "handle",
            &[("grain", Param::from("stack")), ("ask", Param::from("{}"))],
        );
        for (i, s) in ids.iter().enumerate() {
            row(
                reg.store(),
                "handle_member",
                &[
                    ("handle_id", Param::Int(handle)),
                    ("position", Param::Int(i as i64)),
                    ("key", Param::Int(*s)),
                ],
            );
        }
        let pinned = set(reg, Some(handle), 2);
        assert!(
            labels::usable_for_training(reg.store(), pinned.id).is_ok(),
            "{name}"
        );
        labels::seal(reg, "selection:s@1", None, &ids[..1], "op@lab").unwrap();
        assert!(
            labels::usable_for_training(reg.store(), pinned.id).is_err(),
            "{name}"
        );
    }
}

/// R1: a campaign holds back what its maker set, at least a tenth.
#[test]
fn a_campaign_holds_back_what_its_maker_set_and_no_less_than_a_tenth() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 2);
        let q = body_part();
        let adj = json!({"when": "never"});
        let mut n = new("low", &q, &adj, Items::Stacks(ids.clone()), 1);
        n.hold_back = Some(0.0);
        assert!(campaign::create(reg, &n).is_err(), "{name}");
        n.hold_back = Some(0.25);
        let c = campaign::create(reg, &n).unwrap();
        assert_eq!(c.hold_back, 0.25, "{name}");
        assert!(c.as_json().get("hold_back_seed").is_none(), "{name}");
        let seed = campaign::hold_back_seed(reg.store(), c.id).unwrap();
        assert!(seed.len() >= 32, "{name}");
        let d = campaign::create(reg, &new("default", &q, &adj, Items::Stacks(ids), 1)).unwrap();
        assert_eq!(d.hold_back, campaign::HOLD_BACK_MIN, "{name}");
        assert_ne!(
            campaign::hold_back_seed(reg.store(), d.id).unwrap(),
            seed,
            "{name}"
        );
    }
}
