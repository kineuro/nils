// SPDX-License-Identifier: AGPL-3.0-only

//! The same store behaviour on both backends (§4, §9.2): the schema is created
//! from the declaration, inserts return what `RETURNING` names, conflicts do
//! what the writer needs, keys are looked up in bulk, and a registry home
//! initialises and reopens. Postgres runs when `NILS_TEST_POSTGRES_DSN` is set
//! (CI does; a laptop with `docker run postgres:16` can), in a schema of its
//! own that each test drops and recreates, one test at a time.

use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::dialect::Conflict;
use nils_registry::home::{Home, InitOptions};
use nils_registry::migrate::{self, Kind, Standing};
use nils_registry::schema::table;
use nils_registry::{Backend, BulkPath, Insert, Param, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_stores_test";

fn postgres_dsn() -> Option<String> {
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => Some(dsn),
        _ => {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
            None
        }
    }
}

/// A fresh Postgres store in the test schema, under the lock.
fn postgres_store(schema: &str) -> Option<(MutexGuard<'static, ()>, Store)> {
    let dsn = postgres_dsn()?;
    let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
    store
        .batch(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE; CREATE SCHEMA {schema}"
        ))
        .expect("fresh schema");
    Some((guard, store))
}

fn stores() -> Vec<(String, Option<MutexGuard<'static, ()>>, Store)> {
    let mut out = vec![(
        "sqlite".to_string(),
        None,
        Store::sqlite_in_memory().expect("sqlite"),
    )];
    if let Some((guard, mut store)) = postgres_store(SCHEMA) {
        store.set_bulk_path(BulkPath::Copy);
        out.push(("postgres/copy".to_string(), Some(guard), store));
    }
    out
}

fn n(i: i64) -> String {
    format!("1.2.826.0.1.{i}")
}

