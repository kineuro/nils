// SPDX-License-Identifier: AGPL-3.0-only
//! Wave 7a, the Data page (2026-10-09): a dataset brought in through the
//! worker, read whole at `GET /api/datasets/{name}/summary` (what it holds
//! and where it is, step by step, in counts), its jobs at
//! `GET /api/jobs?dataset=`, the main scans of a cohort's members at
//! `GET /api/picks/summary?cohort=` with the subjects per role, and the
//! cohort documents with what brought their members, the datasets holding
//! them, the dataset of each digest's join, the clinical coverage and when
//! each release was made. On SQLite always, and on Postgres where a test
//! DSN is set.

use std::io::Write as _;
use std::process::{Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_registry::Store;
use serde_json::{Value, json};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../packs")
        .to_str()
        .unwrap()
        .to_string()
}

struct Home {
    dir: TempDir,
    /// The Postgres DSN and schema, when the registry lives there.
    pg: Option<(String, String)>,
}

impl Home {
    fn run(&self, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
        let mut child = nils()
            .arg("--registry")
            .arg(self.dir.path())
            .args(args)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_DSN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(text) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        } else {
            drop(child.stdin.take());
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
        match &self.pg {
            Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
            None => Store::open_sqlite(&self.dir.path().join("registry.db")).unwrap(),
        }
    }
}

const OPS: &str = "an-operator-token-of-length";
const READS: &str = "a-reader-token-of-its-length";
const PLACES: &str = "a-places-token-of-its-length";

/// A `nils serve` with its worker, killed when dropped.
struct Worked {
    child: std::process::Child,
    port: u16,
}

