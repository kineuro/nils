// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 ruling 31 (a) on both backends: a registered card gains the
//! claims it lacks, the label set it was trained on, its tested claims and
//! its speed. Additions only: a claim the card states already is refused,
//! and so is any other field; each claim is checked as registration checks
//! it, the label set among them (registered here, and clear of every sample
//! sealed now); the model's `trained_on` follows; each addition is an event
//! holding what was added and an audit row; a refused addition writes
//! nothing. Postgres runs when `NILS_TEST_POSTGRES_DSN` is set.

use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::home::{Home, InitOptions};
use nils_registry::labels::{self, NewSet};
use nils_registry::model::{self, Error};
use nils_registry::schema::{Type, table};
use nils_registry::{Backend, Insert, Param, Registry, Scheme, Store};
use serde_json::{Value, json};

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_card_claims_test";

fn postgres_dsn() -> Option<String> {
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => Some(dsn),
        _ => {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
            None
        }
    }
}

/// A registry on each backend: SQLite, and Postgres in a schema of its own.
fn registries() -> Vec<(String, Option<MutexGuard<'static, ()>>, TempDir, Registry)> {
    let init = |dir: &TempDir, backend: Backend, dsn: Option<String>| {
        let home = Home::new(dir.path());
        home.keys(None).add("k", b"nils-fixture-key").unwrap();
        home.init(&InitOptions {
            backend,
            dsn,
            schema: matches!(backend, Backend::Postgres).then(|| SCHEMA.to_string()),
            scheme: Scheme::Blake2b32,
            key: "k".to_string(),
            display_length: 12,
            session_scheme: None,
        })
        .unwrap()
    };
    let mut out = Vec::new();
    let dir = TempDir::new("claims-sqlite");
    let reg = init(&dir, Backend::Sqlite, None);
    out.push(("sqlite".to_string(), None, dir, reg));
    if let Some(dsn) = postgres_dsn() {
        let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = Store::connect_postgres(&dsn, SCHEMA).unwrap();
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
            ))
            .unwrap();
        drop(store);
        let dir = TempDir::new("claims-pg");
        let reg = init(&dir, Backend::Postgres, Some(dsn));
        out.push(("postgres".to_string(), Some(guard), dir, reg));
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

/// One stack of its own subject, study and series, by the series UID given:
/// its id.
fn stack(reg: &mut Registry, n: usize, series_uid: &str) -> i64 {
    let store = reg.store();
    let subject = row(store, "subject", &[("code", Param::from(format!("s{n}")))]);
    let study = row(
        store,
        "study",
        &[
            ("subject_id", Param::Int(subject)),
            ("study_instance_uid", Param::from(format!("{series_uid}.0"))),
        ],
    );
    let series = row(
        store,
        "series",
        &[
            ("subject_id", Param::Int(subject)),
            ("study_id", Param::Int(study)),
            ("series_instance_uid", Param::from(series_uid)),
        ],
    );
    row(
        store,
        "stack",
        &[
            ("series_id", Param::Int(series)),
            ("stack_index", Param::Int(0)),
            ("stack_key", Param::from(format!("{series_uid}#0"))),
        ],
    )
}

/// A label set registered from outside the engine (record 55 E6), its
/// canonical labels.tsv written where the set says it is: its digest.
fn outside_set(reg: &mut Registry, dir: &TempDir, name: &str, series_uid: &str) -> String {
    let tsv = format!(
        "{}\n{series_uid}.0\t{series_uid}\t{series_uid}#0\t\tbody_part\tbrain\tperson\tann\n",
        labels::OUTSIDE_COLUMNS.join("\t")
    );
    let (canonical, rows) = labels::outside_canonical(&tsv).unwrap();
    let at = dir.path().join(name);
    std::fs::create_dir_all(&at).unwrap();
    std::fs::write(at.join("labels.tsv"), &canonical).unwrap();
    let digest = labels::outside_digest(&canonical);
    let hex = digest.trim_start_matches("sha256:").to_string();
    labels::record(
        reg,
        &NewSet {
            name,
            version: 1,
            kind: labels::OUTSIDE,
            what: "body_part",
            source: json!({"provenance": {"made_by": "a study's tools"}}),
            campaign_id: None,
            handle_id: None,
            pack_version: None,
            scheme_digest: None,
            sealed: false,
            rows: rows.len() as i64,
            digest: &hex,
            place_id: None,
            path: Some(&at.display().to_string()),
            created_by: WHO,
        },
    )
    .unwrap();
    digest
}

