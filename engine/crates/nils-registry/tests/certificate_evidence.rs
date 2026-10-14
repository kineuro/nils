// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 ruling 22 (b) on both backends: a certificate may cite its
//! graded run instead of a sample sealed in the same registry. It names the
//! run, the sha256 of its manifest, the plan's commit and the artifact
//! digest of the one model it graded; the engine works the bound out again
//! from the counts and refuses a stated bound that differs, a result above
//! its bar, an agent or a model, another model's digest and a run that
//! already certifies the model; it unseals nothing; the audit keeps it with
//! its citation; it confirms System 1 and names its task's default model.
//! Migration 99 gives a registry from before the column. Postgres runs when
//! `NILS_TEST_POSTGRES_DSN` is set.

use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::home::{Home, InitOptions};
use nils_registry::labels::{self, Error};
use nils_registry::migrate::{self, Kind};
use nils_registry::{Backend, Scheme, Store, audit, model, opinion};
use serde_json::{Value, json};

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_cited_certificate_test";

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
fn registries() -> Vec<(
    String,
    Option<MutexGuard<'static, ()>>,
    TempDir,
    nils_registry::Registry,
)> {
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
    let dir = TempDir::new("cited-sqlite");
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
        let dir = TempDir::new("cited-pg");
        let reg = init(&dir, Backend::Postgres, Some(dsn));
        out.push(("postgres".to_string(), Some(guard), dir, reg));
    }
    out
}

fn digest(c: char) -> String {
    format!("sha256:{}", c.to_string().repeat(64))
}

const WHO: &str = "nima@lab";
const PLAN: &str = "83a75e5875ce4e1b8fe22714dc4fc3e20deffaff";

fn cited(run: &str, model_digest: &str) -> Value {
    json!({"run": run, "manifest_sha256": format!("SHA256:{}", "7C".repeat(32)),
           "plan_commit": PLAN, "model_digest": model_digest,
           "plan": "the robustness certificate's plan",
           "equivalence": {"run": "serve-2", "manifest_sha256": "6".repeat(64), "class": "I"}})
}

fn graded() -> Value {
    json!({"bar": 0.01, "n": 5482, "errors": 1, "bound": 0.0008650574506395371,
           "coverage": 0.8922526041666666, "test": "T19", "claim": "wrong among accepted stacks"})
}

