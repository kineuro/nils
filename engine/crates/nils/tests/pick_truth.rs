// SPDX-License-Identifier: AGPL-3.0-only
//! One place of truth for the main scans (Nima, 2026-10-11: "chat and the
//! page should not disagree and we have to have one place of truth"). One
//! registry of two datasets: `alpha` with rules of its own saved (2D SE
//! first) and run, `beta` with none, and a cohort `trio` across both with
//! rules of its own (2D TSE first). The pack's own rules take the 3D
//! MPRAGE of every visit. Every reader of the main scans names the same
//! stacks for the same scope: the Main scans page's doors, the ask's
//! `picked`, the viewer, the picks summary and the dataset summary, the
//! command line, a release, and the pipelines' pre-flight; with no scope
//! named, each subject's own dataset's. On SQLite always, and on Postgres
//! where a test DSN is set.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write as _;
use std::process::{Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_registry::Store;
use serde_json::{Value, json};

fn packs() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../packs")
        .to_str()
        .unwrap()
        .to_string()
}

const OPS: &str = "an-operator-token-of-length";

/// A stand-in podman that answers what a pre-flight asks of it.
const FAKE_PODMAN: &str = "#!/bin/sh\ncase \"$1\" in\n  info) case \"$*\" in *GraphRoot*) echo /fake/containers/storage ;; *) echo true ;; esac ;;\n  *) echo \"podman version 9.9.9-test\" ;;\nesac\nexit 0\n";

/// A bids pipeline whose input is each session's T1w, as the pre-flight
/// counts it.
const NEEDS_T1W: &str = r#"name: needs-t1w
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/needs-t1w@sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
command-line: "true [InputDataset] [OutputLocation]"
x-nils:
  analysis-level: session
  input: {layout: bids, roles: [t1w]}
  units: apart
  outputs:
    - id: n4
      kind: output
      path-template: "sub-{subject}/ses-{session}/anat/*_desc-n4_T1w.nii.gz"
  needs: {cores: 1, memory-gb: 1, unit-minutes: 2}
"#;

/// Each visit's three T1w series: number, description, dimension, the
/// technique the test gives it.
const SERIES: [(&str, &str, &str, &str); 3] = [
    ("1", "t1_mprage_sag", "3D", "MPRAGE"),
    ("2", "t1_se_tra", "2D", "SE"),
    ("3", "t1_tse_tra", "2D", "TSE"),
];

/// A dataset's folder under the root, already pseudonymised: each
/// person's visits, three T1w series a visit.
fn tree(dir: &TempDir, dataset: &str, people: &[(&str, &[&str], &str)]) {
    for &(patient, days, root) in people {
        for (v, day) in days.iter().enumerate() {
            let study = format!("{root}.{}", v + 1);
            for (n, description, dimension, _) in SERIES {
                let series = format!("{study}.{n}");
                for slice in 1..=12u32 {
                    let sop = format!("{series}.{slice}");
                    let mut e = synth::minimal_mr(&study, &series, &sop);
                    let pixels: Vec<u8> = (0..32u32 * 32)
                        .flat_map(|i| (200 + (i % 32) as u16 * 7 + slice as u16).to_le_bytes())
                        .collect();
                    e.extend([
                        synth::text(tags::PATIENT_ID, VR::LO, patient),
                        synth::text(tags::STUDY_DATE, VR::DA, day),
                        synth::text(tags::SERIES_TIME, VR::TM, &format!("0{n}1415")),
                        synth::text(tags::SERIES_NUMBER, VR::IS, n),
                        synth::text(tags::SERIES_DESCRIPTION, VR::LO, description),
                        synth::text(tags::PROTOCOL_NAME, VR::LO, description),
                        synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, dimension),
                        synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
                        synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
                        synth::text(tags::BURNED_IN_ANNOTATION, VR::CS, "NO"),
                        synth::us(tags::ROWS, 32),
                        synth::us(tags::COLUMNS, 32),
                        synth::us(tags::BITS_ALLOCATED, 16),
                        synth::us(tags::BITS_STORED, 12),
                        synth::us(tags::HIGH_BIT, 11),
                        synth::us(tags::PIXEL_REPRESENTATION, 0),
                        synth::us(tags::SAMPLES_PER_PIXEL, 1),
                        synth::text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
                        synth::text(tags::PIXEL_SPACING, VR::DS, "8.0\\8.0"),
                        synth::text(tags::SLICE_THICKNESS, VR::DS, "1.0"),
                        synth::text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "0\\1\\0\\0\\0\\-1"),
                        synth::text(
                            tags::IMAGE_POSITION_PATIENT,
                            VR::DS,
                            &format!("{slice}\\0\\0"),
                        ),
                        synth::text(tags::INSTANCE_NUMBER, VR::IS, &slice.to_string()),
                        synth::bytes(tags::PIXEL_DATA, VR::OW, pixels),
                    ]);
                    dir.file(
                        &format!("{dataset}/derivatives/dcm-anon/{patient}/{day}/{n}/{slice}"),
                        &synth::part10(&MetaFields::mr(&sop), &e, true),
                    );
                }
            }
        }
    }
}

