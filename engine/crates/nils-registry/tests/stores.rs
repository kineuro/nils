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
    let every: Vec<i64> = migrate::MIGRATIONS.iter().map(|m| m.version).collect();
    assert_eq!(
        migrate::migrate(store, Kind::Registry).unwrap(),
        every,
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
    // the same values on a list of ids, more than one SQLite chunk of them
    let touched = store
        .update_by_ids(
            instance,
            &[
                ("source_file_id", Param::from(Some(5i64))),
                ("instance_number", Param::from(9i64)),
            ],
            "id",
            &ids[..1_100],
        )
        .unwrap();
    assert_eq!(touched, 1_100, "{name}");
    let back = store
        .select_by_ids(instance, &cols, "id", &ids[1_099..1_101])
        .unwrap();
    let number_of = |id: i64| {
        back.iter()
            .find(|r| r.int(0).unwrap() == id)
            .map(|r| r.opt_int(2).unwrap())
            .unwrap()
    };
    assert_eq!(number_of(ids[1_099]), Some(9), "{name}: the last id listed");
    assert_ne!(
        number_of(ids[1_100]),
        Some(9),
        "{name}: the first id not listed"
    );
    assert_eq!(store.update_by_ids(instance, &[], "id", &[]).unwrap(), 0);

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

/// The linkage store beside a registry store: on SQLite two in-memory
/// stores, on Postgres the test schema and its `_linkage` sibling.
fn store_pairs() -> Vec<(String, Option<MutexGuard<'static, ()>>, Store, Store)> {
    let mut out = vec![(
        "sqlite".to_string(),
        None,
        Store::sqlite_in_memory().expect("sqlite"),
        Store::sqlite_in_memory().expect("sqlite"),
    )];
    if let Some((guard, registry)) = postgres_store(SCHEMA) {
        let dsn = postgres_dsn().unwrap();
        let mut linkage =
            Store::connect_postgres(&dsn, &format!("{SCHEMA}_linkage")).expect("connect");
        linkage
            .batch(&format!("CREATE SCHEMA {SCHEMA}_linkage"))
            .expect("linkage schema");
        out.push(("postgres".to_string(), Some(guard), registry, linkage));
    }
    out
}

#[test]
fn the_linkage_store_files_looks_up_and_imports_on_both_backends() {
    use nils_registry::linkage::{self, ImportError, ImportFault, ImportRow, NewIdentity, Subkeys};
    for (name, _guard, mut registry, mut linkage) in store_pairs() {
        migrate::migrate(&mut registry, Kind::Registry).unwrap();
        migrate::migrate(&mut linkage, Kind::Linkage).unwrap();
        let keys = Subkeys::derive(b"nils-fixture-key");
        // a subject the digest made, with its identity row
        let created = registry
            .insert(
                &Insert::new(
                    table("subject"),
                    &["code", "code_digest", "first_batch_id", "created_at"],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from("771c4326c89c082c"),
                    Param::from(vec![0x77u8, 0x1c]),
                    Param::from(1i64),
                    Param::from("2026-09-02T00:00:00Z"),
                ]],
            )
            .unwrap();
        let subject_id = created[0].int(0).unwrap();
        let lookup = keys.lookup("patient-id", "PID-0001");
        linkage::insert_identities(
            &mut linkage,
            &[NewIdentity {
                subject_id,
                id_type_id: 1,
                lookup: lookup.clone(),
                ciphertext: keys.seal("PID-0001"),
                source: "dicom",
                first_batch_id: Some(1),
            }],
        )
        .unwrap();
        let found =
            linkage::identities_by_lookup(&mut linkage, &[lookup.clone(), vec![1; 32]]).unwrap();
        assert_eq!(found.len(), 1, "{name}");
        assert_eq!(found[0].subject_id, subject_id, "{name}");
        assert_eq!(found[0].lookup, lookup, "{name}");
        let shown = linkage::reveal(&mut linkage, &keys, subject_id, "tester", None).unwrap();
        assert_eq!(shown[0].value, "PID-0001", "{name}");

        // an import that maps the known identifier elsewhere is refused whole
        let rows = |pairs: &[(&str, &str)]| -> Vec<ImportRow> {
            pairs
                .iter()
                .enumerate()
                .map(|(i, (identifier, code))| ImportRow {
                    line: i + 2,
                    identifier: identifier.to_string(),
                    code: code.to_string(),
                })
                .collect()
        };
        let err = linkage::import(
            &mut registry,
            &mut linkage,
            &keys,
            "patient-id",
            &rows(&[("PID-0001", "sub-x"), ("PID-0002", "sub-y")]),
        )
        .unwrap_err();
        match err {
            ImportError::Faults(faults) => assert_eq!(
                faults,
                vec![ImportFault::IdentifierMapped {
                    line: 2,
                    code: "771c4326c89c082c".to_string()
                }],
                "{name}"
            ),
            ImportError::Store(e) => panic!("{name}: {e}"),
        }
        let n = registry.query("SELECT COUNT(*) FROM subject", &[]).unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(n, 1, "{name}: nothing written");

        // the good rows land, and land once
        let report = linkage::import(
            &mut registry,
            &mut linkage,
            &keys,
            "patient-id",
            &rows(&[("PID-0001", "771c4326c89c082c"), ("PID-0002", "sub-y")]),
        )
        .unwrap();
        assert_eq!(report.subjects_created, 1, "{name}");
        assert_eq!(report.identities_added, 1, "{name}");
        assert_eq!(report.unchanged, 1, "{name}");
        let again = linkage::import(
            &mut registry,
            &mut linkage,
            &keys,
            "patient-id",
            &rows(&[("PID-0002", "sub-y")]),
        )
        .unwrap();
        assert_eq!(again.unchanged, 1, "{name}");
        let imported = linkage::subjects_by_code(&mut registry, &["sub-y".to_string()]).unwrap();
        assert_eq!(imported.len(), 1, "{name}");
        let shown =
            linkage::reveal(&mut linkage, &keys, imported[0].id, "tester", Some("why")).unwrap();
        assert_eq!(shown[0].value, "PID-0002", "{name}");
        assert_eq!(shown[0].source, "csv", "{name}");
        let audits = linkage
            .query("SELECT COUNT(*) FROM read_audit", &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(audits, 2, "{name}");
        let digest = registry
            .query("SELECT code_digest FROM subject WHERE code = 'sub-y'", &[])
            .unwrap();
        assert!(
            matches!(digest[0].get(0), nils_registry::store::Cell::Null),
            "{name}: an imported code has no digest"
        );
        // linkages
        let id = linkage::link(
            &mut linkage,
            subject_id,
            imported[0].id,
            "same person",
            "tester",
        )
        .unwrap();
        assert!(
            linkage::unlink(&mut linkage, id, "tester").unwrap(),
            "{name}"
        );
        let of = linkage::linkages_of(&mut linkage, imported[0].id).unwrap();
        assert_eq!(of.len(), 1, "{name}");
        assert!(of[0].reversed_at.is_some(), "{name}");
        assert!(
            of[0].created_at.ends_with('Z'),
            "{name}: {}",
            of[0].created_at
        );
        if let Some(schema) = registry.schema().map(str::to_string) {
            registry
                .batch(&format!(
                    "DROP SCHEMA {schema} CASCADE; DROP SCHEMA {schema}_linkage CASCADE"
                ))
                .unwrap();
        }
    }
}

/// Wave 3 §9: `release_file` is rebuilt by migration 16, so the upgrade path
/// is the one thing about it a fresh registry cannot prove.
///
/// A registry made before it has a `release_file` whose `instance_id` is NOT
/// NULL and which has no `stack_id`, and no `ALTER` relaxes a NOT NULL on both
/// backends. So the migration creates the new shape, copies through the join
/// that says which stack a file's instance belongs to, and renames.
#[test]
fn migration_16_rebuilds_the_release_manifest_and_keeps_its_rows() {
    use nils_registry::schema;

    let dir = TempDir::new("rebuild");
    let path = dir.path().join("registry.db");
    let mut store = Store::open_sqlite(&path).unwrap();
    migrate::migrate(&mut store, Kind::Registry).unwrap();

    // Put `release_file` back into the shape migration 15 left it in, and the
    // version with it, which is what a registry made last week looks like.
    store
        .batch(
            "DROP TABLE release_file;
             CREATE TABLE release_file (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               release_id INTEGER NOT NULL,
               instance_id INTEGER NOT NULL,
               path TEXT NOT NULL,
               digest TEXT NOT NULL,
               bytes INTEGER NOT NULL);
             UPDATE registry_meta SET value = '15' WHERE key = 'schema_version';
             INSERT INTO stack (id, series_id, stack_index, stack_key, modality, orientation, \
                                n_instances, first_batch_id)
               VALUES (7, 1, 0, 'k', 'MR', 'Ax', 1, 1);
             INSERT INTO instance (id, sop_instance_uid, series_id, stack_id, first_batch_id)
               VALUES (3, '1.2.3.4', 1, 7, 1);
             INSERT INTO release_file (release_id, instance_id, path, digest, bytes)
               VALUES (1, 3, 'sub-x/ses-1/anat/T1w/00000003.dcm', 'abc', 42)",
        )
        .unwrap();
    assert_eq!(
        migrate::standing(&mut store, Kind::Registry).unwrap(),
        Standing::Behind(15)
    );

    // Which is what opening it behind a newer binary does.
    assert_eq!(
        migrate::migrate(&mut store, Kind::Registry).unwrap(),
        vec![16]
    );

    let rows = store
        .query(
            "SELECT stack_id, instance_id, path, bytes FROM release_file",
            &[],
        )
        .unwrap();
    assert_eq!(rows.len(), 1, "the row survived");
    assert_eq!(rows[0].int(0).unwrap(), 7, "and gained the stack it is of");
    assert_eq!(rows[0].int(1).unwrap(), 3);
    assert_eq!(rows[0].int(3).unwrap(), 42);

    // And the new shape takes a file that is not one instance written out: a
    // NIfTI is a whole stack, and its sidecar is the stack's too.
    store
        .insert(
            &Insert::new(
                schema::table("release_file"),
                &[
                    "release_id",
                    "stack_id",
                    "instance_id",
                    "path",
                    "digest",
                    "bytes",
                ],
            ),
            &[vec![
                Param::Int(1),
                Param::Int(7),
                Param::Null,
                Param::from("sub-x/ses-1/anat/sub-x_ses-1_T1w.nii.gz"),
                Param::from("def"),
                Param::Int(9),
            ]],
        )
        .unwrap();
    let rows = store
        .query(
            "SELECT COUNT(*) FROM release_file WHERE instance_id IS NULL",
            &[],
        )
        .unwrap();
    assert_eq!(rows[0].int(0).unwrap(), 1);
}
