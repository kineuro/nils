// SPDX-License-Identifier: AGPL-3.0-only
//! The Data page's doors on a dataset of millions of files (2026-10-17): a
//! dataset that finished reading made its summary, the sources door and the
//! held IDs door slow, each request scanning every file of it. Here a
//! dataset is brought in by the engine's own commands on a small tree, then
//! grown by SQL to `NILS_BENCH_FILES` files (three million by default) in
//! some 45,000 scans of 440 subjects, with their pseudonymised files,
//! pictures and 3D views, and the three doors are timed through a real
//! `nils serve`: a first answer, answers after it, answers asked at once,
//! and answers read again once the registry's epoch moved. A measurement,
//! run by hand on a Postgres database of its own, which it fills (the
//! registry in schema `bench`, its home in `NILS_BENCH_HOME`, both kept, so
//! that a later engine is timed on the same registry after it migrates it,
//! and the time it takes to migrate is printed):
//!
//! `NILS_BENCH_POSTGRES_DSN=postgres://.../db NILS_BENCH_HOME=/some/dir
//!  cargo test --release -p nils --test summary_speed -- --ignored --nocapture`
//!
//! A Postgres whose shared memory is small (a container's 64 MB) cannot
//! vacuum or build an index in parallel over millions of rows: set
//! `max_parallel_maintenance_workers` to 0 on the database first.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields};
use nils_registry::Store;
use nils_registry::store::Param;
use serde_json::Value;

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../packs")
        .to_str()
        .unwrap()
        .to_string()
}

const SCHEMA: &str = "bench";
const OPS: &str = "an-operator-token-of-length";
/// Scans a visit, visits a subject and files a scan, as the dataset this
/// stands for has them (30.65 million files, 456,000 scans, 4,399
/// subjects), a tenth of its subjects by default.
const SCANS_A_STUDY: i64 = 26;
const STUDIES_A_SUBJECT: i64 = 4;
const FILES_A_SCAN: i64 = 66;
/// Where the grown rows' ids begin.
const BASE: i64 = 10_000_000;

struct Bench {
    home: PathBuf,
    dsn: String,
}

impl Bench {
    fn run(&self, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
        let mut child = nils()
            .arg("--registry")
            .arg(&self.home)
            .args(args)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_DSN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        match stdin {
            Some(text) => child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap(),
            None => drop(child.stdin.take()),
        }
        let out = child.wait_with_output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }

    fn ok(&self, args: &[&str]) -> String {
        let (good, out, err) = self.run(args, None);
        assert!(good, "nils {args:?} failed: {err}\n{out}");
        out
    }

    fn store(&self) -> Store {
        Store::connect_postgres(&self.dsn, SCHEMA).unwrap()
    }
}

fn one(store: &mut Store, sql: &str) -> i64 {
    store
        .query(&sql.replace("{s}", SCHEMA), &[])
        .unwrap_or_else(|e| panic!("{sql}: {e}"))[0]
        .opt_int(0)
        .unwrap()
        .unwrap_or(0)
}

fn exec(store: &mut Store, sql: &str) -> u64 {
    let started = Instant::now();
    let n = store
        .execute(&sql.replace("{s}", SCHEMA), &[])
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    eprintln!(
        "  {n:>9} rows in {:>6.1} s: {}",
        started.elapsed().as_secs_f64(),
        sql.split_whitespace().take(4).collect::<Vec<_>>().join(" ")
    );
    n
}

/// Copies of the rows `from` selects (aliased `t`, the copy's number `g`),
/// each column the template's unless `set` gives it.
fn copy(store: &mut Store, table: &str, from: &str, set: &[(&str, String)]) -> u64 {
    let columns: Vec<String> = store
        .query(
            "SELECT column_name FROM information_schema.columns \
             WHERE table_schema = $1 AND table_name = $2 ORDER BY ordinal_position",
            &[Param::from(SCHEMA), Param::from(table)],
        )
        .unwrap()
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect();
    assert!(!columns.is_empty(), "no table {table}");
    let values: Vec<String> = columns
        .iter()
        .map(|c| {
            set.iter()
                .find(|(n, _)| n == c)
                .map(|(_, e)| e.clone())
                .unwrap_or_else(|| format!("t.\"{c}\""))
        })
        .collect();
    let quoted: Vec<String> = columns.iter().map(|c| format!("\"{c}\"")).collect();
    exec(
        store,
        &format!(
            "INSERT INTO {{s}}.{table} ({}) OVERRIDING SYSTEM VALUE SELECT {} FROM {from}",
            quoted.join(", "),
            values.join(", ")
        ),
    )
}