struct Lab {
    home: TempDir,
    bin: TempDir,
    work: TempDir,
    out: TempDir,
    root: TempDir,
    path: OsString,
    pg: Option<(String, String)>,
}

impl Lab {
    fn run(&self, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_nils"))
            .arg("--registry")
            .arg(self.home.path())
            .args(args)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env("PATH", &self.path)
            .env("NILS_PACK_DIR", packs())
            .env_remove("NILS_DSN")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        if let Some(text) = stdin {
            input.write_all(text.as_bytes()).unwrap();
        }
        drop(input);
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

    fn json(&self, args: &[&str]) -> Value {
        let out = self.ok(args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    }

    fn store(&self) -> Store {
        match &self.pg {
            Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
            None => Store::open_sqlite(&self.home.path().join("registry.db")).unwrap(),
        }
    }

    /// Two datasets read, fingerprinted and sorted, each visit's three
    /// series a T1w of its own technique, and the pack's picks run.
    fn new(pg: Option<(String, String)>) -> Lab {
        let bin = TempDir::new("pick-truth-bin");
        let podman = bin.file("podman", FAKE_PODMAN.as_bytes());
        let no_gpu = bin.file("nvidia-smi", b"#!/bin/sh\nexit 1\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for f in [&podman, &no_gpu] {
                std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let mut path = bin.path().as_os_str().to_owned();
        if let Some(p) = std::env::var_os("PATH") {
            path.push(":");
            path.push(p);
        }
        let lab = Lab {
            home: TempDir::new("pick-truth-home"),
            bin,
            work: TempDir::new("pick-truth-work"),
            out: TempDir::new("pick-truth-out"),
            root: TempDir::new("pick-truth-root"),
            path,
            pg,
        };
        let (good, _, err) = lab.run(&["key", "add", "k"], Some("a pick truth test key\n"));
        assert!(good, "{err}");
        match &lab.pg {
            Some((dsn, schema)) => {
                lab.ok(&[
                    "init",
                    "--backend",
                    "postgres",
                    "--dsn",
                    dsn,
                    "--schema",
                    schema,
                    "--key",
                    "k",
                ]);
            }
            None => {
                lab.ok(&["init", "--key", "k"]);
            }
        }
        tree(
            &lab.root,
            "alpha",
            &[
                (
                    "A1",
                    &["20220115", "20230115"],
                    "1.2.826.0.1.3680043.8.498.81",
                ),
                ("A2", &["20220301"], "1.2.826.0.1.3680043.8.498.82"),
            ],
        );
        tree(
            &lab.root,
            "beta",
            &[
                ("B1", &["20220401"], "1.2.826.0.1.3680043.8.498.83"),
                ("B2", &["20220501"], "1.2.826.0.1.3680043.8.498.84"),
            ],
        );
        let p = packs();
        let root = lab.root.path().to_str().unwrap().to_string();
        lab.ok(&["place", "add", "data", &root, "--role", "source"]);
        for name in ["alpha", "beta"] {
            lab.ok(&[
                "place",
                "add-dataset",
                "data",
                name,
                "--patient-id",
                "id-type:patient-id",
                "--subjects",
                "generated",
            ]);
            let tree = lab.root.path().join(name).join("derivatives/dcm-anon");
            lab.ok(&[
                "digest",
                "--name",
                name,
                "--no-private",
                tree.to_str().unwrap(),
            ]);
        }
        lab.ok(&["fingerprint"]);
        lab.ok(&["classify", "--pack-dir", &p]);
        // each series a T1w of its own technique and nothing else, so the
        // kinds are 3D MPRAGE, 2D SE and 2D TSE on every visit
        let mut store = lab.store();
        let (axis, fp) = (
            store.qualified("classification_axis"),
            store.qualified("stack_fingerprint"),
        );
        store
            .execute(
                &format!(
                    "DELETE FROM {axis} WHERE axis IN ('role', 'technique', 'disposition', 'post_contrast')"
                ),
                &[],
            )
            .unwrap();
        for (_, description, _, technique) in SERIES {
            for (name, value) in [("role", "t1w"), ("technique", technique)] {
                store
                    .execute(
                        &format!(
                            "INSERT INTO {axis} (stack_id, axis, value, confidence, tier) \
                             SELECT stack_id, '{name}', '{value}', 1.0, 'rule' FROM {fp} \
                             WHERE text_series_description = '{description}'"
                        ),
                        &[],
                    )
                    .unwrap();
            }
        }
        drop(store);
        lab.ok(&["pick", "run", "--pack-dir", &p]);
        lab
    }

    /// Each subject by the patient it was read from, with its dataset.
    fn subjects(&self) -> BTreeMap<String, (i64, String)> {
        let mut store = self.store();
        let sql = format!(
            "SELECT DISTINCT se.subject_id, so.root_canonical, \
               (SELECT COUNT(DISTINCT sy.id) FROM {} sy WHERE sy.subject_id = se.subject_id) \
             FROM {} st JOIN {} se ON se.id = st.series_id \
             JOIN {} b ON b.id = st.first_batch_id JOIN {} so ON so.id = b.source_id \
             ORDER BY se.subject_id",
            store.qualified("study"),
            store.qualified("stack"),
            store.qualified("series"),
            store.qualified("ingest_batch"),
            store.qualified("source"),
        );
        let alpha = std::fs::canonicalize(self.root.path().join("alpha")).unwrap();
        let mut a = Vec::new();
        let mut b = Vec::new();
        for r in store.query(&sql, &[]).unwrap() {
            let id = r.int(0).unwrap();
            let in_alpha = std::path::Path::new(r.text(1).unwrap()).starts_with(&alpha);
            let visits = r.int(2).unwrap();
            if in_alpha {
                a.push((visits, id));
            } else {
                b.push(id);
            }
        }
        a.sort();
        assert_eq!(a.len(), 2, "{a:?}");
        assert_eq!(b.len(), 2, "{b:?}");
        BTreeMap::from([
            ("A1".to_string(), (a[1].1, "alpha".to_string())),
            ("A2".to_string(), (a[0].1, "alpha".to_string())),
            ("B1".to_string(), (b[0], "beta".to_string())),
            ("B2".to_string(), (b[1], "beta".to_string())),
        ])
    }

    /// The stacks of these subjects' series of a technique.
    fn stacks_of(&self, subjects: &[i64], technique: &str) -> BTreeSet<i64> {
        let description = SERIES.iter().find(|s| s.3 == technique).unwrap().1;
        let mut store = self.store();
        let sql = format!(
            "SELECT stack_id FROM {} WHERE text_series_description = '{description}' \
             AND subject_id IN ({})",
            store.qualified("stack_fingerprint"),
            subjects
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        store
            .query(&sql, &[])
            .unwrap()
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect()
    }

    /// The stacks the newest release wrote.
    fn released(&self, name: &str, scope: &[&str]) -> BTreeSet<i64> {
        let dir = self.out.path().join(name);
        let mut args = vec![
            "release",
            "--name",
            name,
            "--out",
            dir.to_str().unwrap(),
            "--picked",
            "--on-unknown",
            "write",
        ];
        args.extend(scope);
        let p = packs();
        args.extend(["--pack-dir", &p]);
        self.ok(&args);
        let mut store = self.store();
        let sql = format!(
            "SELECT DISTINCT rs.stack_id FROM {} rs WHERE rs.release_id = \
             (SELECT MAX(id) FROM {})",
            store.qualified("release_stack"),
            store.qualified("release"),
        );
        store
            .query(&sql, &[])
            .unwrap()
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect()
    }

    /// `nils pick list`'s t1w stacks, of a scope or of none.
    fn listed(&self, scope: &[&str]) -> usize {
        let mut args = vec!["pick", "list", "--role", "t1w", "--json"];
        args.extend(scope);
        let doc = self.json(&args);
        doc.as_array()
            .or_else(|| doc["picks"].as_array())
            .unwrap_or_else(|| panic!("{doc}"))
            .len()
    }
}

/// A `nils serve` with its worker, killed when dropped.
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
    fn start(lab: &Lab) -> Served {
        use std::io::BufRead as _;
        let child = Command::new(env!("CARGO_BIN_EXE_nils"))
            .arg("--registry")
            .arg(lab.home.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "2",
                "--worker",
            ])
            .args(["--auth", "token", "--pack-dir", &packs()])
            .env("NILS_TOKENS", format!("{OPS}=ops@lab:operator"))
            .env("NILS_PACK_DIR", packs())
            .env("PATH", &lab.path)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_DSN")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL")
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

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        use std::io::Read as _;
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: application/json\r\nAuthorization: Bearer {OPS}\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let response = String::from_utf8_lossy(&bytes).to_string();
        let (headers, text) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status: u16 = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        (
            status,
            serde_json::from_str(text).unwrap_or(Value::String(text.to_string())),
        )
    }

    fn ok(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let (status, doc) = self.call(method, path, body);
        assert!(
            (200..300).contains(&status),
            "{method} {path}: {status} {doc}"
        );
        doc
    }

    /// A job once it is over, two minutes at most.
    fn over(&self, job: i64) -> Value {
        for _ in 0..1200 {
            let (_, j) = self.call("GET", &format!("/api/jobs/{job}"), None);
            if matches!(j["state"].as_str(), Some("done" | "failed" | "cancelled")) {
                return j;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("job {job} did not end");
    }

    /// Save a scope's rules with these kinds first, and wait for its run.
    fn save(&self, scope: &str, kinds: &[&str]) {
        let doc = self.ok("GET", &format!("/api/picks/rules?{scope}"), None);
        let mut rules = doc["current"]["rules"].clone();
        rules["roles"]["t1w"]["kinds_in_order"] = json!(kinds);
        rules["roles"]["t1w"]["keep_alike"] = json!("across_the_data");
        let saved = self.ok(
            "POST",
            &format!("/api/picks/rules?{scope}"),
            Some(json!({"rules": rules, "reason": "a pick truth test", "based_on": 1})),
        );
        assert_eq!(saved["version"], 2, "{saved}");
        let job = self.over(saved["job"].as_i64().unwrap());
        assert_eq!(job["state"], "done", "{job}");
    }

    /// The ask's t1w main scans, of a scope or of none.
    fn asked(&self, opts: Value) -> BTreeSet<i64> {
        let mut picked = json!({"role": "t1w"});
        if let Some(o) = opts.as_object() {
            for (k, v) in o {
                picked[k] = v.clone();
            }
        }
        let doc = json!({
            "ast_version": 1,
            "scheme": "default",
            "sets": {"main": {"grain": "stack", "where": [["picked", picked]]}},
            "out": {"set": "main", "level": "record"},
        });
        let ran = self.ok("POST", "/api/ask/run", Some(json!({"document": doc})));
        assert_eq!(ran["truncated"], false, "{ran}");
        ran["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r[0].as_i64().unwrap_or_else(|| panic!("{ran}")))
            .collect()
    }

    /// The stacks the Main scans page names for a scope's t1w visits: every
    /// subject of every cell of its map.
    fn page(&self, scope: Value) -> BTreeSet<i64> {
        let map = self.ok(
            "POST",
            "/api/picks/map",
            Some(json!({"scope": scope, "role": "t1w"})),
        );
        let columns = map["columns"].as_array().unwrap().clone();
        let mut out = BTreeSet::new();
        for row in map["matrix"].as_array().unwrap() {
            for (i, cell) in row["cells"].as_array().unwrap().iter().enumerate() {
                if cell.as_i64().unwrap_or(0) == 0 {
                    continue;
                }
                let page = self.ok(
                    "POST",
                    "/api/picks/map/subjects",
                    Some(json!({
                        "scope": scope, "role": "t1w",
                        "group": {"by": "cell", "kind": row["kind"], "column": columns[i]["key"]},
                        "order": "visits", "page": 0, "per_page": 48,
                    })),
                );
                for s in page["subjects"].as_array().unwrap() {
                    for v in s["visits"].as_array().unwrap() {
                        for st in v["stacks"].as_array().unwrap() {
                            out.insert(st.as_i64().unwrap());
                        }
                    }
                }
            }
        }
        out
    }

    /// The stacks the viewer names as t1w main scans on a scope's
    /// subjects' visits.
    fn viewer(&self, kind: &str, name: &str, subjects: &[i64]) -> BTreeSet<i64> {
        let mut out = BTreeSet::new();
        for s in subjects {
            let doc = self.ok(
                "GET",
                &format!("/api/{kind}/{name}/subjects/{s}/visits"),
                None,
            );
            for v in doc["visits"].as_array().unwrap_or_else(|| panic!("{doc}")) {
                for m in v["main"].as_array().unwrap() {
                    if m["role"] == "t1w" {
                        out.insert(m["stack"].as_i64().unwrap());
                    }
                }
            }
        }
        out
    }
}

/// The Postgres DSN of the tests, and a schema of this file's dropped
/// before and after `run`.
fn on_postgres(schema: &str, run: impl FnOnce(Option<(String, String)>)) {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        return;
    };
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    run(Some((dsn.clone(), schema.to_string())));
    drop();
}

#[test]
fn every_reader_names_the_same_main_scans() {
    agree(None);
}

#[test]
fn every_reader_names_the_same_main_scans_on_postgres_too() {
    on_postgres("nils_pick_truth", agree);
}

fn agree(pg: Option<(String, String)>) {
    let lab = Lab::new(pg);
    let who = lab.subjects();
    let id = |p: &str| who[p].0;
    let (a1, a2, b1, b2) = (id("A1"), id("A2"), id("B1"), id("B2"));
    let codes: BTreeMap<i64, String> = {
        let mut store = lab.store();
        let sql = format!("SELECT id, code FROM {}", store.qualified("subject"));
        store
            .query(&sql, &[])
            .unwrap()
            .iter()
            .map(|r| (r.int(0).unwrap(), r.text(1).unwrap().to_string()))
            .collect()
    };
    lab.ok(&["clinical", "cohort", "make", "trio"]);
    lab.ok(&[
        "clinical",
        "cohort",
        "add",
        "trio",
        &codes[&a2],
        &codes[&b1],
        "--why",
        "a pick truth test",
    ]);

    // before any rules: everyone reads the pack's, the 3D MPRAGE
    let server = Served::start(&lab);
    let pack_all = lab.stacks_of(&[a1, a2, b1, b2], "MPRAGE");
    assert_eq!(pack_all.len(), 5, "five visits");
    assert_eq!(server.asked(json!({})), pack_all);

    // the dataset's own rules take the 2D SE, the cohort's the 2D TSE
    server.save("dataset=alpha", &["2D SE", "2D TSE", "3D MPRAGE"]);
    server.save("cohort=trio", &["2D TSE", "2D SE", "3D MPRAGE"]);

    let alpha = lab.stacks_of(&[a1, a2], "SE");
    let beta = lab.stacks_of(&[b1, b2], "MPRAGE");
    let trio = lab.stacks_of(&[a2, b1], "TSE");
    let own: BTreeSet<i64> = alpha.union(&beta).copied().collect();
    assert_eq!((alpha.len(), beta.len(), trio.len()), (3, 2, 2));

    // the Main scans page
    assert_eq!(
        server.page(json!({"dataset": "alpha"})),
        alpha,
        "the page of alpha"
    );
    assert_eq!(
        server.page(json!({"dataset": "beta"})),
        beta,
        "the page of beta"
    );
    assert_eq!(
        server.page(json!({"cohort": "trio"})),
        trio,
        "the page of trio"
    );

    // the ask: a scope named, and none (each subject's own dataset's)
    assert_eq!(
        server.asked(json!({"dataset": "alpha"})),
        alpha,
        "the ask of alpha"
    );
    assert_eq!(
        server.asked(json!({"dataset": "beta"})),
        beta,
        "the ask of beta"
    );
    assert_eq!(
        server.asked(json!({"cohort": "trio"})),
        trio,
        "the ask of trio"
    );
    assert_eq!(server.asked(json!({})), own, "the ask of no scope");
    let (status, refused) = server.call(
        "POST",
        "/api/ask/run",
        Some(json!({"document": {
            "ast_version": 1, "scheme": "default",
            "sets": {"main": {"grain": "stack", "where": [["picked", {"role": "t1w", "dataset": "nowhere"}]]}},
            "out": {"set": "main", "level": "count"},
        }})),
    );
    assert_eq!(status, 400, "{refused}");

    // the viewer
    assert_eq!(
        server.viewer("datasets", "alpha", &[a1, a2]),
        alpha,
        "the viewer of alpha"
    );
    assert_eq!(
        server.viewer("datasets", "beta", &[b1, b2]),
        beta,
        "the viewer of beta"
    );
    assert_eq!(
        server.viewer("cohorts", "trio", &[a2, b1]),
        trio,
        "the viewer of trio"
    );

    // the picks summary and the dataset summary count the same visits
    for (q, n) in [
        ("dataset=alpha", 3),
        ("dataset=beta", 2),
        ("cohort=trio", 2),
    ] {
        let s = server.ok("GET", &format!("/api/picks/summary?{q}"), None);
        assert_eq!(s["roles"]["t1w"]["picked"], n, "{q}: {s}");
    }
    for (name, n) in [("alpha", 3), ("beta", 2)] {
        let s = server.ok("GET", &format!("/api/datasets/{name}/summary"), None);
        let main = s["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["step"] == "main_scans")
            .unwrap_or_else(|| panic!("{s}"));
        assert_eq!(main["picked"], n, "{name}: {main}");
    }
    drop(server);

    // the command line
    assert_eq!(lab.listed(&["--dataset", "alpha"]), alpha.len());
    assert_eq!(lab.listed(&["--cohort", "trio"]), trio.len());
    assert_eq!(lab.listed(&[]), own.len());

    // a release: of a dataset, of a cohort, and of no scope
    let out = lab.out.path().to_str().unwrap().to_string();
    lab.ok(&["place", "add", "out", &out, "--role", "export"]);
    assert_eq!(lab.released("rel-alpha", &["--dataset", "alpha"]), alpha);
    assert_eq!(lab.released("rel-beta", &["--dataset", "beta"]), beta);
    assert_eq!(lab.released("rel-trio", &["--cohort", "trio"]), trio);
    assert_eq!(lab.released("rel-all", &[]), own);

    // the pipelines' pre-flight takes the stacks a release of the
    // selection takes: each subject's own dataset's
    let work = lab.work.path().to_str().unwrap().to_string();
    lab.ok(&[
        "place", "add", "scratch", &work, "--role", "working", "--fast",
    ]);
    let every = lab.work.path().join("every.json");
    std::fs::write(
        &every,
        json!({"ast_version": 1, "sets": {"every": {"grain": "stack"}}, "out": {"set": "every", "level": "record"}}).to_string(),
    )
    .unwrap();
    let p = packs();
    lab.ok(&[
        "ask",
        "selections",
        "save",
        "--name",
        "every",
        "--file",
        every.to_str().unwrap(),
        "--pack-dir",
        &p,
    ]);
    let descriptor = lab.work.path().join("needs-t1w.yml");
    std::fs::write(&descriptor, NEEDS_T1W).unwrap();
    lab.ok(&["pipeline", "add", descriptor.to_str().unwrap(), "--json"]);
    let pre = lab.json(&[
        "run",
        "needs-t1w",
        "--select",
        "selection:every@1",
        "--preflight",
        "--json",
    ]);
    assert_eq!(pre["stacks"], 15, "{pre}");
    assert_eq!(
        pre["stacks"].as_i64().unwrap() - pre["left_out"]["stacks"].as_i64().unwrap(),
        own.len() as i64,
        "{pre}"
    );
    assert_eq!(pre["units"]["total"], 5, "{pre}");
    let _ = &lab.bin;
}