#[test]
fn a_certificate_cites_its_graded_run_on_both_backends() {
    for (name, _guard, _dir, mut reg) in registries() {
        let gate = json!({"axes": {"base": {"lambda1": 0.5, "lambda2": 0.9}}});
        let s1 = model::register(
            &mut reg,
            &json!({"name": "s1c", "version": "2", "kind": "pass", "digest": digest('e'), "task": "system1", "gate": gate}),
            WHO,
        )
        .unwrap();
        let other = model::register(
            &mut reg,
            &json!({"name": "s1c", "version": "3", "kind": "pass", "digest": digest('f'), "task": "system1", "gate": gate}),
            WHO,
        )
        .unwrap();
        let passed =
            json!({"suite": "s", "passed": true, "checks": [{"name": "c", "passed": true}]});
        for m in [s1.id, other.id] {
            model::admit(&mut reg, m, &passed, WHO).unwrap();
        }
        assert_eq!(
            opinion::confirmed(reg.store(), s1.id).unwrap(),
            None,
            "{name}"
        );
        let by = |reg: &mut nils_registry::Registry| {
            let c = model::chosen(reg.store(), "system1", "site")
                .unwrap()
                .unwrap();
            (c.model.id, c.by, c.certificate_id)
        };
        assert_eq!(by(&mut reg), (other.id, "admitted", None), "{name}");

        // what is refused, and nothing written by it
        let refused = |reg: &mut nils_registry::Registry,
                       ids: &[i64],
                       evidence: Value,
                       result: Value,
                       kind: &str| {
            labels::record_cited_certificate(reg, ids, &evidence, &result, WHO, kind).unwrap_err()
        };
        let e = refused(&mut reg, &[], cited("d2", &digest('e')), graded(), "agent");
        assert!(
            matches!(&e, Error::Refused(m) if m.contains("person's act")),
            "{name}: {e:?}"
        );
        let e = refused(
            &mut reg,
            &[other.id],
            cited("d2", &digest('e')),
            graded(),
            "person",
        );
        assert!(
            matches!(&e, Error::Invalid(m) if m.contains("one model")),
            "{name}: {e:?}"
        );
        let e = refused(&mut reg, &[], cited("d2", &digest('a')), graded(), "person");
        assert!(matches!(&e, Error::NotFound(_)), "{name}: {e:?}");
        let mut off = graded();
        off["bound"] = json!(0.00087);
        let e = refused(&mut reg, &[], cited("d2", &digest('e')), off, "person");
        assert!(
            matches!(&e, Error::Invalid(m) if m.contains("worked out")),
            "{name}: {e:?}"
        );
        let e = refused(
            &mut reg,
            &[],
            cited("d2", &digest('e')),
            json!({"bar": 0.01, "n": 298, "errors": 0}),
            "person",
        );
        assert!(
            matches!(&e, Error::Refused(m) if m.contains("above the bar")),
            "{name}: {e:?}"
        );
        let mut stray = graded();
        stray["wrong"] = json!(1);
        let e = refused(&mut reg, &[], cited("d2", &digest('e')), stray, "person");
        assert!(
            matches!(&e, Error::Invalid(m) if m.contains("result.wrong")),
            "{name}: {e:?}"
        );
        for (k, bad) in [
            ("run", json!("../d2")),
            ("manifest_sha256", json!("sha256:12")),
            ("plan_commit", json!("main")),
            ("model_digest", json!("e")),
        ] {
            let mut c = cited("d2", &digest('e'));
            c[k] = bad;
            let e = refused(&mut reg, &[], c, graded(), "person");
            assert!(
                matches!(&e, Error::Invalid(m) if m.contains(k)),
                "{name}: {k}: {e:?}"
            );
        }
        let mut c = cited("d2", &digest('e'));
        c["equivalence"]["class"] = json!("IV");
        let e = refused(&mut reg, &[], c, graded(), "person");
        assert!(
            matches!(&e, Error::Invalid(m) if m.contains("equivalence")),
            "{name}: {e:?}"
        );
        assert!(
            labels::certificates(reg.store()).unwrap().is_empty(),
            "{name}"
        );
        assert!(
            audit::list(
                reg.store(),
                &audit::Filter {
                    action: Some("labels.certificate".into()),
                    limit: 10,
                    ..Default::default()
                }
            )
            .unwrap()
            .is_empty(),
            "{name}"
        );

        // recorded: by the model's digest alone, normalised as kept
        let cert = labels::record_cited_certificate(
            &mut reg,
            &[],
            &cited("d2", &digest('e')),
            &graded(),
            WHO,
            "person",
        )
        .unwrap();
        assert_eq!(cert.sample, "evidence:d2", "{name}");
        assert_eq!(cert.model_ids, [s1.id], "{name}");
        assert!(cert.cites_evidence(), "{name}");
        let e = cert.evidence.clone().unwrap();
        assert_eq!(
            e["manifest_sha256"],
            format!("sha256:{}", "7c".repeat(32)),
            "{name}"
        );
        assert_eq!(e["plan_commit"], PLAN, "{name}");
        assert_eq!(e["model_digest"], digest('e'), "{name}");
        assert_eq!(e["equivalence"]["manifest_sha256"], digest('6'), "{name}");
        let r = &cert.result;
        assert_eq!(
            (r["n"].as_i64(), r["errors"].as_i64(), r["passes"].as_bool()),
            (Some(5482), Some(1), Some(true)),
            "{name}: {r}"
        );
        assert_eq!(r["confidence"], 0.95, "{name}");
        assert!(
            (r["bound"].as_f64().unwrap() - 0.000_865_057_450_639_537_1).abs() < 1e-12,
            "{name}: {r}"
        );
        assert_eq!(r["coverage"], 0.8922526041666666, "{name}");
        let v = cert.as_json();
        assert_eq!(
            (v["cites"].as_str(), v["unseals"].as_bool()),
            (Some("evidence"), Some(false)),
            "{name}: {v}"
        );
        assert_eq!(
            cert.words(),
            format!(
                "certificate {} cites the graded run d2 (manifest sha256:7c7c7c7c7c7c, plan 83a75e5875ce): 1 of 5482 wrong, bound 0.087 % at bar 1 %",
                cert.id
            ),
            "{name}"
        );
        // it reads back the same, newest first, and names the model
        let listed = labels::certificates(reg.store()).unwrap();
        assert_eq!(listed, [cert.clone()], "{name}");
        assert_eq!(
            labels::certified(reg.store()).unwrap().get(&s1.id),
            Some(&cert),
            "{name}"
        );
        assert_eq!(
            opinion::confirmed(reg.store(), s1.id).unwrap(),
            Some(cert.id),
            "{name}"
        );
        assert_eq!(
            opinion::confirmed(reg.store(), other.id).unwrap(),
            None,
            "{name}"
        );
        assert_eq!(
            by(&mut reg),
            (s1.id, "certificate", Some(cert.id)),
            "{name}: the default of its task"
        );
        // the audit keeps who, the citation and the result
        let rows = audit::list(
            reg.store(),
            &audit::Filter {
                action: Some("labels.certificate".into()),
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1, "{name}");
        assert_eq!(rows[0].principal, WHO, "{name}");
        assert_eq!(rows[0].scope["evidence"], "d2", "{name}");
        assert_eq!(rows[0].scope["certificate"], cert.id, "{name}");
        let details = rows[0].details.clone().unwrap();
        assert_eq!(details["unseals"], false, "{name}");
        assert_eq!(details["evidence"]["run"], "d2", "{name}");
        assert_eq!(details["result"]["errors"], 1, "{name}");
        // the same run again for the model: refused; it unseals nothing
        let e = labels::record_cited_certificate(
            &mut reg,
            &[s1.id],
            &cited("d2", &digest('e')),
            &graded(),
            "russ@lab",
            "person",
        )
        .unwrap_err();
        assert!(
            matches!(&e, Error::Refused(m) if m.contains("already cites")),
            "{name}: {e:?}"
        );
        let e = labels::unseal(&mut reg, "evidence:d2", cert.id, "russ@lab", "person").unwrap_err();
        assert!(
            matches!(&e, Error::Refused(m) if m.contains("unseals nothing")),
            "{name}: {e:?}"
        );
    }
}

/// Migration 99 on both backends: a registry of schema 98 gains the
/// certificate's `evidence`, its certificates reading as on a sealed sample.
#[test]
fn migration_99_lets_a_certificate_cite_its_graded_run_on_both_backends() {
    let mut stores: Vec<(String, Option<MutexGuard<'static, ()>>, Store)> = vec![(
        "sqlite".to_string(),
        None,
        Store::sqlite_in_memory().unwrap(),
    )];
    if let Some(dsn) = postgres_dsn() {
        let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = Store::connect_postgres(&dsn, SCHEMA).unwrap();
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE; CREATE SCHEMA {SCHEMA}"
            ))
            .unwrap();
        stores.push(("postgres".to_string(), Some(guard), store));
    }
    for (name, _guard, mut store) in stores {
        migrate::migrate(&mut store, Kind::Registry).unwrap();
        let (c, meta) = (
            store.qualified("certificate"),
            store.qualified("registry_meta"),
        );
        store
            .batch(&format!(
                "ALTER TABLE {c} DROP COLUMN evidence;
                 INSERT INTO {c} (sample, model_ids, result, created_by, created_at)
                   VALUES ('selection:s@1', '[3]', '{{\"risk\": 0.05, \"n\": 300, \"errors\": 0}}', 'anna', '2026-10-01T00:00:00Z');
                 UPDATE {meta} SET value = '98' WHERE key = 'schema_version'"
            ))
            .unwrap();
        assert!(
            !migrate::column_exists(&mut store, "certificate", "evidence").unwrap(),
            "{name}"
        );
        assert_eq!(
            migrate::migrate(&mut store, Kind::Registry).unwrap(),
            [99],
            "{name}"
        );
        assert!(
            migrate::column_exists(&mut store, "certificate", "evidence").unwrap(),
            "{name}"
        );
        let old = labels::certificates(&mut store).unwrap();
        assert_eq!(old.len(), 1, "{name}");
        assert!(!old[0].cites_evidence(), "{name}");
        assert_eq!(old[0].as_json()["unseals"], true, "{name}");
        assert!(
            migrate::migrate(&mut store, Kind::Registry)
                .unwrap()
                .is_empty(),
            "{name}"
        );
    }
}
