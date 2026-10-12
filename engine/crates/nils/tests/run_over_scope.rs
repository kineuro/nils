// SPDX-License-Identifier: AGPL-3.0-only
//! A run straight over a dataset's or a cohort's main scans by role (record
//! 55, rulings of 2026-10-12, item 6: "run SynthSeg on study-anon's main T1
//! scans", without making a selection first; the run is still frozen over
//! the list it was given). One registry of two datasets: `alpha`, whose own
//! rules take the 2D SE once saved, and `beta`, with one visit whose scans
//! hold no T1w; a cohort `trio` across both, whose own rules take the 2D
//! TSE. The pack's rules take the 3D MPRAGE. `--select
//! main:t1w@dataset:alpha` and `main:t1w@cohort:trio` resolve to the scope's
//! main scans through the one resolver, with the scope's own rules and with
//! the pack's; the pre-flight counts and names the visits with none; the
//! list is frozen with the picks it was read under, so a run over it reads
//! those picks again after the rules change; and the run door takes it. On
//! SQLite always, and on Postgres where a test DSN is set.

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

fn have(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

const OPS: &str = "an-operator-token-of-length";

/// A stand-in podman that answers what a pre-flight asks of it, and runs
/// nothing.
const FAKE_PODMAN: &str = "#!/bin/sh\ncase \"$1\" in\n  info) case \"$*\" in *GraphRoot*) echo /fake/containers/storage ;; *) echo true ;; esac ;;\n  *) echo \"podman version 9.9.9-test\" ;;\nesac\nexit 0\n";

/// A bids pipeline whose input is each session's T1w.
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

/// Each visit's three series: number, description, dimension, technique.
const SERIES: [(&str, &str, &str, &str); 3] = [
    ("1", "t1_mprage_sag", "3D", "MPRAGE"),
    ("2", "t1_se_tra", "2D", "SE"),
    ("3", "t1_tse_tra", "2D", "TSE"),
];

/// The study of B2's second visit, whose scans the test makes FLAIRs.
const NO_T1W_STUDY: &str = "1.2.826.0.1.3680043.8.498.84.2";

/// A dataset's folder under the root, already pseudonymised: each
/// person's visits, three series a visit.
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
    _bin: TempDir,
    work: TempDir,
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
    /// series a T1w of its own technique but B2's second visit's, which are
    /// FLAIRs; the pack's picks run; a working place and a bids pipeline.
    fn new(pg: Option<(String, String)>) -> Lab {
        let bin = TempDir::new("run-over-bin");
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
            home: TempDir::new("run-over-home"),
            _bin: bin,
            work: TempDir::new("run-over-work"),
            root: TempDir::new("run-over-root"),
            path,
            pg,
        };
        let (good, _, err) = lab.run(&["key", "add", "k"], Some("a run over a scope test key\n"));
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
                (
                    "B2",
                    &["20220501", "20230601"],
                    "1.2.826.0.1.3680043.8.498.84",
                ),
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
        // each series a T1w of its own technique and nothing else; B2's
        // second visit's are FLAIRs, so that visit holds no T1w
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
        let flairs = lab.study_stacks(&mut store, NO_T1W_STUDY);
        assert_eq!(flairs.len(), 3, "{flairs:?}");
        store
            .execute(
                &format!(
                    "UPDATE {axis} SET value = 'flair' WHERE axis = 'role' AND stack_id IN ({})",
                    list(&flairs)
                ),
                &[],
            )
            .unwrap();
        drop(store);
        lab.ok(&["pick", "run", "--pack-dir", &p]);
        let work = lab.work.path().join("place");
        std::fs::create_dir_all(&work).unwrap();
        lab.ok(&[
            "place",
            "add",
            "scratch",
            work.to_str().unwrap(),
            "--role",
            "working",
            "--fast",
        ]);
        let descriptor = lab.work.path().join("needs-t1w.yml");
        std::fs::write(&descriptor, NEEDS_T1W).unwrap();
        lab.ok(&["pipeline", "add", descriptor.to_str().unwrap(), "--json"]);
        lab
    }

    /// The stacks of one study, by its instance UID.
    fn study_stacks(&self, store: &mut Store, uid: &str) -> BTreeSet<i64> {
        let sql = format!(
            "SELECT st.id FROM {} st JOIN {} se ON se.id = st.series_id \
             JOIN {} sy ON sy.id = se.study_id WHERE sy.study_instance_uid = '{uid}'",
            store.qualified("stack"),
            store.qualified("series"),
            store.qualified("study"),
        );
        store
            .query(&sql, &[])
            .unwrap()
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect()
    }

    /// Each subject by the patient it was read from: by its dataset, and by
    /// how many visits it has.
    fn subjects(&self) -> BTreeMap<&'static str, i64> {
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
            let visits = r.int(2).unwrap();
            if std::path::Path::new(r.text(1).unwrap()).starts_with(&alpha) {
                a.push((visits, id));
            } else {
                b.push((visits, id));
            }
        }
        a.sort();
        b.sort();
        assert_eq!((a.len(), b.len()), (2, 2), "{a:?} {b:?}");
        BTreeMap::from([
            ("A1", a[1].1),
            ("A2", a[0].1),
            ("B1", b[0].1),
            ("B2", b[1].1),
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
            list(subjects)
        );
        store
            .query(&sql, &[])
            .unwrap()
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect()
    }

    /// The stacks a handle froze.
    fn frozen(&self, handle: &Value) -> BTreeSet<i64> {
        let id = handle
            .as_i64()
            .unwrap_or_else(|| panic!("no handle: {handle}"));
        let mut store = self.store();
        let sql = format!(
            "SELECT key FROM {} WHERE handle_id = {id}",
            store.qualified("handle_member")
        );
        store
            .query(&sql, &[])
            .unwrap()
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect()
    }

    /// The pre-flight of needs-t1w at the keyboard.
    fn preflight(&self, over: &[&str]) -> Value {
        let mut args = vec!["run", "needs-t1w"];
        args.extend(over);
        args.extend(["--preflight", "--json"]);
        self.json(&args)
    }

    /// The stacks a run's input release wrote, and the picks it recorded.
    fn input_of(&self, run: i64) -> (BTreeSet<i64>, Value) {
        let mut store = self.store();
        let sql = format!(
            "SELECT r.id, {} FROM {} pr JOIN {} r ON r.id = pr.input_release_id WHERE pr.id = {run}",
            if self.pg.is_some() {
                "CAST(r.pick_rules AS TEXT)"
            } else {
                "r.pick_rules"
            },
            store.qualified("pipeline_run"),
            store.qualified("release"),
        );
        let rows = store.query(&sql, &[]).unwrap();
        let r = rows
            .first()
            .unwrap_or_else(|| panic!("run {run} has no input release"));
        let release = r.int(0).unwrap();
        let picks: Value = serde_json::from_str(r.text(1).unwrap()).unwrap();
        let sql = format!(
            "SELECT DISTINCT rs.stack_id FROM {} rs WHERE rs.release_id = {release}",
            store.qualified("release_stack"),
        );
        let stacks = store
            .query(&sql, &[])
            .unwrap()
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect();
        (stacks, picks)
    }

    /// What the run's input release kept of the picks it read, by role
    /// (`release_pick`).
    fn kept_of(&self, run: i64) -> BTreeMap<String, BTreeSet<i64>> {
        let mut store = self.store();
        let sql = format!(
            "SELECT k.role, k.stack_id FROM {} k JOIN {} pr ON pr.input_release_id = k.release_id \
             WHERE pr.id = {run}",
            store.qualified("release_pick"),
            store.qualified("pipeline_run"),
        );
        let mut out: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
        for r in store.query(&sql, &[]).unwrap() {
            out.entry(r.text(0).unwrap().to_string())
                .or_default()
                .insert(r.int(1).unwrap());
        }
        out
    }
}