fn digest(c: char) -> String {
    format!("sha256:{}", c.to_string().repeat(64))
}

const WHO: &str = "nima@lab";

fn speed(rate: f64) -> Value {
    json!({"stacks_per_second": rate, "hardware": "48 cores, the archive over NFS", "evidence": "a production run"})
}

fn tested() -> Value {
    json!([{"claim": "wrong among answered stacks", "value": "0 of 650", "bound": "0.46 %", "n": 650, "evidence": "a graded run"}])
}

/// The audit rows of the claims added, by principal.
fn audited(reg: &mut Registry) -> Vec<String> {
    let sql = format!(
        "SELECT principal FROM {} WHERE action = 'model.add_claims' ORDER BY id",
        reg.store().qualified("audit")
    );
    reg.store()
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect()
}

fn transitions(reg: &mut Registry, id: i64) -> Vec<String> {
    model::events(reg.store(), id)
        .unwrap()
        .iter()
        .map(|e| e["transition"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_registered_card_gains_its_missing_claims_and_nothing_else_on_both_backends() {
    for (name, _guard, dir, mut reg) in registries() {
        let clear_stack = stack(&mut reg, 1, "1.2.840.7.1");
        let held_stack = stack(&mut reg, 2, "1.2.840.7.2");
        assert!(clear_stack != held_stack, "{name}");
        let clear = outside_set(&mut reg, &dir, "clear", "1.2.840.7.1");
        let held = outside_set(&mut reg, &dir, "held", "1.2.840.7.2");

        let enc = model::register(
            &mut reg,
            &json!({"name": "enc", "version": "1", "kind": "encoder", "digest": digest('e'), "task": "features:image"}),
            WHO,
        )
        .unwrap();
        // a CPU model whose card lacks its label set, its tests and its speed
        let thin = json!({
            "name": "head", "version": "1", "kind": "head", "digest": digest('1'),
            "task": "axis:body_part", "encoder": {"digest": digest('e')}, "threshold": 0.9,
            "gpu": "none", "intended_use": "body_part per MR stack", "limits": ["site data only"],
        });
        let head = model::register(&mut reg, &thin, WHO).unwrap();
        assert_eq!(
            head.missing(),
            ["trained_on", "tested", "speed.cpu"],
            "{name}"
        );
        let card_before = head.card.clone();

        // refused, and nothing is written
        let refusals: Vec<(Value, Option<String>, &str)> = vec![
            (json!({}), None, "an object of trained_on, tested or speed"),
            (json!([]), None, "an object of trained_on, tested or speed"),
            (
                json!({"limits": ["more"]}),
                None,
                "limits is not a claim a registered card gains",
            ),
            (
                json!({"gpu": "optional"}),
                None,
                "gpu is not a claim a registered card gains",
            ),
            (json!({"tested": null}), None, "given as null"),
            (
                json!({"trained_on": {"label_set": "sha256:zz"}}),
                None,
                "by digest",
            ),
            (
                json!({"trained_on": {"label_set": digest('9')}}),
                None,
                "no label set has the digest",
            ),
            (json!({"tested": []}), None, "at least one claim"),
            (
                json!({"tested": [{"claim": "x", "value": 1}]}),
                None,
                "names the evidence run",
            ),
            (
                json!({"speed": {"cpu": {"stacks_per_second": 0, "hardware": "h", "evidence": "e"}}}),
                None,
                "a number above 0",
            ),
            (
                json!({"speed": {"cpu": {"stacks_per_second": 3.4, "evidence": "e"}}}),
                None,
                "names the hardware",
            ),
            (json!({"speed": {"tpu": speed(1.0)}}), None, "not tpu"),
            (json!({"speed": {}}), None, "an object of cpu, gpu or both"),
            (json!({"speed": {"gpu": speed(50.0)}}), None, "needs no GPU"),
            (
                json!({"tested": tested()}),
                Some(digest('7')),
                "the claims are for",
            ),
        ];
        for (claims, guard, words) in &refusals {
            let e = model::add_claims(&mut reg, head.id, claims, WHO, None, guard.as_deref())
                .unwrap_err();
            assert!(e.to_string().contains(words), "{name}: {claims}: {e}");
        }
        // the label set must be clear of every sample sealed now
        labels::seal(&mut reg, "cert-a", None, &[held_stack], WHO).unwrap();
        let sealed = model::add_claims(
            &mut reg,
            head.id,
            &json!({"trained_on": {"label_set": held}}),
            WHO,
            None,
            None,
        );
        assert!(
            matches!(&sealed, Err(Error::Refused(m)) if m.contains("sealed")),
            "{name}: {sealed:?}"
        );
        let still = model::get(reg.store(), head.id).unwrap().unwrap();
        assert_eq!(still.card, card_before, "{name}: a refusal changes nothing");
        assert_eq!(still.trained_on, None, "{name}");
        assert_eq!(transitions(&mut reg, head.id), ["registered"], "{name}");
        assert!(audited(&mut reg).is_empty(), "{name}");

        // the label set and the speed on the CPU, in one addition
        let first = model::add_claims(
            &mut reg,
            head.id,
            &json!({"speed": {"cpu": speed(3.44)}, "trained_on": {"label_set": clear, "name": "clear", "rows": 1}}),
            WHO,
            Some("  record 55, ruling 31 (a)  "),
            Some(digest('1').as_str()),
        )
        .unwrap();
        assert_eq!(first.fields, ["trained_on", "speed.cpu"], "{name}");
        assert_eq!(first.added["trained_on"]["label_set"], clear, "{name}");
        assert_eq!(first.added["speed"]["cpu"], speed(3.44), "{name}");
        assert_eq!(
            first.model.trained_on.as_deref(),
            Some(clear.as_str()),
            "{name}"
        );
        assert_eq!(first.model.missing(), ["tested"], "{name}");
        // nothing the card stated changed
        for (k, v) in card_before.as_object().unwrap() {
            assert_eq!(&first.model.card[k], v, "{name}: {k}");
        }
        let read = model::get(reg.store(), head.id).unwrap().unwrap();
        assert_eq!(read.card, first.model.card, "{name}");
        assert_eq!(read.trained_on.as_deref(), Some(clear.as_str()), "{name}");

        // a claim the card states now is refused
        for (claims, words) in [
            (
                json!({"trained_on": {"label_set": clear}}),
                "states trained_on",
            ),
            (json!({"speed": {"cpu": speed(9.0)}}), "states speed.cpu"),
            (
                json!({"tested": tested(), "speed": {"cpu": speed(9.0)}}),
                "states speed.cpu",
            ),
        ] {
            let e = model::add_claims(&mut reg, head.id, &claims, WHO, None, None).unwrap_err();
            assert!(
                matches!(&e, Error::Refused(m) if m.contains(words)),
                "{name}: {claims}: {e}"
            );
        }
        assert_eq!(
            model::get(reg.store(), head.id).unwrap().unwrap().card,
            first.model.card,
            "{name}: the refused tested claim was not added either"
        );

        // its tested claims complete it
        let second = model::add_claims(
            &mut reg,
            head.id,
            &json!({"tested": tested()}),
            "ada@lab",
            None,
            None,
        )
        .unwrap();
        assert_eq!(second.fields, ["tested"], "{name}");
        assert!(second.model.missing().is_empty(), "{name}");
        assert!(second.model.to_json()["complete"] == true, "{name}");

        // each addition is an event and an audit row
        assert_eq!(
            transitions(&mut reg, head.id),
            ["registered", "claims_added", "claims_added"],
            "{name}"
        );
        let events = model::events(reg.store(), head.id).unwrap();
        let detail = &events[1]["detail"];
        assert_eq!(
            detail["fields"],
            json!(["trained_on", "speed.cpu"]),
            "{name}"
        );
        assert_eq!(
            detail["missing_before"],
            json!(["trained_on", "tested", "speed.cpu"]),
            "{name}"
        );
        assert_eq!(detail["missing_after"], json!(["tested"]), "{name}");
        assert_eq!(detail["why"], "record 55, ruling 31 (a)", "{name}");
        let additions = model::additions(&events);
        assert_eq!(additions.len(), 2, "{name}");
        assert_eq!(additions[0]["by"], WHO, "{name}");
        assert_eq!(additions[1]["by"], "ada@lab", "{name}");
        assert_eq!(additions[1]["fields"], json!(["tested"]), "{name}");
        assert_eq!(additions[1]["added"]["tested"], tested(), "{name}");
        assert_eq!(audited(&mut reg), [WHO, "ada@lab"], "{name}");

        // a GPU model gains its speed with a GPU beside the one it states
        let mut gpu_card = thin.clone();
        for (k, v) in [
            ("name", json!("gpuhead")),
            ("digest", json!(digest('2'))),
            ("gpu", json!("optional")),
            ("trained_on", json!({"label_set": clear})),
            ("tested", tested()),
            ("speed", json!({"cpu": speed(3.44)})),
            (
                "equivalence",
                json!({"class": "II", "tolerance": 0.027, "set": "round 4", "evidence": "pcr4-speed"}),
            ),
        ] {
            gpu_card[k] = v;
        }
        let gpu = model::register(&mut reg, &gpu_card, WHO).unwrap();
        assert_eq!(gpu.missing(), ["speed.gpu"], "{name}");
        let e = model::add_claims(
            &mut reg,
            gpu.id,
            &json!({"speed": {"cpu": speed(4.0), "gpu": speed(54.72)}}),
            WHO,
            None,
            None,
        )
        .unwrap_err();
        assert!(e.to_string().contains("states speed.cpu"), "{name}: {e}");
        let done = model::add_claims(
            &mut reg,
            gpu.id,
            &json!({"speed": {"gpu": speed(54.72)}}),
            WHO,
            None,
            None,
        )
        .unwrap();
        assert_eq!(done.fields, ["speed.gpu"], "{name}");
        assert_eq!(done.model.card["speed"]["cpu"], speed(3.44), "{name}");
        assert_eq!(done.model.card["speed"]["gpu"], speed(54.72), "{name}");
        assert!(done.model.missing().is_empty(), "{name}");

        // a retired model may gain a claim: it says what the model was
        model::retire(&mut reg, enc.id, WHO, Some("replaced")).unwrap();
        let retired = model::add_claims(
            &mut reg,
            enc.id,
            &json!({"tested": tested()}),
            WHO,
            None,
            None,
        )
        .unwrap();
        assert_eq!(retired.model.state, "retired", "{name}");
        assert_eq!(retired.fields, ["tested"], "{name}");
        // no such model
        let none = model::add_claims(
            &mut reg,
            9999,
            &json!({"tested": tested()}),
            WHO,
            None,
            None,
        );
        assert!(matches!(none, Err(Error::Unknown(_))), "{name}: {none:?}");
        assert_eq!(audited(&mut reg).len(), 4, "{name}");
    }
}

/// A model from elsewhere (2026-10-30): a card whose label set this
/// registry does not hold registers as a foreign claim, the digest kept as
/// the card wrote it and the set marked as held at the training site; it
/// is admitted as a model on a set held here is; a card on a set held here
/// is checked against the seals as before; a part named by another card is
/// still registered first.
#[test]
fn a_card_trained_on_a_set_held_elsewhere_registers_as_a_foreign_claim_on_both_backends() {
    for (name, _guard, dir, mut reg) in registries() {
        let held_stack = stack(&mut reg, 1, "1.2.840.8.1");
        let local = outside_set(&mut reg, &dir, "local", "1.2.840.8.1");
        let foreign = digest('f');

        // a head naming an encoder not registered yet is refused, saying which
        let early = model::register(
            &mut reg,
            &json!({"name": "head", "version": "1", "kind": "head", "digest": digest('1'),
                    "task": "axis:body_part", "encoder": {"digest": digest('e')},
                    "trained_on": {"label_set": foreign}}),
            WHO,
        )
        .unwrap_err();
        assert!(early.to_string().contains(&digest('e')), "{name}: {early}");

        let enc = model::register(
            &mut reg,
            &json!({"name": "enc", "version": "1", "kind": "encoder", "digest": digest('e'),
                    "task": "features:image",
                    "trained_on": {"label_set": foreign, "name": "theirs", "rows": 9}}),
            WHO,
        )
        .unwrap();
        assert_eq!(enc.trained_on.as_deref(), Some(foreign.as_str()), "{name}");
        assert!(enc.trained_on_foreign, "{name}");
        assert_eq!(enc.trained_on_held(), Some("training_site"), "{name}");
        assert_eq!(
            enc.trained_on_words().unwrap(),
            format!("a label set held at the training site (digest {foreign})"),
            "{name}"
        );
        assert_eq!(enc.to_json()["trained_on_held"], "training_site", "{name}");
        assert_eq!(
            enc.card["trained_on"]["label_set"], foreign,
            "{name}: the card is kept as written"
        );
        let read = model::get(reg.store(), enc.id).unwrap().unwrap();
        assert!(read.trained_on_foreign, "{name}");
        assert!(!read.missing().contains(&"trained_on"), "{name}");

        // the head on the same foreign set, once its encoder is registered
        let head = model::register(
            &mut reg,
            &json!({"name": "head", "version": "1", "kind": "head", "digest": digest('1'),
                    "task": "axis:body_part", "encoder": {"digest": digest('e')},
                    "trained_on": {"label_set": foreign}}),
            WHO,
        )
        .unwrap();
        assert!(head.trained_on_foreign, "{name}");
        let check = json!({"suite": "heldout", "passed": true,
                           "checks": [{"name": "ece", "passed": true, "value": 0.02, "threshold": 0.05}]});
        let admitted = model::admit(&mut reg, head.id, &check, WHO).unwrap();
        assert_eq!(admitted.state, "admitted", "{name}");
        assert!(admitted.trained_on_foreign, "{name}");

        // a set held here keeps today's path: held, and checked against the seals
        let here = model::register(
            &mut reg,
            &json!({"name": "enc", "version": "2", "kind": "encoder", "digest": digest('2'),
                    "task": "features:image", "trained_on": {"label_set": local}}),
            WHO,
        )
        .unwrap();
        assert!(!here.trained_on_foreign, "{name}");
        assert_eq!(here.trained_on_held(), Some("here"), "{name}");
        assert_eq!(here.trained_on_words().unwrap(), local, "{name}");
        assert_eq!(here.to_json()["trained_on_held"], "here", "{name}");
        labels::seal(&mut reg, "cert-f", None, &[held_stack], WHO).unwrap();
        let sealed = model::register(
            &mut reg,
            &json!({"name": "enc", "version": "3", "kind": "encoder", "digest": digest('3'),
                    "task": "features:image", "trained_on": {"label_set": local}}),
            WHO,
        );
        assert!(
            matches!(&sealed, Err(Error::Refused(m)) if m.contains("sealed")),
            "{name}: {sealed:?}"
        );

        // no set named: held is none
        let none = model::register(
            &mut reg,
            &json!({"name": "enc", "version": "4", "kind": "encoder", "digest": digest('4'),
                    "task": "features:image"}),
            WHO,
        )
        .unwrap();
        assert_eq!(none.trained_on_held(), None, "{name}");
        assert!(none.to_json()["trained_on_held"].is_null(), "{name}");

        // the audit row of the registration says where the set is held
        let sql = format!(
            "SELECT {} FROM {} WHERE action = 'model.register' ORDER BY id",
            reg.store()
                .dialect()
                .text_of(table("audit").column("details").unwrap()),
            reg.store().qualified("audit")
        );
        let held: Vec<Value> = reg
            .store()
            .query(&sql, &[])
            .unwrap()
            .iter()
            .map(|r| {
                serde_json::from_str::<Value>(r.text(0).unwrap()).unwrap()["trained_on_held"]
                    .clone()
            })
            .collect();
        assert_eq!(
            held,
            [
                json!("training_site"),
                json!("training_site"),
                json!("here"),
                Value::Null
            ],
            "{name}"
        );
    }
}