fn exercise(name: &str, store: &mut Store) {
    assert_eq!(
        migrate::migrate(store, Kind::Registry).unwrap(),
        vec![1],
        "{name}"
    );
    assert_eq!(
        migrate::standing(store, Kind::Registry).unwrap(),
        Standing::Current,
        "{name}"
    );

    // a source row, RETURNING its id
    let source = table("source");
    let spec = Insert::new(source, &["root", "root_canonical", "first_seen_at"]).returning(&["id"]);
    let rows = store
        .insert(
            &spec,
            &[vec![
                Param::from("/data/a"),
                Param::from("/data/a"),
                Param::from("2026-09-02T00:00:00Z"),
            ]],
        )
        .unwrap();
    assert_eq!(rows.len(), 1, "{name}");
    let source_id = rows[0].int(0).unwrap();
    assert!(source_id >= 1, "{name}");

    // instances in bulk: 2,500 rows, RETURNING id and uid; then the same rows
    // again with DO NOTHING, which returns nothing
    let instance = table("instance");
    let spec = Insert::new(
        instance,
        &[
            "sop_instance_uid",
            "series_id",
            "transfer_syntax_uid",
            "instance_number",
            "first_batch_id",
        ],
    )
    .on_conflict(Conflict::Nothing(&["sop_instance_uid"]))
    .returning(&["id", "sop_instance_uid"]);
    let rows: Vec<Vec<Param>> = (0..2_500)
        .map(|i| {
            vec![
                Param::from(n(i)),
                Param::from(1 + i % 7),
                Param::from("1.2.840.10008.1.2.1"),
                Param::from(Some(i)),
                Param::from(1i64),
            ]
        })
        .collect();
    let got = store.insert(&spec, &rows).unwrap();
    assert_eq!(got.len(), 2_500, "{name}");
    let mut ids: Vec<i64> = got.iter().map(|r| r.int(0).unwrap()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 2_500, "{name}: ids are distinct");
    let again = store.insert(&spec, &rows[..10]).unwrap();
    assert!(again.is_empty(), "{name}: conflicts return nothing");

    // the conflicting rows' ids come from a bulk lookup by key
    let cols = [
        instance.column("id").unwrap(),
        instance.column("sop_instance_uid").unwrap(),
        instance.column("instance_number").unwrap(),
    ];
    let keys: Vec<String> = (0..1_200).map(n).collect();
    let found = store
        .select_by_keys(instance, &cols, "sop_instance_uid", &keys)
        .unwrap();
    assert_eq!(found.len(), 1_200, "{name}");
    let r = found.iter().find(|r| r.text(1).unwrap() == n(7)).unwrap();
    assert_eq!(r.opt_int(2).unwrap(), Some(7), "{name}");
    let none = store
        .select_by_keys(instance, &cols, "sop_instance_uid", &["nope".to_string()])
        .unwrap();
    assert!(none.is_empty(), "{name}");

    // the same by integer key, and an update from a list of pairs
    let by_id = store
        .select_by_ids(instance, &cols, "id", &ids[..600])
        .unwrap();
    assert_eq!(by_id.len(), 600, "{name}");
    let pairs: Vec<(i64, i64)> = ids[..700].iter().map(|&id| (id, 1_000 + id)).collect();
    let updated = store
        .update_from_values(instance, "source_file_id = v.val", "id", &pairs)
        .unwrap();
    assert_eq!(updated, 700, "{name}");
    let bumped = store
        .update_from_values(
            instance,
            "instance_number = instance_number + v.val",
            "id",
            &pairs[..3],
        )
        .unwrap();
    assert_eq!(bumped, 3, "{name}");
    let sf_col = [
        instance.column("id").unwrap(),
        instance.column("source_file_id").unwrap(),
    ];
    let back = store
        .select_by_ids(instance, &sf_col, "id", &ids[..2])
        .unwrap();
    for r in &back {
        assert_eq!(
            r.opt_int(1).unwrap(),
            Some(1_000 + r.int(0).unwrap()),
            "{name}"
        );
    }

    // an upsert on source_file: the second batch overwrites status and batch
    let sf = table("source_file");
    let columns = [
        "source_id",
        "batch_id",
        "dir",
        "path",
        "size",
        "mtime_ns",
        "status",
        "reason",
        "detail",
        "instance_id",
        "seen_at",
    ];
    let spec = Insert::new(sf, &columns)
        .on_conflict(Conflict::Update {
            target: &["source_id", "path"],
            set: &[
                "batch_id",
                "size",
                "mtime_ns",
                "status",
                "reason",
                "detail",
                "instance_id",
                "seen_at",
            ],
        })
        .returning(&["id"]);
    let row = |batch: i64, status: &str| {
        vec![
            Param::from(source_id),
            Param::from(batch),
            Param::from("d"),
            Param::from("d/f.dcm"),
            Param::from(1024i64),
            Param::from(1_700_000_000_000_000_000i64),
            Param::from(status),
            Param::Null,
            Param::Null,
            Param::from(Some(ids[0])),
            Param::from("2026-09-02T00:00:00Z"),
        ]
    };
    let first = store.insert(&spec, &[row(1, "ingested")]).unwrap();
    let second = store.insert(&spec, &[row(2, "unchanged")]).unwrap();
    assert_eq!(
        first[0].int(0).unwrap(),
        second[0].int(0).unwrap(),
        "{name}: same row"
    );
    let sql = format!(
        "SELECT batch_id, status, COUNT(*) OVER () FROM {} WHERE source_id = {}",
        store.qualified("source_file"),
        store.dialect().param(1, nils_registry::schema::Type::Int)
    );
    let r = store
        .query_opt(&sql, &[Param::from(source_id)])
        .unwrap()
        .unwrap();
    assert_eq!(r.int(0).unwrap(), 2, "{name}");
    assert_eq!(r.text(1).unwrap(), "unchanged", "{name}");
    assert_eq!(r.int(2).unwrap(), 1, "{name}");

    // an update by id casts by the columns' types: a JSON value and a stamp
    let sf_id = first[0].int(0).unwrap();
    let job = table("job");
    let job_rows = store
        .insert(
            &Insert::new(job, &["kind", "state", "started_at"]).returning(&["id"]),
            &[vec![
                Param::from("digest"),
                Param::from("running"),
                Param::from("2026-09-02T00:00:00Z"),
            ]],
        )
        .unwrap();
    let job_id = job_rows[0].int(0).unwrap();
    let n = store
        .update_by_id(
            job,
            &[
                ("state", Param::from("done")),
                ("progress", Param::from("{\"seen\": 3}")),
                ("finished_at", Param::from("2026-09-02T00:01:00Z")),
                ("pid", Param::from(Some(sf_id))),
            ],
            "id",
            job_id,
        )
        .unwrap();
    assert_eq!(n, 1, "{name}");
    let cols = [
        job.column("state").unwrap(),
        job.column("progress").unwrap(),
        job.column("finished_at").unwrap(),
    ];
    let back = store.select_by_ids(job, &cols, "id", &[job_id]).unwrap();
    assert_eq!(back[0].text(0).unwrap(), "done", "{name}");
    assert!(back[0].text(1).unwrap().contains("\"seen\""), "{name}");
    assert_eq!(back[0].text(2).unwrap(), "2026-09-02T00:01:00Z", "{name}");

    // typed columns round-trip as text: a study with a date, a JSON value, a
    // double and a null
    let study = table("study");
    let spec = Insert::new(
        study,
        &[
            "study_instance_uid",
            "subject_id",
            "study_date",
            "study_time",
            "study_description",
            "first_batch_id",
        ],
    )
    .returning(&["id"]);
    let rows = store
        .insert(
            &spec,
            &[vec![
                Param::from("1.2.3"),
                Param::from(1i64),
                Param::from("2024-02-29"),
                Param::from("14:03:07.250000"),
                Param::Null,
                Param::from(1i64),
            ]],
        )
        .unwrap();
    assert_eq!(rows.len(), 1, "{name}");
    let cols = [
        study.column("study_date").unwrap(),
        study.column("study_time").unwrap(),
        study.column("study_description").unwrap(),
    ];
    let back = store
        .select_by_keys(study, &cols, "study_instance_uid", &["1.2.3".to_string()])
        .unwrap();
    assert_eq!(back[0].text(0).unwrap(), "2024-02-29", "{name}");
    assert_eq!(back[0].text(1).unwrap(), "14:03:07.250000", "{name}");
    assert_eq!(back[0].opt_text(2).unwrap(), None, "{name}");

    // a transaction that rolls back leaves nothing
    store.begin().unwrap();
    store
        .insert(
            &Insert::new(source, &["root", "root_canonical", "first_seen_at"]),
            &[vec![
                Param::from("/data/b"),
                Param::from("/data/b"),
                Param::from("2026-09-02T00:00:00Z"),
            ]],
        )
        .unwrap();
    store.rollback().unwrap();
    let count = store
        .query(
            &format!("SELECT COUNT(*) FROM {}", store.qualified("source")),
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(count, 1, "{name}");
}

#[test]
fn both_backends_create_the_schema_and_insert_the_same_way() {
    for (name, _guard, mut store) in stores() {
        exercise(&name, &mut store);
    }
}

#[test]
fn the_postgres_insert_path_behaves_like_copy() {
    let Some((_guard, mut store)) = postgres_store(SCHEMA) else {
        return;
    };
    store.set_bulk_path(BulkPath::Insert);
    exercise("postgres/insert", &mut store);
}

#[test]
fn a_home_on_postgres_initialises_and_reopens() {
    let Some((_guard, mut store)) = postgres_store("nils_home_test") else {
        return;
    };
    // the home wants an empty database side: drop what the helper created
    store.batch("DROP SCHEMA nils_home_test CASCADE").unwrap();
    drop(store);
    let dsn = postgres_dsn().unwrap();
    let dir = TempDir::new("home-pg");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-fixture-key").unwrap();
    let opts = InitOptions {
        backend: Backend::Postgres,
        dsn: Some(dsn),
        schema: Some("nils_home_test".to_string()),
        scheme: Scheme::DEFAULT,
        key: "k".to_string(),
        display_length: 12,
        session_scheme: None,
    };
    let mut reg = home.init(&opts).unwrap();
    assert_eq!(reg.config().linkage_schema(), "nils_home_test_linkage");
    assert_eq!(reg.store().schema(), Some("nils_home_test"));
    reg.store().begin().unwrap();
    assert_eq!(reg.next_epoch().unwrap(), 1);
    reg.store().commit().unwrap();
    drop(reg);
    let err = home.init(&opts).unwrap_err().to_string();
    assert!(err.contains("already a registry"), "{err}");

    let mut reg = home.open().unwrap();
    assert_eq!(reg.meta().epoch, 1);
    let mut linkage = reg.open_linkage().unwrap();
    assert_eq!(linkage.schema(), Some("nils_home_test_linkage"));
    let n = linkage.query("SELECT COUNT(*) FROM id_type", &[]).unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(n, 2);
    drop(linkage);
    reg.store()
        .batch("DROP SCHEMA nils_home_test CASCADE; DROP SCHEMA nils_home_test_linkage CASCADE")
        .unwrap();
}