fn list(ids: impl IntoIterator<Item = impl std::borrow::Borrow<i64>>) -> String {
    ids.into_iter()
        .map(|i| i.borrow().to_string())
        .collect::<Vec<_>>()
        .join(", ")
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

    /// A job once it is over, three minutes at most.
    fn over(&self, job: i64) -> Value {
        for _ in 0..1800 {
            let (_, j) = self.call("GET", &format!("/api/jobs/{job}"), None);
            if matches!(j["state"].as_str(), Some("done" | "failed" | "cancelled")) {
                return j;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("job {job} did not end");
    }

    /// Save a scope's rules with these kinds first as the version after
    /// `based_on`, and wait for its run.
    fn save(&self, scope: &str, kinds: &[&str], based_on: i64) {
        let doc = self.ok("GET", &format!("/api/picks/rules?{scope}"), None);
        let mut rules = doc["current"]["rules"].clone();
        rules["roles"]["t1w"]["kinds_in_order"] = json!(kinds);
        rules["roles"]["t1w"]["keep_alike"] = json!("across_the_data");
        let saved = self.ok(
            "POST",
            &format!("/api/picks/rules?{scope}"),
            Some(
                json!({"rules": rules, "reason": "a run over a scope test", "based_on": based_on}),
            ),
        );
        assert_eq!(saved["version"], based_on + 1, "{saved}");
        let job = self.over(saved["job"].as_i64().unwrap());
        assert_eq!(job["state"], "done", "{job}");
    }

    /// The pre-flight of needs-t1w at its door.
    fn preflight(&self, body: Value) -> Value {
        self.ok("POST", "/api/pipelines/needs-t1w/preflight", Some(body))
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
fn a_run_goes_over_a_scope_s_main_scans_frozen_with_the_picks_it_read() {
    over_scopes(None);
}

#[test]
fn a_run_goes_over_a_scope_s_main_scans_on_postgres_too() {
    on_postgres("nils_run_over_scope", over_scopes);
}

fn over_scopes(pg: Option<(String, String)>) {
    let lab = Lab::new(pg);
    let who = lab.subjects();
    let (a1, a2, b1, b2) = (who["A1"], who["A2"], who["B1"], who["B2"]);
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
        "a run over a scope test",
    ]);
    let no_t1w: BTreeSet<i64> = lab.study_stacks(&mut lab.store(), NO_T1W_STUDY);

    // 1. before any rules of their own: the pack's picks, the 3D MPRAGE
    let pre = lab.preflight(&["--select", "main:t1w@dataset:alpha"]);
    assert_eq!(pre["selection"], "main:t1w@dataset:alpha", "{pre}");
    let main = &pre["main"];
    assert_eq!(main["of"], "main:t1w@dataset:alpha", "{main}");
    assert_eq!(main["main"], "t1w", "{main}");
    assert_eq!(main["scope"]["name"], "alpha", "{main}");
    assert_eq!(main["picks"]["defaults"], true, "the pack's: {main}");
    assert_eq!(main["picks"]["pack"], "mri", "{main}");
    assert_eq!(
        (
            &main["visits"],
            &main["main_visits"],
            &main["without"]["visits"]
        ),
        (&json!(3), &json!(3), &json!(0)),
        "{main}"
    );
    assert_eq!(
        lab.frozen(&pre["handle"]),
        lab.stacks_of(&[a1, a2], "MPRAGE"),
        "alpha's main T1w scans under the pack's rules"
    );
    assert_eq!(pre["stacks"], 3, "{pre}");
    assert_eq!(pre["units"]["total"], 3, "{pre}");
    assert_eq!(pre["units"]["missing"], 0, "{pre}");
    assert_eq!(pre["left_out"]["stacks"], 0, "{pre}");

    // a visit whose scans hold no T1w is counted and named
    let pre = lab.preflight(&["--select", "main:T1w@dataset:beta"]);
    let main = &pre["main"];
    assert_eq!(main["of"], "main:t1w@dataset:beta", "{main}");
    assert_eq!(
        (
            &main["visits"],
            &main["main_visits"],
            &main["without"]["visits"]
        ),
        (&json!(3), &json!(2), &json!(1)),
        "{main}"
    );
    assert_eq!(
        main["without"]["why"],
        json!([{"why": "it holds no t1w scan", "visits": 1}]),
        "{main}"
    );
    let named = &main["without"]["named"][0];
    assert_eq!(named["subject"], codes[&b2].as_str(), "{named}");
    assert_eq!(named["subject_id"], b2, "{named}");
    assert_eq!(named["session_day"], "2023-06-01", "{named}");
    let beta: BTreeSet<i64> = lab
        .stacks_of(&[b1, b2], "MPRAGE")
        .difference(&no_t1w)
        .copied()
        .collect();
    assert_eq!(beta.len(), 2);
    assert_eq!(lab.frozen(&pre["handle"]), beta, "beta's main T1w scans");
    let said = lab.ok(&[
        "run",
        "needs-t1w",
        "--select",
        "main:t1w@dataset:beta",
        "--preflight",
    ]);
    assert!(
        said.contains("1 visit has no main t1w (1: it holds no t1w scan)"),
        "{said}"
    );
    assert!(said.contains(&codes[&b2]), "the visit is named: {said}");

    // what is not a scope's main scans is refused, and says why
    for (spec, says) in [
        (
            "main:t1x@dataset:alpha",
            "t1x is not a main-scan role of mri",
        ),
        ("main:t1w@dataset:nowhere", "no dataset named nowhere"),
        ("main:t1w@cohort:nobody", "no cohort named nobody"),
        ("main:t1w@alpha", "main:<role>@dataset:<name>"),
    ] {
        let (good, _, err) = lab.run(
            &[
                "run",
                "needs-t1w",
                "--select",
                spec,
                "--preflight",
                "--json",
            ],
            None,
        );
        assert!(!good, "{spec}");
        assert!(err.contains(says), "{spec}: {err}");
    }
    // nor is it a campaign's: a campaign is made from a saved selection
    let (good, _, err) = lab.run(
        &[
            "campaign",
            "create",
            "c",
            "--select",
            "main:t1w@dataset:alpha",
            "--axis",
            "role",
        ],
        None,
    );
    assert!(!good);
    assert!(err.contains("a run's input"), "{err}");

    // 2. the scopes' own rules: alpha's take the 2D SE, trio's the 2D TSE
    let server = Served::start(&lab);
    server.save("dataset=alpha", &["2D SE", "2D TSE", "3D MPRAGE"], 1);
    server.save("cohort=trio", &["2D TSE", "2D SE", "3D MPRAGE"], 1);

    let alpha_se = lab.stacks_of(&[a1, a2], "SE");
    let pre = server.preflight(json!({"select": "main:t1w@dataset:alpha"}));
    let held = pre["handle"].clone();
    assert_eq!(pre["main"]["picks"]["version"], 2, "{pre}");
    assert_eq!(pre["main"]["picks"]["scope"]["name"], "alpha", "{pre}");
    assert!(pre["main"]["picks"]["digest"].is_string(), "{pre}");
    assert!(
        pre["main"]["read"]
            .as_str()
            .is_some_and(|r| r.contains("under its own rules, version 2")),
        "{pre}"
    );
    assert_eq!(lab.frozen(&held), alpha_se, "alpha's own main T1w scans");
    assert_eq!(pre["units"]["total"], 3, "{pre}");
    assert_eq!(pre["units"]["missing"], 0, "{pre}");
    assert_eq!(pre["left_out"]["stacks"], 0, "{pre}");

    // a cohort's own picks: the release of each subject's own dataset's
    // would take none of them, the input under the cohort's takes them all
    let trio = lab.stacks_of(&[a2, b1], "TSE");
    let pre = server.preflight(json!({"select": "main:t1w@cohort:trio"}));
    assert_eq!(
        lab.frozen(&pre["handle"]),
        trio,
        "trio's own main T1w scans"
    );
    assert_eq!(pre["main"]["picks"]["scope"]["kind"], "cohort", "{pre}");
    assert_eq!(pre["main"]["picks"]["version"], 2, "{pre}");
    assert_eq!(
        (&pre["main"]["visits"], &pre["main"]["main_visits"]),
        (&json!(2), &json!(2)),
        "{pre}"
    );
    assert_eq!(pre["units"]["total"], 2, "{pre}");
    assert_eq!(pre["units"]["missing"], 0, "{pre}");
    assert_eq!(pre["left_out"]["stacks"], 0, "{pre}");

    // 3. the frozen list reproduces after alpha's rules change
    server.save("dataset=alpha", &["2D TSE", "2D SE", "3D MPRAGE"], 2);
    assert_eq!(lab.frozen(&held), alpha_se, "the list stays as frozen");
    let again = server.preflight(json!({"handle": held}));
    assert_eq!(again["main"]["picks"]["version"], 2, "{again}");
    assert_eq!(again["main"]["of"], "main:t1w@dataset:alpha", "{again}");
    assert_eq!(again["units"]["total"], 3, "{again}");
    assert_eq!(
        again["left_out"]["stacks"], 0,
        "its input is released under version 2's picks: {again}"
    );
    // made now, the list is version 3's
    let now = server.preflight(json!({"select": "main:t1w@dataset:alpha"}));
    assert_eq!(now["main"]["picks"]["version"], 3, "{now}");
    assert_eq!(lab.frozen(&now["handle"]), lab.stacks_of(&[a1, a2], "TSE"));

    // 4. a visit that holds a T1w and has no pick is counted and named
    {
        let mut store = lab.store();
        let picks = store.qualified("pick");
        let latest = store
            .query(
                &format!(
                    "SELECT id FROM {picks} WHERE rules_scope_kind = 'dataset' AND role = 't1w' \
                     AND subject_id = {a1} AND withdrawn_at IS NULL ORDER BY session_day DESC"
                ),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        store
            .execute(
                &format!(
                    "DELETE FROM {} WHERE pick_id = {latest}",
                    store.qualified("pick_stack")
                ),
                &[],
            )
            .unwrap();
        store
            .execute(&format!("DELETE FROM {picks} WHERE id = {latest}"), &[])
            .unwrap();
    }
    let pre = server.preflight(json!({"select": "main:t1w@dataset:alpha"}));
    let main = &pre["main"];
    assert_eq!(
        (
            &main["visits"],
            &main["main_visits"],
            &main["without"]["visits"]
        ),
        (&json!(3), &json!(2), &json!(1)),
        "{main}"
    );
    assert_eq!(
        main["without"]["named"][0]["why"], "it holds a t1w scan and none is picked",
        "{main}"
    );
    assert_eq!(main["without"]["named"][0]["session_day"], "2023-01-15");
    assert_eq!(pre["stacks"], 2, "{pre}");

    // 5. the run door takes it, and a run's input is released under the
    // picks its list was resolved from
    if !have("dcm2niix") {
        eprintln!(
            "dcm2niix is not installed; a bids input is released with it, so the runs are skipped"
        );
        return;
    }
    let queued = server.ok(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["run", "needs-t1w", "--select", "main:t1w@cohort:trio"]})),
    );
    let job = queued["id"]
        .as_i64()
        .or_else(|| queued["job"].as_i64())
        .unwrap_or_else(|| panic!("{queued}"));
    server.over(job);
    let run = {
        let mut store = lab.store();
        store
            .query(
                &format!(
                    "SELECT id FROM {} WHERE job_id = {job}",
                    store.qualified("pipeline_run")
                ),
                &[],
            )
            .unwrap()
            .first()
            .unwrap_or_else(|| panic!("job {job} made no run"))
            .int(0)
            .unwrap()
    };
    let doc = server.ok("GET", &format!("/api/pipeline-runs/{run}"), None);
    assert_eq!(doc["selection"], "main:t1w@cohort:trio", "{doc}");
    assert_eq!(doc["main"]["picks"]["scope"]["name"], "trio", "{doc}");
    let (released, picks) = lab.input_of(run);
    assert_eq!(released, trio, "the input holds trio's main scans");
    assert_eq!(picks["scope"]["name"], "trio", "{picks}");
    assert_eq!(picks["version"], 2, "{picks}");
    drop(server);

    // and a run over the list frozen under alpha's version 2, now that
    // version 3 is read
    let (_, out, err) = lab.run(
        &["run", "needs-t1w", "--handle", &held.to_string(), "--json"],
        None,
    );
    let run = {
        let mut store = lab.store();
        store
            .query(
                &format!(
                    "SELECT MAX(id) FROM {} WHERE handle_id = {held}",
                    store.qualified("pipeline_run")
                ),
                &[],
            )
            .unwrap()[0]
            .opt_int(0)
            .unwrap()
            .unwrap_or_else(|| panic!("no run over handle {held}: {err}\n{out}"))
    };
    let (released, picks) = lab.input_of(run);
    assert_eq!(released, alpha_se, "the input holds the list as frozen");
    assert_eq!(picks["version"], 2, "{picks}");
    assert_eq!(picks["scope"]["name"], "alpha", "{picks}");

    // split with --chunk, the list is resolved once and each part is a run
    // of its own over a handle that records the same list and its part
    let before = {
        let mut store = lab.store();
        store
            .query(
                &format!(
                    "SELECT COALESCE(MAX(id), 0) FROM {}",
                    store.qualified("pipeline_run")
                ),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap()
    };
    let (_, out, err) = lab.run(
        &[
            "run",
            "needs-t1w",
            "--select",
            "main:t1w@dataset:alpha",
            "--chunk",
            "1",
            "--json",
        ],
        None,
    );
    assert!(err.contains("2 stack(s) in 2 parts"), "{err}\n{out}");
    let parts: Vec<(i64, i64)> = {
        let mut store = lab.store();
        store
            .query(
                &format!(
                    "SELECT id, handle_id FROM {} WHERE id > {before} ORDER BY id",
                    store.qualified("pipeline_run")
                ),
                &[],
            )
            .unwrap()
            .iter()
            .map(|r| (r.int(0).unwrap(), r.int(1).unwrap()))
            .collect()
    };
    assert_eq!(parts.len(), 2, "{err}\n{out}");
    let mut each = BTreeSet::new();
    for (i, (run, handle)) in parts.iter().enumerate() {
        let doc = lab.json(&["pipeline", "runs", &run.to_string(), "--json"]);
        assert_eq!(doc["selection"], "main:t1w@dataset:alpha", "{doc}");
        assert_eq!(
            doc["main"]["part"],
            json!({"part": i + 1, "of": 2}),
            "{doc}"
        );
        assert_eq!(doc["main"]["picks"]["version"], 3, "{doc}");
        each.extend(lab.frozen(&json!(handle)));
    }
    let tse = lab.stacks_of(&[a1, a2], "TSE");
    assert_eq!(each.len(), 2);
    assert!(each.is_subset(&tse), "{each:?} of {tse:?}");
}

// ----------------------------------- every role a pipeline needs (2026-10-12)

/// A bids pipeline whose input is each session's T1w and FLAIR, as
/// samseg-lesions and segcsvd need them.
const NEEDS_BOTH: &str = r#"name: needs-both
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/needs-both@sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
command-line: "true [InputDataset] [OutputLocation]"
x-nils:
  analysis-level: session
  input: {layout: bids, roles: [t1w, flair]}
  units: apart
  outputs:
    - id: lesions
      kind: output
      path-template: "sub-{subject}/ses-{session}/anat/*_desc-lesions_mask.nii.gz"
  needs: {cores: 1, memory-gb: 1, unit-minutes: 2}
"#;

/// A bids pipeline that names no role a unit needs.
const NO_ROLES: &str = r#"name: no-roles
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/no-roles@sha256:abababababababababababababababababababababababababababababababab"
command-line: "true [InputDataset] [OutputLocation]"
x-nils:
  analysis-level: session
  input: {layout: bids}
  outputs:
    - id: copy
      kind: output
      path-template: "sub-{subject}/ses-{session}/anat/*_copy.nii.gz"
"#;

/// A2's one visit, whose 2D TSE stays a T1w, so it has no FLAIR.
const NO_FLAIR_STUDY: &str = "1.2.826.0.1.3680043.8.498.82.1";

impl Lab {
    /// Every visit's 2D TSE made a T2 FLAIR, as the pack names one (role
    /// flair, base T2w, modifier FLAIR, which the release spells
    /// `acq-...+FLAIR+..._T2w`), but A2's visit's, which keeps its T1w, so
    /// that visit has no FLAIR; B2's second visit's three FLAIRs the same,
    /// so it has no T1w; the pack's picks run again; and the two pipelines.
    fn with_flairs(self) -> Lab {
        let mut store = self.store();
        let (axis, fp) = (
            store.qualified("classification_axis"),
            store.qualified("stack_fingerprint"),
        );
        let tse: BTreeSet<i64> = store
            .query(
                &format!("SELECT stack_id FROM {fp} WHERE text_series_description = 't1_tse_tra'"),
                &[],
            )
            .unwrap()
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect();
        let keep = self.study_stacks(&mut store, NO_FLAIR_STUDY);
        let flairs: BTreeSet<i64> = tse
            .difference(&keep)
            .copied()
            .chain(self.study_stacks(&mut store, NO_T1W_STUDY))
            .collect();
        let ids = list(&flairs);
        store
            .execute(
                &format!(
                    "DELETE FROM {axis} WHERE axis IN ('role', 'base', 'modifier') AND stack_id IN ({ids})"
                ),
                &[],
            )
            .unwrap();
        for (name, value) in [("role", "flair"), ("base", "T2w"), ("modifier", "FLAIR")] {
            store
                .execute(
                    &format!(
                        "INSERT INTO {axis} (stack_id, axis, value, confidence, tier) \
                         SELECT DISTINCT stack_id, '{name}', '{value}', 1.0, 'rule' FROM {fp} \
                         WHERE stack_id IN ({ids})"
                    ),
                    &[],
                )
                .unwrap();
        }
        drop(store);
        self.ok(&["pick", "run", "--pack-dir", &packs()]);
        for (name, text) in [("needs-both", NEEDS_BOTH), ("no-roles", NO_ROLES)] {
            let file = self.work.path().join(format!("{name}.yml"));
            std::fs::write(&file, text).unwrap();
            self.ok(&["pipeline", "add", file.to_str().unwrap(), "--json"]);
        }
        self
    }

    /// The pre-flight of a pipeline at the keyboard.
    fn preflight_of(&self, pipeline: &str, over: &[&str]) -> Value {
        let mut args = vec!["run", pipeline];
        args.extend(over);
        args.extend(["--preflight", "--json"]);
        self.json(&args)
    }

    /// The newest run's id, or 0 where there is none.
    fn newest_run(&self) -> i64 {
        let mut store = self.store();
        store
            .query(
                &format!(
                    "SELECT COALESCE(MAX(id), 0) FROM {}",
                    store.qualified("pipeline_run")
                ),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap()
    }
}

#[test]
fn a_run_over_a_scope_s_main_scans_takes_every_role_its_pipeline_needs() {
    every_role(None);
}

#[test]
fn a_run_over_a_scope_s_main_scans_takes_every_role_on_postgres_too() {
    on_postgres("nils_run_every_role", every_role);
}

/// 2026-10-12: a pipeline whose bids input needs several roles (a T1w and
/// a FLAIR) goes over a dataset's or a cohort's main scans written without
/// a role, `main@dataset:<name>`: the main scans of each role it needs, one
/// pick a visit a role, a visit taken only where it has a main scan of
/// every one; the visits that lack one are counted by role and by why, and
/// named; each stack is named by the role it was taken for, so every visit
/// taken is a unit that runs; `main:<role>@...` is as it was; a pipeline
/// that names no role refuses the spec; and `--chunk` splits the list by
/// whole visits.
fn every_role(pg: Option<(String, String)>) {
    let lab = Lab::new(pg).with_flairs();
    let who = lab.subjects();
    let (a1, a2, b1) = (who["A1"], who["A2"], who["B1"]);
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
    let mprage = lab.stacks_of(&[a1], "MPRAGE");
    let flair = lab.stacks_of(&[a1], "TSE");
    assert_eq!((mprage.len(), flair.len()), (2, 2));

    // 1. a dataset: A1's two visits have both, A2's has no FLAIR
    let pre = lab.preflight_of("needs-both", &["--select", "main@dataset:alpha"]);
    assert_eq!(pre["selection"], "main@dataset:alpha", "{pre}");
    let main = &pre["main"];
    assert_eq!(main["of"], "main@dataset:alpha", "{main}");
    assert_eq!(main["roles"], json!(["t1w", "flair"]), "{main}");
    assert_eq!(main["main"], "t1w+flair", "{main}");
    assert_eq!(main["picks"]["defaults"], true, "the pack's: {main}");
    assert_eq!(
        (
            &main["visits"],
            &main["main_visits"],
            &main["stacks"],
            &main["without"]["visits"]
        ),
        (&json!(3), &json!(2), &json!(4), &json!(1)),
        "{main}"
    );
    assert_eq!(
        main["without"]["why"],
        json!([{"role": "flair", "why": "it holds no flair scan", "visits": 1}]),
        "{main}"
    );
    let named = &main["without"]["named"][0];
    assert_eq!(named["role"], "flair", "{main}");
    assert_eq!(named["subject"], codes[&a2].as_str(), "{main}");
    assert_eq!(named["session_day"], "2022-03-01", "{main}");
    let roles: Vec<&str> = main["by_role"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["main"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["t1w", "flair"], "{main}");
    assert_eq!(main["by_role"][0]["main_visits"], 3, "{main}");
    assert_eq!(main["by_role"][1]["main_visits"], 2, "{main}");
    assert_eq!(main["by_role"][1]["stacks"], 2, "{main}");
    assert!(
        main["by_role"][0].get("members").is_none(),
        "a role's own stacks are the run's alone: {main}"
    );
    let both: BTreeSet<i64> = mprage.union(&flair).copied().collect();
    assert_eq!(
        lab.frozen(&pre["handle"]),
        both,
        "each visit's T1w and FLAIR"
    );
    // every visit taken is a unit that runs: each stack named by its role
    assert_eq!(
        pre["units"],
        json!({"total": 2, "ready": 2, "missing": 0}),
        "{pre}"
    );
    assert_eq!(pre["left_out"]["stacks"], 0, "{pre}");
    let said = lab.ok(&[
        "run",
        "needs-both",
        "--select",
        "main@dataset:alpha",
        "--preflight",
    ]);
    assert!(
        said.contains("4 t1w and flair scans of 2 visits with each; 1 visit has no main scan of one (1: it holds no flair scan)"),
        "{said}"
    );
    assert!(said.contains(&codes[&a2]), "the visit is named: {said}");

    // 2. one role, as it was; and a spec without a role over a pipeline of
    // one role is that role's
    let one = lab.preflight_of("needs-both", &["--select", "main:t1w@dataset:alpha"]);
    assert_eq!(one["main"]["main"], "t1w", "{one}");
    assert!(one["main"].get("roles").is_none(), "{one}");
    assert_eq!(one["main"]["main_visits"], 3, "{one}");
    let t1w_only = lab.preflight(&["--select", "main@dataset:alpha"]);
    assert_eq!(t1w_only["main"]["main"], "t1w", "{t1w_only}");
    assert_eq!(
        t1w_only["main"]["of"], "main:t1w@dataset:alpha",
        "{t1w_only}"
    );
    assert_eq!(
        lab.frozen(&t1w_only["handle"]),
        lab.stacks_of(&[a1, a2], "MPRAGE")
    );
    // a T1w pipeline over a list of main T1w scans of visits that hold a
    // FLAIR as well: the input holds the list's stacks alone, and the
    // FLAIR is no part of it (found on a laptop install, 2026-10-12: the
    // pre-flight answered 500)
    let t1w = lab.preflight(&["--select", "main:t1w@dataset:alpha"]);
    assert_eq!(t1w["units"]["ready"], 3, "A1's two visits and A2's: {t1w}");
    assert_eq!(t1w["units"]["missing"], 0, "{t1w}");
    assert_eq!(
        lab.frozen(&t1w["handle"]),
        lab.stacks_of(&[a1, a2], "MPRAGE")
    );
    // a list of part of the scope: A2's visit lost its picks
    {
        let mut store = lab.store();
        let sql = format!(
            "DELETE FROM {ps} WHERE pick_id IN (SELECT id FROM {p} WHERE subject_id = {a2}); \
             DELETE FROM {p} WHERE subject_id = {a2}",
            ps = store.qualified("pick_stack"),
            p = store.qualified("pick"),
        );
        store.batch(&sql).unwrap();
    }
    let part = lab.preflight(&["--select", "main:t1w@dataset:alpha"]);
    assert_eq!(lab.frozen(&part["handle"]), mprage, "A1's alone: {part}");
    assert_eq!(part["units"]["ready"], 2, "{part}");
    assert_eq!(part["main"]["without"]["visits"], 1, "{part}");
    let part_both = lab.preflight_of("needs-both", &["--select", "main@dataset:alpha"]);
    assert_eq!(part_both["units"]["ready"], 2, "{part_both}");
    {
        // the pack's pick run gives A2 its picks back
        lab.ok(&["pick", "run", "--pack-dir", &packs()]);
    }

    // a pipeline that names no role refuses it, with the form that names one
    let (good, _, err) = lab.run(
        &[
            "run",
            "no-roles",
            "--select",
            "main@dataset:alpha",
            "--preflight",
            "--json",
        ],
        None,
    );
    assert!(!good, "{err}");
    assert!(err.contains("write main:<role>@dataset:alpha"), "{err}");

    // 3. a cohort across both datasets: B1's visit has both, A2's no FLAIR
    lab.ok(&["clinical", "cohort", "make", "duo"]);
    lab.ok(&[
        "clinical",
        "cohort",
        "add",
        "duo",
        &codes[&a2],
        &codes[&b1],
        "--why",
        "a run over every role test",
    ]);
    let server = Served::start(&lab);
    let pre = server.ok(
        "POST",
        "/api/pipelines/needs-both/preflight",
        Some(json!({"select": "main@cohort:duo"})),
    );
    assert_eq!(pre["main"]["of"], "main@cohort:duo", "{pre}");
    assert_eq!(
        (&pre["main"]["visits"], &pre["main"]["main_visits"]),
        (&json!(2), &json!(1)),
        "{pre}"
    );
    let b1_both: BTreeSet<i64> = lab
        .stacks_of(&[b1], "MPRAGE")
        .union(&lab.stacks_of(&[b1], "TSE"))
        .copied()
        .collect();
    assert_eq!(lab.frozen(&pre["handle"]), b1_both, "B1's T1w and FLAIR");
    assert_eq!(pre["units"]["ready"], 1, "{pre}");

    // 4. the runs, where a bids input can be released
    if !have("dcm2niix") {
        eprintln!(
            "dcm2niix is not installed; a bids input is released with it, so the runs are skipped"
        );
        return;
    }
    // the jobs door takes it, and the run's input holds each visit's two
    let queued = server.ok(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["run", "needs-both", "--select", "main@cohort:duo"]})),
    );
    let job = queued["job"].as_i64().unwrap_or_else(|| panic!("{queued}"));
    server.over(job);
    let run = lab.newest_run();
    let doc = server.ok("GET", &format!("/api/pipeline-runs/{run}"), None);
    assert_eq!(doc["selection"], "main@cohort:duo", "{doc}");
    assert_eq!(doc["main"]["roles"], json!(["t1w", "flair"]), "{doc}");
    assert!(doc["main"]["by_role"][0].get("members").is_none(), "{doc}");
    let (released, picks) = lab.input_of(run);
    assert_eq!(released, b1_both, "the input holds B1's T1w and FLAIR");
    assert_eq!(picks["defaults"], true, "{picks}");
    // the release keeps each stack as the role it was taken for, and
    // records the roles, not each role's stacks
    assert_eq!(picks["main"], "t1w+flair", "{picks}");
    assert_eq!(picks["roles"], json!(["t1w", "flair"]), "{picks}");
    assert!(picks.get("by_role").is_none(), "{picks}");
    assert_eq!(
        picks["frozen"]["listed"], 0,
        "the live picks named each: {picks}"
    );
    let kept = lab.kept_of(run);
    assert_eq!(kept["t1w"], lab.stacks_of(&[b1], "MPRAGE"), "{kept:?}");
    assert_eq!(kept["flair"], lab.stacks_of(&[b1], "TSE"), "{kept:?}");
    let history = lab.ok(&[
        "release",
        "--history",
        "--name",
        &format!("pipeline-run-{run}"),
    ]);
    assert!(
        history.contains("a run's list of main t1w and flair scans"),
        "{history}"
    );
    let skipped = doc["units_run"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|u| u["status"] == "skipped")
        .count();
    assert_eq!(skipped, 0, "no visit taken lacks a role: {doc}");
    assert_eq!(doc["summary"]["units"]["total"], 1, "{doc}");
    drop(server);

    // split with --chunk, by whole visits: a part holds a visit's T1w and
    // its FLAIR, however few stacks a part was asked to hold
    let before = lab.newest_run();
    let (_, out, err) = lab.run(
        &[
            "run",
            "needs-both",
            "--select",
            "main@dataset:alpha",
            "--chunk",
            "1",
            "--json",
        ],
        None,
    );
    assert!(
        err.contains("4 stack(s) in 2 parts of at most 1, whole visits"),
        "{err}\n{out}"
    );
    let parts: Vec<(i64, i64)> = {
        let mut store = lab.store();
        store
            .query(
                &format!(
                    "SELECT id, handle_id FROM {} WHERE id > {before} ORDER BY id",
                    store.qualified("pipeline_run")
                ),
                &[],
            )
            .unwrap()
            .iter()
            .map(|r| (r.int(0).unwrap(), r.int(1).unwrap()))
            .collect()
    };
    assert_eq!(parts.len(), 2, "{err}\n{out}");
    let mut each = BTreeSet::new();
    for (i, (run, handle)) in parts.iter().enumerate() {
        let doc = lab.json(&["pipeline", "runs", &run.to_string(), "--json"]);
        assert_eq!(
            doc["main"]["part"],
            json!({"part": i + 1, "of": 2}),
            "{doc}"
        );
        assert_eq!(doc["main"]["by_role"][0]["stacks"], 1, "{doc}");
        assert_eq!(doc["main"]["by_role"][1]["stacks"], 1, "{doc}");
        let mine = lab.frozen(&json!(handle));
        assert_eq!(mine.len(), 2, "one visit's T1w and FLAIR: {mine:?}");
        assert_eq!(mine.intersection(&mprage).count(), 1, "{mine:?}");
        assert_eq!(mine.intersection(&flair).count(), 1, "{mine:?}");
        let (released, _) = lab.input_of(*run);
        assert_eq!(released, mine, "{doc}");
        each.extend(mine);
    }
    assert_eq!(each, both);
}

