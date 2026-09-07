// SPDX-License-Identifier: AGPL-3.0-only

//! The synthetic registry builds the same on both backends and its manifest
//! says what it planted (`docs/specs/wave4b-the-ask.md`, §13.1 and §15,
//! slice 1).

use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::home::{Home, InitOptions};
use nils_registry::{Backend, Registry, Scheme};
use nils_synth::{Manifest, Plan, build};

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_synth_test";

struct Lab {
    name: &'static str,
    registry: Registry,
    _dir: TempDir,
    _guard: Option<MutexGuard<'static, ()>>,
}

impl Drop for Lab {
    fn drop(&mut self) {
        if self._guard.is_some() {
            let store = self.registry.store();
            store
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
                ))
                .ok();
        }
    }
}

fn lab(name: &'static str, backend: Backend, dsn: Option<String>) -> Lab {
    let dir = TempDir::new("synth-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-synth-test-key").unwrap();
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
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => {
            let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
            let mut l = lab("postgres", Backend::Postgres, Some(dsn));
            l._guard = Some(guard);
            out.push(l);
        }
        _ => eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped"),
    }
    out
}

fn count(registry: &mut Registry, table: &str, filter: &str) -> i64 {
    let store = registry.store();
    let sql = format!("SELECT COUNT(*) FROM {}{filter}", store.qualified(table));
    store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
}

#[test]
fn the_same_seed_builds_the_same_registry_on_every_backend() {
    let plan = Plan {
        seed: 7,
        subjects: 60,
    };
    let mut manifests: Vec<(&str, Manifest)> = Vec::new();
    for mut l in labs() {
        let m = build(&mut l.registry, &plan).unwrap();
        // the manifest counts what the tables hold
        assert_eq!(
            count(&mut l.registry, "subject", ""),
            m.counts.subjects as i64,
            "{}",
            l.name
        );
        assert_eq!(
            count(&mut l.registry, "study", ""),
            m.counts.studies as i64,
            "{}",
            l.name
        );
        assert_eq!(
            count(&mut l.registry, "stack", ""),
            m.counts.stacks as i64,
            "{}",
            l.name
        );
        assert_eq!(
            count(&mut l.registry, "stack_fingerprint", ""),
            m.counts.stacks as i64,
            "{}",
            l.name
        );
        assert_eq!(
            count(&mut l.registry, "classification", ""),
            m.counts.stacks as i64,
            "{}",
            l.name
        );
        assert_eq!(
            count(
                &mut l.registry,
                "event",
                " WHERE event_date_precision = 'year'"
            ),
            m.counts.transitions as i64,
            "{}",
            l.name
        );
        assert_eq!(
            count(&mut l.registry, "cohort_member", " WHERE left_at IS NULL"),
            m.counts.memberships_open as i64,
            "{}",
            l.name
        );
        assert_eq!(
            count(
                &mut l.registry,
                "subject_disease_type",
                " WHERE assigned_on_precision = 'year'"
            ),
            m.counts.course_rows_at_year as i64,
            "{}",
            l.name
        );
        // what the spec asks the seed to plant
        assert_eq!(m.cases.len(), 24, "{}", l.name);
        assert_eq!(
            m.cases
                .iter()
                .filter(|c| c.falls_out_at == "answer")
                .count(),
            13
        );
        assert!(m.counts.subject_days_with_two_studies > 0);
        assert!(m.counts.memberships_closed > 0);
        assert!(m.counts.dispositions["scanner_derived"] > 0);
        assert!(m.counts.dispositions["scout"] > 0);
        // a second build into the same registry is refused
        assert!(build(&mut l.registry, &plan).is_err(), "{}", l.name);
        // the epoch moved
        l.registry.refresh_meta().unwrap();
        assert!(l.registry.meta().epoch >= 1, "{}", l.name);
        manifests.push((l.name, m));
    }
    // the same seed is the same registry on SQLite and on Postgres
    if manifests.len() == 2 {
        assert_eq!(manifests[0].1, manifests[1].1);
    }
    // and the same seed twice on one backend
    let mut again = lab("sqlite again", Backend::Sqlite, None);
    let m2 = build(&mut again.registry, &plan).unwrap();
    assert_eq!(manifests[0].1, m2);
    // a different seed is a different registry
    let mut other = lab("sqlite other", Backend::Sqlite, None);
    let m3 = build(
        &mut other.registry,
        &Plan {
            seed: 8,
            subjects: 60,
        },
    )
    .unwrap();
    assert_ne!(manifests[0].1.counts, m3.counts);
}
