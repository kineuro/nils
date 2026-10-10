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
/// Data work at detail plain: it may mark held files, never queue the
/// pseudonymiser, which reads the identifiers it replaces.
const WORKS: &str = "a-data-work-token-of-length";
/// A reader with the certificate's grant, who reads a sample sealed now.
const SEALED: &str = "a-sealed-token-of-its-length";

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
        Worked::serve(home, true)
    }

    /// A `nils serve`, with its worker or without one.
    fn serve(home: &Home, worker: bool) -> Worked {
        use std::io::BufRead as _;
        let tokens = [
            format!("{OPS}=ops@lab:operator"),
            format!("{READS}=lou@lab:reader"),
            format!("{PLACES}=pia@lab:places:see"),
            format!("{WORKS}=wes@lab:data:work,data:see"),
            format!("{SEALED}=sam@lab:reader,sealed:see"),
        ]
        .join(",");
        let child = nils()
            .arg("--registry")
            .arg(home.dir.path())
            .args(["serve", "--bind", "127.0.0.1:0", "--workers", "2"])
            .args(if worker { &["--worker"][..] } else { &[][..] })
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
        // held from here, so that a panic below kills it too
        let mut held = Worked { child, port: 0 };
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
        [
            "found",
            "read",
            "sorted",
            "body_part",
            "post_contrast",
            "main_scans",
            "pictures",
            "views"
        ],
        "no originals, so no pseudonymised step: {s}"
    );
    // record 56: body part and post-contrast are steps of their own after
    // the sort, and nothing serves them yet
    for name in ["body_part", "post_contrast"] {
        let op = step(&s, name);
        assert_eq!(op["state"], "off", "{name}: {s}");
        assert_eq!(op["served"], false, "{name}: {s}");
        assert_eq!(op["answered"], 0, "{name}: {s}");
        assert_eq!(op["of"], 3, "{name}: {s}");
        assert!(op["job"].is_null(), "{name}: {s}");
        assert_eq!(op["jobs"], json!([]), "{name}: {s}");
    }
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

    // record 56: how sure the sort is counts only what the sort asks, and a
    // question about body part or post-contrast waits under its own step
    let (stack_ids, now) = {
        let mut store = home.store();
        let [stack, item] = ["stack", "review_item"].map(|t| store.qualified(t));
        // what a sort asked about them before record 56 is let go, so the
        // counts below are this test's own
        store
            .execute(
                &format!(
                    "UPDATE {item} SET status = 'superseded' \
                     WHERE kind LIKE 'body%' OR kind LIKE 'post%'"
                ),
                &[],
            )
            .unwrap();
        let ids: Vec<i64> = store
            .query(&format!("SELECT id FROM {stack} ORDER BY id"), &[])
            .unwrap()
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect();
        (ids, nils_registry::time::now_iso())
    };
    assert_eq!(stack_ids.len(), 3, "{stack_ids:?}");
    let before = server.get("/api/datasets/ds/summary", READS);
    {
        let mut store = home.store();
        let item = store.qualified("review_item");
        for (kind, stack) in [
            ("body_part:low_confidence", stack_ids[0]),
            ("post_contrast:missing", stack_ids[1]),
            ("base:missing", stack_ids[2]),
        ] {
            store
                .execute(
                    &format!(
                        "INSERT INTO {item} (kind, scope, ref, evidence, status, created_at) VALUES \
                         ('{kind}', 'stack', '{{\"stack_id\": {stack}}}', '{{}}', 'open', '{now}')"
                    ),
                    &[],
                )
                .unwrap();
        }
    }
    let asked = server.get("/api/datasets/ds/summary", READS);
    let look_kinds = |doc: &Value| doc["look_kinds"].as_object().unwrap().clone();
    let of_kind =
        |doc: &Value, k: &str| look_kinds(doc).get(k).and_then(Value::as_i64).unwrap_or(0);
    assert!(
        look_kinds(&asked)
            .keys()
            .all(|k| !k.starts_with("body_") && !k.starts_with("post_contrast")),
        "{asked}"
    );
    assert_eq!(
        of_kind(&asked, "base:missing"),
        of_kind(&before, "base:missing") + 1,
        "{asked}"
    );
    assert_eq!(step(&asked, "body_part")["look"], 1, "{asked}");
    assert_eq!(step(&asked, "post_contrast")["look"], 1, "{asked}");
    assert_eq!(
        step(&asked, "sorted")["look"],
        asked["need_a_look"],
        "{asked}"
    );
    assert_eq!(
        n(&asked, "sure") + n(&asked, "need_a_look") + n(&asked, "unsorted"),
        3,
        "{asked}"
    );

    // a pipeline in the catalog proposes the body part and a model is
    // admitted to answer it: the step waits to be run
    {
        let mut store = home.store();
        let [pipeline, model] = ["pipeline", "model"].map(|t| store.qualified(t));
        store
            .execute(
                &format!(
                    "INSERT INTO {pipeline} (name, version, tool_version, descriptor, descriptor_digest, \
                     image, image_digest, layout, level, state, added_by, added_at) VALUES \
                     ('bp-infer', 1, '1', \
                     '{{\"x-nils\": {{\"proposals\": [{{\"axis\": \"body_part\"}}, {{\"axis\": \"body_region\"}}]}}}}', \
                     'sha256:d', 'bp', 'sha256:i', 'stacks', 'stack', 'active', 'anna', '{now}')"
                ),
                &[],
            )
            .unwrap();
        store
            .execute(
                &format!(
                    "INSERT INTO {model} (name, version, kind, digest, task, slot, state, card, \
                     registered_by, registered_at) VALUES \
                     ('bp-head', '1', 'head', 'sha256:h', 'axis:body_part', 'site', 'admitted', '{{}}', \
                     'anna', '{now}')"
                ),
                &[],
            )
            .unwrap();
    }
    let served = server.get("/api/datasets/ds/summary", READS);
    let bp = step(&served, "body_part");
    assert_eq!(bp["state"], "waiting", "{served}");
    assert_eq!(bp["served"], true, "{served}");
    assert_eq!(
        step(&served, "post_contrast")["state"],
        "off",
        "nothing serves post-contrast: {served}"
    );

    // a run of it over the dataset's scans, one unit over of three
    let (job_id, run_id) = {
        let mut store = home.store();
        let [job, handle, member, pipeline, run, unit] = [
            "job",
            "handle",
            "handle_member",
            "pipeline",
            "pipeline_run",
            "pipeline_unit",
        ]
        .map(|t| store.qualified(t));
        let newest = |store: &mut Store, t: &str| {
            store
                .query(&format!("SELECT MAX(id) FROM {t}"), &[])
                .unwrap()[0]
                .int(0)
                .unwrap()
        };
        store
            .execute(
                &format!(
                    "INSERT INTO {job} (kind, name, args, state, started_at, heartbeat_at) VALUES \
                     ('pipeline', 'bp-infer@1', '{{}}', 'running', '{now}', '{now}')"
                ),
                &[],
            )
            .unwrap();
        let job_id = newest(&mut store, job.as_str());
        store
            .execute(
                &format!(
                    "INSERT INTO {handle} (grain, columns, row_count, ast_version, ask, principal, \
                     created_at, node, epoch, disclosure, truncated) VALUES \
                     ('stack', '[]', 3, 1, '{{}}', 'anna', '{now}', 'ward-3', 0, 'plain', 0)"
                ),
                &[],
            )
            .unwrap();
        let handle_id = newest(&mut store, handle.as_str());
        for (i, s) in stack_ids.iter().enumerate() {
            store
                .execute(
                    &format!(
                        "INSERT INTO {member} (handle_id, position, key) VALUES ({handle_id}, {i}, {s})"
                    ),
                    &[],
                )
                .unwrap();
        }
        let pipeline_id = newest(&mut store, pipeline.as_str());
        store
            .execute(
                &format!(
                    "INSERT INTO {run} (pipeline_id, job_id, handle_id, params, runtime, runtime_version, \
                     host, device, model_ids, status, started_at, principal) VALUES \
                     ({pipeline_id}, {job_id}, {handle_id}, '{{}}', 'podman', '5', 'ward-3', 'cpu', '[]', \
                     'running', '{now}', 'anna')"
                ),
                &[],
            )
            .unwrap();
        let run_id = newest(&mut store, run.as_str());
        for (i, s) in stack_ids.iter().enumerate() {
            let state = if i == 0 { "over" } else { "queued" };
            store
                .execute(
                    &format!(
                        "INSERT INTO {unit} (run_id, unit, position, state, attempts) VALUES \
                         ({run_id}, 'stack-{s}', {i}, '{state}', 0)"
                    ),
                    &[],
                )
                .unwrap();
        }
        (job_id, run_id)
    };
    let running = server.get("/api/datasets/ds/summary", READS);
    let bp = step(&running, "body_part");
    assert_eq!(bp["state"], "running", "{running}");
    assert_eq!(bp["job"], job_id, "{running}");
    assert_eq!(bp["run"], run_id, "{running}");
    assert_eq!(bp["progress"], json!({"done": 1, "total": 3}), "{running}");
    // the run's job is in the dataset's log, and in no other's
    let log = server.get("/api/jobs?dataset=ds&all=1", OPS);
    assert!(
        log["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|j| j["id"] == job_id),
        "{log}"
    );
    let none = server.get("/api/jobs?dataset=other&all=1", OPS);
    assert_eq!(none["count"], 0, "{none}");

    // the run over: its model answered two scans at or above its
    // threshold and was unsure of the third, which waits on a person
    {
        let mut store = home.store();
        let [job, run, unit, item, member] = [
            "job",
            "pipeline_run",
            "pipeline_unit",
            "review_item",
            "review_member",
        ]
        .map(|t| store.qualified(t));
        let later = nils_registry::time::now_iso();
        for sql in [
            format!("UPDATE {job} SET state = 'done', finished_at = '{later}' WHERE id = {job_id}"),
            format!(
                "UPDATE {run} SET status = 'done', finished_at = '{later}' WHERE id = {run_id}"
            ),
            format!("UPDATE {unit} SET state = 'over' WHERE run_id = {run_id}"),
        ] {
            store.execute(&sql, &[]).unwrap();
        }
        for (status, members) in [("staged", &stack_ids[..2]), ("open", &stack_ids[2..])] {
            store
                .execute(
                    &format!(
                        "INSERT INTO {item} (kind, scope, ref, evidence, status, created_at, job_id, \
                         members, group_key) VALUES ('body_part:model', 'group', '{{}}', '{{}}', \
                         '{status}', '{later}', {job_id}, {}, 'run:{run_id}|body_part:model|brain|{status}|model:1')",
                        members.len()
                    ),
                    &[],
                )
                .unwrap();
            let item_id = store
                .query(&format!("SELECT MAX(id) FROM {item}"), &[])
                .unwrap()[0]
                .int(0)
                .unwrap();
            for s in members {
                store
                    .execute(
                        &format!(
                            "INSERT INTO {member} (item_id, stack_id) VALUES ({item_id}, {s})"
                        ),
                        &[],
                    )
                    .unwrap();
            }
        }
    }
    let done = server.get("/api/datasets/ds/summary", READS);
    let bp = step(&done, "body_part");
    assert_eq!(bp["state"], "done", "{done}");
    assert_eq!(bp["answered"], 2, "{done}");
    // the scan it was unsure of, and the one the sort asked about before
    assert_eq!(bp["look"], 2, "{done}");
    assert_eq!(bp["jobs"], json!([job_id]), "{done}");
    assert!(bp["progress"].is_null(), "{done}");
    assert!(bp["finished_at"].is_string(), "{done}");
    // the model's groups are no question of the sort's
    assert_eq!(of_kind(&done, "body_part:model"), 0, "{done}");
    assert_eq!(done["need_a_look"], asked["need_a_look"], "{done}");

    // a dataset nothing has read: every step waits but the found one
    let o = server.get("/api/datasets/other/summary", READS);
    assert_eq!(o["scans"], 0, "{o}");
    assert_eq!(o["kinds"], json!([]), "{o}");
    for name in ["read", "sorted", "main_scans", "pictures", "views"] {
        assert_eq!(step(&o, name)["state"], "waiting", "{name}: {o}");
    }
    // the body-part model is served, and has nothing of it to answer yet
    assert_eq!(step(&o, "body_part")["state"], "waiting", "{o}");
    assert_eq!(step(&o, "body_part")["of"], 0, "{o}");
    assert_eq!(step(&o, "post_contrast")["state"], "off", "{o}");
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
    let shown = server.get("/api/cohorts/fed", SEALED);
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
    // its steps: its members' scans sorted, then body part and
    // post-contrast over them (record 56)
    let names: Vec<String> = steps(&shown).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["sorted", "body_part", "post_contrast"], "{shown}");
    let sorted_step = step(&shown, "sorted");
    assert_eq!(sorted_step["state"], "done", "{shown}");
    assert_eq!(sorted_step["scans"], 3, "{shown}");
    assert_eq!(sorted_step["of"], 3, "{shown}");
    assert_eq!(sorted_step["look"], done["need_a_look"], "{shown}");
    let bp = step(&shown, "body_part");
    assert_eq!(bp["state"], "done", "{shown}");
    assert_eq!(bp["answered"], 2, "{shown}");
    assert_eq!(bp["look"], 2, "{shown}");
    assert_eq!(bp["of"], 3, "{shown}");
    assert_eq!(bp["job"], job_id, "{shown}");
    let pc = step(&shown, "post_contrast");
    assert_eq!(pc["state"], "off", "{shown}");
    assert_eq!(pc["look"], 1, "{shown}");
    // the review of Wave 7a's merge (2026-10-10): for a caller who does not
    // read sealed stacks, the cohort's steps leave the sample sealed above
    // out, its scan and the model's answers on it alike
    let unsealed = server.get("/api/cohorts/fed", READS);
    assert_eq!(step(&unsealed, "sorted")["of"], 2, "{unsealed}");
    assert_eq!(step(&unsealed, "body_part")["of"], 2, "{unsealed}");
    assert!(
        step(&unsealed, "body_part")["answered"].as_i64() <= bp["answered"].as_i64(),
        "{unsealed}"
    );
    // a cohort of one subject counts that subject's scans
    let one = server.get("/api/cohorts/hands", SEALED);
    assert_eq!(
        step(&one, "body_part")["of"],
        hands["datasets"][0]["scans"],
        "{one}"
    );

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