#[test]
fn a_scope_with_nothing_to_run_over_says_why() {
    nothing_to_run(None);
}

#[test]
fn a_scope_with_nothing_to_run_over_says_why_on_postgres_too() {
    on_postgres("nils_run_nothing_over", nothing_to_run);
}

/// 2026-10-12, found by the chat bench: the pre-flight over a scope's main
/// scans that leave nothing to run says why. Picked stacks of which the
/// registry holds no file to read are named as such, never as unpicked;
/// and a dataset with no main scan of the role is nothing to run over,
/// its visits counted by why, never a panic.
fn nothing_to_run(pg: Option<(String, String)>) {
    let lab = Lab::new(pg);
    let who = lab.subjects();
    let (b1, b2) = (who["B1"], who["B2"]);
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

    // 1. a cohort whose main scans are picked, and whose files the
    // registry no longer holds
    lab.ok(&["clinical", "cohort", "make", "filess"]);
    lab.ok(&[
        "clinical",
        "cohort",
        "add",
        "filess",
        &codes[&b1],
        &codes[&b2],
        "--why",
        "a pre-flight with nothing to read",
    ]);
    let all = lab.stacks_of(&[b1, b2], "MPRAGE");
    assert!(!all.is_empty());
    {
        let mut store = lab.store();
        let sql = format!(
            "DELETE FROM {} WHERE instance_id IN (SELECT id FROM {} WHERE stack_id IN ({}))",
            store.qualified("source_file"),
            store.qualified("instance"),
            list(&all)
        );
        store.batch(&sql).unwrap();
    }
    let pre = lab.preflight(&["--select", "main:t1w@cohort:filess"]);
    let listed = lab.frozen(&pre["handle"]);
    assert!(!listed.is_empty(), "its main scans are picked: {pre}");
    assert_eq!(pre["ready"], false, "{pre}");
    assert_eq!(pre["units"]["total"], 0, "{pre}");
    let said = pre["blockers"].to_string();
    assert!(
        said.contains(&format!(
            "the release of the input would hold no subject's images: {n} of the selection's {n} stacks: the registry holds no file of it to read",
            n = listed.len()
        )),
        "{pre}"
    );
    assert!(!said.contains("is picked"), "they are picked: {pre}");
    assert!(!said.contains("nils pick run"), "{pre}");

    // 2. a dataset none of whose visits has a main scan of the role
    {
        let mut store = lab.store();
        let sql = format!(
            "DELETE FROM {}; DELETE FROM {}",
            store.qualified("pick_stack"),
            store.qualified("pick")
        );
        store.batch(&sql).unwrap();
    }
    let (good, out, err) = lab.run(
        &[
            "run",
            "needs-t1w",
            "--select",
            "main:t1w@dataset:alpha",
            "--preflight",
            "--json",
        ],
        None,
    );
    assert!(!err.contains("panicked"), "{err}");
    assert!(good, "{err}\n{out}");
    let pre: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(pre["stacks"], 0, "{pre}");
    assert_eq!(pre["ready"], false, "{pre}");
    let main = &pre["main"];
    assert_eq!(main["main_visits"], 0, "{main}");
    let visits = main["visits"].as_u64().unwrap();
    assert!(visits > 0, "{main}");
    assert_eq!(main["without"]["visits"], visits, "{main}");
    let counted: u64 = main["without"]["why"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["visits"].as_u64().unwrap())
        .sum();
    assert_eq!(counted, visits, "each visit counted by why: {main}");
    assert!(
        pre["blockers"]
            .to_string()
            .contains("main:t1w@dataset:alpha has no main t1w scan to run over"),
        "{pre}"
    );
    // the words a person reads say it too
    let text = lab.ok(&[
        "run",
        "needs-t1w",
        "--select",
        "main:t1w@dataset:alpha",
        "--preflight",
    ]);
    assert!(text.contains("has no main t1w scan to run over"), "{text}");
}
