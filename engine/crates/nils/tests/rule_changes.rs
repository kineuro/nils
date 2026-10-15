// SPDX-License-Identifier: AGPL-3.0-only
//! Record 56, sections 6.4 and 6.5: the Rules page's doors, and adopting a
//! proposed rule change. Two datasets, `alpha` and `beta`, each of two
//! subjects with a T1 and a series named `zzqx`, which no rule reads, and
//! in `beta` a third named `qqtest`. Over them:
//!
//! - the pack's quality report, whole and for one dataset;
//! - a scoped change (`zzqx` as a word of modifier MT for `dataset:alpha`)
//!   proposed, read, listed, counted on the dataset's summary, refused to a
//!   reviewer and to an agent, adopted by an operator, and its scope
//!   re-sorted: only alpha's stacks are judged, by counting, and its
//!   proposal says what moved; a full sort after it keeps the change on
//!   alpha and never on beta (an overlay never leaves its scope);
//! - a pack edit (`qqtest` for every site) adopted as the pack's next
//!   version, the version before it kept, recorded as a rules version to
//!   release, and the registry re-sorted by it; a proposal rehearsed before
//!   it is stale until rehearsed again; a rejection; the jobs door's
//!   `--scope` and its refusal of `--proposal`.
//!
//! On SQLite always, and on Postgres where a test DSN is set. Every scan is
//! invented.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_registry::Store;
use serde_json::{Value, json};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn repo_packs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let at = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &at);
        } else {
            std::fs::copy(e.path(), &at).unwrap();
        }
    }
}

struct Home {
    dir: TempDir,
    pg: Option<(String, String)>,
    /// A copy of the repository's packs: adopting a pack edit writes here.
    packs: PathBuf,
    _packs_root: TempDir,
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
            .env_remove("NILS_PACK_DIR")
            .env_remove("NILS_JOB_ID")
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

