// SPDX-License-Identifier: AGPL-3.0-only

//! Site participation (2026-11-01), on a lab registry, through the commands
//! and the doors: off by default and nothing leaves; a send before a
//! preview is refused; the counts document holds no subject code, path,
//! stack id or header text and holds a cell under 5 back; a signed
//! document verifies and a changed byte does not; the receiver takes the
//! file once; the people path's draw leaves the localizers out and gives a
//! seeded share of the items to two readers, whose disagreements the
//! sealed read counts; `nils audit settle` settles a model-first draw.
//! SQLite, and Postgres where `NILS_TEST_POSTGRES_DSN` is set.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use base64::Engine as _;
use dicom_core::VR;
use dicom_dictionary_std::tags;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_registry::Store;
use serde_json::{Map, Value, json};

const ISSUER: &str = "https://desk.example/";

/// How many stacks the lab registry holds.
const STACKS: usize = 40;

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/oidc")
}

fn is_philips(i: usize) -> bool {
    i % 10 == 9
}

fn is_localizer(i: usize) -> bool {
    i % 8 == 7
}

/// A lab registry of `STACKS` one-file stacks of three makers (one of them
/// under five stacks) and two years, a few of them localizers, classified,
/// made by the command line.
fn registry(pg: Option<&(String, String)>) -> (TempDir, TempDir) {
    let home = TempDir::new("participation-home");
    let dir = TempDir::new("participation-src");
    for i in 0..STACKS {
        let study = format!("1.2.826.0.1.3680043.8.498.{}", 5000 + i);
        let series = format!("{study}.1");
        let sop = format!("{series}.1");
        let mut e = synth::minimal_mr(&study, &series, &sop);
        e.push(synth::text(
            tags::PATIENT_ID,
            VR::LO,
            &format!("P{}", i / 2),
        ));
        let description = if is_localizer(i) {
            "localizer"
        } else {
            "t1 mprage"
        };
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
        let (maker, model) = if is_philips(i) {
            ("Philips Medical Systems", "Achieva")
        } else if i % 3 == 0 {
            ("GE MEDICAL SYSTEMS", "SIGNA Premier")
        } else {
            ("SIEMENS", "Prisma")
        };
        e.push(synth::text(tags::MANUFACTURER, VR::LO, maker));
        e.push(synth::text(tags::MANUFACTURER_MODEL_NAME, VR::LO, model));
        e.push(synth::text(tags::MAGNETIC_FIELD_STRENGTH, VR::DS, "3"));
        e.push(synth::text(tags::BODY_PART_EXAMINED, VR::CS, "BRAIN"));
        let day = if i % 2 == 0 { "20190304" } else { "20240611" };
        e.push(synth::text(tags::STUDY_DATE, VR::DA, day));
        e.push(synth::text(tags::ACQUISITION_DATE, VR::DA, day));
        dir.file(
            &format!("{study}/{sop}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
    run(
        &home,
        &["key", "add", "k"],
        Some("a participation test key\n"),
    )
    .ok("key add");
    match pg {
        Some((dsn, schema)) => run(
            &home,
            &[
                "init",
                "--backend",
                "postgres",
                "--dsn",
                dsn,
                "--schema",
                schema,
                "--key",
                "k",
            ],
            None,
        )
        .ok("init"),
        None => run(&home, &["init", "--key", "k"], None).ok("init"),
    };
    run(
        &home,
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            dir.path().to_str().unwrap(),
        ],
        None,
    )
    .ok("digest");
    run(&home, &["fingerprint"], None).ok("fingerprint");
    run(
        &home,
        &["classify", "--pack-dir", packs().to_str().unwrap()],
        None,
    )
    .ok("classify");
    (home, dir)
}

/// What a command printed and how it ended.
struct Ran {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

impl Ran {
    fn ok(self, what: &str) -> Ran {
        assert!(
            self.status.success(),
            "{what}: {}\n{}",
            self.stderr,
            self.stdout
        );
        self
    }

    fn refused(self, what: &str, words: &str) -> Ran {
        assert!(
            !self.status.success(),
            "{what} went through: {}",
            self.stdout
        );
        assert!(
            self.stderr.contains(words) || self.stdout.contains(words),
            "{what}: expected {words:?} in\n{}\n{}",
            self.stderr,
            self.stdout
        );
        self
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| panic!("{e}: {}", self.stdout))
    }
}

fn run(home: &TempDir, args: &[&str], stdin: Option<&str>) -> Ran {
    let mut cmd = nils();
    cmd.arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "cleo")
        .env("HOSTNAME", "lab")
        .env("NILS_PRINCIPAL", "cleo@lab")
        .env_remove("NILS_DSN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
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
    Ran {
        status: out.status,
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

/// `nils participate <verb> ...` with the packs' directory, so a preview
/// and a send read the same pack.
fn participate(home: &TempDir, args: &[&str]) -> Ran {
    let packs = packs();
    let mut all: Vec<&str> = vec!["participate"];
    all.extend_from_slice(args);
    if matches!(args.first(), Some(&"preview") | Some(&"send")) {
        all.push("--pack-dir");
        all.push(packs.to_str().unwrap());
    }
    run(home, &all, None)
}

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
        let trust = format!(
            "issuer={ISSUER},audience=nils,jwks={}",
            fixtures().join("jwks.json").display()
        );
        let child = nils()
            .arg("--registry")
            .arg(home.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "2",
                "--auth",
                "oidc",
                "--oidc-trust",
                &trust,
            ])
            .args(["--pack-dir", packs().to_str().unwrap()])
            .env("USER", "cleo")
            .env("HOSTNAME", "lab")
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
        http(
            self.port,
            method,
            path,
            body.map(|b| b.to_string()),
            Some(token),
        )
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

/// One HTTP call on a local port, as the desk would make it.
fn http(
    port: u16,
    method: &str,
    path: &str,
    body: Option<String>,
    token: Option<&str>,
) -> (u16, Value) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let body = body.unwrap_or_default();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(t) = token {
        head.push_str(&format!("Authorization: Bearer {t}\r\n"));
    }
    if !body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
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

/// The receiver, `nils participate receive`, on a port of its own.
struct Receiver {
    child: Child,
    port: u16,
    dir: TempDir,
}

impl Drop for Receiver {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Receiver {
    fn start(sites: &Path) -> Receiver {
        let dir = TempDir::new("participation-received");
        let child = nils()
            .args(["participate", "receive", "--listen", "127.0.0.1:0", "--dir"])
            .arg(dir.path())
            .arg("--sites")
            .arg(sites)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut held = Receiver {
            child,
            port: 0,
            dir,
        };
        let stdout = held.child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            panic!("the receiver did not listen");
        };
        let addr = first.split_whitespace().nth(5).unwrap();
        held.port = addr.rsplit(':').next().unwrap().parse().unwrap();
        held
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn files(&self, site: &str) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(self.dir.path().join(site))
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }
}

fn token(sub: &str, name: &str, grants: &[&str], detail: &str) -> String {
    let key =
        EncodingKey::from_rsa_pem(&std::fs::read(fixtures().join("signing-key.pem")).unwrap())
            .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test-2026".to_string());
    encode(
        &header,
        &json!({"iss": ISSUER, "aud": "nils", "sub": sub, "name": name, "preferred_username": sub,
                "grants": grants, "detail": detail, "exp": now + 3600, "iat": now}),
        &key,
    )
    .unwrap()
}

/// Claim and answer every item a token is given in a campaign. Answers
/// the stacks it answered.
fn answer_all(
    server: &Server,
    campaign: i64,
    token: &str,
    answer: impl FnMut(i64, &Value) -> Value,
) -> Vec<i64> {
    answer_all_as(server, campaign, token, "rater", answer)
}

fn answer_all_as(
    server: &Server,
    campaign: i64,
    token: &str,
    role: &str,
    mut answer: impl FnMut(i64, &Value) -> Value,
) -> Vec<i64> {
    let mut done = Vec::new();
    loop {
        let claimed = server.ok(
            "POST",
            &format!("/api/campaigns/{campaign}/claim"),
            Some(json!({"role": role})),
            token,
        );
        if claimed["assignment"].is_null() {
            return done;
        }
        let item = claimed["item"]["id"].as_i64().unwrap();
        let stack = claimed["item"]["stack_id"].as_i64().unwrap();
        let why = server.ok(
            "GET",
            &format!("/api/campaigns/{campaign}/items/{item}/why"),
            None,
            token,
        );
        let value = answer(stack, &why);
        let a = claimed["assignment"]["id"].as_i64().unwrap();
        server.ok(
            "POST",
            &format!("/api/campaigns/{campaign}/assignments/{a}/answer"),
            Some(json!({"value": value.to_string()})),
            token,
        );
        done.push(stack);
    }
}

/// Whether an accepted answer calls the stack a localizer (the provenance
/// axis, one value or a list).
fn is_loc(accepted: &Value) -> bool {
    let is = |v: &Value| {
        v.as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("localizer"))
    };
    match &accepted["provenance"] {
        Value::Array(l) => l.iter().any(is),
        other => is(other),
    }
}

/// A document with its keys sorted at every level, on one line: what the
/// engine signs.
fn canonical(v: &Value) -> String {
    fn sorted(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                let mut out = Map::new();
                for k in keys {
                    out.insert(k.clone(), sorted(&m[k]));
                }
                Value::Object(out)
            }
            Value::Array(l) => Value::Array(l.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    sorted(v).to_string()
}

fn verifies(doc: &Value, public_key_b64: &str) -> bool {
    let public = base64::engine::general_purpose::STANDARD
        .decode(public_key_b64)
        .unwrap();
    let signature = base64::engine::general_purpose::STANDARD
        .decode(doc["signature"].as_str().unwrap_or_default())
        .unwrap_or_default();
    let mut unsigned = doc.clone();
    unsigned.as_object_mut().unwrap().remove("signature");
    unsigned.as_object_mut().unwrap().remove("signed_by");
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public)
        .verify(canonical(&unsigned).as_bytes(), &signature)
        .is_ok()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// What a document must never carry: the registry's subject codes, the
/// source files' paths, the stacks' ids, the header's words.
fn forbidden(store: &mut Store, src: &TempDir) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in store
        .query(
            &format!("SELECT code FROM {}", store.qualified("subject")),
            &[],
        )
        .unwrap()
    {
        out.push(r.text(0).unwrap().to_string());
    }
    for r in store
        .query(
            &format!(
                "SELECT path FROM {} LIMIT 5",
                store.qualified("source_file")
            ),
            &[],
        )
        .unwrap()
    {
        out.push(r.text(0).unwrap().to_string());
    }
    out.push(src.path().display().to_string());
    out.push("1.2.826.0.1.3680043".to_string());
    out.push("t1 mprage".to_string());
    out.push("stack_id".to_string());
    out.push("\"P0\"".to_string());
    out.push("cleo".to_string());
    out
}

fn holds_none_of(doc: &Value, words: &[String], what: &str) {
    let text = doc.to_string();
    for w in words {
        assert!(
            !w.is_empty() && !text.contains(w.as_str()),
            "{what} carries {w:?}"
        );
    }
}

fn flow(pg: Option<(String, String)>) {
    let (home, src) = registry(pg.as_ref());
    let open = || match &pg {
        Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
        None => Store::open_sqlite(&home.path().join("registry.db")).unwrap(),
    };
    let never = forbidden(&mut open(), &src);

    // ------------------------------------------------ off by default
    let status = participate(&home, &["status", "--json"]).ok("status");
    assert_eq!(status.json()["enabled"], false, "{}", status.stdout);
    assert!(
        participate(&home, &["status"])
            .ok("status")
            .stdout
            .contains("nothing leaves"),
    );
    participate(&home, &["send", "--tier", "1"]).refused("send while off", "takes no part");
    participate(&home, &["preview", "--tier", "1"]).refused("preview while off", "takes no part");

    // ------------------------------------------------ enable, the key pair
    let key_file = home.path().join("receiver.key");
    std::fs::write(&key_file, "the-site-bearer-key-0001\n").unwrap();
    let enabled = participate(
        &home,
        &[
            "enable",
            "--tier",
            "1",
            "--tier",
            "3",
            "--site",
            "lab",
            "--key-file",
            key_file.to_str().unwrap(),
            "--json",
        ],
    )
    .ok("enable")
    .json();
    assert_eq!(enabled["enabled"], true, "{enabled}");
    assert_eq!(enabled["site"], "lab", "{enabled}");
    assert_eq!(enabled["tiers"], json!([1, 3]), "{enabled}");
    assert_eq!(enabled["auto"], false, "{enabled}");
    let public_key = enabled["public_key"].as_str().unwrap().to_string();
    let fingerprint = enabled["key_fingerprint"].as_str().unwrap().to_string();
    assert_eq!(fingerprint.len(), 16);
    let private = home.path().join("participation").join("signing.key");
    assert!(private.exists(), "the private half is in the registry home");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&private).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the signing key is the owner's alone");
    }
    // nothing of the key is in the registry
    let mut store = open();
    let row = store
        .query_opt(
            &format!(
                "SELECT public_key, key_file FROM {}",
                store.qualified("participation")
            ),
            &[],
        )
        .unwrap()
        .unwrap();
    assert_eq!(row.text(0).unwrap(), public_key);
    assert_eq!(row.text(1).unwrap(), key_file.display().to_string());
    drop(store);
    // a key file that cannot be read is refused
    participate(
        &home,
        &[
            "enable",
            "--tier",
            "1",
            "--key-file",
            home.path().join("missing.key").to_str().unwrap(),
        ],
    )
    .refused("enable with a missing key file", "no key could be read");

    // ------------------------------------------------ the receiver
    let sites = home.path().join("sites.toml");
    std::fs::write(
        &sites,
        format!(
            "[[sites]]\nname = \"lab\"\nkey_sha256 = \"{}\"\npublic_key = \"{}\"\n",
            sha256_hex(b"the-site-bearer-key-0001"),
            public_key
        ),
    )
    .unwrap();
    let receiver = Receiver::start(&sites);
    let (status, health) = http(receiver.port, "GET", "/healthz", None, None);
    assert_eq!((status, health), (200, Value::String("ok".into())));
    participate(
        &home,
        &["enable", "--tier", "1", "--receiver", &receiver.url()],
    )
    .ok("enable with the receiver");

    // ------------------------------------------------ preview before send
    participate(&home, &["send", "--tier", "1"]).refused("send before preview", "never previewed");
    let out1 = home.path().join("tier1.json");
    let previewed = participate(
        &home,
        &["preview", "--tier", "1", "--out", out1.to_str().unwrap()],
    )
    .ok("preview tier 1");
    assert!(
        previewed.stdout.contains("nothing else leaves"),
        "{}",
        previewed.stdout
    );
    let doc1: Value = serde_json::from_str(&std::fs::read_to_string(&out1).unwrap()).unwrap();
    assert_eq!(doc1["kind"], "nils participation counts", "{doc1}");
    assert_eq!(doc1["site"], "lab");
    assert_eq!(doc1["trains"], false);
    assert_eq!(doc1["signed_by"], json!(fingerprint));
    assert_eq!(doc1["versions"]["engine"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        doc1["versions"]["pack"]["name"], "mri",
        "{}",
        doc1["versions"]
    );
    holds_none_of(&doc1, &never, "the counts document");
    // the archive's counts: the stacks, by vendor with the small maker held
    let archive = &doc1["archive"];
    assert_eq!(archive["stacks"], STACKS, "{archive}");
    assert_eq!(archive["subjects"], STACKS / 2, "{archive}");
    assert_eq!(archive["by_modality"]["MR"], STACKS, "{archive}");
    let philips = (0..STACKS).filter(|i| is_philips(*i)).count();
    assert!(philips < 5, "the fixture keeps one maker under the minimum");
    assert_eq!(
        archive["by_vendor"]["philips"],
        Value::Null,
        "a cell under 5 is held back: {archive}"
    );
    // siemens, the largest maker, is shown whatever else is held
    assert!(
        archive["by_vendor"]["siemens"].as_u64().unwrap() > 5,
        "{archive}"
    );
    let held: Vec<String> = doc1["min_cell"]["held_back"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(
        held.contains(&"archive.vendor:philips".to_string()),
        "{held:?}"
    );
    assert_eq!(archive["by_year"]["2019"], STACKS / 2, "{archive}");
    assert_eq!(archive["by_year"]["2024"], STACKS / 2, "{archive}");
    assert_eq!(
        archive["by_scanner_model"]["prisma"].as_u64().is_some(),
        true,
        "{archive}"
    );
    assert!(
        doc1["answers"]["base"]["by_value"].is_object(),
        "{}",
        doc1["answers"]
    );
    assert_eq!(
        doc1["releases"]["count"],
        Value::Null,
        "no releases: under 5, held"
    );
    assert_eq!(doc1["audit"]["rounds_graded"], 0);
    // the signature holds, and a changed byte does not
    assert!(verifies(&doc1, &public_key), "the preview is signed");
    let mut changed = doc1.clone();
    changed["archive"]["stacks"] = json!(STACKS + 1);
    assert!(!verifies(&changed, &public_key), "a changed count fails");
    assert!(!verifies(
        &doc1,
        &base64::engine::general_purpose::STANDARD.encode([7u8; 32])
    ));
    // the preview is on record
    let status = participate(&home, &["status", "--json"])
        .ok("status")
        .json();
    assert!(status["previewed"]["1"]["digest"].is_string(), "{status}");

    // ------------------------------------------------ the send
    let sent = participate(&home, &["send", "--tier", "1", "--json"])
        .ok("send tier 1")
        .json();
    assert_eq!(sent["ok"], true, "{sent}");
    assert_eq!(sent["tier"], 1);
    assert_eq!(sent["sent_by"], "cleo@lab");
    assert!(sent["receipt"].as_str().unwrap().len() == 64, "{sent}");
    assert_eq!(
        sent["receipt"], sent["digest"],
        "the receipt is the body's digest"
    );
    let files = receiver.files("lab");
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].contains("-tier1-"), "{files:?}");
    assert_eq!(sent["stored_as"], json!(files[0]));
    // what the receiver kept is the signed document, byte for byte
    let kept = std::fs::read(receiver.dir.path().join("lab").join(&files[0])).unwrap();
    assert_eq!(sha256_hex(&kept), sent["digest"].as_str().unwrap());
    let kept: Value = serde_json::from_slice(&kept).unwrap();
    assert!(verifies(&kept, &public_key));
    holds_none_of(&kept, &never, "the kept document");
    // the audit row
    let audit = run(
        &home,
        &["audit", "list", "--action", "participation.send", "--json"],
        None,
    )
    .ok("audit list")
    .json();
    assert_eq!(audit.as_array().unwrap().len(), 1, "{audit}");
    assert_eq!(audit[0]["action"], "participation.send");
    assert_eq!(audit[0]["scope"]["tier"], 1, "{audit}");
    assert!(audit.to_string().contains(sent["digest"].as_str().unwrap()));
    assert!(
        !audit.to_string().contains("the-site-bearer-key"),
        "never the key"
    );
    // the same counts again, still previewed: a new file, since the body
    // carries the time it was made (the receiver's 200 for the very same
    // body is the receiver's own test)
    let again = participate(&home, &["send", "--tier", "1", "--json"])
        .ok("send tier 1 again")
        .json();
    assert_eq!(again["ok"], true, "{again}");
    // the same second gives the same body, which the receiver keeps once
    let kept_now = receiver.files("lab").len();
    assert!(
        (again["receipt"] == sent["receipt"] && kept_now == 1)
            || (again["receipt"] != sent["receipt"] && kept_now == 2),
        "{again} {:?}",
        receiver.files("lab")
    );
    // the sends list
    let sends = participate(&home, &["status", "--json"])
        .ok("status")
        .json();
    assert_eq!(sends["last_send"]["1"]["ok"], true, "{sends}");
    assert_eq!(sends["receiver"], json!(receiver.url()));

    // ------------------------------------------------ auto
    participate(&home, &["send", "--tier", "1", "--auto"]).refused("auto while off", "auto is off");
    participate(&home, &["enable", "--tier", "1", "--auto"]).ok("auto on");
    let auto = participate(&home, &["send", "--tier", "1", "--auto", "--json"])
        .ok("auto send")
        .json();
    assert_eq!(auto["sent_by"], "auto", "{auto}");
    participate(&home, &["send", "--tier", "1", "--auto"])
        .refused("a second auto send today", "today already");
    participate(&home, &["send", "--tier", "3", "--auto"])
        .refused("auto of a sealed read", "sent by a person");

    // ------------------------------------------------ a wrong key at the receiver
    std::fs::write(&key_file, "not-the-key\n").unwrap();
    let bad = participate(&home, &["send", "--tier", "1", "--json"]);
    assert!(!bad.status.success(), "a refused send fails the command");
    let bad = bad.json();
    assert_eq!(bad["ok"], false, "{bad}");
    assert!(bad["error"].as_str().unwrap().contains("401"), "{bad}");
    std::fs::write(&key_file, "the-site-bearer-key-0001\n").unwrap();

    // ------------------------------------------------ the people path through the door
    let server = Server::start(&home);
    let cleo = token(
        "cleo",
        "Cleo Curator",
        &[
            "review:see",
            "review:work",
            "campaigns:see",
            "campaigns:work",
            "database:see",
            "database:work",
        ],
        "quasi",
    );
    let reader = |sub: &str| token(sub, sub, &["campaigns:see", "campaigns:work"], "quasi");
    let (nora, omar, rater_a, rater_b) = (
        reader("nora"),
        reader("omar"),
        reader("rater-a"),
        reader("rater-b"),
    );
    let principal = |t: &str| {
        server.ok("GET", "/api/capabilities", None, t)["principal"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (nora_p, omar_p, a_p, b_p) = (
        principal(&nora),
        principal(&omar),
        principal(&rater_a),
        principal(&rater_b),
    );
    let since = "2000-01-01T00:00:00Z";
    // the door's status
    let st = server.ok("GET", "/api/participation", None, &cleo);
    assert_eq!(st["enabled"], true, "{st}");
    assert_eq!(st["receiver_reachable"], true, "{st}");
    assert_eq!(
        st["assistant_rater_door"],
        Value::Null,
        "no assistant here: {st}"
    );
    // a reader without the grant is refused
    let (status, _) = server.call("GET", "/api/participation", None, &nora);
    assert_eq!(status, 403);
    // the whole frame, localizers in, to count them
    let all = server.ok(
        "POST",
        "/api/audit/draws",
        Some(
            json!({"count": 1000, "since": since, "seed": "all", "axes": ["base", "provenance"],
                    "raters": [a_p.clone(), b_p.clone()]}),
        ),
        &cleo,
    );
    let whole = all["draw"]["frame"]["stacks"].as_i64().unwrap();
    let shown = server.ok(
        "GET",
        &format!("/api/audit/draws/{}", all["draw"]["id"]),
        None,
        &cleo,
    );
    let localizers = shown["stacks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| is_loc(&s["accepted"]))
        .count() as i64;
    assert!(
        localizers >= 1,
        "the fixture's localizers are in the round: {shown}"
    );
    // the people path: no localizers, a fifth read twice
    let (status, doc) = server.call(
        "POST",
        "/api/audit/draws",
        Some(
            json!({"count": 20, "since": since, "seed": "site-2026-11", "axes": ["base"],
                    "readers": [nora_p, omar_p], "double_read_share": 2, "localizers": false}),
        ),
        &cleo,
    );
    assert_eq!(status, 400, "a share over 1: {doc}");
    let people = server.ok(
        "POST",
        "/api/audit/draws",
        Some(
            json!({"count": 20, "since": since, "seed": "site-2026-11", "axes": ["base"],
                    "readers": [nora_p, omar_p], "double_read_share": 0.25, "localizers": false}),
        ),
        &cleo,
    );
    assert_eq!(
        people["draw"]["frame"]["stacks"],
        json!(whole - localizers),
        "the localizers are out of the frame: {people}"
    );
    assert_ne!(
        people["draw"]["frame"]["digest"], all["draw"]["frame"]["digest"],
        "the digest covers the frame as drawn"
    );
    assert_eq!(people["campaign"]["purpose"], "verify", "{people}");
    assert_eq!(people["campaign"]["raters_per_item"], 1, "{people}");
    assert_eq!(people["campaign"]["double_read_share"], 0.25, "{people}");
    let read_twice = people["campaign"]["read_twice"].as_u64().unwrap();
    assert_eq!(read_twice, 5, "a quarter of 20, rounded up: {people}");
    assert_eq!(
        people["campaign"]["planted"]["count"], 2,
        "10 % of 20: {people}"
    );
    let draw3 = people["draw"]["id"].as_i64().unwrap();
    let verify = people["campaign"]["id"].as_i64().unwrap();
    let drawn = server.ok("GET", &format!("/api/audit/draws/{draw3}"), None, &cleo);
    let localizer_stacks: BTreeSet<i64> = shown["stacks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| is_loc(&s["accepted"]))
        .map(|s| s["stack_id"].as_i64().unwrap())
        .collect();
    for s in drawn["stacks"].as_array().unwrap() {
        assert!(
            !localizer_stacks.contains(&s["stack_id"].as_i64().unwrap()),
            "a localizer was drawn: {s}"
        );
    }
    let accepted: BTreeMap<i64, String> = drawn["stacks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["stack_id"].as_i64().unwrap(),
                s["accepted"]["base"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    // the items wanting two readers say so
    let c = server.ok("GET", &format!("/api/campaigns/{verify}"), None, &cleo);
    let twice: BTreeSet<i64> = c["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["raters"] == 2)
        .map(|i| i["stack_id"].as_i64().unwrap())
        .collect();
    assert_eq!(twice.len() as u64, read_twice, "{c}");
    // nora reads every item, accepting the proposal
    let other = |b: &str| if b == "PDw" { "T2w" } else { "PDw" }.to_string();
    let n = answer_all(&server, verify, &nora, |stack, why| {
        assert_eq!(why["purpose"], "verify", "{why}");
        json!({"base": accepted[&stack]})
    });
    assert_eq!(n.len(), 20);
    // omar is given the items read twice, and only those; he differs
    let m = answer_all(
        &server,
        verify,
        &omar,
        |stack, _| json!({"base": other(&accepted[&stack])}),
    );
    assert_eq!(m.iter().copied().collect::<BTreeSet<i64>>(), twice, "{m:?}");
    // where the two differ, a third person settles the item
    let j = answer_all_as(
        &server,
        verify,
        &cleo,
        "adjudicator",
        |stack, _| json!({"base": accepted[&stack]}),
    );
    assert_eq!(j.iter().copied().collect::<BTreeSet<i64>>(), twice, "{j:?}");
    server.ok(
        "POST",
        &format!("/api/campaigns/{verify}/close"),
        Some(json!({})),
        &cleo,
    );
    // the sealed read of the draw, through the door: previewed and sent
    let (status, doc) = server.call(
        "GET",
        &format!(
            "/api/participation/preview?tier=3&draw={}",
            all["draw"]["id"]
        ),
        None,
        &cleo,
    );
    assert_eq!(status, 409, "an unsettled draw: {doc}");
    let pv = server.ok(
        "GET",
        &format!("/api/participation/preview?tier=3&draw={draw3}"),
        None,
        &cleo,
    );
    let doc3 = &pv["document"];
    assert_eq!(doc3["kind"], "nils participation sealed read", "{doc3}");
    assert_eq!(doc3["export_kind"], "nils audit export");
    assert_eq!(doc3["draw"]["id"], draw3);
    assert_eq!(doc3["protocol"]["path"], "people", "{}", doc3["protocol"]);
    assert_eq!(doc3["protocol"]["readers"], 2, "{}", doc3["protocol"]);
    assert_eq!(
        doc3["protocol"]["planted"]["planted"].as_u64().is_some(),
        true,
        "{}",
        doc3["protocol"]
    );
    assert_eq!(doc3["counts"]["drawn"], 20, "{}", doc3["counts"]);
    assert_eq!(doc3["counts"]["read"], 20, "{}", doc3["counts"]);
    assert_eq!(
        doc3["disagreement"],
        json!({"read_by_two_or_more": read_twice, "disagreed": read_twice}),
        "{doc3}"
    );
    assert_eq!(doc3["composition"]["drawn"], 20, "{}", doc3["composition"]);
    assert_eq!(
        doc3["composition"]["by_field_strength"]["3.0T"], 20,
        "{}",
        doc3["composition"]
    );
    assert!(
        doc3["composition"]["by_vendor"].is_object(),
        "{}",
        doc3["composition"]
    );
    assert_eq!(doc3["trains"], false);
    assert!(verifies(doc3, &public_key));
    holds_none_of(doc3, &never, "the sealed read document");
    assert!(
        pv["digest"].is_string() && pv["previewed_at"].is_string(),
        "{pv}"
    );
    let kept_before = receiver.files("lab").len();
    let sent3 = server.ok(
        "POST",
        "/api/participation/send",
        Some(json!({"tier": 3, "draw": draw3})),
        &cleo,
    );
    assert_eq!(sent3["ok"], true, "{sent3}");
    assert_eq!(sent3["draw"], draw3, "{sent3}");
    let files = receiver.files("lab");
    assert_eq!(files.len(), kept_before + 1, "{files:?}");
    assert!(files.iter().any(|f| f.contains("-tier3-")), "{files:?}");
    let listed = server.ok("GET", "/api/participation/sends", None, &cleo);
    assert!(listed["sends"].as_array().unwrap().len() >= 4, "{listed}");
    assert_eq!(listed["sends"][0]["tier"], 3, "newest first: {listed}");

    // ------------------------------------------------ the model-first path, settled at the keyboard
    let model = server.ok(
        "POST",
        "/api/audit/draws",
        Some(
            json!({"count": 1000, "since": since, "seed": "model", "axes": ["base"],
                    "raters": [a_p, b_p], "localizers": false}),
        ),
        &cleo,
    );
    let raters = model["campaign"]["id"].as_i64().unwrap();
    let draw1 = model["draw"]["id"].as_i64().unwrap();
    let whole = model["draw"]["frame"]["stacks"].as_i64().unwrap();
    let shown = server.ok("GET", &format!("/api/audit/draws/{draw1}"), None, &cleo);
    let frame_accepted: BTreeMap<i64, String> = shown["stacks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["stack_id"].as_i64().unwrap(),
                s["accepted"]["base"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    let first: i64 = *frame_accepted.keys().next().unwrap();
    run(
        &home,
        &[
            "audit",
            "settle",
            "--draw",
            &draw1.to_string(),
            "--share",
            "0.2",
        ],
        None,
    )
    .refused("settle before the raters answered", "wait on a rater");
    for t in [&rater_a, &rater_b] {
        let n = answer_all(&server, raters, t, |stack, why| {
            assert_eq!(why["blind"], true);
            let b = &frame_accepted[&stack];
            json!({"base": if stack == first { other(b) } else { b.clone() }})
        });
        assert_eq!(n.len() as i64, whole);
    }
    let settled = run(
        &home,
        &[
            "audit",
            "settle",
            "--draw",
            &draw1.to_string(),
            "--share",
            "0.2",
            "--settlers",
            &nora_p,
            "--json",
        ],
        None,
    )
    .ok("audit settle")
    .json();
    assert_eq!(settled["settle"]["share"], 0.2, "{settled}");
    assert_eq!(settled["settle"]["differences"], 1, "{settled}");
    let settle = settled["settle"]["campaign"]["id"].as_i64().unwrap();
    let again = run(
        &home,
        &["audit", "settle", "--draw", &draw1.to_string(), "--json"],
        None,
    )
    .ok("audit settle again")
    .json();
    assert_eq!(again["settle"]["again"], true, "{again}");
    assert_eq!(again["settle"]["campaign"]["id"], settle, "{again}");
    let n = answer_all(
        &server,
        settle,
        &nora,
        |stack, _| json!({"base": frame_accepted[&stack]}),
    );
    assert!(!n.is_empty());
    server.ok(
        "POST",
        &format!("/api/campaigns/{settle}/close"),
        Some(json!({})),
        &cleo,
    );
    let out3 = home.path().join("tier3-model-first.json");
    participate(
        &home,
        &[
            "preview",
            "--tier",
            "3",
            "--draw",
            &draw1.to_string(),
            "--out",
            out3.to_str().unwrap(),
        ],
    )
    .ok("preview tier 3 of the model-first draw");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&out3).unwrap()).unwrap();
    assert_eq!(
        doc["protocol"]["path"], "model-first",
        "{}",
        doc["protocol"]
    );
    assert_eq!(doc["protocol"]["share"], 0.2, "{}", doc["protocol"]);
    assert_eq!(doc["protocol"]["readers"], 1, "{}", doc["protocol"]);
    holds_none_of(&doc, &never, "the model-first sealed read");
    // a preview of one draw does not let another be sent
    participate(
        &home,
        &["send", "--tier", "3", "--draw", &draw3.to_string()],
    )
    .refused(
        "send of a draw not the one previewed",
        "changed since it was previewed",
    );

    // ------------------------------------------------ disable
    participate(&home, &["disable", "--tier", "3"]).ok("disable tier 3");
    let st = participate(&home, &["status", "--json"])
        .ok("status")
        .json();
    assert_eq!(st["tiers"], json!([1]), "{st}");
    participate(&home, &["preview", "--tier", "3"])
        .refused("preview of a tier turned off", "does not share tier 3");
    participate(&home, &["disable"]).ok("disable");
    let st = participate(&home, &["status", "--json"])
        .ok("status")
        .json();
    assert_eq!(st["enabled"], false, "{st}");
    assert!(!private.exists(), "the signing key goes with the row");
    let door = server.ok("GET", "/api/participation", None, &cleo);
    assert_eq!(door["enabled"], false, "{door}");
    assert_eq!(door["note"], "Nothing leaves this install.", "{door}");
    // the sends stay on record
    let listed = server.ok("GET", "/api/participation/sends", None, &cleo);
    assert!(listed["sends"].as_array().unwrap().len() >= 4, "{listed}");
    // and the door enables again, a new pair
    let on = server.ok(
        "POST",
        "/api/participation",
        Some(json!({"tiers": [1], "site": "lab-2", "auto": true})),
        &cleo,
    );
    assert_eq!(on["enabled"], true, "{on}");
    assert_eq!(on["site"], "lab-2", "{on}");
    assert_ne!(on["public_key"], json!(public_key), "a new pair");
    let (status, doc) = server.call(
        "POST",
        "/api/participation",
        Some(json!({"site": "not a name!"})),
        &cleo,
    );
    assert_eq!(status, 400, "{doc}");
    let off = server.ok(
        "POST",
        "/api/participation",
        Some(json!({"disable": true})),
        &cleo,
    );
    assert_eq!(off["enabled"], false, "{off}");
}

#[test]
fn a_site_takes_part_and_nothing_leaves_that_a_person_did_not_read() {
    flow(None);
}

#[test]
fn a_site_takes_part_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    let schema = "nils_participation";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    flow(Some((dsn.clone(), schema.to_string())));
    drop();
}

#[test]
fn the_receiver_refuses_what_is_not_a_site_report() {
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
    let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
    let public = base64::engine::general_purpose::STANDARD
        .encode(ring::signature::KeyPair::public_key(&pair).as_ref());
    let home = TempDir::new("participation-receiver-sites");
    let sites = home.path().join("sites.toml");
    std::fs::write(
        &sites,
        format!(
            "[[sites]]\nname = \"lab\"\nkey_sha256 = \"{}\"\npublic_key = \"{public}\"\n",
            sha256_hex(b"k1")
        ),
    )
    .unwrap();
    let receiver = Receiver::start(&sites);
    let sign = |mut doc: Value| -> String {
        let message = canonical(&doc);
        let signature = pair.sign(message.as_bytes());
        doc["signature"] =
            json!(base64::engine::general_purpose::STANDARD.encode(signature.as_ref()));
        doc["signed_by"] =
            json!(&sha256_hex(ring::signature::KeyPair::public_key(&pair).as_ref())[..16]);
        canonical(&doc)
    };
    let good = sign(json!({"kind": "nils participation counts", "site": "lab",
                           "made_at": "2026-11-01T12:00:00Z", "archive": {"stacks": 40}, "trains": false}));
    let post = |site: &str, key: Option<&str>, body: &str| {
        http(
            receiver.port,
            "POST",
            &format!("/v1/sites/{site}/reports"),
            Some(body.to_string()),
            key,
        )
    };
    // accept
    let (status, a) = post("lab", Some("k1"), &good);
    assert_eq!(status, 201, "{a}");
    assert_eq!(a["receipt"], json!(sha256_hex(good.as_bytes())));
    assert_eq!(receiver.files("lab").len(), 1);
    // the same again: 200, the same receipt
    let (status, b) = post("lab", Some("k1"), &good);
    assert_eq!((status, &b["receipt"]), (200, &a["receipt"]), "{b}");
    // bad key
    let (status, e) = post("lab", Some("k2"), &good);
    assert_eq!(status, 401, "{e}");
    let (status, _) = post("lab", None, &good);
    assert_eq!(status, 401);
    // bad signature
    let tampered = good.replace("\"stacks\":40", "\"stacks\":41");
    let (status, e) = post("lab", Some("k1"), &tampered);
    assert_eq!(status, 403, "{e}");
    // unknown site
    let (status, e) = post("other", Some("k1"), &good);
    assert_eq!(status, 404, "{e}");
    // oversize
    let big = format!("{{\"pad\": \"{}\"}}", "x".repeat(2 * 1024 * 1024 + 10));
    let (status, e) = post("lab", Some("k1"), &big);
    assert_eq!(status, 413, "{e}");
    // nothing more was kept
    assert_eq!(receiver.files("lab").len(), 1);
    // and no other door
    let (status, _) = http(
        receiver.port,
        "GET",
        "/v1/sites/lab/reports",
        None,
        Some("k1"),
    );
    assert_eq!(status, 404);
}