/// Wave 7a, the pseudonymise step: a dataset's held files, two thousand
/// over four hundred identifiers held round by round, answered one row an
/// identifier at `GET /api/linkage/held/ids` in the order its first file
/// was held, each with its own count and state, and identifiers chosen by
/// their rows coded anyway and no others. The grouping is the store's, so
/// on SQLite and on Postgres.
fn held_ids(pg: Option<(String, String)>) {
    use nils_registry::schema::Type;
    use nils_registry::store::Param;
    const IDS: i64 = 400;
    const EACH: i64 = 5;
    let home = Home {
        dir: TempDir::new("held-ids-home"),
        pg,
    };
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a held ids test key\n"));
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
    let ward = TempDir::new("held-ids-ward");
    std::fs::create_dir_all(ward.path().join("derivatives/dcm-original")).unwrap();
    home.ok(&[
        "place",
        "add",
        "ward",
        ward.path().to_str().unwrap(),
        "--role",
        "source",
    ]);
    // every seventh identifier released by a map, every fifth of the rest
    // coded anyway; an identifier's files lie four hundred rows apart
    let released = |i: i64| i % 7 == 0;
    let anyway = |i: i64| i % 5 == 0 && !released(i);
    {
        let mut store = home.store();
        let place = store.qualified("place");
        let id = store
            .query(&format!("SELECT id FROM {place} WHERE name = 'ward'"), &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        let d = store.dialect();
        let sql = format!(
            "INSERT INTO {} (place_id, path, size, mtime, state, shape, lookup, id_type, first_seen, released_at, code_anyway) \
             VALUES ({}, {}, 0, 0, 'held', 'AA9999', {}, 'study-id', {}, {}, {})",
            store.qualified("pseudonym_file"),
            d.param(1, Type::Int),
            d.param(2, Type::Text),
            d.param(3, Type::Bytes),
            d.param(4, Type::Timestamp),
            d.param(5, Type::Timestamp),
            d.param(6, Type::Int),
        );
        store.begin().unwrap();
        for round in 0..EACH {
            for i in 0..IDS {
                store
                    .execute(
                        &sql,
                        &[
                            Param::Int(id),
                            Param::from(format!("r{round}/f{i}")),
                            Param::Bytes(format!("lookup-{i:04}").into_bytes()),
                            Param::from("2026-10-09T00:00:00Z"),
                            if released(i) {
                                Param::from("2026-10-09T01:00:00Z")
                            } else {
                                Param::Null
                            },
                            Param::Int(i64::from(anyway(i))),
                        ],
                    )
                    .unwrap();
            }
        }
        store.commit().unwrap();
    }
    let served = Worked::serve(&home, false);
    let doc = served.get("/api/linkage/held/ids?place=ward", OPS);
    assert_eq!(doc["identifiers"], IDS, "{doc}");
    assert_eq!(doc["files"], IDS * EACH, "{doc}");
    let ids = doc["ids"].as_array().unwrap().clone();
    assert_eq!(ids.len() as i64, IDS);
    let rows: Vec<i64> = ids.iter().map(|h| h["id"].as_i64().unwrap()).collect();
    assert!(
        rows.windows(2).all(|w| w[0] < w[1]),
        "in the order the first file of each was held"
    );
    for (i, h) in (0..IDS).zip(&ids) {
        assert_eq!(h["files"], EACH, "identifier {i}: {h}");
        assert_eq!(h["shape"], "AA9999", "identifier {i}: {h}");
        let state = if released(i) {
            "mapped"
        } else if anyway(i) {
            "generated"
        } else {
            "held"
        };
        assert_eq!(h["state"], state, "identifier {i}: {h}");
    }
    // forty that wait with no code, chosen by their rows: their files and no
    // others are coded anyway, and nothing is queued
    let chosen: Vec<i64> = (0..IDS)
        .zip(&rows)
        .filter(|&(i, _)| !released(i) && !anyway(i))
        .take(40)
        .map(|(_, r)| *r)
        .collect();
    let (status, answer) = served.call(
        "POST",
        "/api/linkage/held/code",
        Some(json!({"place": "ward", "ids": chosen, "run": false})),
        OPS,
    );
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["files"], 40 * EACH, "{answer}");
    assert_eq!(answer["job"], Value::Null, "{answer}");
    let doc = served.get("/api/linkage/held/ids?place=ward", OPS);
    let generated: Vec<i64> = doc["ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|h| h["state"] == "generated")
        .map(|h| h["id"].as_i64().unwrap())
        .collect();
    let before = (0..IDS).filter(|&i| anyway(i)).count();
    assert_eq!(generated.len(), before + 40, "{doc}");
    assert!(chosen.iter().all(|c| generated.contains(c)));
    // the review of Wave 7a's merge (2026-10-10): a run queues the
    // pseudonymiser, which needs detail sensitive as at the jobs door, so
    // Data work at plain is refused before anything is marked; marking
    // alone stays open to it
    let more: Vec<i64> = (0..IDS)
        .zip(&rows)
        .filter(|&(i, _)| !released(i) && !anyway(i))
        .skip(40)
        .take(5)
        .map(|(_, r)| *r)
        .collect();
    let (status, refused) = served.call(
        "POST",
        "/api/linkage/held/code",
        Some(json!({"place": "ward", "ids": more, "run": true})),
        WORKS,
    );
    assert_eq!(status, 403, "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("sensitive"),
        "{refused}"
    );
    let doc = served.get("/api/linkage/held/ids?place=ward", OPS);
    let after = doc["ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|h| h["state"] == "generated")
        .count();
    assert_eq!(
        after,
        before + 40,
        "nothing marked by the refused run: {doc}"
    );
    let (status, marked) = served.call(
        "POST",
        "/api/linkage/held/code",
        Some(json!({"place": "ward", "ids": more, "run": false})),
        WORKS,
    );
    assert_eq!(status, 200, "{marked}");
    assert_eq!(marked["job"], Value::Null, "{marked}");
}

#[test]
fn a_dataset_s_held_ids_group_one_row_an_identifier() {
    held_ids(None);
}

#[test]
fn a_dataset_s_held_ids_group_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    let schema = "nils_held_ids";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    held_ids(Some((dsn.clone(), schema.to_string())));
    drop();
}

/// One synthetic MR file, a series of its own, where a digest of the
/// dataset reads it.
fn scan_file(tree: &TempDir, patient: &str, study: &str, series: &str, date: &str, what: &str) {
    let sop = format!("{series}.1");
    let mut e = synth::minimal_mr(study, series, &sop);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
    e.push(synth::text(tags::STUDY_DATE, VR::DA, date));
    e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, what));
    tree.file(
        &format!("derivatives/dcm-anon/{study}/{sop}"),
        &synth::part10(&MetaFields::mr(&sop), &e, true),
    );
}