    fn json(&self, args: &[&str]) -> Value {
        let out = self.ok(args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }

    fn store(&self) -> Store {
        match &self.pg {
            Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
            None => Store::open_sqlite(&self.dir.path().join("registry.db")).unwrap(),
        }
    }

    fn packs(&self) -> &str {
        self.packs.to_str().unwrap()
    }
}

const OPS: &str = "an-operator-token-of-length";
const REV: &str = "a-reviewer-token-of-its-length";
const READS: &str = "a-reader-token-of-its-length";

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
    fn start(home: &Home) -> Served {
        use std::io::BufRead as _;
        let tokens = [
            format!("{OPS}=ops@lab:operator"),
            format!("{REV}=rev@lab:reviewer"),
            format!("{READS}=lou@lab:reader"),
        ]
        .join(",");
        let child = nils()
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
            .args(["--auth", "token", "--pack-dir", home.packs()])
            .env("NILS_TOKENS", tokens)
            .env("NILS_PACK_DIR", home.packs())
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

    fn call_with(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        token: &str,
        extra: &str,
    ) -> (u16, Value) {
        use std::io::{Read as _, Write as _};
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: application/json\r\nAuthorization: Bearer {token}\r\n{extra}\r\n",
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

    fn call(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> (u16, Value) {
        self.call_with(method, path, body, token, "")
    }

    fn get(&self, path: &str, token: &str) -> Value {
        let (status, doc) = self.call("GET", path, None, token);
        assert_eq!(status, 200, "{path}: {doc}");
        doc
    }

    /// A job once it is over, three minutes at most.
    fn over(&self, job: i64) -> Value {
        for _ in 0..1800 {
            let j = self.get(&format!("/api/jobs/{job}"), OPS);
            if matches!(j["state"].as_str(), Some("done" | "failed" | "cancelled")) {
                return j;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("job {job} did not end");
    }

    /// Every queued and running job over, three minutes at most.
    fn quiet(&self) {
        for _ in 0..1800 {
            let open = self.get("/api/jobs", OPS);
            if open["count"] == 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("the jobs did not settle");
    }
}

/// A dataset of invented scans: per subject, one series per description.
fn dataset(home: &Home, name: &str, n: usize, subjects: &[&str], descriptions: &[&str]) -> TempDir {
    let dir = TempDir::new(&format!("rules-{name}"));
    for (p, patient) in subjects.iter().enumerate() {
        let study = format!("1.2.826.0.1.3680043.8.499.{n}{}.1", p + 1);
        for (s, description) in descriptions.iter().enumerate() {
            let series = format!("{study}.{}", s + 1);
            for instance in 1..=2u32 {
                let sop = format!("{series}.{instance}");
                let mut e = synth::minimal_mr(&study, &series, &sop);
                e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
                e.push(synth::text(tags::STUDY_DATE, VR::DA, "20240131"));
                e.push(synth::text(
                    tags::SERIES_NUMBER,
                    VR::IS,
                    &(s + 1).to_string(),
                ));
                e.push(synth::text(
                    tags::INSTANCE_NUMBER,
                    VR::IS,
                    &instance.to_string(),
                ));
                e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
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
                dir.file(
                    &format!("sub-{p}/s{s}/IM_{instance:04}"),
                    &synth::part10(&MetaFields::mr(&sop), &e, true),
                );
            }
        }
    }
    home.ok(&[
        "place",
        "add",
        name,
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
    ]);
    home.ok(&["digest", "--name", name, &format!("@{name}")]);
    dir
}

fn home(pg: Option<(String, String)>) -> Home {
    let root = TempDir::new("rules-packs");
    let packs = root.path().join("packs");
    copy_tree(&repo_packs(), &packs);
    let home = Home {
        dir: TempDir::new("rules-home"),
        pg,
        packs,
        _packs_root: root,
    };
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a rules page test key\n"));
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
    home
}

/// The stacks of a dataset whose series description is `named`.
fn stacks_named(store: &mut Store, named: &str) -> Vec<i64> {
    let f = store.qualified("stack_fingerprint");
    store
        .query(
            &format!(
                "SELECT stack_id FROM {f} WHERE text_series_description = '{named}' ORDER BY stack_id"
            ),
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| r.int(0).unwrap())
        .collect()
}

/// The stacks of a dataset, by the sources its tree holds.
fn stacks_of(store: &mut Store, dataset: &str) -> Vec<i64> {
    let sql = format!(
        "SELECT DISTINCT ss.stack_id FROM {} ss JOIN {} s ON s.id = ss.source_id WHERE s.root_canonical LIKE '%rules-{dataset}%' ORDER BY ss.stack_id",
        store.qualified("source_stack"),
        store.qualified("source"),
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| r.int(0).unwrap())
        .collect()
}

/// Whether a stack's stored modifier holds a value.
fn modifier_holds(store: &mut Store, stack: i64, value: &str) -> bool {
    let a = store.qualified("classification_axis");
    !store
        .query(
            &format!(
                "SELECT 1 FROM {a} WHERE stack_id = {stack} AND axis = 'modifier' AND value = '{value}'"
            ),
            &[],
        )
        .unwrap()
        .is_empty()
}

/// The job that last judged a stack, and what sorted it beside the pack.
fn judged_by(store: &mut Store, stack: i64) -> (i64, Option<String>, String) {
    let c = store.qualified("classification");
    let r = &store
        .query(
            &format!("SELECT job_id, overlay, pack_version FROM {c} WHERE stack_id = {stack}"),
            &[],
        )
        .unwrap()[0];
    (
        r.int(0).unwrap(),
        r.opt_text(1).unwrap().map(str::to_string),
        r.text(2).unwrap().to_string(),
    )
}

fn round(pg: Option<(String, String)>) {
    let home = home(pg);
    let _alpha = dataset(
        &home,
        "alpha",
        1,
        &["A-ONE", "A-TWO"],
        &["t1 mprage", "zzqx"],
    );
    let _beta = dataset(
        &home,
        "beta",
        2,
        &["B-ONE", "B-TWO"],
        &["t1 mprage", "zzqx", "qqtest"],
    );
    home.ok(&["fingerprint"]);
    home.ok(&["classify", "--pack-dir", home.packs(), "--no-previews"]);
    let mut store = home.store();
    let alpha = stacks_of(&mut store, "alpha");
    let beta = stacks_of(&mut store, "beta");
    assert_eq!((alpha.len(), beta.len()), (4, 6), "{alpha:?} {beta:?}");
    let zzqx = stacks_named(&mut store, "zzqx");
    let (alpha_z, beta_z): (Vec<i64>, Vec<i64>) = zzqx.iter().partition(|s| alpha.contains(s));
    assert_eq!((alpha_z.len(), beta_z.len()), (2, 2), "{zzqx:?}");
    for s in &zzqx {
        assert!(
            !modifier_holds(&mut store, *s, "MT"),
            "stack {s} is MT before"
        );
    }
    let first_sort = judged_by(&mut store, beta[0]).0;

    let server = Served::start(&home);

    // --- the pack's quality: three parts, whole and for one dataset
    let q = server.get("/api/packs/mri/quality", REV);
    assert_eq!(q["pack"], "mri", "{q}");
    assert_eq!(q["version"], "1.2.1", "{q}");
    assert_eq!(q["stacks"], 10, "{q}");
    assert!(q["disagreements"]["pairs"].is_array(), "{q}");
    assert!(q["overrides"]["pairs"].is_array(), "{q}");
    assert_eq!(q["system1"]["served"], false, "{q}");
    assert_eq!(q["system1"]["disagreements"], json!([]), "{q}");
    for p in q["overrides"]["pairs"].as_array().unwrap() {
        assert!(p["pair"].as_str().unwrap().contains(" > "), "{p}");
        assert!(
            p["winner"]["rule"].is_string() && p["over"]["rule"].is_string(),
            "{p}"
        );
    }
    let qa = server.get("/api/packs/mri/quality?dataset=alpha", REV);
    assert_eq!(qa["dataset"], "alpha", "{qa}");
    assert_eq!(qa["stacks"], 4, "{qa}");
    let (status, _) = server.call("GET", "/api/packs/mri/quality?dataset=nowhere", None, REV);
    assert_eq!(status, 404);
    let (status, _) = server.call("GET", "/api/packs/mri/quality?limit=0", None, REV);
    assert_eq!(status, 400);
    let (status, _) = server.call("GET", "/api/packs/mri/quality", None, READS);
    assert_eq!(status, 403, "a reader does not tune the pack");

    // --- a scoped change: the word for alpha alone
    let scoped = json!({
        "title": "zzqx is magnetisation transfer at alpha",
        "operations": [{
            "op": "add_words", "axis": "modifier", "value": "MT", "words": ["zzqx"],
            "scope": "dataset:alpha",
        }],
        "reason": "alpha writes zzqx for its MT series",
        "evidence": {"dataset": "alpha", "found": "this test"},
    });
    let (status, doc) = server.call(
        "POST",
        "/api/packs/mri/proposals",
        Some(scoped.clone()),
        READS,
    );
    assert_eq!(status, 403, "a reader proposes nothing: {doc}");
    let (status, p1) = server.call("POST", "/api/packs/mri/proposals", Some(scoped), REV);
    assert_eq!(status, 201, "{p1}");
    let id1 = p1["id"].as_i64().unwrap();
    assert_eq!(p1["kind"], "scoped", "{p1}");
    assert_eq!(p1["status"], "proposed", "{p1}");
    assert_eq!(p1["on"], "1.2.1", "{p1}");
    assert_eq!(p1["scopes"], json!(["dataset:alpha"]), "{p1}");
    assert_eq!(p1["datasets"], json!(["alpha"]), "{p1}");
    assert_eq!(p1["stale"], false, "{p1}");
    assert_eq!(p1["card"]["moved"], 2, "{p1}");
    let mt = p1["card"]["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["axis"] == "modifier")
        .unwrap_or_else(|| panic!("no modifier move: {p1}"));
    assert_eq!(mt["to"], "MT", "{p1}");
    assert_eq!(mt["stacks"], 2, "{p1}");
    assert_eq!(p1["card"]["breaks"], 0, "{p1}");
    assert!(
        p1["card"]["says"]
            .to_string()
            .contains("applies to dataset:alpha and to no other scan"),
        "{p1}"
    );
    assert_eq!(p1["ships"]["as"], "scoped change", "{p1}");
    assert_eq!(p1["report"]["scope"]["stacks"], 4, "{p1}");
    assert_eq!(p1["operations"][0]["scope"], "dataset:alpha", "{p1}");
    assert!(p1["version"].is_null() && p1["job"].is_null(), "{p1}");
    // a proposal that mixes a pack edit and a scoped change is two
    let (status, doc) = server.call(
        "POST",
        "/api/packs/mri/proposals",
        Some(json!({
            "operations": [
                {"op": "add_words", "axis": "modifier", "value": "MT", "words": ["zzqx"], "scope": "dataset:alpha"},
                {"op": "add_words", "axis": "modifier", "value": "MT", "words": ["qqtest"]},
            ],
            "reason": "r", "evidence": "e",
        })),
        REV,
    );
    assert_eq!(status, 400, "{doc}");
    assert!(
        doc["error"]
            .as_str()
            .unwrap()
            .contains("propose them as two"),
        "{doc}"
    );

    // listed, and counted on the datasets it touches
    let list = server.get("/api/packs/mri/proposals", REV);
    assert_eq!(list["count"], 1, "{list}");
    assert_eq!(list["counts"]["proposed"], 1, "{list}");
    assert!(list["proposals"][0].get("report").is_none(), "{list}");
    assert_eq!(
        server.get("/api/packs/mri/proposals?dataset=beta", REV)["count"],
        0
    );
    assert_eq!(
        server.get(
            "/api/packs/mri/proposals?dataset=alpha&status=proposed",
            REV
        )["count"],
        1
    );
    let (status, _) = server.call("GET", "/api/packs/mri/proposals?status=maybe", None, REV);
    assert_eq!(status, 400);
    assert_eq!(
        server.get("/api/datasets/alpha/summary", READS)["proposals"],
        json!({"proposed": 1, "adopted": 0, "rejected": 0})
    );
    assert_eq!(
        server.get("/api/datasets/beta/summary", READS)["proposals"]["proposed"],
        0
    );

    // a reviewer proposes and an operator adopts; an agent never decides
    let decide = format!("/api/packs/mri/proposals/{id1}/decide");
    let (status, doc) = server.call("POST", &decide, Some(json!({"decision": "adopt"})), REV);
    assert_eq!(status, 403, "{doc}");
    let (status, doc) = server.call_with(
        "POST",
        &decide,
        Some(json!({"decision": "adopt"})),
        OPS,
        "X-Nils-Actor: {\"kind\": \"agent\", \"name\": \"ask-help\"}\r\n",
    );
    assert_eq!(status, 403, "{doc}");
    let (status, doc) = server.call("POST", &decide, Some(json!({"decision": "maybe"})), OPS);
    assert_eq!(status, 400, "{doc}");
    let (status, adopted) = server.call(
        "POST",
        &decide,
        Some(json!({"decision": "adopt", "why": "alpha's habit"})),
        OPS,
    );
    assert_eq!(status, 202, "{adopted}");
    assert_eq!(adopted["status"], "adopted", "{adopted}");
    assert!(adopted["version"].is_null(), "{adopted}");
    assert_eq!(adopted["scopes"], json!(["dataset:alpha"]), "{adopted}");
    let job = adopted["job"].as_i64().unwrap();
    let (status, doc) = server.call("POST", &decide, Some(json!({"decision": "reject"})), OPS);
    assert_eq!(status, 409, "decided once: {doc}");
    let j = server.over(job);
    assert_eq!(j["state"], "done", "{j}");
    server.quiet();

    // the scoped re-sort judged alpha's stacks, and no other: by counting
    let judged: Vec<i64> = {
        let c = store.qualified("classification");
        store
            .query(
                &format!("SELECT stack_id FROM {c} WHERE job_id = {job} ORDER BY stack_id"),
                &[],
            )
            .unwrap()
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect()
    };
    assert_eq!(judged, alpha, "the scope's stacks and only those");
    for s in &beta {
        assert_eq!(
            judged_by(&mut store, *s).0,
            first_sort,
            "beta's stack {s} untouched"
        );
    }
    for s in &alpha_z {
        assert!(
            modifier_holds(&mut store, *s, "MT"),
            "alpha's zzqx {s} is MT"
        );
        let (_, overlay, _) = judged_by(&mut store, *s);
        assert_eq!(overlay, Some(format!("proposal:{id1}")), "stack {s}");
    }
    for s in &beta_z {
        assert!(
            !modifier_holds(&mut store, *s, "MT"),
            "beta's zzqx {s} is not"
        );
    }
    let p1 = server.get(&format!("/api/packs/mri/proposals/{id1}"), REV);
    assert_eq!(p1["status"], "adopted", "{p1}");
    assert_eq!(p1["decided_by"], "ops@lab", "{p1}");
    assert_eq!(p1["job"]["state"], "done", "{p1}");
    let m = &p1["applied"]["measured"];
    assert_eq!(p1["applied"]["state"], "done", "{p1}");
    assert_eq!(m["stacks"], 4, "{p1}");
    assert_eq!(m["scope"], json!(["dataset:alpha"]), "{p1}");
    assert_eq!(m["moved"]["stacks"], 2, "{p1}");
    assert_eq!(m["moved"]["axes"]["modifier"], 2, "{p1}");
    assert_eq!(p1["applied"]["expected"]["moved"]["stacks"], 2, "{p1}");
    assert_eq!(
        server.get("/api/datasets/alpha/summary", READS)["proposals"]["adopted"],
        1
    );

    // an overlay never leaves its scope: a whole sort keeps the change on
    // alpha and never puts it on beta
    let report = home.json(&[
        "classify",
        "--pack-dir",
        home.packs(),
        "--no-previews",
        "--json",
    ]);
    assert_eq!(report["scoped"][0]["proposal"], id1, "{report}");
    assert_eq!(report["scoped"][0]["stacks"], 4, "{report}");
    for s in &alpha_z {
        assert!(
            modifier_holds(&mut store, *s, "MT"),
            "alpha's zzqx {s} kept"
        );
    }
    for s in &beta_z {
        assert!(
            !modifier_holds(&mut store, *s, "MT"),
            "beta's zzqx {s} never"
        );
    }
    // a rehearsal reads the registry as it is sorted: the adopted change
    // moves nothing more
    let (status, again) = server.call(
        "POST",
        "/api/packs/mri/rehearse",
        Some(json!({"operations": [{"op": "add_words", "axis": "modifier", "value": "MT", "words": ["yyqq"], "scope": "dataset:alpha", "reason": "r", "evidence": "e"}]})),
        REV,
    );
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["moved"]["stacks"], 0, "{again}");
    assert_eq!(again["adopted"]["changes"][0]["proposal"], id1, "{again}");

    // --- a pack edit, and a proposal rehearsed before it
    let (status, stale) = server.call(
        "POST",
        "/api/packs/mri/proposals",
        Some(json!({
            "operations": [{"op": "add_words", "axis": "modifier", "value": "MT", "words": ["t1 mprage"], "scope": "dataset:beta"}],
            "reason": "beta's T1 is MT", "evidence": "this test",
        })),
        REV,
    );
    assert_eq!(status, 201, "{stale}");
    let id_stale = stale["id"].as_i64().unwrap();
    let (status, p2) = server.call(
        "POST",
        "/api/packs/mri/proposals",
        Some(json!({
            "title": "qqtest is MT everywhere",
            "operations": [{"op": "add_words", "axis": "modifier", "value": "MT", "words": ["qqtest"]}],
            "reason": "every site writes qqtest for MT", "evidence": "this test",
        })),
        REV,
    );
    assert_eq!(status, 201, "{p2}");
    let id2 = p2["id"].as_i64().unwrap();
    assert_eq!(p2["kind"], "pack", "{p2}");
    assert_eq!(p2["ships"]["as"], "rules release", "{p2}");
    assert_eq!(p2["ships"]["version"], "1.2.2", "{p2}");
    assert_eq!(p2["card"]["moved"], 2, "{p2}");
    let (status, adopted) = server.call(
        "POST",
        &format!("/api/packs/mri/proposals/{id2}/decide"),
        Some(json!({"decision": "adopt"})),
        OPS,
    );
    assert_eq!(status, 202, "{adopted}");
    assert_eq!(adopted["version"]["version"], "1.2.2", "{adopted}");
    assert_eq!(adopted["version"]["from"], "1.2.1", "{adopted}");
    assert_eq!(adopted["version"]["tag"], "pack-mri-v1.2.2", "{adopted}");
    assert_eq!(adopted["version"]["state"], "to_release", "{adopted}");
    // the version in use is the new one, and the one before it is kept
    let manifest = std::fs::read_to_string(home.packs.join("mri/pack.yml")).unwrap();
    assert!(manifest.contains("version: 1.2.2"), "{manifest}");
    let kept = home
        .packs
        .with_file_name("packs.versions")
        .join("mri/1.2.1/pack.yml");
    assert!(
        std::fs::read_to_string(&kept)
            .unwrap()
            .contains("version: 1.2.1"),
        "{}",
        kept.display()
    );
    let j = server.over(adopted["job"].as_i64().unwrap());
    assert_eq!(j["state"], "done", "{j}");
    server.quiet();
    assert_eq!(server.get("/api/packs/mri", REV)["version"], "1.2.2");
    let p2 = server.get(&format!("/api/packs/mri/proposals/{id2}"), REV);
    assert_eq!(p2["applied"]["measured"]["stacks"], 10, "{p2}");
    assert_eq!(p2["applied"]["measured"]["moved"]["stacks"], 2, "{p2}");
    assert_eq!(p2["version"]["state"], "to_release", "{p2}");
    for s in stacks_named(&mut store, "qqtest") {
        assert!(modifier_holds(&mut store, s, "MT"), "qqtest {s} is MT");
        assert_eq!(judged_by(&mut store, s).2, "1.2.2", "stack {s}");
    }
    // the scoped change applies to the new version too, and to alpha alone
    for s in &alpha_z {
        assert!(
            modifier_holds(&mut store, *s, "MT"),
            "alpha's zzqx {s} kept"
        );
    }
    for s in &beta_z {
        assert!(
            !modifier_holds(&mut store, *s, "MT"),
            "beta's zzqx {s} never"
        );
    }

    // the proposal rehearsed against 1.2.1 is stale until rehearsed again
    let p = server.get(&format!("/api/packs/mri/proposals/{id_stale}"), REV);
    assert_eq!(p["stale"], true, "{p}");
    let (status, doc) = server.call(
        "POST",
        &format!("/api/packs/mri/proposals/{id_stale}/decide"),
        Some(json!({"decision": "adopt"})),
        OPS,
    );
    assert_eq!(status, 409, "{doc}");
    assert!(
        doc["error"].as_str().unwrap().contains("rehearse it again"),
        "{doc}"
    );
    let (status, p) = server.call(
        "POST",
        &format!("/api/packs/mri/proposals/{id_stale}/rehearse"),
        Some(json!({})),
        REV,
    );
    assert_eq!(status, 200, "{p}");
    assert_eq!(p["stale"], false, "{p}");
    assert_eq!(p["on"], "1.2.2", "{p}");
    // and a rejection closes it with its why
    let (status, doc) = server.call(
        "POST",
        &format!("/api/packs/mri/proposals/{id_stale}/decide"),
        Some(json!({"decision": "reject", "why": "beta's T1 is not MT"})),
        OPS,
    );
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["status"], "rejected", "{doc}");
    let (status, _) = server.call(
        "POST",
        &format!("/api/packs/mri/proposals/{id_stale}/rehearse"),
        Some(json!({})),
        REV,
    );
    assert_eq!(status, 409, "a decided one is not rehearsed again");
    let list = server.get("/api/packs/mri/proposals", REV);
    assert_eq!(
        list["counts"],
        json!({"proposed": 0, "adopted": 2, "rejected": 1}),
        "{list}"
    );

    // --- the jobs door: a scoped sort, and never a proposal's
    let (status, doc) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["classify", "--proposal", id1.to_string()]})),
        OPS,
    );
    assert_eq!(status, 400, "{doc}");
    let (status, queued) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["classify", "--scope", "dataset:beta", "--no-previews"]})),
        OPS,
    );
    assert_eq!(status, 202, "{queued}");
    let job = queued["job"].as_i64().unwrap();
    let j = server.over(job);
    assert_eq!(j["state"], "done", "{j}");
    for s in &beta {
        assert_eq!(judged_by(&mut store, *s).0, job, "beta's {s} judged");
    }
    for s in &alpha {
        assert_ne!(judged_by(&mut store, *s).0, job, "alpha's {s} left");
    }
    // a scope that names nothing the registry holds is refused at the keyboard
    let (good, _, err) = home.run(
        &[
            "classify",
            "--pack-dir",
            home.packs(),
            "--scope",
            "dataset:nowhere",
        ],
        None,
    );
    assert!(!good && err.contains("no dataset named nowhere"), "{err}");

    // --- a word overlay adopted at the overlays door is a scoped change
    // too: it stays in its origin and every sort keeps it
    let batch_of = |store: &mut Store, stacks: &[i64]| -> i64 {
        let k = store.qualified("stack");
        let ids = stacks
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let rows = store
            .query(
                &format!("SELECT DISTINCT first_batch_id FROM {k} WHERE id IN ({ids})"),
                &[],
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        rows[0].int(0).unwrap()
    };
    let overlay = |name: &str, batch: i64| {
        json!({
            "overlay": name, "version": "1.0.0", "pack": "mri",
            "scope": {"batch": batch.to_string()},
            "lists": {"modifier.MoCo": {"add": ["zzqx"]}},
            "cases": [{"name": "the batch's word", "stack": {"text_series_description": "zzqx"},
                       "axes": {"modifier": "MoCo"}}],
        })
    };
    let beta_batch = batch_of(&mut store, &beta);
    let alpha_batch = batch_of(&mut store, &alpha);
    let (status, proposed) = server.call(
        "POST",
        "/api/overlays",
        Some(
            json!({"name": "beta words", "overlay": overlay("beta-words", beta_batch),
                    "scope": format!("batch:{beta_batch}")}),
        ),
        REV,
    );
    assert_eq!(status, 201, "{proposed}");
    let oid = proposed["overlay"]["id"].as_i64().unwrap();
    let (status, adopted) = server.call("POST", &format!("/api/overlays/{oid}/adopt"), None, OPS);
    assert_eq!(status, 202, "{adopted}");
    let kept = adopted["proposal"].as_i64().unwrap();
    assert_eq!(
        server.over(adopted["job"].as_i64().unwrap())["state"],
        "done"
    );
    server.quiet();
    let p = server.get(&format!("/api/packs/mri/proposals/{kept}"), REV);
    assert_eq!(p["kind"], "scoped", "{p}");
    assert_eq!(p["status"], "adopted", "{p}");
    assert_eq!(p["scopes"], json!([format!("batch:{beta_batch}")]), "{p}");
    assert_eq!(p["evidence"]["overlay"], oid, "{p}");
    assert_eq!(
        p["applied"]["measured"]["stacks"], 6,
        "only the batch's stacks: {p}"
    );
    let moco = |store: &mut Store, s: &[i64]| {
        s.iter()
            .filter(|x| modifier_holds(store, **x, "MoCo"))
            .count()
    };
    assert_eq!(moco(&mut store, &beta_z), 2);
    assert_eq!(moco(&mut store, &alpha_z), 0, "never outside its origin");
    home.ok(&["classify", "--pack-dir", home.packs(), "--no-previews"]);
    assert_eq!(moco(&mut store, &beta_z), 2, "a later sort keeps it");
    assert_eq!(moco(&mut store, &alpha_z), 0);

    // an overlay adopted before (the old adoption sorted the whole registry
    // under it once) becomes its scoped change as the registry migrates
    let (status, proposed) = server.call(
        "POST",
        "/api/overlays",
        Some(
            json!({"name": "alpha words", "overlay": overlay("alpha-words", alpha_batch),
                    "scope": format!("batch:{alpha_batch}")}),
        ),
        REV,
    );
    assert_eq!(status, 201, "{proposed}");
    let old = proposed["overlay"]["id"].as_i64().unwrap();
    let o = store.qualified("overlay");
    store
        .execute(
            &format!("UPDATE {o} SET status = 'adopted', decided_by = 'ops@lab' WHERE id = {old}"),
            &[],
        )
        .unwrap();
    let made = nils_registry::rule_proposal::adopt_overlays(&mut store).unwrap();
    assert_eq!(made.len(), 1, "{made:?}");
    assert!(
        nils_registry::rule_proposal::adopt_overlays(&mut store)
            .unwrap()
            .is_empty()
    );
    let migrated = made[0];
    let report = home.json(&[
        "classify",
        "--pack-dir",
        home.packs(),
        "--no-previews",
        "--json",
    ]);
    // the next sort applies it to alpha's batch alone, beside the change
    // adopted for alpha before (whose MT already holds the word there)
    assert!(report.get("scoped_skipped").is_none(), "{report}");
    let reach = report["scoped"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["proposal"] == migrated)
        .unwrap_or_else(|| panic!("{report}"));
    assert_eq!(reach["stacks"], 4, "{report}");
    for s in &alpha {
        let (_, overlay, _) = judged_by(&mut store, *s);
        assert_eq!(
            overlay,
            Some(format!("proposal:{id1},proposal:{migrated}")),
            "stack {s}"
        );
    }
    for s in &beta {
        let (_, overlay, _) = judged_by(&mut store, *s);
        assert!(
            !overlay
                .unwrap_or_default()
                .contains(&format!("proposal:{migrated}")),
            "beta's {s} is outside it"
        );
    }
    assert_eq!(moco(&mut store, &beta_z), 2);
    for s in alpha.iter().chain(&beta).filter(|s| !zzqx.contains(s)) {
        assert!(!modifier_holds(&mut store, *s, "MoCo"), "{s} has no word");
    }
    drop(server);
}

#[test]
fn rules_are_proposed_adopted_and_re_sorted_on_sqlite() {
    round(None);
}

#[test]
fn rules_are_proposed_adopted_and_re_sorted_on_postgres() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        return;
    };
    let schema = "rules_page";
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
