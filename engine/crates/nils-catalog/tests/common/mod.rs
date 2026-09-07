// SPDX-License-Identifier: AGPL-3.0-only

//! The laboratory the compile and handle tests share: a synthetic registry
//! on each backend with its sessions ensured, an uploaded list, and the
//! catalog over the MR pack.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_ask::ast::Ask;
use nils_ask::compile::{Compiled, Context, compile};
use nils_ask::exec::{Answer, Bounds, run};
use nils_ask::validate::Scope;
use nils_ask::{parse, prepare};
use nils_catalog::Catalog;
use nils_dicom::synth::TempDir;
use nils_registry::home::{Home, InitOptions};
use nils_registry::schema::table;
use nils_registry::session::Scheme;
use nils_registry::{Backend, Insert, Param, Registry};
use serde_json::Value;

static POSTGRES: Mutex<()> = Mutex::new(());

pub struct Lab {
    pub name: &'static str,
    pub registry: Registry,
    pub catalog: Catalog,
    pub manifest: nils_synth::Manifest,
    pub schema: &'static str,
    _dir: TempDir,
    _guard: Option<MutexGuard<'static, ()>>,
}

impl Drop for Lab {
    fn drop(&mut self) {
        if self._guard.is_some() {
            self.registry
                .store()
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {0} CASCADE; DROP SCHEMA IF EXISTS {0}_linkage CASCADE",
                    self.schema
                ))
                .ok();
        }
    }
}

pub fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

pub fn lab(name: &'static str, backend: Backend, dsn: Option<String>, schema: &'static str) -> Lab {
    // A run killed mid-test leaves its schema behind, and init refuses a
    // schema that already holds a registry: the lab starts clean.
    if let Some(dsn) = &dsn
        && let Ok(mut store) = nils_registry::store::Store::connect_postgres(dsn, "public")
    {
        let _ = store.batch(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
        ));
    }
    let dir = TempDir::new("compile-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-compile-test-key").unwrap();
    let mut registry = home
        .init(&InitOptions {
            backend,
            dsn,
            schema: (backend == Backend::Postgres).then(|| schema.to_string()),
            scheme: nils_registry::Scheme::DEFAULT,
            key: "k".to_string(),
            display_length: 12,
            session_scheme: None,
        })
        .unwrap();
    let manifest = nils_synth::build(
        &mut registry,
        &nils_synth::Plan {
            seed: 11,
            subjects: 48,
        },
    )
    .unwrap();
    // the sessions, under the default scheme
    let scheme = Scheme::default();
    let anchors = nils_session::Anchors::resolve(&mut registry, &scheme, BTreeMap::new()).unwrap();
    nils_session::ensure(&mut registry, &scheme, &anchors, None, false).unwrap();
    // an uploaded list of 600 subjects, already resolved (slice 7 does the
    // resolving; the rows are what the compiler joins)
    {
        let store = registry.store();
        let rows = store
            .insert(
                &Insert::new(
                    table("values_source"),
                    &[
                        "upload_id",
                        "namespace",
                        "digest",
                        "n",
                        "unresolved",
                        "principal",
                        "created_at",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from("u-600"),
                    Param::from("patient-id"),
                    Param::from("d"),
                    Param::Int(600),
                    Param::Int(0),
                    Param::from("test"),
                    Param::from("2026-09-07T00:00:00Z"),
                ]],
            )
            .unwrap();
        let source = rows[0].int(0).unwrap();
        let members: Vec<Vec<Param>> = (0..600)
            .map(|i| vec![Param::Int(source), Param::Int(i), Param::Int(1 + (i % 48))])
            .collect();
        store
            .insert(
                &Insert::new(
                    table("values_member"),
                    &["source_id", "position", "subject_id"],
                ),
                &members,
            )
            .unwrap();
    }
    let pack = nils_pack::load(&root().join("packs/mri"), None).unwrap();
    let catalog = Catalog::build(&mut registry, &pack).unwrap();
    Lab {
        name,
        registry,
        catalog,
        manifest,
        schema,
        _dir: dir,
        _guard: None,
    }
}

pub fn labs(schema: &'static str) -> Vec<Lab> {
    let mut out = vec![lab("sqlite", Backend::Sqlite, None, schema)];
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => {
            let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
            let mut l = lab("postgres", Backend::Postgres, Some(dsn), schema);
            l._guard = Some(guard);
            out.push(l);
        }
        _ => eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped"),
    }
    out
}

pub fn ask_of(l: &mut Lab, text: &str) -> (Compiled, Answer) {
    let ask = parse(text).unwrap_or_else(|e| panic!("{}: {e}", l.name));
    run_ask(l, ask)
}

pub fn run_ask(l: &mut Lab, ask: Ask) -> (Compiled, Answer) {
    let prepared =
        prepare(ask, &l.catalog, &Scope::default()).unwrap_or_else(|e| panic!("{}: {e}", l.name));
    let store = l.registry.store();
    let ctx = Context {
        names: &l.catalog,
        dialect: store.dialect(),
        schema: store.schema().map(str::to_string),
        window_days: 0,
        scheme_digest: Scheme::default().digest(),
        after: None,
        limit: None,
    };
    let compiled = compile(&prepared.ask, &prepared.validated, &ctx)
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
    let answer = run(
        store,
        &compiled,
        Bounds {
            timeout_ms: 20_000,
            max_rows: 5_000,
            max_bytes: 4 * 1024 * 1024,
        },
    )
    .unwrap_or_else(|e| panic!("{}: {e}\n{}", l.name, compiled.sql));
    (compiled, answer)
}

/// Rebuild the catalog after the registry changed (a selection, a handle,
/// an upload).
pub fn refresh(l: &mut Lab) {
    let pack = nils_pack::load(&root().join("packs/mri"), None).unwrap();
    l.catalog = Catalog::build(&mut l.registry, &pack).unwrap();
}

pub fn fixture(name: &str) -> Ask {
    let path = root().join(format!("engine/crates/nils-ask/fixtures/{name}.ask.yml"));
    parse(&std::fs::read_to_string(&path).unwrap()).unwrap_or_else(|e| panic!("{name}: {e}"))
}

pub fn set_param(ask: &mut Ask, name: &str, v: Value) {
    ask.params
        .get_mut(name)
        .unwrap_or_else(|| panic!("no parameter {name}"))
        .value = Some(v);
}