impl Drop for Worked {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Worked {
    fn start(home: &Home) -> Worked {
        use std::io::BufRead as _;
        let tokens = [
            format!("{OPS}=ops@lab:operator"),
            format!("{READS}=lou@lab:reader"),
            format!("{PLACES}=pia@lab:places:see"),
        ]
        .join(",");
        let mut child = nils()
            .arg("--registry")
            .arg(home.dir.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "2",
                "--worker",
            ])
            .args(["--auth", "token", "--pack-dir", &packs()])
            .env("NILS_TOKENS", tokens)
            .env("NILS_PACK_DIR", packs())
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut lines = std::io::BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            let _ = child.kill();
            panic!("nils serve did not listen");
        };
        let addr = first.split_whitespace().nth(2).unwrap();
        let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
        std::thread::spawn(move || for _ in lines {});
        Worked { child, port }
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> (u16, Value) {
        use std::io::{Read as _, Write as _};
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: application/json\r\nAuthorization: Bearer {token}\r\n\r\n",
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

    fn get(&self, path: &str, token: &str) -> Value {
        let (status, doc) = self.call("GET", path, None, token);
        assert_eq!(status, 200, "{path}: {doc}");
        doc
    }

    /// The dataset's jobs once every kind of `kinds` has one and all are
    /// over, two minutes at most.
    fn settled(&self, dataset: &str, kinds: &[&str]) -> Vec<Value> {
        for _ in 0..1200 {
            let doc = self.get(&format!("/api/jobs?dataset={dataset}&all=1"), OPS);
            let jobs = doc["jobs"].as_array().unwrap().clone();
            let over = jobs
                .iter()
                .all(|j| matches!(j["state"].as_str(), Some("done" | "failed" | "cancelled")));
            let every = kinds.iter().all(|k| jobs.iter().any(|j| j["kind"] == *k));
            if over && every {
                return jobs;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("the jobs of {dataset} did not settle");
    }
}

fn steps(summary: &Value) -> Vec<(String, Value)> {
    summary["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["step"].as_str().unwrap().to_string(), s.clone()))
        .collect()
}

fn step(summary: &Value, name: &str) -> Value {
    steps(summary)
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no step {name}: {summary}"))
        .1
}

fn round(pg: Option<(String, String)>) {
    let home = Home {
        dir: TempDir::new("data-page-home"),
        pg,
    };
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a data page test key\n"));
    assert!(good, "{err}");
    match &home.pg {
        Some((dsn, schema)) => {
            home.ok(&[
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
            home.ok(&["init", "--key", "k"]);
        }
    }
    // two subjects with a T1 each, the first with a series no rule reads,
    // and a file a read refuses
    let dir = TempDir::new("data-page-ds");
    for (p, patient) in ["S-ONE", "S-TWO"].iter().enumerate() {
        let study = format!("1.2.826.0.1.3680043.8.499.{}.1", p + 1);
        for (n, description) in ["t1 mprage", "zzqx"].iter().enumerate() {
            if p == 1 && n == 1 {
                continue;
            }
            let series = format!("{study}.{}", n + 1);
            for instance in 1..=3u32 {
                let sop = format!("{series}.{instance}");
                let mut e = synth::minimal_mr(&study, &series, &sop);
                e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
                e.push(synth::text(tags::STUDY_DATE, VR::DA, "20240131"));
                e.push(synth::text(
                    tags::SERIES_NUMBER,
                    VR::IS,
                    &(n + 1).to_string(),
                ));
                e.push(synth::text(
                    tags::INSTANCE_NUMBER,
                    VR::IS,
                    &instance.to_string(),
                ));
                e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
                // pixels, so the sort makes a picture and the lane a 3D view
                e.push(synth::text(
                    tags::IMAGE_POSITION_PATIENT,
                    VR::DS,
                    &format!("0\\0\\{}", instance * 2),
                ));
                e.push(synth::text(
                    tags::IMAGE_ORIENTATION_PATIENT,
                    VR::DS,
                    "1\\0\\0\\0\\1\\0",
                ));
                e.push(synth::text(tags::PIXEL_SPACING, VR::DS, "1\\1"));
                e.push(synth::text(tags::SLICE_THICKNESS, VR::DS, "2"));
                e.push(synth::us(tags::SAMPLES_PER_PIXEL, 1));
                e.push(synth::text(
                    tags::PHOTOMETRIC_INTERPRETATION,
                    VR::CS,
                    "MONOCHROME2",
                ));
                e.push(synth::us(tags::ROWS, 16));
                e.push(synth::us(tags::COLUMNS, 16));
                e.push(synth::us(tags::BITS_ALLOCATED, 16));
                e.push(synth::us(tags::BITS_STORED, 16));
                e.push(synth::us(tags::HIGH_BIT, 15));
                e.push(synth::us(tags::PIXEL_REPRESENTATION, 0));
                let px: Vec<u8> = (0..256u32)
                    .flat_map(|i| (((i * 7 + instance * 101) % 4096) as u16).to_le_bytes())
                    .collect();
                e.push(synth::bytes(tags::PIXEL_DATA, VR::OW, px));
                dir.file(
                    &format!("sub-{p}/s{n}/IM_{instance:04}"),
                    &synth::part10(&MetaFields::mr(&sop), &e, true),
                );
            }
        }
    }
    dir.file("sub-0/notes.dcm", b"not a scan, refused by the read");
    home.ok(&[
        "place",
        "add",
        "ds",
        dir.path().to_str().unwrap(),
        "--role",
        "source",
        "--move-into",
        "anon",
        "--confirm-move",
        "--patient-id",
        "id-type:patient-id",
        "--subjects",
        "generated",
        "--cohort",
        "fed",
    ]);
    // another dataset, which holds nothing yet
    let other = TempDir::new("data-page-other");
    std::fs::create_dir_all(other.path().join("derivatives/dcm-anon")).unwrap();
    home.ok(&[
        "place",
        "add",
        "other",
        other.path().to_str().unwrap(),
        "--role",
        "source",
        "--patient-id",
        "id-type:patient-id",
        "--subjects",
        "generated",
    ]);
    // a working place, where the sort makes the pictures and the
    // pictures lane the 3D views
    let work = TempDir::new("data-page-work");
    home.ok(&[
        "place",
        "add",
        "work",
        work.path().to_str().unwrap(),
        "--role",
        "working",
    ]);

    let server = Worked::start(&home);
    let (status, queued) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["bring-in", "@ds", "--name", "first"]})),
        OPS,
    );
    assert_eq!(status, 202, "{queued}");
    let first = queued["job"].as_i64().unwrap();
    let jobs = server.settled(
        "ds",
        &["digest", "fingerprint", "classify", "pick", "pyramid"],
    );
    let id_of = |kind: &str| -> i64 {
        jobs.iter()
            .find(|j| j["kind"] == kind)
            .and_then(|j| j["id"].as_i64())
            .unwrap_or_else(|| panic!("no {kind} job: {jobs:?}"))
    };
    for j in &jobs {
        assert_eq!(j["state"], "done", "{j}");
    }

    // the jobs door: a dataset's jobs, newest first, and nobody else's
    assert_eq!(id_of("digest"), first, "{jobs:?}");
    let ids: Vec<i64> = jobs.iter().map(|j| j["id"].as_i64().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(ids, sorted, "newest first: {ids:?}");
    let open = server.get("/api/jobs?dataset=ds", OPS);
    assert_eq!(open["count"], 0, "none open: {open}");
    let two = server.get("/api/jobs?dataset=ds&all=1&limit=2", OPS);
    assert_eq!(two["count"], 2, "{two}");
    assert_eq!(two["jobs"][0]["id"], ids[0], "{two}");
    let none = server.get("/api/jobs?dataset=other&all=1", OPS);
    assert_eq!(
        none["count"], 0,
        "another dataset's jobs are not its: {none}"
    );
    let (status, _) = server.call("GET", "/api/jobs?dataset=nowhere&all=1", None, OPS);
    assert_eq!(status, 404);

    // the summary: what it holds and where it is, at plain
    let s = server.get("/api/datasets/ds/summary", READS);
    assert_eq!(s["dataset"], "ds", "{s}");
    assert_eq!(s["detail"], "plain", "{s}");
    assert_eq!(s["state"], "anonymised", "{s}");
    assert_eq!(s["subjects"], 2, "{s}");
    assert_eq!(s["scans"], 3, "{s}");
    assert_eq!(s["studies"], 2, "{s}");
    // the visits, out of the session cache the sort's pick run built
    assert_eq!(s["sessions"], 2, "{s}");
    let n = |v: &Value, k: &str| v[k].as_i64().unwrap_or_else(|| panic!("{k}: {v}"));
    assert_eq!(
        n(&s, "sure") + n(&s, "need_a_look") + n(&s, "unsorted"),
        3,
        "{s}"
    );
    let kinds = s["kinds"].as_array().unwrap();
    // as many as the sort said are T1w
    let said = {
        let mut store = home.store();
        let axis = store.qualified("classification_axis");
        store
            .query(
                &format!("SELECT COUNT(*) FROM {axis} WHERE axis = 'base' AND value = 'T1w'"),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap()
    };
    assert!(said >= 2, "{said}");
    let t1w = |doc: &Value| -> i64 {
        doc["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["kind"] == "T1w")
            .map(|k| k["scans"].as_i64().unwrap())
            .unwrap_or(0)
    };
    assert_eq!(t1w(&s), said, "{kinds:?}");
    assert!(s["body_regions"].is_array(), "{s}");
    assert_eq!(s["files"]["read"], 9, "{s}");
    assert_eq!(s["files"]["refused"], 1, "{s}");
    assert!(s["files"]["refused_batch"].as_i64().is_some(), "{s}");
    assert_eq!(s["pictures_place"], "work", "{s}");
    let names: Vec<String> = steps(&s).into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        names,
        ["found", "read", "sorted", "main_scans", "pictures", "views"],
        "no originals, so no pseudonymised step: {s}"
    );
    let found = step(&s, "found");
    assert_eq!(found["state"], "done", "{s}");
    assert_eq!(found["tree"], "anon", "{s}");
    let read = step(&s, "read");
    assert_eq!(read["state"], "done", "{s}");
    assert_eq!(read["job"], id_of("digest"), "{s}");
    assert_eq!(read["files"], 9, "{s}");
    assert_eq!(read["refused"], 1, "{s}");
    assert_eq!(read["reads"], 1, "{s}");
    assert!(read["finished_at"].is_string(), "{s}");
    let sorted_step = step(&s, "sorted");
    assert_eq!(sorted_step["state"], "done", "{s}");
    assert_eq!(sorted_step["job"], id_of("classify"), "{s}");
    assert_eq!(sorted_step["scans"], 3, "{s}");
    assert_eq!(sorted_step["of"], 3, "{s}");
    let main = step(&s, "main_scans");
    assert_eq!(main["state"], "done", "{s}");
    assert_eq!(main["job"], id_of("pick"), "{s}");
    assert_eq!(main["picked"], 2, "{s}");
    let pictures = step(&s, "pictures");
    assert_eq!(pictures["state"], "done", "{s}");
    assert_eq!(pictures["made"], 3, "the sort made every picture: {s}");
    assert_eq!(pictures["of"], 3, "{s}");
    assert_eq!(pictures["in_sort"], true, "{s}");
    let views = step(&s, "views");
    assert_eq!(views["state"], "done", "{s}");
    assert_eq!(views["job"], id_of("pyramid"), "{s}");
    assert_eq!(views["made"], 3, "{s}");
    // a sort's run goes on making its pictures after its row says done, and
    // writes them into its result at the end: until then they are being made
    {
        let mut store = home.store();
        let [job, stack] = ["job", "stack"].map(|t| store.qualified(t));
        let first = store
            .query(&format!("SELECT MIN(id) FROM {stack}"), &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        let picture = work
            .path()
            .join("previews")
            .join(format!("{:03}", first.rem_euclid(1000)))
            .join(format!("{first}.preview"));
        std::fs::remove_file(&picture).unwrap();
        store
            .execute(
                &format!(
                    "UPDATE {job} SET result = NULL WHERE id = {}",
                    id_of("classify")
                ),
                &[],
            )
            .unwrap();
    }
    let making = server.get("/api/datasets/ds/summary", READS);
    let pictures = step(&making, "pictures");
    assert_eq!(pictures["state"], "running", "{making}");
    assert_eq!(pictures["made"], 2, "{making}");
    assert_eq!(
        pictures["progress"],
        json!({"done": 2, "total": 3}),
        "{making}"
    );
    // the sort itself is done
    assert_eq!(step(&making, "sorted")["state"], "done", "{making}");

    // counts only: no subject's code, no date
    let text = s.to_string();
    assert!(
        !text.contains("S-ONE") && !text.contains("2024-01-31"),
        "{s}"
    );

    // with the cache emptied the visits are no number rather than none,
    // since building them is a person's act
    {
        let mut store = home.store();
        for t in ["session_cache_study", "session_cache"] {
            let t = store.qualified(t);
            store.execute(&format!("DELETE FROM {t}"), &[]).unwrap();
        }
    }
    let unbuilt = server.get("/api/datasets/ds/summary", READS);
    assert!(unbuilt["sessions"].is_null(), "{unbuilt}");
    home.ok(&["session", "rebuild"]);
    let built = server.get("/api/datasets/ds/summary", READS);
    assert_eq!(built["sessions"], 2, "{built}");

    // a stack of a sample sealed now is counted by no kind
    {
        let mut store = home.store();
        let [axis, stack, series, sealed] =
            ["classification_axis", "stack", "series", "sealed_stack"].map(|t| store.qualified(t));
        let row = store
            .query(
                &format!(
                    "SELECT a.stack_id, se.subject_id FROM {axis} a JOIN {stack} st ON st.id = a.stack_id \
                     JOIN {series} se ON se.id = st.series_id \
                     WHERE a.axis = 'base' AND a.value = 'T1w' ORDER BY a.stack_id"
                ),
                &[],
            )
            .unwrap();
        let (stack_id, subject) = (row[0].int(0).unwrap(), row[0].int(1).unwrap());
        store
            .execute(
                &format!(
                    "INSERT INTO {sealed} (sample, stack_id, subject_id, sealed_by, sealed_at) \
                     VALUES ('s@1', {stack_id}, {subject}, 'anna', '2026-10-09T10:00:00Z')"
                ),
                &[],
            )
            .unwrap();
    }
    let after_seal = server.get("/api/datasets/ds/summary", READS);
    assert_eq!(t1w(&after_seal), said - 1, "{after_seal}");
    // the scans are still counted, as the card counts them
    assert_eq!(after_seal["scans"], 3, "{after_seal}");

    // a dataset nothing has read: every step waits but the found one
    let o = server.get("/api/datasets/other/summary", READS);
    assert_eq!(o["scans"], 0, "{o}");
    assert_eq!(o["kinds"], json!([]), "{o}");
    for name in ["read", "sorted", "main_scans", "pictures", "views"] {
        assert_eq!(step(&o, name)["state"], "waiting", "{name}: {o}");
    }
    // no such dataset, and the grant is data:see
    let (status, _) = server.call("GET", "/api/datasets/nowhere/summary", None, READS);
    assert_eq!(status, 404);
    let (status, _) = server.call("GET", "/api/datasets/ds/summary", None, PLACES);
    assert_eq!(status, 403);

    // the main scans of a dataset and of a cohort: the subjects per role
    let picks = server.get("/api/picks/summary?dataset=ds", READS);
    assert_eq!(picks["roles"]["t1w"]["picked"], 2, "{picks}");
    assert_eq!(picks["roles"]["t1w"]["subjects"], 2, "{picks}");
    let fed = server.get("/api/picks/summary?cohort=fed", READS);
    assert_eq!(fed["cohort"], "fed", "{fed}");
    assert_eq!(fed["subjects"], 2, "{fed}");
    assert_eq!(fed["roles"]["t1w"]["picked"], 2, "{fed}");
    assert_eq!(fed["roles"]["t1w"]["subjects"], 2, "{fed}");
    assert!(fed.get("last_run").is_some(), "{fed}");
    let (status, _) = server.call("GET", "/api/picks/summary?cohort=nobody", None, READS);
    assert_eq!(status, 404);
    let (status, _) = server.call("GET", "/api/picks/summary", None, READS);
    assert_eq!(status, 400);

    // the cohorts: what brought the members, and the datasets holding them
    let (status, made) = server.call("POST", "/api/cohorts", Some(json!({"name": "hands"})), OPS);
    assert_eq!(status, 201, "{made}");
    let code = {
        let mut store = home.store();
        let subject = store.qualified("subject");
        store
            .query(&format!("SELECT code FROM {subject} ORDER BY id"), &[])
            .unwrap()[0]
            .text(0)
            .unwrap()
            .to_string()
    };
    let (status, added) = server.call(
        "POST",
        "/api/cohorts/hands/members",
        Some(json!({"add": [code], "why": "a sample"})),
        OPS,
    );
    assert_eq!(status, 200, "{added}");
    let listed = server.get("/api/cohorts", READS);
    let cohort = |name: &str| -> Value {
        listed
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no {name}: {listed}"))
            .clone()
    };
    assert_eq!(
        cohort("fed")["parts"],
        json!([{"from": "dataset", "dataset": "ds", "subjects": 2}]),
        "{listed}"
    );
    assert_eq!(
        cohort("fed")["datasets"],
        json!([{"name": "ds", "subjects": 2, "scans": 3, "feeds": true}]),
        "{listed}"
    );
    assert_eq!(
        cohort("hands")["parts"],
        json!([{"from": "hand", "dataset": null, "subjects": 1}]),
        "{listed}"
    );
    let hands = cohort("hands");
    let held = &hands["datasets"][0];
    assert_eq!(held["name"], "ds", "{hands}");
    assert_eq!(held["subjects"], 1, "{hands}");
    assert_eq!(held["feeds"], false, "{hands}");
    assert!(held["scans"].as_i64().unwrap() >= 1, "{hands}");

    // a cohort whole: the dataset of its digest's join, its clinical
    // coverage (never a kind marked sensitive) and when its releases were made
    {
        let mut store = home.store();
        let [kind, event, subject, release] =
            ["observation_type", "event", "subject", "release"].map(|t| store.qualified(t));
        store
            .execute(
                &format!(
                    "INSERT INTO {kind} (name, category, is_primary, is_sensitive) VALUES \
                     ('EDSS', 'scale', 1, 0), ('Relapse', 'event', 0, 0), ('Hidden', 'note', 0, 1)"
                ),
                &[],
            )
            .unwrap();
        let first = store
            .query(&format!("SELECT id FROM {subject} ORDER BY id"), &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        for name in ["EDSS", "Relapse", "Hidden"] {
            store
                .execute(
                    &format!(
                        "INSERT INTO {event} (subject_id, observation_type_id, event_date, value, created_at) \
                         SELECT {first}, id, '2024-02-01', '2', '2026-10-09T10:00:00Z' FROM {kind} WHERE name = '{name}'"
                    ),
                    &[],
                )
                .unwrap();
        }
        store
            .execute(
                &format!(
                    "INSERT INTO {release} (name, version, root, policy, selection, categories, session_scheme, \
                     layout, placements, pack, pack_version, actor, started_at, finished_at, files, subjects, \
                     unchanged, moved, rewritten, added, removed) VALUES \
                     ('fed-r1', '1', '/x', '{{}}', '{{\"cohorts\": [\"fed\"]}}', 'imaging', 'default', 'bids', \
                     '{{}}', 'mri', '1', 'anna', '2026-10-05T09:00:00Z', '2026-10-05T10:00:00Z', 6, 2, 0, 0, 0, 6, 0)"
                ),
                &[],
            )
            .unwrap();
    }
    let shown = server.get("/api/cohorts/fed", READS);
    let joins = shown["joins"].as_array().unwrap();
    assert_eq!(joins[0]["what"], "digest", "{shown}");
    assert_eq!(joins[0]["dataset"], "ds", "{shown}");
    assert_eq!(
        shown["clinical"],
        json!([
            {"kind": "EDSS", "primary": true, "subjects": 1},
            {"kind": "Relapse", "primary": false, "subjects": 1},
        ]),
        "{shown}"
    );
    assert_eq!(shown["releases"][0]["name"], "fed-r1", "{shown}");
    let at = shown["releases"][0]["finished_at"].as_str().unwrap();
    assert!(at.starts_with("2026-10-05"), "{shown}");
    assert_eq!(shown["parts"], cohort("fed")["parts"], "{shown}");

    // the doors are listed
    let caps = server.get("/api/capabilities", READS);
    assert!(
        caps["doors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d == "GET /api/datasets/{name}/summary"),
        "{caps}"
    );
    drop(server);
    drop(work);
    drop(other);
    drop(dir);
}

#[test]
fn a_dataset_reads_whole_with_its_steps_its_jobs_and_its_cohorts() {
    round(None);
}

#[test]
fn a_dataset_reads_whole_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    let schema = "nils_data_page";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    round(Some((dsn.clone(), schema.to_string())));
    drop();
}
