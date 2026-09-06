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

/// Wave 4a §4: migration 18 folds a row per stack per version and a row per
/// file per version into one current state per stack, so the upgrade path is
/// the one thing about it a fresh registry cannot prove.
///
/// A registry made before it has two versions of one stack; after it there is
/// one row, the newest, with the bytes the manifest knew, and no manifest.
#[test]
fn migration_18_folds_the_versions_into_one_current_state_per_stack() {
    use nils_registry::schema;

    let dir = TempDir::new("fold");
    let path = dir.path().join("registry.db");
    let mut store = Store::open_sqlite(&path).unwrap();
    migrate::migrate(&mut store, Kind::Registry).unwrap();

    // Put the release's bookkeeping back into the shape migration 17 left it
    // in, which is what a registry made last week looks like.
    let release = "(name, version, root, policy, selection, categories, session_scheme, \
                    layout, placements, pack, pack_version, actor, started_at, finished_at, \
                    files, subjects, unchanged, moved, rewritten, added, removed)";
    store
        .batch(&format!(
            "DROP TABLE release_plan;
             DROP TABLE dataset;
             DROP TABLE release_stack;
             ALTER TABLE release DROP COLUMN dataset_id;
             CREATE TABLE release_stack (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               release_id INTEGER NOT NULL,
               stack_id INTEGER NOT NULL,
               content TEXT NOT NULL,
               dir TEXT NOT NULL,
               stem TEXT,
               route TEXT NOT NULL,
               files INTEGER NOT NULL);
             CREATE TABLE release_file (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               release_id INTEGER NOT NULL,
               stack_id INTEGER NOT NULL,
               instance_id INTEGER,
               path TEXT NOT NULL,
               digest TEXT NOT NULL,
               bytes INTEGER NOT NULL);
             UPDATE registry_meta SET value = '17' WHERE key = 'schema_version';
             INSERT INTO release {release} VALUES
               ('c', '2026.09.01.1', '/r', '{{}}', '{{}}', 'c', 's', 'descriptive', '{{}}', \
                'mri', '1', 'a', '2026-09-01T10:00:00', '2026-09-01T10:01:00', 2, 1, 0, 0, 0, 1, 0),
               ('c', '2026.09.02.1', '/r', '{{}}', '{{}}', 'c', 's', 'descriptive', '{{}}', \
                'mri', '1', 'a', '2026-09-02T10:00:00', '2026-09-02T10:01:00', 2, 1, 0, 1, 0, 0, 0);
             INSERT INTO release_stack (release_id, stack_id, content, dir, stem, route, files) VALUES
               (1, 7, 'same', 'sub-x/ses-1/anat/T1w', NULL, 'raw', 2),
               (2, 7, 'same', 'sub-x/ses-1/anat/SC_T1w', NULL, 'raw', 2);
             INSERT INTO release_file (release_id, stack_id, instance_id, path, digest, bytes) VALUES
               (1, 7, 3, 'sub-x/ses-1/anat/T1w/00000003.dcm', 'a', 40),
               (1, 7, 4, 'sub-x/ses-1/anat/T1w/00000004.dcm', 'b', 2),
               (2, 7, 3, 'sub-x/ses-1/anat/SC_T1w/00000003.dcm', 'a', 40),
               (2, 7, 4, 'sub-x/ses-1/anat/SC_T1w/00000004.dcm', 'b', 2)"
        ))
        .unwrap();
    assert_eq!(
        migrate::standing(&mut store, Kind::Registry).unwrap(),
        Standing::Behind(17)
    );

    let applied = migrate::migrate(&mut store, Kind::Registry).unwrap();
    assert!(applied.contains(&18), "{applied:?}");

    // One dataset, which both versions are versions of.
    let datasets = store
        .query("SELECT id, name, root FROM dataset", &[])
        .unwrap();
    assert_eq!(datasets.len(), 1);
    assert_eq!(datasets[0].text(1).unwrap(), "c");
    assert_eq!(datasets[0].text(2).unwrap(), "/r");
    let dataset = datasets[0].int(0).unwrap();
    let rows = store
        .query("SELECT dataset_id FROM release ORDER BY id", &[])
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.int(0).unwrap() == dataset));

    // One current state for the stack: the newest version's place, the
    // manifest's bytes, and no digest, since a digest the old shape held per
    // file cannot be made into one per stack without reading the tree.
    let rows = store
        .query(
            "SELECT dataset_id, stack_id, release_id, dir, route, files, bytes, digest \
             FROM release_stack",
            &[],
        )
        .unwrap();
    assert_eq!(rows.len(), 1, "two versions of one stack are one state");
    let r = &rows[0];
    assert_eq!(r.int(0).unwrap(), dataset);
    assert_eq!(r.int(1).unwrap(), 7);
    assert_eq!(r.int(2).unwrap(), 2, "the newest version's");
    assert_eq!(r.text(3).unwrap(), "sub-x/ses-1/anat/SC_T1w");
    assert_eq!(r.text(4).unwrap(), "raw");
    assert_eq!(r.int(5).unwrap(), 2);
    assert_eq!(r.int(6).unwrap(), 42, "the bytes the manifest knew");
    assert_eq!(r.text(7).unwrap(), "");

    // The manifest is gone with it.
    assert!(
        store
            .query_opt(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'release_file'",
                &[],
            )
            .unwrap()
            .is_none()
    );

    // And the new shape holds one row per stack of a dataset, whatever the
    // number of versions: a second row for the same stack is refused.
    let again = store.insert(
        &Insert::new(
            schema::table("release_stack"),
            &[
                "dataset_id",
                "stack_id",
                "release_id",
                "content",
                "dir",
                "route",
                "files",
                "bytes",
                "digest",
            ],
        ),
        &[vec![
            Param::Int(dataset),
            Param::Int(7),
            Param::Int(3),
            Param::from("other"),
            Param::from("sub-x/ses-1/anat/T1w"),
            Param::from("raw"),
            Param::Int(2),
            Param::Int(42),
            Param::from("d"),
        ]],
    );
    assert!(again.is_err(), "one state per stack");
}