/// Record 55 H3 and record 56 (2026-10-09): "need a look" means one thing
/// wherever it is shown, the sort's own questions that wait for a person.
/// A registry is sorted, what the sort asked is let go, and a question of
/// every asker is planted: the sort's (grouped, about one stack, staged,
/// a broken constraint, System 1's, a person's decision the rules disagree
/// with), a model's (its group about the body part, its disagreement with
/// a person's decision), a pass's (a vote, a session answer), a pick
/// border, an identity question, and questions nobody waits on any more.
/// The card (the sources door's totals and its digest's line), the
/// summary, the scans door's marks and the Grid count the same scans, by
/// the same kinds; the body part's and the post-contrast's questions are
/// their steps' to look at, the passes' the sorted step's `passes`, the
/// border the main scans', and none of them a look. The same over a
/// cohort of both subjects.
fn looks(pg: Option<(String, String)>) {
    use std::collections::BTreeMap;

    let home = Home {
        dir: TempDir::new("data-page-looks"),
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
    // two subjects, two visits each, two scans a visit
    let tree = TempDir::new("data-page-looks-ds");
    const FILES: &str = "\
S-0001|1.2.9.A|1.2.9.A.1|20260102|t1 mprage
S-0001|1.2.9.A|1.2.9.A.2|20260102|flair
S-0001|1.2.9.B|1.2.9.B.1|20260305|pd tse
S-0001|1.2.9.B|1.2.9.B.2|20260305|swi
S-0002|1.2.9.C|1.2.9.C.1|20260407|dwi
S-0002|1.2.9.C|1.2.9.C.2|20260407|t2 tse
S-0002|1.2.9.D|1.2.9.D.1|20260510|t1 post
S-0002|1.2.9.D|1.2.9.D.2|20260510|sc t2";
    for line in FILES.lines() {
        let f: Vec<&str> = line.split('|').collect();
        scan_file(&tree, f[0], f[1], f[2], f[3], f[4]);
    }
    home.ok(&[
        "place",
        "add",
        "looks",
        tree.path().to_str().unwrap(),
        "--role",
        "source",
        "--patient-id",
        "id-type:patient-id",
        "--subjects",
        "map",
    ]);
    let map = home.dir.file(
        "map.csv",
        b"PatientID,subject_code\nS-0001,looks-0001\nS-0002,looks-0002\n",
    );
    home.ok(&[
        "linkage",
        "import",
        map.to_str().unwrap(),
        "--id-column",
        "PatientID",
        "--code-column",
        "subject_code",
    ]);
    home.ok(&["digest", "--name", "looks", "--no-private", "@looks"]);
    home.ok(&["fingerprint"]);
    home.ok(&["classify", "--pack-dir", &packs()]);

    // the registry's ids, and the questions planted
    let (st, subjects) = {
        let mut store = home.store();
        let [stack, fp, subject] =
            ["stack", "stack_fingerprint", "subject"].map(|t| store.qualified(t));
        let stacks: BTreeMap<String, i64> = store
            .query(
                &format!(
                    "SELECT st.id, f.text_series_description FROM {stack} st \
                     JOIN {fp} f ON f.stack_id = st.id"
                ),
                &[],
            )
            .unwrap()
            .iter()
            .map(|r| (r.text(1).unwrap().to_string(), r.int(0).unwrap()))
            .collect();
        let subjects: BTreeMap<String, i64> = store
            .query(&format!("SELECT code, id FROM {subject}"), &[])
            .unwrap()
            .iter()
            .map(|r| (r.text(0).unwrap().to_string(), r.int(1).unwrap()))
            .collect();
        (stacks, subjects)
    };
    assert_eq!(st.len(), 8, "{st:?}");
    let s = |what: &str| st[what];
    let (one, two) = (subjects["looks-0001"], subjects["looks-0002"]);
    {
        let mut store = home.store();
        let [item, member, batch, cohort, cohort_member] = [
            "review_item",
            "review_member",
            "ingest_batch",
            "cohort",
            "cohort_member",
        ]
        .map(|t| store.qualified(t));
        // what the sort asked is let go, so every question below is planted
        store
            .execute(
                &format!(
                    "UPDATE {item} SET status = 'superseded' WHERE status IN ('open', 'staged')"
                ),
                &[],
            )
            .unwrap();
        let first_batch = store
            .query(&format!("SELECT MIN(id) FROM {batch}"), &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        let mut plant = |kind: &str,
                         scope: &str,
                         reference: String,
                         evidence: &str,
                         status: &str,
                         members: &[i64]| {
            store
                .execute(
                    &format!(
                        "INSERT INTO {item} (kind, scope, ref, evidence, status, created_at) \
                         VALUES ('{kind}', '{scope}', '{reference}', '{evidence}', '{status}', \
                         '2026-10-09T10:00:00Z')"
                    ),
                    &[],
                )
                .unwrap();
            let id = store
                .query(&format!("SELECT MAX(id) FROM {item}"), &[])
                .unwrap()[0]
                .int(0)
                .unwrap();
            for m in members {
                store
                    .execute(
                        &format!("INSERT INTO {member} (item_id, stack_id) VALUES ({id}, {m})"),
                        &[],
                    )
                    .unwrap();
            }
        };
        let group = |key: &str| format!("{{\"group\": \"{key}\"}}");
        let on = |stack: i64| format!("{{\"stack_id\": {stack}}}");
        // the sort's: a look
        plant(
            "base:missing",
            "group",
            group("base:missing||missing"),
            "{}",
            "open",
            &[s("t1 mprage"), s("flair")],
        );
        plant("base:missing", "stack", on(s("flair")), "{}", "open", &[]);
        plant(
            "classify.excluded",
            "group",
            group("classify.excluded|x|constraint"),
            "{}",
            "open",
            &[s("t1 mprage")],
        );
        plant("classify.asked", "stack", on(s("dwi")), "{}", "staged", &[]);
        plant(
            "base:decision",
            "stack",
            on(s("t2 tse")),
            "{\"axis\": \"base\", \"rule\": \"T2w\", \"decision\": \"PDw\"}",
            "open",
            &[],
        );
        // a model's: its disagreement with a person's decision, and its
        // body part, one group it was unsure of and one staged
        plant(
            "base:decision",
            "stack",
            on(s("pd tse")),
            "{\"axis\": \"base\", \"source\": \"model\", \"model_id\": 1}",
            "open",
            &[],
        );
        plant(
            "body_part:model",
            "group",
            group("body_part:model|run:1|below"),
            "{}",
            "open",
            &[s("pd tse"), s("swi")],
        );
        plant(
            "body_part:model",
            "group",
            group("body_part:model|run:1|p>=0.9"),
            "{}",
            "staged",
            &[s("t1 post")],
        );
        // what an older sort asked about the post-contrast
        plant(
            "post_contrast:missing",
            "stack",
            on(s("swi")),
            "{}",
            "open",
            &[],
        );
        // the passes'
        plant(
            "technique:session",
            "stack",
            on(s("t1 post")),
            "{}",
            "open",
            &[],
        );
        plant(
            "base:vote",
            "group",
            group("base:vote|x|vote"),
            "{}",
            "open",
            &[s("t1 post"), s("sc t2")],
        );
        // a pick border, an identity question, and nobody's any more
        plant(
            "pick.border",
            "subject",
            format!(
                "{{\"subject_id\": {one}, \"session_day\": \"2026-01-02\", \"role\": \"t1w\", \"model\": \"main\"}}"
            ),
            "{\"borders\": [\"too_close\"]}",
            "open",
            &[],
        );
        plant(
            "identity.unmapped",
            "batch",
            format!("{{\"batch_id\": {first_batch}}}"),
            "{}",
            "open",
            &[],
        );
        plant(
            "base:missing",
            "group",
            group("base:missing|old|missing"),
            "{}",
            "accepted",
            &[s("sc t2")],
        );
        plant(
            "base:missing",
            "stack",
            on(s("sc t2")),
            "{}",
            "superseded",
            &[],
        );
        plant(
            "split:one_image_per_stack",
            "stack",
            on(s("sc t2")),
            "{}",
            "open",
            &[],
        );
        // a cohort of both
        store
            .execute(
                &format!(
                    "INSERT INTO {cohort} (name, owner, created_at) \
                     VALUES ('both', 'anna', '2026-10-09T10:00:00Z')"
                ),
                &[],
            )
            .unwrap();
        let both = store
            .query(&format!("SELECT id FROM {cohort} WHERE name = 'both'"), &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        for subject in [one, two] {
            store
                .execute(
                    &format!(
                        "INSERT INTO {cohort_member} (cohort_id, subject_id, joined_at, source) \
                         VALUES ({both}, {subject}, '2026-10-09T10:00:00Z', 'manual')"
                    ),
                    &[],
                )
                .unwrap();
        }
    }

    // what needs a look, by the planting: the sort's questions alone
    let marks: BTreeMap<i64, Vec<&str>> = [
        (s("t1 mprage"), vec!["base:missing", "classify.excluded"]),
        (s("flair"), vec!["base:missing"]),
        (s("dwi"), vec!["classify.asked"]),
        (s("t2 tse"), vec!["base:decision"]),
    ]
    .into_iter()
    .collect();
    let kinds = json!({
        "base:decision": 1, "base:missing": 2, "classify.asked": 1, "classify.excluded": 1,
    });
    let marked = |page: &Value| -> BTreeMap<i64, Vec<String>> {
        page["scans"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|x| !x["questions"].as_array().unwrap().is_empty())
            .map(|x| {
                (
                    x["stack"].as_i64().unwrap(),
                    x["questions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|q| q.as_str().unwrap().to_string())
                        .collect(),
                )
            })
            .collect()
    };
    let tally = |marks: &BTreeMap<i64, Vec<String>>| -> Value {
        let mut out: BTreeMap<String, i64> = BTreeMap::new();
        for k in marks.values().flatten() {
            *out.entry(k.clone()).or_insert(0) += 1;
        }
        json!(out)
    };
    let expected: BTreeMap<i64, Vec<String>> = marks
        .iter()
        .map(|(k, v)| (*k, v.iter().map(|x| x.to_string()).collect()))
        .collect();

    let server = Worked::serve(&home, false);
    // the card: the sources door's totals, and its digest's line
    let sources = server.get("/api/sources", READS);
    let ds = sources["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "looks")
        .cloned()
        .unwrap_or_else(|| panic!("{sources}"));
    let totals = &ds["totals"];
    assert_eq!(totals["stacks"], 8, "{totals}");
    assert_eq!(totals["to_sort"], 4, "{totals}");
    assert_eq!(totals["sure"], 4, "{totals}");
    assert_eq!(totals["unsorted"], 0, "{totals}");
    assert_eq!(totals["need_a_look"], kinds, "{totals}");
    let digests: i64 = ds["digests"]["recent"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["to_sort"].as_i64().unwrap())
        .sum();
    assert_eq!(digests, 4, "{ds}");

    // the summary, step by step
    let summary = server.get("/api/datasets/looks/summary", READS);
    assert_eq!(summary["need_a_look"], totals["to_sort"], "{summary}");
    assert_eq!(summary["sure"], totals["sure"], "{summary}");
    assert_eq!(summary["unsorted"], totals["unsorted"], "{summary}");
    assert_eq!(summary["look_kinds"], kinds, "{summary}");
    let sorted = step(&summary, "sorted");
    assert_eq!(sorted["look"], 4, "{summary}");
    assert_eq!(
        sorted["passes"], 2,
        "the vote's and the session's: {summary}"
    );
    let body = step(&summary, "body_part");
    assert_eq!(body["look"], 2, "what the model was unsure of: {summary}");
    assert_eq!(body["answered"], 1, "{summary}");
    assert_eq!(step(&summary, "post_contrast")["look"], 1, "{summary}");
    assert_eq!(step(&summary, "main_scans")["borders"], 1, "{summary}");

    // the scans door's marks, with pictures asked for and without
    for path in [
        "/api/datasets/looks/scans?limit=200",
        "/api/datasets/looks/scans?limit=200&pictures=1",
    ] {
        let page = server.get(path, READS);
        assert_eq!(page["total"], 8, "{page}");
        let got = marked(&page);
        assert_eq!(got, expected, "{path}: {page}");
        assert_eq!(tally(&got), kinds, "{path}");
    }

    // the Grid: each subject, each of its visits, and the filter
    let grid = server.get("/api/datasets/looks/subjects", READS);
    assert_eq!(grid["totals"]["look"], 4, "{grid}");
    let mut seen = 0;
    for subject in grid["subjects"].as_array().unwrap() {
        let id = subject["id"].as_i64().unwrap();
        assert_eq!(subject["look"], 2, "{grid}");
        let visits = server.get(&format!("/api/datasets/looks/subjects/{id}/visits"), READS);
        assert_eq!(visits["totals"]["look"], subject["look"], "{visits}");
        let by_visit: i64 = visits["visits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["look"].as_i64().unwrap())
            .sum();
        assert_eq!(by_visit, 2, "{visits}");
        seen += subject["look"].as_i64().unwrap();
    }
    assert_eq!(seen, 4, "{grid}");
    let filtered = server.get("/api/datasets/looks/subjects?filter=look", READS);
    assert_eq!(filtered["matched"], 2, "{filtered}");

    // the same over the cohort of both
    let cohort = server.get("/api/cohorts/both", READS);
    let sorted = cohort["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["step"] == "sorted")
        .cloned()
        .unwrap_or_else(|| panic!("{cohort}"));
    assert_eq!(sorted["look"], 4, "{cohort}");
    assert_eq!(sorted["passes"], 2, "{cohort}");
    let members = server.get("/api/cohorts/both/subjects", READS);
    assert_eq!(members["totals"]["look"], 4, "{members}");
    let page = server.get("/api/cohorts/both/scans?limit=200", READS);
    assert_eq!(marked(&page), expected, "{page}");
    drop(server);
    drop(tree);
}

#[test]
fn a_look_is_counted_the_same_on_the_card_the_summary_the_scans_and_the_grid() {
    looks(None);
}

#[test]
fn a_look_is_counted_the_same_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    let schema = "nils_data_page_looks";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    looks(Some((dsn.clone(), schema.to_string())));
    drop();
}

/// 2026-10-10, found trying the desk: a folder added again after its
/// dataset was removed showed the removed dataset's failed run as its own,
/// a "Stopped" with the other dataset's words. A dataset's jobs are its own:
/// none made for another dataset, and where its folder held a dataset
/// before, none from before it was added.
#[test]
fn a_folder_added_again_shows_none_of_the_removed_dataset_s_runs() {
    let home = Home {
        dir: TempDir::new("data-page-again-home"),
        pg: None,
    };
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a data page test key\n"));
    assert!(good, "{err}");
    home.ok(&["init", "--key", "k"]);
    let dir = TempDir::new("data-page-again");
    std::fs::create_dir_all(dir.path().join("derivatives/dcm-anon")).unwrap();
    let folder = dir.path().to_str().unwrap();
    let declare = |name: &str| {
        home.ok(&[
            "place",
            "add",
            name,
            folder,
            "--role",
            "source",
            "--patient-id",
            "id-type:patient-id",
            "--subjects",
            "generated",
        ]);
    };
    declare("first");

    // a run of the first dataset that fails: a read of a folder in its tree that is not there
    let server = Worked::start(&home);
    let (status, queued) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["digest", "@first/not-there", "--name", "first-2026-10-10"]})),
        OPS,
    );
    assert_eq!(status, 202, "{queued}");
    let jobs = server.settled("first", &["digest"]);
    assert!(
        jobs.iter().any(|j| j["state"] == "failed"),
        "the first dataset's read failed: {jobs:?}"
    );
    let before = server.get("/api/datasets/first/summary", OPS);
    assert_eq!(step(&before, "read")["state"], "failed", "{before}");

    // the first dataset removed as the desk removes it, the folder added
    // again as another, a moment later
    let places = server.get("/api/places", OPS);
    let id = places["places"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "first")
        .and_then(|p| p["id"].as_i64())
        .unwrap_or_else(|| panic!("no place first: {places}"));
    let (status, retired) = server.call(
        "PUT",
        &format!("/api/places/{id}"),
        Some(json!({"retired": true})),
        OPS,
    );
    assert_eq!(status, 200, "{retired}");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    declare("again");
    let mine = server.get("/api/jobs?dataset=again&all=1", OPS);
    assert_eq!(
        mine["count"], 0,
        "the removed dataset's runs are not the new one's: {mine}"
    );
    let s = server.get("/api/datasets/again/summary", OPS);
    for (name, st) in steps(&s) {
        assert_ne!(
            st["state"], "failed",
            "{name} says another dataset's run failed: {s}"
        );
    }
}

/// 2026-10-10, found trying the desk: a removed dataset kept its name, so
/// its folder added again came back under the root's name before it. A
/// removed dataset's name is free for a new one, and the removed row keeps
/// its id, its folder and its history under its name and id.
#[test]
fn a_removed_dataset_s_name_is_free_for_its_folder_again() {
    let home = Home {
        dir: TempDir::new("data-page-name-home"),
        pg: None,
    };
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a data page test key\n"));
    assert!(good, "{err}");
    home.ok(&["init", "--key", "k"]);
    let root = TempDir::new("data-page-name-root");
    std::fs::create_dir_all(root.path().join("fresh/derivatives/dcm-anon")).unwrap();
    std::fs::create_dir_all(root.path().join("other/derivatives/dcm-anon")).unwrap();
    home.ok(&[
        "place",
        "add",
        "data-test",
        root.path().to_str().unwrap(),
        "--role",
        "source",
    ]);
    let server = Worked::serve(&home, false);
    let add = || {
        server.call(
            "POST",
            "/api/places",
            Some(json!({
                "role": "source", "root": "data-test", "folder": "fresh",
                "patient_id": "id-type:patient-id", "subjects": "generated",
            })),
            OPS,
        )
    };
    let (status, first) = add();
    assert_eq!(status, 201, "{first}");
    assert_eq!(first["name"], "fresh", "{first}");
    let id = first["id"].as_i64().unwrap();
    let (status, retired) = server.call(
        "PUT",
        &format!("/api/places/{id}"),
        Some(json!({"retired": true})),
        OPS,
    );
    assert_eq!(status, 200, "{retired}");

    // added again: its own name, not the root's before it
    let (status, again) = add();
    assert_eq!(status, 201, "{again}");
    assert_eq!(again["name"], "fresh", "{again}");
    assert_ne!(again["id"], first["id"], "{again}");
    let places = server.get("/api/places", OPS);
    let old = places["places"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == first["id"])
        .unwrap_or_else(|| panic!("the removed dataset is kept: {places}"))
        .clone();
    assert_eq!(old["name"], format!("fresh.retired-{id}"), "{old}");
    assert_eq!(old["path"], first["path"], "{old}");
    assert!(!old["retired_at"].is_null(), "{old}");

    // a name in force stays taken, for another folder too
    let (status, taken) = server.call(
        "POST",
        "/api/places",
        Some(json!({"role": "source", "root": "data-test", "folder": "other", "name": "fresh"})),
        OPS,
    );
    assert_eq!(status, 409, "{taken}");
}

/// Record 55 (Nima's duplicate policy, 2026-10-10): a dataset holds every
/// scan its tree has a file of, whoever read it first. A second dataset
/// holding copies of one subject's scans (one of them twice) and a file
/// whose instance UID the registry holds under the other subject counts and
/// lists the copied scans as its own, on the card, in the summary, at the
/// scans door and in the Grid; its read says how many files were copies of
/// what another dataset read, how many it holds twice, and how many are
/// held for the question it raises; and the first dataset is as it was.
fn copies(pg: Option<(String, String)>) {
    let home = Home {
        dir: TempDir::new("data-page-copies"),
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
    let map = home.dir.file(
        "map.csv",
        b"PatientID,subject_code\nS-0001,copies-0001\nS-0002,copies-0002\n",
    );
    home.ok(&[
        "linkage",
        "import",
        map.to_str().unwrap(),
        "--id-column",
        "PatientID",
        "--code-column",
        "subject_code",
    ]);
    let add = |name: &str, tree: &TempDir| {
        home.ok(&[
            "place",
            "add",
            name,
            tree.path().to_str().unwrap(),
            "--role",
            "source",
            "--patient-id",
            "id-type:patient-id",
            "--subjects",
            "map",
        ]);
        home.ok(&[
            "digest",
            "--name",
            name,
            "--no-private",
            &format!("@{name}"),
        ]);
    };

    // the first dataset: two subjects, two scans each
    let first = TempDir::new("data-page-copies-first");
    const FILES: &str = "\
S-0001|1.2.9.A|1.2.9.A.1|20260102|t1 mprage
S-0001|1.2.9.A|1.2.9.A.2|20260102|flair
S-0002|1.2.9.C|1.2.9.C.1|20260407|dwi
S-0002|1.2.9.C|1.2.9.C.2|20260407|t2 tse";
    for line in FILES.lines() {
        let f: Vec<&str> = line.split('|').collect();
        scan_file(&first, f[0], f[1], f[2], f[3], f[4]);
    }
    add("first", &first);

    // the second: the first subject's two scans, one of them twice, and a
    // file of the second subject's scan that names the first subject
    let second = TempDir::new("data-page-copies-second");
    let copy = |from: &str, to: &str| {
        let bytes = std::fs::read(first.path().join(from)).unwrap();
        second.file(to, &bytes);
    };
    copy(
        "derivatives/dcm-anon/1.2.9.A/1.2.9.A.1.1",
        "derivatives/dcm-anon/a/one.dcm",
    );
    copy(
        "derivatives/dcm-anon/1.2.9.A/1.2.9.A.1.1",
        "derivatives/dcm-anon/a/one-again.dcm",
    );
    copy(
        "derivatives/dcm-anon/1.2.9.A/1.2.9.A.2.1",
        "derivatives/dcm-anon/a/two.dcm",
    );
    scan_file(&second, "S-0001", "1.2.9.C", "1.2.9.C.1", "20260407", "dwi");
    // and a file of the first subject's first scan, its instance UID, filed
    // under another series of the same subject: no merge answers that, a
    // person lets it go (the duplicate policy's defaults, 2026-10-10)
    {
        let sop = "1.2.9.A.1.1";
        let mut e = synth::minimal_mr("1.2.9.A", "1.2.9.A.7", sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, "S-0001"));
        e.push(synth::text(tags::STUDY_DATE, VR::DA, "20260102"));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
        second.file(
            "derivatives/dcm-anon/a/other-series.dcm",
            &synth::part10(&MetaFields::mr(sop), &e, true),
        );
    }
    add("second", &second);

    let server = Worked::serve(&home, false);
    let sources = server.get("/api/sources", OPS);
    let card = |name: &str| -> Value {
        sources["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no source {name}: {sources}"))
    };
    let (one, two) = (card("first"), card("second"));
    assert_eq!(one["totals"]["stacks"], 4, "{one}");
    assert_eq!(one["totals"]["subjects"], 2, "{one}");
    assert_eq!(two["totals"]["stacks"], 2, "the copied scans: {two}");
    assert_eq!(two["totals"]["subjects"], 1, "{two}");
    assert_eq!(two["totals"]["studies"], 1, "{two}");
    assert_eq!(two["totals"]["refused_files"], 0, "{two}");
    assert_eq!(two["totals"]["same_instance_files"], 2, "{two}");
    let files = &two["digests"]["recent"][0]["files"];
    assert_eq!(
        (
            files["new"].as_i64(),
            files["known"].as_i64(),
            files["twice"].as_i64(),
            files["same_instance"].as_i64()
        ),
        (Some(0), Some(2), Some(1), Some(2)),
        "{two}"
    );

    let summary = server.get("/api/datasets/second/summary", READS);
    assert_eq!(summary["scans"], 2, "{summary}");
    assert_eq!(summary["subjects"], 1, "{summary}");
    let read = step(&summary, "read");
    assert_eq!(
        (
            read["new"].as_i64(),
            read["known"].as_i64(),
            read["twice"].as_i64(),
            read["same_instance"].as_i64(),
            read["refused"].as_i64()
        ),
        (Some(0), Some(2), Some(1), Some(2), Some(0)),
        "{summary}"
    );
    assert_eq!(summary["files"]["known"], 2, "{summary}");
    assert_eq!(summary["identity_questions"], 2, "{summary}");
    assert_eq!(
        (
            summary["files"]["left_out"].as_i64(),
            summary["files"]["gone"].as_i64()
        ),
        (Some(0), Some(0)),
        "{summary}"
    );
    let page = server.get("/api/datasets/second/scans?limit=200", READS);
    assert_eq!(page["total"], 2, "{page}");
    let grid = server.get("/api/datasets/second/subjects", READS);
    assert_eq!(grid["subjects"].as_array().unwrap().len(), 1, "{grid}");
    // the first dataset is as it was
    let summary = server.get("/api/datasets/first/summary", READS);
    assert_eq!(summary["scans"], 4, "{summary}");
    assert_eq!(step(&summary, "read")["new"], 4, "{summary}");

    // one question about the file the registry holds under another subject
    let mut store = home.store();
    let item = store.qualified("review_item");
    let open = store
        .query(
            &format!(
                "SELECT COUNT(*) FROM {item} WHERE kind = 'identity.same_instance' AND status = 'open'"
            ),
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(open, 2);

    // the dataset's Review lists both, as its summary counts them
    let listed = server.get("/api/review?dataset=second&status=open", OPS);
    let same: Vec<&Value> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == "identity.same_instance")
        .collect();
    assert_eq!(same.len(), 2, "{listed}");
    let by_kind = server.get("/api/review/summary?dataset=second", OPS);
    assert_eq!(by_kind["by_kind"]["identity.same_instance"], 2, "{by_kind}");
    // a scan under another subject is a merge's, not a let-go's
    let of = |differs: &str| -> i64 {
        same.iter()
            .find(|i| i["evidence"]["differs"][differs].is_i64())
            .and_then(|i| i["id"].as_i64())
            .unwrap_or_else(|| panic!("no item differing by {differs}: {listed}"))
    };
    let (by_subject, by_series) = (of("subject"), of("series"));
    let (status, refused) = server.call(
        "POST",
        &format!("/api/review/{by_subject}/let-go"),
        Some(serde_json::json!({"keep": true, "why": "the same scan"})),
        OPS,
    );
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused["error"].as_str().unwrap_or("").contains("merge"),
        "{refused}"
    );
    let (status, _) = server.call(
        "POST",
        &format!("/api/review/{by_series}/let-go"),
        Some(serde_json::json!({"keep": true})),
        OPS,
    );
    assert_eq!(status, 400, "a let-go says why");
    let (status, done) = server.call(
        "POST",
        &format!("/api/review/{by_series}/let-go"),
        Some(serde_json::json!({"keep": true, "why": "a resend of the same scan"})),
        OPS,
    );
    assert_eq!(status, 200, "{done}");
    assert_eq!(
        (done["files"].as_i64(), done["keep"].as_bool()),
        (Some(1), Some(true)),
        "{done}"
    );
    // the next read files it as a copy; the other question stays
    let before = step(&server.get("/api/datasets/second/summary", READS), "read");
    drop(server);
    home.ok(&["digest", "--name", "second", "--no-private", "@second"]);
    let server = Worked::serve(&home, false);
    let summary = server.get("/api/datasets/second/summary", READS);
    let read = step(&summary, "read");
    assert_eq!(read["same_instance"], 1, "{summary}");
    assert_eq!(
        read["twice"].as_i64().unwrap() + read["known"].as_i64().unwrap(),
        before["twice"].as_i64().unwrap() + before["known"].as_i64().unwrap() + 1,
        "{summary}"
    );
    assert_eq!(summary["identity_questions"], 1, "{summary}");
    let mut store = home.store();
    let audit = store.qualified("audit");
    let acts = store
        .query(
            &format!("SELECT COUNT(*) FROM {audit} WHERE action = 'review.let_go'"),
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(acts, 1);
    drop(server);
}

#[test]
fn a_dataset_holds_the_scans_its_tree_has_a_file_of() {
    copies(None);
}

#[test]
fn a_dataset_holds_the_scans_its_tree_has_a_file_of_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    let schema = "nils_data_page_copies";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    copies(Some((dsn.clone(), schema.to_string())));
    drop();
}
