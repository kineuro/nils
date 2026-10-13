// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 D4, E6 and A5 (spec 10.2, 10.3, 10.5; the wave's tests T22 and
//! T23): models as parts. A label set made outside the engine is
//! registered with its provenance and the sha256 of every file, its rows
//! keyed by DICOM identity, and refused where a row is of a sample sealed
//! now; a model's card states its tested claims, its speed on a CPU and on
//! a GPU, its GPU need and its equivalence class, and names that set; the
//! admission suite measures the class of the difference between its CPU
//! and GPU answers and refuses a class worse than its card's; several
//! models answer a task, and the site chooses one, by default the one a
//! recorded certificate names.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use serde_json::{Value, json};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// Run the command line in the registry: its status, stdout and stderr.
fn cli(home: &TempDir, args: &[&str]) -> (bool, String, String) {
    let out = nils()
        .arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "cleo")
        .env("HOSTNAME", "lab")
        .env("NILS_PRINCIPAL", "cleo@lab")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn ok(home: &TempDir, args: &[&str]) -> String {
    let (good, out, err) = cli(home, args);
    assert!(good, "{}: {err}", args.join(" "));
    out
}

/// A registry of four MR stacks in four studies of two people.
fn registry() -> TempDir {
    let home = TempDir::new("parts-home");
    let dir = TempDir::new("parts-src");
    for (patient, study, sop, description) in [
        ("P1", "1.2.3.A", "1.2.3.A.1.1", "t1 mprage"),
        ("P1", "1.2.3.B", "1.2.3.B.1.1", "flair axial"),
        ("P2", "1.2.3.C", "1.2.3.C.1.1", "t1 mprage"),
        ("P2", "1.2.3.D", "1.2.3.D.1.1", "t2 spine"),
    ] {
        let mut e = synth::minimal_mr(study, &format!("{study}.1"), sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
        dir.file(
            &format!("{study}/{sop}"),
            &synth::part10(&MetaFields::mr(sop), &e, true),
        );
    }
    let mut key = nils()
        .arg("--registry")
        .arg(home.path())
        .args(["key", "add", "k"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    key.stdin
        .take()
        .unwrap()
        .write_all(b"a models parts test key\n")
        .unwrap();
    assert!(key.wait().unwrap().success());
    ok(&home, &["init", "--key", "k"]);
    ok(
        &home,
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            dir.path().to_str().unwrap(),
        ],
    );
    ok(&home, &["fingerprint"]);
    ok(
        &home,
        &["classify", "--pack-dir", packs().to_str().unwrap()],
    );
    std::mem::forget(dir);
    home
}

/// Each stack's id and its DICOM identity: study, series, stack key and a
/// SOP Instance UID of it, read from the registry itself.
fn identities(home: &TempDir) -> Vec<(i64, String, String, String, String)> {
    let mut registry = nils_registry::home::Home::new(home.path()).open().unwrap();
    let store = registry.store();
    let sql = format!(
        "SELECT k.id, st.study_instance_uid, se.series_instance_uid, k.stack_key, \
         (SELECT MIN(i.sop_instance_uid) FROM {} i WHERE i.stack_id = k.id) \
         FROM {} k JOIN {} se ON se.id = k.series_id JOIN {} st ON st.id = se.study_id ORDER BY k.id",
        store.qualified("instance"),
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("study"),
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.int(0).unwrap(),
                r.text(1).unwrap().to_string(),
                r.text(2).unwrap().to_string(),
                r.text(3).unwrap().to_string(),
                r.opt_text(4).unwrap().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

const ADMIN: &str = "admin-parts-token-of-length";
const SEER: &str = "seer-parts-token-of-length";

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn start(home: &TempDir) -> Server {
        let tokens = [
            format!("{ADMIN}=ada@lab:admin"),
            format!("{SEER}=sam@lab:models:see"),
        ]
        .join(",");
        let child = nils()
            .arg("--registry")
            .arg(home.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "1",
                "--auth",
                "token",
            ])
            .args(["--pack-dir", packs().to_str().unwrap()])
            .env("NILS_TOKENS", tokens)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut held = Server { child, port: 0 };
        let stdout = held.child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            panic!("nils serve did not listen");
        };
        let addr = first.split_whitespace().nth(2).unwrap();
        held.port = addr.rsplit(':').next().unwrap().parse().unwrap();
        held
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> (u16, Value) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nAuthorization: Bearer {token}\r\n",
            body.len()
        );
        if !body.is_empty() {
            head.push_str("Content-Type: application/json\r\n");
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let response = String::from_utf8_lossy(&response).to_string();
        let (headers, text) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status: u16 = headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let json = serde_json::from_str(text).unwrap_or(Value::String(text.to_string()));
        (status, json)
    }

    fn ok(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> Value {
        let (status, doc) = self.call(method, path, body, token);
        assert!(
            (200..300).contains(&status),
            "{method} {path}: {status} {doc}"
        );
        doc
    }
}

const HEADER: &str = "study_instance_uid\tseries_instance_uid\tstack_key\tsop_instance_uid\twhat\tvalue\tauthor_kind\tauthor";

/// A label set's register body: its labels.tsv, its provenance and the
/// sha256 of both.
fn set_body(name: &str, tsv: &str) -> Value {
    let provenance = json!({"made_by": "a study's tools", "from": "a reading round"}).to_string();
    json!({
        "name": name,
        "labels_tsv": tsv,
        "provenance_json": provenance,
        "sha256": {"labels.tsv": sha256(tsv.as_bytes()), "provenance.json": sha256(provenance.as_bytes())},
    })
}

fn digest(c: char) -> String {
    format!("sha256:{}", c.to_string().repeat(64))
}

/// A complete card: its tested claims, speeds, GPU need and equivalence,
/// trained on `set`.
fn card(name: &str, version: &str, d: &str, set: &str, class: &str, tolerance: f64) -> Value {
    json!({
        "name": name, "version": version, "kind": "head", "digest": d,
        "task": "axis:body_part", "encoder": {"digest": digest('e')}, "threshold": 0.9,
        "trained_on": {"label_set": set, "name": "gold", "rows": 3},
        "tested": [
            {"claim": "fine agreement with the certified model, class III, E1", "value": 0.99746, "bound": 0.99674, "n": 17709, "evidence": "ims-a522a-bp"},
        ],
        "speed": {
            "cpu": {"stacks_per_second": 29.1, "hardware": "64 cores", "evidence": "ims-a523e-bp"},
            "gpu": {"stacks_per_second": 117.2, "hardware": "64 cores and one card", "evidence": "ims-a523e-bp"},
        },
        "gpu": "optional",
        "equivalence": {"class": class, "tolerance": tolerance, "set": "round 1's blocks", "evidence": "ims-a523e-bp"},
        "intended_use": "Propose body_part per MR stack",
        "limits": ["New patients from the scanners seen in training"],
    })
}

/// A results.json of a model's proposals: (stack, value, probability).
fn results(path: &Path, model: &str, answers: &[(i64, &str, f64)]) {
    let proposals: Vec<Value> = answers
        .iter()
        .map(|(s, v, p)| {
            let other = if *v == "brain" { "spine" } else { "brain" };
            json!({"stack_id": s, "axis": "body_part", "value": v,
                   "probabilities": {(*v): p, other: 1.0 - p}, "model_digest": model})
        })
        .collect();
    std::fs::write(
        path,
        json!({"schema_version": "1", "units": [], "proposals": proposals}).to_string(),
    )
    .unwrap();
}

/// T22: label sets from outside, keyed by DICOM identity and held to the
/// seals; complete cards naming them; several models per task, the site
/// choosing one, by default the one a certificate names.
#[test]
fn models_are_parts_with_cards_and_a_site_chooses_one_per_task() {
    let home = registry();
    let out = TempDir::new("parts-out");
    let server = Server::start(&home);
    server.ok(
        "POST",
        "/api/places",
        Some(json!({"name": "labels-out", "role": "export", "path": out.path().to_str().unwrap()})),
        ADMIN,
    );
    let ids = identities(&home);
    assert_eq!(ids.len(), 4, "{ids:?}");
    let row = |i: usize, how: &str, value: &str| -> String {
        let (_, study, series, key, sop) = &ids[i];
        match how {
            "study" => format!("{study}\t\t\t\tbody_part\t{value}\tperson\tann"),
            "series" => format!("{study}\t{series}\t\t\tbody_part\t{value}\tperson\tann"),
            "sop" => format!("{study}\t{series}\t\t{sop}\tbody_part\t{value}\tperson\tann"),
            _ => format!("{study}\t{series}\t{key}\t\tbody_part\t{value}\tperson\tann"),
        }
    };

    // ------------------------------------------- label sets from outside
    // out of order and with CRLF: registered in canonical form
    let first = format!(
        "{HEADER}\r\n{}\r\n{}\r\n{}\r\n",
        row(3, "stack", "spine"),
        row(0, "stack", "brain"),
        row(1, "stack", "brain")
    );
    let set1 = server.ok(
        "POST",
        "/api/label-sets/register",
        Some(set_body("gold-outside", &first)),
        ADMIN,
    );
    assert_eq!(set1["kind"], "outside", "{set1}");
    assert_eq!(set1["rows"], 3, "{set1}");
    assert_eq!(set1["what"], "body_part", "{set1}");
    assert_eq!(set1["version"], 1, "{set1}");
    assert_eq!(set1["training"], "allowed", "{set1}");
    let mut lines = [
        row(0, "stack", "brain"),
        row(1, "stack", "brain"),
        row(3, "stack", "spine"),
    ];
    lines.sort();
    let canonical = format!("{HEADER}\n{}\n", lines.join("\n"));
    assert_eq!(
        set1["digest"].as_str(),
        Some(sha256(canonical.as_bytes()).as_str()),
        "the digest of the canonical labels.tsv: {set1}"
    );
    let dir = Path::new(set1["path"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(dir.join("labels.tsv")).unwrap(),
        canonical
    );
    assert!(
        std::fs::read_to_string(dir.join("SHA256SUMS"))
            .unwrap()
            .contains("  provenance.json")
    );
    // the same file given again is the name's next version
    let again = server.ok(
        "POST",
        "/api/label-sets/register",
        Some(set_body("gold-outside", &canonical)),
        ADMIN,
    );
    assert_eq!(again["version"], 2, "{again}");
    assert_eq!(again["digest"], set1["digest"], "{again}");
    // refused: a sha256 that is not the file's, another header, a stack
    // key without its series, a row given twice; and without models:work
    let mut lying = set_body("bad", &canonical);
    lying["sha256"]["labels.tsv"] = json!("0".repeat(64));
    for (body, what) in [
        (lying, "hashes to"),
        (
            set_body("bad", &canonical.replacen("stack_key", "key", 1)),
            "header",
        ),
        (
            set_body(
                "bad",
                &format!("{HEADER}\n1.2.3\t\tk\t\tbody_part\tbrain\tperson\tann\n"),
            ),
            "within its series",
        ),
        (
            set_body(
                "bad",
                &format!(
                    "{HEADER}\n{}\n{}\n",
                    row(0, "stack", "brain"),
                    row(0, "stack", "brain")
                ),
            ),
            "twice",
        ),
    ] {
        let (status, doc) = server.call("POST", "/api/label-sets/register", Some(body), ADMIN);
        assert_eq!(status, 400, "{what}: {doc}");
        assert!(doc.to_string().contains(what), "{what}: {doc}");
    }
    let (status, _) = server.call(
        "POST",
        "/api/label-sets/register",
        Some(set_body("x", &canonical)),
        SEER,
    );
    assert_eq!(status, 403);
    // at the keyboard, from a folder with SHA256SUMS
    let folder = TempDir::new("parts-set");
    let open_tsv = format!(
        "{HEADER}\n{}\n{}\n",
        row(0, "series", "brain"),
        row(2, "sop", "brain")
    );
    let provenance = "{\"made_by\": \"a study\"}";
    std::fs::write(folder.path().join("labels.tsv"), &open_tsv).unwrap();
    std::fs::write(folder.path().join("provenance.json"), provenance).unwrap();
    std::fs::write(folder.path().join("README.md"), "how it was made\n").unwrap();
    std::fs::write(
        folder.path().join("SHA256SUMS"),
        format!(
            "{}  labels.tsv\n{}  provenance.json\n{}  README.md\n",
            sha256(open_tsv.as_bytes()),
            sha256(provenance.as_bytes()),
            sha256(b"how it was made\n")
        ),
    )
    .unwrap();
    let typed: Value = serde_json::from_str(&ok(
        &home,
        &[
            "labels",
            "register",
            folder.path().to_str().unwrap(),
            "--name",
            "open-set",
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(typed["kind"], "outside", "{typed}");
    assert_eq!(
        typed["source"]["sha256"]["README.md"],
        sha256(b"how it was made\n")
    );
    let open_set = typed["digest"].as_str().unwrap().to_string();

    // seal the FLAIR (stack 1): a set with a row of it, by stack, series,
    // study or SOP Instance UID, is refused; the earlier set is now held
    server.ok(
        "PUT",
        "/api/ask/selections/sealed",
        Some(json!({"document": {
            "ast_version": 1,
            "params": {"ids": {"type": "list", "value": [ids[1].0]}},
            "sets": {"s": {"grain": "stack", "where": [["in", {}, ["field", {}, "id"], ["param", {}, "ids"]]]}},
            "out": {"set": "s", "level": "record"},
        }})),
        ADMIN,
    );
    let seal: Value = serde_json::from_str(&ok(
        &home,
        &[
            "labels",
            "seal",
            "--select",
            "selection:sealed@1",
            "--pack-dir",
            packs().to_str().unwrap(),
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(seal["stacks"], 1, "{seal}");
    for how in ["stack", "series", "study", "sop"] {
        let tsv = format!(
            "{HEADER}\n{}\n{}\n",
            row(0, "stack", "brain"),
            row(1, how, "brain")
        );
        let (status, doc) = server.call(
            "POST",
            "/api/label-sets/register",
            Some(set_body("sealed-row", &tsv)),
            ADMIN,
        );
        assert_eq!(status, 409, "{how}: {doc}");
        assert!(
            doc.to_string().contains("1 of the set's 2 rows"),
            "{how}: {doc}"
        );
    }
    let held = server.ok(
        "GET",
        &format!("/api/label-sets/{}", set1["id"]),
        None,
        ADMIN,
    );
    assert_eq!(held["withheld_sealed"], 1, "{held}");
    assert!(
        held["training"].as_str().unwrap().starts_with("refused"),
        "{held}"
    );
    let tsv = held["files"]["labels.tsv"].as_str().unwrap();
    assert!(
        !tsv.contains(&ids[1].2),
        "the sealed stack's row is withheld: {tsv}"
    );

    // ------------------------------------------------- cards
    server.ok(
        "POST",
        "/api/models",
        Some(json!({"name": "tiny", "version": "1", "kind": "encoder", "digest": digest('e'), "task": "features:image"})),
        ADMIN,
    );
    // a card may name a set registered from outside, never one now sealed
    let held_set = format!("sha256:{}", set1["digest"].as_str().unwrap());
    let (status, doc) = server.call(
        "POST",
        "/api/models",
        Some(card("held", "1", &digest('9'), &held_set, "I", 0.001)),
        ADMIN,
    );
    assert_eq!(status, 409, "{doc}");
    assert!(doc.to_string().contains("sealed"), "{doc}");
    let open_set = format!("sha256:{open_set}");
    let one = server.ok(
        "POST",
        "/api/models",
        Some(card("fusion", "1", &digest('1'), &open_set, "I", 0.001)),
        ADMIN,
    );
    assert_eq!(one["complete"], true, "{one}");
    assert_eq!(one["missing"], json!([]), "{one}");
    assert_eq!(one["trained_on"], open_set, "{one}");
    let two = server.ok(
        "POST",
        "/api/models",
        Some(card("fusion", "2", &digest('2'), &open_set, "III", 0.5)),
        ADMIN,
    );
    let mut thin = card("fusion", "3", &digest('3'), &open_set, "I", 0.001);
    for k in ["tested", "speed", "equivalence"] {
        thin.as_object_mut().unwrap().remove(k);
    }
    let three = server.ok("POST", "/api/models", Some(thin), ADMIN);
    assert_eq!(three["complete"], false, "{three}");
    assert_eq!(
        three["missing"],
        json!(["tested", "speed.cpu", "speed.gpu", "equivalence"]),
        "{three}"
    );
    // a speed without its hardware is no speed
    let mut bad = card("fusion", "4", &digest('4'), &open_set, "I", 0.001);
    bad["speed"]["gpu"]["hardware"] = json!("");
    let (status, doc) = server.call("POST", "/api/models", Some(bad), ADMIN);
    assert_eq!(status, 400, "{doc}");

    // ------------------------------------------------- admission
    let passed =
        json!({"suite": "heldout", "passed": true, "checks": [{"name": "ece", "passed": true}]});
    let (id1, id2, id3) = (
        one["id"].as_i64().unwrap(),
        two["id"].as_i64().unwrap(),
        three["id"].as_i64().unwrap(),
    );
    // a model that takes a GPU is not admitted without its equivalence
    let unmeasured = server.ok(
        "POST",
        &format!("/api/models/{id1}/admit"),
        Some(passed.clone()),
        ADMIN,
    );
    assert_eq!(unmeasured["state"], "registered", "{unmeasured}");
    assert_eq!(unmeasured["check"]["passed"], false, "{unmeasured}");
    let runs = TempDir::new("parts-runs");
    let s: Vec<i64> = ids.iter().map(|i| i.0).collect();
    let (cpu, gpu, gpu_unsure) = (
        runs.path().join("cpu.json"),
        runs.path().join("gpu.json"),
        runs.path().join("gpu-unsure.json"),
    );
    results(
        &cpu,
        &digest('1'),
        &[
            (s[0], "brain", 0.97),
            (s[2], "brain", 0.95),
            (s[3], "spine", 0.93),
        ],
    );
    results(
        &gpu,
        &digest('1'),
        &[
            (s[0], "brain", 0.97001),
            (s[2], "brain", 0.95),
            (s[3], "spine", 0.93),
        ],
    );
    results(
        &gpu_unsure,
        &digest('1'),
        &[
            (s[0], "brain", 0.97),
            (s[2], "brain", 0.95),
            (s[3], "spine", 0.85),
        ],
    );
    let check = runs.path().join("check.json");
    std::fs::write(&check, passed.to_string()).unwrap();
    let admitted: Value = serde_json::from_str(&ok(
        &home,
        &[
            "model",
            "admit",
            "fusion@1",
            "--check",
            check.to_str().unwrap(),
            "--cpu",
            cpu.to_str().unwrap(),
            "--gpu",
            gpu.to_str().unwrap(),
            "--set",
            "blocks",
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(admitted["state"], "admitted", "{admitted}");
    assert_eq!(admitted["equivalence_measured"]["class"], "I", "{admitted}");
    assert_eq!(admitted["equivalence_measured"]["n"], 3, "{admitted}");
    // class III where the card says III; refused where the card says I
    for (model, cards_class, state) in [
        ("fusion@2", "III", "admitted"),
        ("fusion@3", "I", "registered"),
    ] {
        let done: Value = serde_json::from_str(&ok(
            &home,
            &[
                "model",
                "admit",
                model,
                "--check",
                check.to_str().unwrap(),
                "--cpu",
                cpu.to_str().unwrap(),
                "--gpu",
                gpu_unsure.to_str().unwrap(),
                "--json",
            ],
        ))
        .unwrap();
        assert_eq!(done["state"], state, "{cards_class}: {done}");
        assert_eq!(done["check"]["equivalence"]["class"], "III", "{done}");
        assert_eq!(done["check"]["equivalence"]["changes"], 1, "{done}");
    }
    let _ = id3;

    // ------------------------------------------------- the choice
    let tasks =
        |token: &str| server.ok("GET", "/api/models/tasks?task=axis:body_part", None, token);
    let entry = tasks(SEER);
    let t = &entry["tasks"][0];
    assert_eq!(
        (t["task"].as_str(), t["slot"].as_str()),
        (Some("axis:body_part"), Some("site")),
        "{entry}"
    );
    assert_eq!(
        t["models"].as_array().unwrap().len(),
        3,
        "every model of the task: {entry}"
    );
    // nobody chose, no certificate, none promoted: the newest admitted
    assert_eq!(
        (t["chosen"]["model_id"].as_i64(), t["chosen"]["by"].as_str()),
        (Some(id2), Some("admitted")),
        "{entry}"
    );
    // promoting retires none; the newest promoted answers
    server.ok(
        "POST",
        &format!("/api/models/{id1}/promote"),
        Some(json!({})),
        ADMIN,
    );
    let p = server.ok(
        "POST",
        &format!("/api/models/{id2}/promote"),
        Some(json!({})),
        ADMIN,
    );
    assert_eq!(p["retired_models"], json!([]), "{p}");
    let t = tasks(ADMIN)["tasks"][0].clone();
    assert_eq!(
        (t["chosen"]["model_id"].as_i64(), t["chosen"]["by"].as_str()),
        (Some(id2), Some("promoted")),
        "{t}"
    );
    // the site chooses; a seer may not
    let (status, _) = server.call(
        "POST",
        "/api/models/choose",
        Some(json!({"model": id1})),
        SEER,
    );
    assert_eq!(status, 403);
    let chose = server.ok(
        "POST",
        "/api/models/choose",
        Some(json!({"model": "fusion@1", "why": "the certified one"})),
        ADMIN,
    );
    assert_eq!(
        (
            chose["chosen"]["model_id"].as_i64(),
            chose["chosen"]["by"].as_str()
        ),
        (Some(id1), Some("site")),
        "{chose}"
    );
    assert_eq!(chose["chosen"]["chosen_by"], "ada@lab", "{chose}");
    let flags: Vec<(i64, bool)> = chose["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["id"].as_i64().unwrap(), m["chosen"].as_bool().unwrap()))
        .collect();
    assert!(
        flags.contains(&(id1, true)) && flags.contains(&(id2, false)),
        "{flags:?}"
    );
    let listed = server.ok("GET", "/api/models?task=axis:body_part", None, SEER);
    assert_eq!(
        listed["models"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["chosen"] == true)
            .count(),
        1,
        "{listed}"
    );
    // a registered model is not chosen
    let (status, doc) = server.call(
        "POST",
        "/api/models/choose",
        Some(json!({"model": id3})),
        ADMIN,
    );
    assert_eq!(status, 409, "{doc}");
    // back to the default, then a certificate names the model that answers
    let back = server.ok(
        "POST",
        "/api/models/choose",
        Some(json!({"task": "axis:body_part", "model": null})),
        ADMIN,
    );
    assert_eq!(back["chosen"]["by"], "promoted", "{back}");
    let cert = server.ok(
        "POST",
        "/api/certificates",
        Some(json!({"sample": "selection:sealed@1", "models": [id1],
                    "result": {"sample": "selection:sealed@1", "sample_digest": seal["digest"], "risk": 0.02, "n": 1, "errors": 0}})),
        ADMIN,
    );
    let t = tasks(ADMIN)["tasks"][0].clone();
    assert_eq!(
        (
            t["chosen"]["model_id"].as_i64(),
            t["chosen"]["by"].as_str(),
            t["chosen"]["certificate_id"].as_i64()
        ),
        (Some(id1), Some("certificate"), cert["id"].as_i64()),
        "{t}"
    );
    // the choice is audited, and kept on the models' history
    let events = server.ok("GET", &format!("/api/models/{id1}"), None, ADMIN)["events"].clone();
    let moves: Vec<&str> = events
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["transition"].as_str())
        .collect();
    assert!(moves.ends_with(&["chosen", "unchosen"]), "{moves:?}");
    let (good, shown, _) = cli(&home, &["model", "chosen"]);
    assert!(good);
    assert!(
        shown.contains("fusion@1") && shown.contains("certificate"),
        "{shown}"
    );
    let audit = server.ok("GET", "/api/audit?action=model.choose", None, ADMIN);
    assert!(audit.to_string().contains("model.choose"), "{audit}");
}

/// T23: the admission suite's CPU and GPU equivalence, measured from two
/// runs over one set: class I within the card's tolerance, III where an
/// answer turns to not sure, none where a value turns into another; and a
/// model that needs no GPU is admitted without it.
#[test]
fn the_admission_suite_measures_the_cpu_and_gpu_equivalence() {
    let home = registry();
    let runs = TempDir::new("eq-runs");
    let card_path = runs.path().join("card.json");
    let reg = |c: &Value| {
        std::fs::write(&card_path, c.to_string()).unwrap();
        ok(
            &home,
            &["model", "register", "--card", card_path.to_str().unwrap()],
        );
    };
    reg(
        &json!({"name": "tiny", "version": "1", "kind": "encoder", "digest": digest('e'), "task": "features:image"}),
    );
    let mut c = card("fusion", "1", &digest('1'), "", "I", 0.001);
    c.as_object_mut().unwrap().remove("trained_on");
    reg(&c);
    let s = [11i64, 12, 13];
    let (cpu, near, far, flip) = (
        runs.path().join("cpu.json"),
        runs.path().join("near.json"),
        runs.path().join("far.json"),
        runs.path().join("flip.json"),
    );
    results(
        &cpu,
        &digest('1'),
        &[
            (s[0], "brain", 0.97),
            (s[1], "brain", 0.95),
            (s[2], "spine", 0.93),
        ],
    );
    results(
        &near,
        &digest('1'),
        &[
            (s[0], "brain", 0.9705),
            (s[1], "brain", 0.95),
            (s[2], "spine", 0.93),
        ],
    );
    results(
        &far,
        &digest('1'),
        &[
            (s[0], "brain", 0.975),
            (s[1], "brain", 0.95),
            (s[2], "spine", 0.93),
        ],
    );
    results(
        &flip,
        &digest('1'),
        &[
            (s[0], "spine", 0.97),
            (s[1], "brain", 0.95),
            (s[2], "spine", 0.93),
        ],
    );
    let measure = |gpu: &Path| -> Value {
        serde_json::from_str(&ok(
            &home,
            &[
                "model",
                "equivalence",
                "fusion@1",
                "--cpu",
                cpu.to_str().unwrap(),
                "--gpu",
                gpu.to_str().unwrap(),
                "--json",
            ],
        ))
        .unwrap()
    };
    let m = measure(&near);
    assert_eq!(m["class"], "I", "within 0.001: {m}");
    assert!(
        (m["max_difference"].as_f64().unwrap() - 0.0005).abs() < 1e-9,
        "{m}"
    );
    let m = measure(&far);
    assert_eq!(
        m["class"], "III",
        "0.005 apart is past class I's and II's tolerance: {m}"
    );
    assert_eq!(m["changes"], 0, "{m}");
    let m = measure(&flip);
    assert_eq!(m["class"], Value::Null, "a value turned into another: {m}");
    assert_eq!(m["flips"], 1, "{m}");
    // the run of another model's answers: refused, the thresholds are read
    // from the registered model each answer names
    results(&far, &digest('7'), &[(s[0], "brain", 0.97)]);
    let (good, _, err) = cli(
        &home,
        &[
            "model",
            "equivalence",
            "fusion@1",
            "--cpu",
            cpu.to_str().unwrap(),
            "--gpu",
            far.to_str().unwrap(),
        ],
    );
    assert!(!good);
    assert!(err.contains("not registered"), "{err}");
    // a model that needs no GPU is admitted without the measure
    let check = runs.path().join("check.json");
    std::fs::write(
        &check,
        json!({"suite": "heldout", "passed": true, "checks": [{"name": "ece", "passed": true}]})
            .to_string(),
    )
    .unwrap();
    let out = ok(
        &home,
        &[
            "model",
            "admit",
            "tiny@1",
            "--check",
            check.to_str().unwrap(),
        ],
    );
    assert!(out.starts_with("admitted"), "{out}");
    // and one that takes a GPU says why it was not
    let out = ok(
        &home,
        &[
            "model",
            "admit",
            "fusion@1",
            "--check",
            check.to_str().unwrap(),
        ],
    );
    assert!(out.contains("cpu_gpu_equivalence failed"), "{out}");
    let out = ok(
        &home,
        &[
            "model",
            "admit",
            "fusion@1",
            "--check",
            check.to_str().unwrap(),
            "--cpu",
            cpu.to_str().unwrap(),
            "--gpu",
            flip.to_str().unwrap(),
        ],
    );
    assert!(out.contains("worse than class III"), "{out}");
}