/// Wave 4a §6.1, fault 3: a date, a time, a timestamp or a JSON column
/// selected raw reads as the same text on both backends. The cast the
/// dialect renders stays right; forgetting it stops being a failure that
/// waits for the first row of that shape.
#[test]
fn a_raw_projection_of_a_date_a_time_a_timestamp_and_json_reads_as_text() {
    for (name, _guard, mut store) in stores() {
        let (date, time, ts, json) = match store {
            Store::Sqlite(_) => ("TEXT", "TEXT", "TEXT", "TEXT"),
            Store::Postgres { .. } => ("DATE", "TIME", "TIMESTAMPTZ", "JSONB"),
        };
        store
            .batch(&format!(
                "DROP TABLE IF EXISTS raw_shapes; \
                 CREATE TABLE raw_shapes (d {date}, t {time}, ts {ts}, j {json}); \
                 INSERT INTO raw_shapes (d, t, ts, j) VALUES \
                   ('2026-09-06', '03:25:45.678901', '2026-09-06T15:20:00Z', '{{\"a\": 1}}')"
            ))
            .unwrap();
        let rows = store
            .query("SELECT d, t, ts, j FROM raw_shapes", &[])
            .unwrap();
        assert_eq!(rows.len(), 1, "{name}");
        let r = &rows[0];
        assert_eq!(r.text(0).unwrap(), "2026-09-06", "{name}");
        assert_eq!(r.text(1).unwrap(), "03:25:45.678901", "{name}");
        assert_eq!(r.text(2).unwrap(), "2026-09-06T15:20:00Z", "{name}");
        let j: serde_json::Value = serde_json::from_str(r.text(3).unwrap()).unwrap();
        assert_eq!(j["a"], 1, "{name}");
        store.batch("DROP TABLE raw_shapes").unwrap();
    }
}