/// A file of the originals: a slice of a T1, or an object a read sets
/// aside (`class`).
fn original(dir: &Path, patient: &str, study: &str, series: &str, n: u32, class: Option<&str>) {
    let sop = format!("{series}.{n}");
    let mut e = synth::minimal_mr(study, series, &sop);
    let meta = match class {
        Some(class) => {
            e.retain(|x| x.tag != tags::SOP_CLASS_UID);
            e.push(synth::text(tags::SOP_CLASS_UID, VR::UI, class));
            MetaFields::with("1.2.840.10008.1.2.1", class, &sop)
        }
        None => MetaFields::mr(&sop),
    };
    e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
    e.push(synth::text(tags::STUDY_DATE, VR::DA, "20240131"));
    e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
    e.push(synth::text(tags::SERIES_NUMBER, VR::IS, "1"));
    e.push(synth::text(tags::INSTANCE_NUMBER, VR::IS, &n.to_string()));
    e.push(synth::text(
        tags::IMAGE_POSITION_PATIENT,
        VR::DS,
        &format!("0\\0\\{}", n * 2),
    ));
    e.push(synth::text(
        tags::IMAGE_ORIENTATION_PATIENT,
        VR::DS,
        "1\\0\\0\\0\\1\\0",
    ));
    e.push(synth::text(tags::PIXEL_SPACING, VR::DS, "1\\1"));
    e.push(synth::text(tags::SLICE_THICKNESS, VR::DS, "1"));
    let path = dir.join(patient).join(&sop);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, synth::part10(&meta, &e, true)).unwrap();
}