/// Wave 4a §6.1, fault 4: migration 20 splits a comma-joined axis value into
/// a row per value, and the new key lets two rows of one axis stand.
#[test]
fn migration_20_makes_an_axis_value_a_row() {
    let dir = TempDir::new("axis-rows");
    let path = dir.path().join("registry.db");
    let mut store = Store::open_sqlite(&path).unwrap();
    migrate::migrate(&mut store, Kind::Registry).unwrap();
    store
        .batch(
            "DROP TABLE classification_axis;
             CREATE TABLE classification_axis (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               stack_id INTEGER NOT NULL,
               axis TEXT NOT NULL,
               value TEXT,
               confidence REAL NOT NULL,
               tier TEXT NOT NULL);
             CREATE UNIQUE INDEX ux_old ON classification_axis (stack_id, axis);
             UPDATE registry_meta SET value = '19' WHERE key = 'schema_version';
             INSERT INTO classification_axis (stack_id, axis, value, confidence, tier) VALUES
               (7, 'role', 't1w,flair', 0.9, 'keywords'),
               (7, 'base', 'T1w', 0.95, 'exclusive'),
               (7, 'modifier', NULL, 0.0, 'default')",
        )
        .unwrap();
    let applied = migrate::migrate(&mut store, Kind::Registry).unwrap();
    assert!(applied.contains(&20), "{applied:?}");
    let rows = store
        .query(
            "SELECT axis, value FROM classification_axis WHERE stack_id = 7 ORDER BY axis, value",
            &[],
        )
        .unwrap();
    let got: Vec<(String, Option<String>)> = rows
        .iter()
        .map(|r| {
            (
                r.text(0).unwrap().to_string(),
                r.opt_text(1).unwrap().map(str::to_string),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            ("base".to_string(), Some("T1w".to_string())),
            ("modifier".to_string(), None),
            ("role".to_string(), Some("flair".to_string())),
            ("role".to_string(), Some("t1w".to_string())),
        ]
    );
    // The same value twice for one axis is refused; a third value is not.
    assert!(
        store
            .batch("INSERT INTO classification_axis (stack_id, axis, value, confidence, tier) VALUES (7, 'role', 't1w', 0.9, 'keywords')")
            .is_err()
    );
    store
        .batch("INSERT INTO classification_axis (stack_id, axis, value, confidence, tier) VALUES (7, 'role', 'swi', 0.9, 'keywords')")
        .unwrap();
}

/// Wave 4a §7.1: the clinical vocabulary is pack data, loaded by upsert on
/// both backends: a second load changes nothing, a changed description is
/// updated in place, and nothing is ever removed.
#[test]
fn a_vocabulary_loads_by_name_and_a_second_load_changes_nothing() {
    use nils_registry::clinical::{self, Vocabulary};
    for (name, _guard, mut store) in stores() {
        migrate::migrate(&mut store, Kind::Registry).unwrap();
        let v = Vocabulary::parse(
            "vocabulary:\n  diseases:\n    - name: MS\n      code: G35\n      types: [{name: RRMS}, {name: SPMS}]\n  observation_types:\n    - {name: EDSS, category: scale, value_type: numeric, unit: points, min: 0, max: 10, primary: true}\n    - {name: Diagnosis, category: assessment}\n    - {name: Delivery, category: event, sensitive: true}\n",
        )
        .unwrap();
        let first = clinical::load(&mut store, &v).unwrap();
        assert_eq!(first.diseases_added, 1, "{name}");
        assert_eq!(first.disease_types_added, 2, "{name}");
        assert_eq!(first.observation_types_added, 3, "{name}");
        let again = clinical::load(&mut store, &v).unwrap();
        assert_eq!(again.changed(), 0, "{name}: idempotent");

        let kinds = clinical::observation_types(&mut store).unwrap();
        assert_eq!(kinds.len(), 3, "{name}");
        let edss = kinds.iter().find(|k| k.name == "EDSS").unwrap();
        assert!(edss.primary, "{name}");
        assert!(!edss.sensitive, "{name}");
        assert_eq!(edss.unit.as_deref(), Some("points"), "{name}");
        // §7.4: the mark survives the load and is read back with the kind.
        let delivery = clinical::kind_named(&mut store, "delivery")
            .unwrap()
            .unwrap();
        assert!(delivery.sensitive, "{name}");
        assert!(!delivery.primary, "{name}");
        let diseases = clinical::diseases(&mut store).unwrap();
        assert_eq!(diseases[0].1.types.len(), 2, "{name}");

        // A changed description is an update; a dropped type is not a removal.
        let changed = Vocabulary::parse(
            "vocabulary:\n  diseases:\n    - name: MS\n      code: G35\n      description: multiple sclerosis\n      types: [{name: RRMS}]\n  observation_types:\n    - {name: EDSS, category: scale, value_type: numeric, unit: points, min: 0, max: 10, primary: true, description: the scale}\n",
        )
        .unwrap();
        let third = clinical::load(&mut store, &changed).unwrap();
        assert_eq!(third.diseases_updated, 1, "{name}");
        assert_eq!(third.observation_types_updated, 1, "{name}");
        assert_eq!(third.changed(), 2, "{name}");
        let diseases = clinical::diseases(&mut store).unwrap();
        assert_eq!(
            diseases[0].1.types.len(),
            2,
            "{name}: nothing is removed by a load"
        );
        assert_eq!(
            clinical::observation_types(&mut store).unwrap().len(),
            3,
            "{name}"
        );
        // Marking a kind sensitive is an update, and unmarking one is too.
        let marked = Vocabulary::parse(
            "vocabulary:\n  observation_types:\n    - {name: Diagnosis, category: assessment, sensitive: true}\n    - {name: Delivery, category: event}\n",
        )
        .unwrap();
        let fourth = clinical::load(&mut store, &marked).unwrap();
        assert_eq!(fourth.observation_types_updated, 2, "{name}");
        assert!(
            clinical::kind_named(&mut store, "Diagnosis")
                .unwrap()
                .unwrap()
                .sensitive,
            "{name}"
        );
        assert!(
            !clinical::kind_named(&mut store, "Delivery")
                .unwrap()
                .unwrap()
                .sensitive,
            "{name}"
        );
    }
}

/// Wave 4a §7.3: month zero from the clinical layer, and the nearest event
/// of a kind to a day, with its tie rule, on both backends.
#[test]
fn the_earliest_event_anchors_and_the_nearest_event_is_found_with_its_tie_rule() {
    use nils_registry::clinical::{self, Vocabulary};
    use nils_registry::day::Day;
    use nils_registry::schema;
    for (name, _guard, mut store) in stores() {
        migrate::migrate(&mut store, Kind::Registry).unwrap();
        let v = Vocabulary::parse(
            "vocabulary:\n  observation_types:\n    - {name: EDSS, category: scale, value_type: numeric}\n    - {name: Diagnosis, category: assessment}\n",
        )
        .unwrap();
        clinical::load(&mut store, &v).unwrap();
        let edss = clinical::kind_named(&mut store, "edss")
            .unwrap()
            .unwrap()
            .id;
        let diagnosis = clinical::kind_named(&mut store, "Diagnosis")
            .unwrap()
            .unwrap()
            .id;
        let subjects = store
            .insert(
                &Insert::new(schema::table("subject"), &["code", "created_at"]).returning(&["id"]),
                &[
                    vec![Param::from("a"), Param::from("2026-09-06T00:00:00Z")],
                    vec![Param::from("b"), Param::from("2026-09-06T00:00:00Z")],
                ],
            )
            .unwrap();
        let (a, b) = (subjects[0].int(0).unwrap(), subjects[1].int(0).unwrap());
        let event = |store: &mut Store,
                     subject: i64,
                     kind: i64,
                     date: &str,
                     number: Option<f64>,
                     superseded: Option<i64>|
         -> i64 {
            let rows = store
                .insert(
                    &Insert::new(
                        schema::table("event"),
                        &[
                            "subject_id",
                            "observation_type_id",
                            "event_date",
                            "number",
                            "created_at",
                            "superseded_by",
                        ],
                    )
                    .returning(&["id"]),
                    &[vec![
                        Param::Int(subject),
                        Param::Int(kind),
                        Param::from(date),
                        number.map_or(Param::Null, Param::Double),
                        Param::from("2026-09-06T00:00:00Z"),
                        superseded.map_or(Param::Null, Param::Int),
                    ]],
                )
                .unwrap();
            rows[0].int(0).unwrap()
        };
        // Two diagnoses for a, the later one superseding nothing: the
        // earliest anchors. A superseded one does not count.
        event(&mut store, a, diagnosis, "2020-03-01", None, None);
        event(&mut store, a, diagnosis, "2019-06-15", None, None);
        let dead = event(&mut store, a, diagnosis, "2010-01-01", None, None);
        let newer = event(&mut store, a, diagnosis, "2020-03-01", None, None);
        store
            .execute(
                &format!(
                    "UPDATE {} SET superseded_by = {newer} WHERE id = {dead}",
                    store.qualified("event")
                ),
                &[],
            )
            .unwrap();
        let anchors = clinical::anchor_events(&mut store, diagnosis).unwrap();
        assert_eq!(anchors.get("a").copied(), Day::parse("20190615"), "{name}");
        assert!(!anchors.contains_key("b"), "{name}: b has no diagnosis");

        // EDSS at 2022-01-01 (3.0), 2022-03-01 (3.5), 2022-05-01 (4.0).
        event(&mut store, a, edss, "2022-01-01", Some(3.0), None);
        event(&mut store, a, edss, "2022-03-01", Some(3.5), None);
        event(&mut store, a, edss, "2022-05-01", Some(4.0), None);
        let near = |store: &mut Store, day: &str| {
            clinical::nearest(store, a, edss, Day::parse(day).unwrap()).unwrap()
        };
        let n = near(&mut store, "20220310").unwrap();
        assert_eq!(n.number, Some(3.5), "{name}");
        assert_eq!(
            n.offset_days, -9,
            "{name}: nine days before the day asked about"
        );
        let n = near(&mut store, "20220420").unwrap();
        assert_eq!(
            n.number,
            Some(4.0),
            "{name}: eleven days on beats fifty back"
        );
        // The tie: an EDSS on the last day of January too, and the sixteenth
        // is fifteen days from both; the earlier one wins.
        event(&mut store, a, edss, "2022-01-31", Some(3.2), None);
        let n = near(&mut store, "20220116").unwrap();
        assert_eq!(
            n.number,
            Some(3.0),
            "{name}: the earlier of two equidistant"
        );
        assert!(
            near(&mut store, "19990101").is_some(),
            "{name}: far is still nearest"
        );
        assert!(
            clinical::nearest(&mut store, b, edss, Day::parse("20220201").unwrap())
                .unwrap()
                .is_none(),
            "{name}"
        );
    }
}