/// The registry brought in by the engine on a small tree, then grown by
/// SQL; once, kept for the next run.
fn grow(bench: &Bench, files: i64) {
    let grown = bench.home.join("grown");
    if grown.exists() {
        eprintln!("the registry grown before: {}", read(&grown));
        return;
    }
    std::fs::create_dir_all(&bench.home).unwrap();
    let (good, _, err) = bench.run(&["key", "add", "k"], Some("a summary bench key\n"));
    assert!(good, "{err}");
    bench.ok(&[
        "init",
        "--backend",
        "postgres",
        "--dsn",
        &bench.dsn,
        "--schema",
        SCHEMA,
        "--key",
        "k",
    ]);
    let ds = bench.home.join("ds");
    for (p, patient) in ["PAT-A1", "PAT-B2"].iter().enumerate() {
        let study = format!("1.2.826.0.1.3680043.8.499.7{p}");
        for n in 1..=3 {
            original(&ds, patient, &study, &format!("{study}.1"), n, None);
        }
        original(
            &ds,
            patient,
            &study,
            &format!("{study}.9"),
            1,
            Some("1.2.840.10008.5.1.4.1.1.11.1"),
        );
    }
    bench.ok(&[
        "place",
        "add",
        "ds",
        ds.to_str().unwrap(),
        "--role",
        "source",
        "--move-into",
        "originals",
        "--confirm-move",
        "--unmapped",
        "code",
    ]);
    let work = bench.home.join("work");
    std::fs::create_dir_all(&work).unwrap();
    bench.ok(&[
        "place",
        "add",
        "work",
        work.to_str().unwrap(),
        "--role",
        "working",
    ]);
    bench.ok(&["pseudonymize", "@ds"]);
    bench.ok(&["digest", "--name", "ds", "--no-private", "@ds"]);
    bench.ok(&["fingerprint"]);
    bench.ok(&["classify", "--pack-dir", &packs()]);

    let mut store = bench.store();
    let place = one(&mut store, "SELECT id FROM {s}.place WHERE name = 'ds'");
    let source = one(&mut store, "SELECT MIN(id) FROM {s}.source");
    let digest = one(
        &mut store,
        &format!(
            "SELECT MAX(id) FROM {{s}}.ingest_batch WHERE source_id = {source} \
             AND (kind IS NULL OR kind = 'digest')"
        ),
    );
    let pseudonymize = one(
        &mut store,
        "SELECT MAX(id) FROM {s}.ingest_batch WHERE kind = 'pseudonymize'",
    );
    let stack = one(
        &mut store,
        "SELECT MIN(x.id) FROM {s}.stack x JOIN {s}.classification c ON c.stack_id = x.id",
    );
    let series = one(
        &mut store,
        &format!("SELECT series_id FROM {{s}}.stack WHERE id = {stack}"),
    );
    let study = one(
        &mut store,
        &format!("SELECT study_id FROM {{s}}.series WHERE id = {series}"),
    );
    let subject = one(
        &mut store,
        &format!("SELECT subject_id FROM {{s}}.series WHERE id = {series}"),
    );
    let file = one(
        &mut store,
        &format!(
            "SELECT MIN(id) FROM {{s}}.source_file WHERE source_id = {source} AND status = 'ingested'"
        ),
    );
    let written = one(
        &mut store,
        &format!(
            "SELECT MIN(id) FROM {{s}}.pseudonym_file WHERE place_id = {place} AND state = 'written'"
        ),
    );
    let instance = one(&mut store, "SELECT MIN(id) FROM {s}.instance");
    let rows = store
        .query(
            &format!(
                "SELECT f.path, p.out_path FROM {SCHEMA}.source_file f, {SCHEMA}.pseudonym_file p \
                 WHERE f.id = {file} AND p.id = {written}"
            ),
            &[],
        )
        .unwrap();
    eprintln!(
        "templates: place {place}, source {source}, batches {digest} and {pseudonymize}, \
         stack {stack}, file {:?}, written {:?}",
        rows[0].opt_text(0).unwrap(),
        rows[0].opt_text(1).unwrap()
    );

    let subjects = (files / (FILES_A_SCAN * SCANS_A_STUDY * STUDIES_A_SUBJECT)).max(1);
    let studies = subjects * STUDIES_A_SUBJECT;
    let scans = studies * SCANS_A_STUDY;
    let files = scans * FILES_A_SCAN;
    let a_subject = SCANS_A_STUDY * STUDIES_A_SUBJECT;
    let g = |n: i64| format!("generate_series(0, {}) g", n - 1);
    copy(
        &mut store,
        "subject",
        &format!(
            "{{s}}.subject t CROSS JOIN {} WHERE t.id = {subject}",
            g(subjects + 5)
        ),
        &[
            ("id", format!("{BASE} + g")),
            ("code", "'BENCH-' || g".into()),
            ("code_digest", "NULL".into()),
            ("merged_into", "NULL".into()),
            ("merged_at", "NULL".into()),
            (
                "provisional",
                "CASE WHEN g % 20 = 0 THEN 1 ELSE t.provisional END".into(),
            ),
        ],
    );
    copy(
        &mut store,
        "study",
        &format!(
            "{{s}}.study t CROSS JOIN {} WHERE t.id = {study}",
            g(studies)
        ),
        &[
            ("id", format!("{BASE} + g")),
            ("study_instance_uid", "'2.25.9.' || g".into()),
            ("subject_id", format!("{BASE} + g / {STUDIES_A_SUBJECT}")),
        ],
    );
    copy(
        &mut store,
        "series",
        &format!(
            "{{s}}.series t CROSS JOIN {} WHERE t.id = {series}",
            g(scans)
        ),
        &[
            ("id", format!("{BASE} + g")),
            ("series_instance_uid", "'2.25.8.' || g".into()),
            ("study_id", format!("{BASE} + g / {SCANS_A_STUDY}")),
            ("subject_id", format!("{BASE} + g / {a_subject}")),
            ("n_instances", FILES_A_SCAN.to_string()),
            ("n_stacks", "1".into()),
        ],
    );
    copy(
        &mut store,
        "stack",
        &format!("{{s}}.stack t CROSS JOIN {} WHERE t.id = {stack}", g(scans)),
        &[
            ("id", format!("{BASE} + g")),
            ("series_id", format!("{BASE} + g")),
            ("n_instances", FILES_A_SCAN.to_string()),
        ],
    );
    copy(
        &mut store,
        "stack_fingerprint",
        &format!(
            "{{s}}.stack_fingerprint t CROSS JOIN {} WHERE t.stack_id = {stack}",
            g(scans)
        ),
        &[
            ("id", format!("{BASE} + g")),
            ("stack_id", format!("{BASE} + g")),
            ("series_id", format!("{BASE} + g")),
            ("study_id", format!("{BASE} + g / {SCANS_A_STUDY}")),
            ("subject_id", format!("{BASE} + g / {a_subject}")),
        ],
    );
    copy(
        &mut store,
        "classification",
        &format!(
            "{{s}}.classification t CROSS JOIN {} WHERE t.stack_id = {stack}",
            g(scans)
        ),
        &[
            ("id", format!("{BASE} + g")),
            ("stack_id", format!("{BASE} + g")),
        ],
    );
    copy(
        &mut store,
        "classification_axis",
        &format!(
            "(SELECT a.*, ROW_NUMBER() OVER (ORDER BY a.id) - 1 AS rn FROM {{s}}.classification_axis a \
             WHERE a.stack_id = {stack}) t CROSS JOIN {}",
            g(scans)
        ),
        &[
            ("id", format!("{BASE} + g * 64 + t.rn")),
            ("stack_id", format!("{BASE} + g")),
            (
                "value",
                "CASE WHEN t.axis = 'base' THEN \
                 (ARRAY['T1w', 'T2w', 'FLAIR', 'DWI', 'SWI', 'T2starw'])[1 + g % 6] ELSE t.value END"
                    .into(),
            ),
        ],
    );
    if one(
        &mut store,
        &format!(
            "SELECT COUNT(*) FROM {{s}}.classification_axis WHERE stack_id = {stack} AND axis = 'body_region'"
        ),
    ) == 0
    {
        exec(
            &mut store,
            &format!(
                "INSERT INTO {{s}}.classification_axis (id, stack_id, axis, value, confidence, tier) \
                 OVERRIDING SYSTEM VALUE SELECT {BASE} + g * 64 + 63, {BASE} + g, 'body_region', \
                 (ARRAY['head', 'neck', 'spine'])[1 + g % 3], 1.0, 'rule' FROM {}",
                g(scans)
            ),
        );
    }
    copy(
        &mut store,
        "source_stack",
        &format!(
            "{{s}}.source_stack t CROSS JOIN {} WHERE t.stack_id = {stack} AND t.source_id = {source}",
            g(scans)
        ),
        &[
            ("id", format!("{BASE} + g")),
            ("stack_id", format!("{BASE} + g")),
        ],
    );
    // the files: in a thousand, ten copies of an instance (half of them
    // another dataset's, half this one's), seven set aside by policy, one
    // held for its instance and one let go, three refused, one gone
    let scan = format!("{BASE} + g / {FILES_A_SCAN}");
    copy(
        &mut store,
        "source_file",
        &format!(
            "{{s}}.source_file t CROSS JOIN {} WHERE t.id = {file}",
            g(files)
        ),
        &[
            ("id", format!("{BASE} + g")),
            (
                "path",
                format!(
                    "'bench/' || (g / {FILES_A_SCAN}) || '/' || (g % {FILES_A_SCAN}) || '.dcm'"
                ),
            ),
            ("dir", format!("'bench/' || (g / {FILES_A_SCAN})")),
            (
                "status",
                "CASE WHEN g % 1000 < 10 THEN 'duplicate' WHEN g % 1000 < 21 THEN 'quarantined' \
                 WHEN g % 1000 = 21 THEN 'gone' ELSE 'ingested' END"
                    .into(),
            ),
            (
                "reason",
                "CASE WHEN g % 1000 BETWEEN 10 AND 14 THEN 'unsupported_sop_class' \
                 WHEN g % 1000 BETWEEN 15 AND 16 THEN 'unsupported_modality' \
                 WHEN g % 1000 = 17 THEN 'identity.same_instance' \
                 WHEN g % 1000 = 18 THEN 'identity.same_instance.drop' \
                 WHEN g % 1000 BETWEEN 19 AND 20 THEN 'parse_error' ELSE NULL END"
                    .into(),
            ),
            (
                "detail",
                "CASE WHEN g % 1000 BETWEEN 10 AND 14 THEN (ARRAY['1.2.840.10008.5.1.4.1.1.11.1', \
                 '1.2.840.10008.5.1.4.1.1.7', '1.2.840.10008.5.1.4.1.1.88.11'])[1 + g % 3] \
                 WHEN g % 1000 BETWEEN 15 AND 16 THEN 'SR' \
                 WHEN g % 1000 BETWEEN 19 AND 20 THEN 'the header ends early' ELSE NULL END"
                    .into(),
            ),
            (
                "instance_id",
                format!("CASE WHEN g % 1000 < 10 THEN {BASE} + g ELSE NULL END"),
            ),
            ("batch_id", digest.to_string()),
        ],
    );
    copy(
        &mut store,
        "instance",
        &format!(
            "{{s}}.instance t CROSS JOIN {} WHERE t.id = {instance} AND g % 1000 < 10",
            g(files)
        ),
        &[
            ("id", format!("{BASE} + g")),
            ("sop_instance_uid", "'2.25.7.' || g".into()),
            ("series_id", scan.clone()),
            ("stack_id", scan.clone()),
            (
                "source_file_id",
                format!("CASE WHEN g % 2 = 0 THEN NULL ELSE {BASE} + g + 100 END"),
            ),
        ],
    );
    // the originals as the pseudonymiser recorded them, each copy where the
    // read found it; in a thousand, five unchanged, five refused and five
    // held, the held ones under a hundred and fifty identifiers
    copy(
        &mut store,
        "pseudonym_file",
        &format!(
            "{{s}}.pseudonym_file t CROSS JOIN {} WHERE t.id = {written}",
            g(files)
        ),
        &[
            ("id", format!("{BASE} + g")),
            (
                "path",
                format!("'orig/' || (g / {FILES_A_SCAN}) || '/' || (g % {FILES_A_SCAN}) || '.dcm'"),
            ),
            ("dir", format!("'orig/' || (g / {FILES_A_SCAN})")),
            (
                "state",
                "CASE WHEN g % 1000 < 985 THEN 'written' WHEN g % 1000 < 990 THEN 'unchanged' \
                 WHEN g % 1000 < 995 THEN 'refused' ELSE 'held' END"
                    .into(),
            ),
            (
                "out_path",
                format!(
                    "CASE WHEN g % 1000 < 990 THEN 'bench/' || (g / {FILES_A_SCAN}) || '/' || \
                     (g % {FILES_A_SCAN}) || '.dcm' ELSE NULL END"
                ),
            ),
            (
                "subject_id",
                format!(
                    "CASE WHEN g % 1000 < 990 THEN {BASE} + g / {FILES_A_SCAN} / {a_subject} ELSE NULL END"
                ),
            ),
            (
                "lookup",
                "CASE WHEN g % 1000 >= 995 THEN decode(md5((g / 20000)::text), 'hex') ELSE NULL END"
                    .into(),
            ),
            (
                "shape",
                "CASE WHEN g % 1000 >= 995 THEN '999999999999' ELSE NULL END".into(),
            ),
            (
                "id_type",
                "CASE WHEN g % 1000 >= 995 THEN 'patient-id' ELSE t.id_type END".into(),
            ),
            ("batch_id", pseudonymize.to_string()),
        ],
    );
    // five subjects whose files here were all set aside: no scan of theirs
    let nostack = format!("{BASE} + {subjects} + g / 3");
    copy(
        &mut store,
        "source_file",
        &format!(
            "{{s}}.source_file t CROSS JOIN {} WHERE t.id = {file}",
            g(15)
        ),
        &[
            ("id", format!("{BASE} + {files} + g")),
            ("path", "'bench/nostack/' || g || '.dcm'".into()),
            ("dir", "'bench/nostack'".into()),
            ("status", "'quarantined'".into()),
            ("reason", "'unsupported_sop_class'".into()),
            ("detail", "'1.2.840.10008.5.1.4.1.1.7'".into()),
            ("instance_id", "NULL".into()),
            ("batch_id", digest.to_string()),
        ],
    );
    copy(
        &mut store,
        "pseudonym_file",
        &format!(
            "{{s}}.pseudonym_file t CROSS JOIN {} WHERE t.id = {written}",
            g(15)
        ),
        &[
            ("id", format!("{BASE} + {files} + g")),
            ("path", "'orig/nostack/' || g || '.dcm'".into()),
            ("dir", "'orig/nostack'".into()),
            ("state", "'written'".into()),
            ("out_path", "'bench/nostack/' || g || '.dcm'".into()),
            ("subject_id", nostack),
            ("batch_id", pseudonymize.to_string()),
        ],
    );
    // the jobs of other datasets a busy registry holds beside its own
    exec(
        &mut store,
        "INSERT INTO {s}.job (kind, name, args, state, started_at, finished_at) \
         SELECT (ARRAY['digest', 'classify', 'pyramid', 'preview'])[1 + g % 4], 'other-' || g, \
         jsonb_build_array('nils', 'digest', '@other-' || (g % 7)), 'done', \
         now() - interval '2 days', now() - interval '1 day' FROM generate_series(1, 1500) g",
    );
    for table in [
        "subject",
        "study",
        "series",
        "stack",
        "stack_fingerprint",
        "classification",
        "classification_axis",
        "source_stack",
        "source_file",
        "instance",
        "pseudonym_file",
    ] {
        store
            .query(
                &format!(
                    "SELECT setval(pg_get_serial_sequence('{SCHEMA}.{table}', 'id'), \
                     (SELECT MAX(id) FROM {SCHEMA}.{table}))"
                ),
                &[],
            )
            .unwrap();
    }
    let started = Instant::now();
    store.batch("VACUUM (ANALYZE, PARALLEL 0)").unwrap();
    eprintln!(
        "vacuum and analyze: {:.1} s",
        started.elapsed().as_secs_f64()
    );
    // a picture and a 3D view of every scan
    let started = Instant::now();
    for k in 0..scans {
        let id = BASE + k;
        let preview = work
            .join("previews")
            .join(format!("{:03}", id.rem_euclid(1000)))
            .join(format!("{id}.preview"));
        std::fs::create_dir_all(preview.parent().unwrap()).unwrap();
        std::fs::write(&preview, b"bench").unwrap();
        let pyramid = work.join("pyramids").join(id.to_string());
        std::fs::create_dir_all(&pyramid).unwrap();
        std::fs::write(pyramid.join("manifest.json"), b"{}").unwrap();
    }
    eprintln!(
        "{scans} pictures and views written: {:.1} s",
        started.elapsed().as_secs_f64()
    );
    let line = format!(
        "{subjects} subjects, {studies} studies, {scans} scans, {files} files, place {place}, source {source}"
    );
    std::fs::write(&grown, &line).unwrap();
    eprintln!("grown: {line}");
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// A `nils serve`, killed when dropped.
struct Served {
    child: std::process::Child,
    port: u16,
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Served {
    fn start(bench: &Bench) -> Served {
        use std::io::BufRead as _;
        let child = nils()
            .arg("--registry")
            .arg(&bench.home)
            .args(["serve", "--bind", "127.0.0.1:0", "--workers", "16"])
            .args(["--auth", "token", "--pack-dir", &packs()])
            .env("NILS_TOKENS", format!("{OPS}=ops@lab:operator"))
            .env("NILS_PACK_DIR", packs())
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_JOB_ID")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut held = Served { child, port: 0 };
        let stdout = held.child.stdout.take().unwrap();
        let mut lines = std::io::BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            panic!("nils serve did not listen");
        };
        let addr = first.split_whitespace().nth(2).unwrap();
        held.port = addr.rsplit(':').next().unwrap().parse().unwrap();
        std::thread::spawn(move || for _ in lines {});
        held
    }

    fn get(&self, path: &str) -> (Duration, Value) {
        get(self.port, path)
    }
}

fn get(port: u16, path: &str) -> (Duration, Value) {
    use std::io::Read as _;
    let started = Instant::now();
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let head = format!(
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {OPS}\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    let took = started.elapsed();
    let response = String::from_utf8_lossy(&bytes).to_string();
    let (headers, text) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let status: u16 = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
    assert_eq!(status, 200, "{path}: {text}");
    (took, serde_json::from_str(text).unwrap())
}

fn ms(d: Duration) -> String {
    format!("{:.0} ms", d.as_secs_f64() * 1000.0)
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// A door asked once, then `again` times one after another, then
/// `together` times at once.
fn time(served: &Served, label: &str, path: &str, again: usize, together: usize) -> Value {
    let (first, doc) = served.get(path);
    let after: Vec<Duration> = (0..again).map(|_| served.get(path).0).collect();
    let port = served.port;
    let started = Instant::now();
    let each: Vec<Duration> = (0..together)
        .map(|_| {
            let path = path.to_string();
            std::thread::spawn(move || get(port, &path).0)
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    let wall = started.elapsed();
    eprintln!(
        "{label:<22} first {:>9}  then median {:>9} (max {:>9})  {together} at once: {:>9} for all, slowest {:>9}",
        ms(first),
        ms(median(after.clone())),
        ms(after.iter().max().copied().unwrap_or_default()),
        ms(wall),
        ms(each.iter().max().copied().unwrap_or_default()),
    );
    doc
}

#[test]
#[ignore = "a measurement on a database it fills, run by hand"]
fn the_data_page_doors_on_a_dataset_of_millions_of_files() {
    let Some(dsn) = std::env::var("NILS_BENCH_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_BENCH_POSTGRES_DSN is not set: nothing measured");
        return;
    };
    let home = std::env::var("NILS_BENCH_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("nils-summary-bench"));
    let files: i64 = std::env::var("NILS_BENCH_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3_000_000);
    let bench = Bench { home, dsn };
    grow(&bench, files);
    // the engine opens the registry, and migrates it where it is behind
    let started = Instant::now();
    bench.ok(&["status"]);
    eprintln!(
        "the registry opened (and migrated) in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    let mut store = bench.store();
    for (what, sql) in [
        (
            "files of the source",
            "SELECT COUNT(*) FROM {s}.source_file",
        ),
        (
            "pseudonymised files",
            "SELECT COUNT(*) FROM {s}.pseudonym_file",
        ),
        ("scans", "SELECT COUNT(*) FROM {s}.stack"),
    ] {
        eprintln!("{what:<22} {}", one(&mut store, sql));
    }
    let served = Served::start(&bench);
    let summary = time(&served, "summary", "/api/datasets/ds/summary", 5, 10);
    let sources = time(&served, "sources door", "/api/sources", 5, 10);
    let held = time(
        &served,
        "held IDs door",
        "/api/linkage/held/ids?place=ds",
        3,
        4,
    );
    // read again: the registry's epoch moved, as a judgement moves it, so
    // nothing kept holds; one request, then ten at once
    let mut moved = || {
        store
            .execute(
                &format!(
                    "UPDATE {SCHEMA}.registry_meta SET value = CAST(CAST(value AS BIGINT) + 1 AS TEXT) \
                     WHERE key = 'epoch'"
                ),
                &[],
            )
            .unwrap();
    };
    for (label, path) in [
        ("summary", "/api/datasets/ds/summary"),
        ("sources door", "/api/sources"),
    ] {
        let mut fresh = Vec::new();
        for _ in 0..3 {
            moved();
            fresh.push(served.get(path).0);
        }
        moved();
        let port = served.port;
        let started = Instant::now();
        let each: Vec<Duration> = (0..10)
            .map(|_| {
                let path = path.to_string();
                std::thread::spawn(move || get(port, &path).0)
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect();
        eprintln!(
            "{label:<22} read again: median {:>9} (max {:>9})  10 at once: {:>9} for all, slowest {:>9}",
            ms(median(fresh.clone())),
            ms(fresh.iter().max().copied().unwrap_or_default()),
            ms(started.elapsed()),
            ms(each.iter().max().copied().unwrap_or_default()),
        );
    }
    drop(store);
    eprintln!(
        "summary: {} scans, {} subjects, files {}",
        summary["scans"], summary["subjects"], summary["files"]
    );
    eprintln!(
        "sources door: {} datasets; held IDs door: {} identifiers, subjects {}",
        sources["count"], held["identifiers"], held["subjects"]
    );
    let out = bench.home.join("answers");
    std::fs::create_dir_all(&out).unwrap();
    let stamp = std::env::var("NILS_BENCH_LABEL").unwrap_or_else(|_| "run".into());
    for (name, doc) in [
        ("summary", &summary),
        ("sources", &sources),
        ("held", &held),
    ] {
        std::fs::write(
            out.join(format!("{stamp}-{name}.json")),
            serde_json::to_string_pretty(doc).unwrap(),
        )
        .unwrap();
    }
}
