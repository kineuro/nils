// SPDX-License-Identifier: AGPL-3.0-only
//! The damaged-header flag set at ingest (record 56 §3; record 55, ruling
//! 31 (c)), end to end: at the end of a sort each dataset holding a scan
//! the sort judged has its three signs read with the sort's pack and kept
//! beside its flag; a dataset is flagged when one share reaches the pack's
//! threshold (nameless scans, words the pack does not know, a maker the
//! rules were not built on that no dataset brought in before it holds); a dataset no sort
//! has read since is read when asked; a person's word wins and the signs
//! stay under it until the person lets them decide again; a pack before
//! contract 11 flags by the nameless share at a half alone. SQLite.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use serde_json::{Value, json};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

/// A registry's home, its store in SQLite or in a Postgres schema.
struct Home {
    dir: TempDir,
    pg: Option<(String, String)>,
}

impl Home {
    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn store(&self) -> nils_registry::Store {
        match &self.pg {
            Some((dsn, schema)) => nils_registry::Store::connect_postgres(dsn, schema).unwrap(),
            None => {
                nils_registry::Store::open_sqlite(&self.dir.path().join("registry.db")).unwrap()
            }
        }
    }
}

fn run(home: &Home, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
    let mut child = nils()
        .arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "anna")
        .env("HOSTNAME", "ward-3")
        .env_remove("NILS_DSN")
        .env_remove("NILS_JOB_ID")
        .env_remove("NILS_JOB_DETAIL")
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

fn ok(home: &Home, args: &[&str]) -> (String, String) {
    let (good, out, err) = run(home, args, None);
    assert!(good, "nils {args:?} failed: {err}\n{out}");
    (out, err)
}

/// One series of two images, named `name` (none: neither a description nor
/// a protocol name) by `maker`.
fn series(dir: &TempDir, patient: &str, study: &str, number: i64, name: Option<&str>, maker: &str) {
    let series = format!("{study}.{number}");
    for i in 0..2u32 {
        let sop = format!("{series}.{}", i + 1);
        let mut e = synth::minimal_mr(study, &series, &sop);
        let mut fields = vec![
            (tags::PATIENT_ID, VR::LO, patient.to_string()),
            (tags::STUDY_DATE, VR::DA, "20240310".to_string()),
            (tags::SERIES_DATE, VR::DA, "20240310".to_string()),
            (tags::MANUFACTURER, VR::LO, maker.to_string()),
            (tags::MAGNETIC_FIELD_STRENGTH, VR::DS, "3".into()),
            (tags::SERIES_NUMBER, VR::IS, number.to_string()),
            (tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND".into()),
            (tags::SCANNING_SEQUENCE, VR::CS, "GR\\IR".into()),
            (tags::SEQUENCE_VARIANT, VR::CS, "SP\\MP".into()),
            (tags::MR_ACQUISITION_TYPE, VR::CS, "3D".into()),
            (tags::REPETITION_TIME, VR::DS, "2300".into()),
            (tags::ECHO_TIME, VR::DS, "3".into()),
            (tags::INVERSION_TIME, VR::DS, "900".into()),
            (tags::SLICE_THICKNESS, VR::DS, "1".into()),
            (tags::PIXEL_SPACING, VR::DS, "1\\1".into()),
            (
                tags::IMAGE_ORIENTATION_PATIENT,
                VR::DS,
                "1\\0\\0\\0\\1\\0".into(),
            ),
            (tags::IMAGE_POSITION_PATIENT, VR::DS, format!("0\\0\\{i}")),
            (tags::FRAME_OF_REFERENCE_UID, VR::UI, format!("{study}.0")),
            (tags::INSTANCE_NUMBER, VR::IS, (i + 1).to_string()),
        ];
        if let Some(n) = name {
            fields.push((tags::SERIES_DESCRIPTION, VR::LO, n.to_string()));
            fields.push((tags::PROTOCOL_NAME, VR::LO, n.to_string()));
        }
        for (tag, vr, v) in fields {
            e.push(synth::text(tag, vr, &v));
        }
        e.push(synth::num(tags::ROWS, VR::US, 32.0));
        e.push(synth::num(tags::COLUMNS, VR::US, 32.0));
        dir.file(
            &format!("s{number}/IM_{i:04}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
}

/// A dataset of one study, its series each `(name, maker)`.
fn dataset(
    home: &Home,
    name: &str,
    patient: &str,
    study: &str,
    of: &[(Option<&str>, &str)],
) -> TempDir {
    let dir = TempDir::new(&format!("dh-{name}"));
    for (i, (n, maker)) in of.iter().enumerate() {
        series(&dir, patient, study, i as i64 + 1, *n, maker);
    }
    ok(
        home,
        &[
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
        ],
    );
    ok(
        home,
        &[
            "digest",
            "--name",
            name,
            "--no-private",
            &format!("@{name}"),
        ],
    );
    dir
}

const OPS: &str = "an-operator-token-of-length";

/// A `nils serve` with no worker, killed when dropped.
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
    fn start(home: &Home, pack_dir: &Path) -> Served {
        use std::io::BufRead as _;
        let mut child = nils()
            .arg("--registry")
            .arg(home.path())
            .args(["serve", "--bind", "127.0.0.1:0", "--workers", "1"])
            .args(["--auth", "token", "--pack-dir", pack_dir.to_str().unwrap()])
            .env("NILS_TOKENS", format!("{OPS}=ops@lab:operator"))
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_DSN")
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
        Served { child, port }
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

    fn headers(&self, dataset: &str) -> Value {
        let (status, doc) = self.call("GET", &format!("/api/datasets/{dataset}/headers"), None);
        assert_eq!(status, 200, "{dataset}: {doc}");
        doc
    }
}

/// The newest classify job's result, as the registry keeps it.
fn last_sort(home: &Home) -> Value {
    let mut store = home.store();
    let sql = format!(
        "SELECT result FROM {} WHERE kind = 'classify' ORDER BY id DESC LIMIT 1",
        store.qualified("job")
    );
    let rows = store.query(&sql, &[]).unwrap();
    let text = rows[0].opt_text(0).unwrap().unwrap_or("{}").to_string();
    serde_json::from_str(&text).unwrap()
}

fn by_name<'a>(read: &'a Value, dataset: &str) -> &'a Value {
    read["datasets"]
        .as_array()
        .and_then(|d| d.iter().find(|x| x["dataset"] == dataset))
        .unwrap_or_else(|| panic!("{dataset} not read: {read}"))
}

#[test]
fn the_signs_flag_a_dataset_at_ingest_and_a_persons_word_wins_on_sqlite() {
    flow(None);
}

#[test]
fn the_signs_flag_a_dataset_at_ingest_and_a_persons_word_wins_on_postgres() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    let schema = "nils_damaged_headers";
    let drop = || {
        let mut store = nils_registry::Store::connect_postgres(&dsn, schema).expect("connect");
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

fn flow(pg: Option<(String, String)>) {
    let home = Home {
        dir: TempDir::new("dh-home"),
        pg,
    };
    let (good, _, err) = run(
        &home,
        &["key", "add", "k"],
        Some("a damaged headers test key\n"),
    );
    assert!(good, "{err}");
    match &home.pg {
        Some((dsn, schema)) => {
            ok(
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
            );
        }
        None => {
            ok(&home, &["init", "--key", "k"]);
        }
    }

    // names the rules read, by a maker they were built on
    let _usual = dataset(
        &home,
        "usual",
        "P-1",
        "1.2.826.0.1.3680043.8.497.31.1",
        &[
            (Some("t1_mprage_sag"), "SIEMENS"),
            (Some("AX FLAIR"), "SIEMENS"),
        ],
    );
    // one of two names in words the pack does not know
    let _foreign = dataset(
        &home,
        "foreign",
        "P-2",
        "1.2.826.0.1.3680043.8.497.31.2",
        &[
            (Some("zzqx_qqvv"), "SIEMENS"),
            (Some("t1_mprage_sag"), "SIEMENS"),
        ],
    );
    // one of two scans with neither a description nor a protocol name
    let _stripped = dataset(
        &home,
        "stripped",
        "P-3",
        "1.2.826.0.1.3680043.8.497.31.3",
        &[(None, "SIEMENS"), (Some("t1_mprage_sag"), "SIEMENS")],
    );
    // a maker the rules were not built on, and no dataset before it holds
    let _maker = dataset(
        &home,
        "maker",
        "P-4",
        "1.2.826.0.1.3680043.8.497.31.4",
        &[(Some("t1_mprage_sag"), "ZETA IMAGING")],
    );
    ok(&home, &["fingerprint"]);

    // before any sort since the signs are kept, they are read when asked
    let server = Served::start(&home, &packs());
    let asked = server.headers("foreign");
    assert_eq!(asked["damaged"], true, "{asked}");
    assert_eq!(asked["by"], "signs", "{asked}");
    assert!(
        asked["signs"]["read"].as_str().unwrap().starts_with("now"),
        "{asked}"
    );
    drop(server);

    let (_, said) = ok(
        &home,
        &["classify", "--pack-dir", packs().to_str().unwrap()],
    );
    assert!(
        said.contains("headers: 4 datasets read, 3 flagged as damaged"),
        "{said}"
    );
    let read = last_sort(&home)["headers"].clone();
    assert_eq!(by_name(&read, "usual")["damaged"], false, "{read}");
    assert_eq!(by_name(&read, "foreign")["damaged"], true, "{read}");
    assert_eq!(by_name(&read, "stripped")["damaged"], true, "{read}");
    assert_eq!(by_name(&read, "maker")["damaged"], true, "{read}");
    assert_eq!(read["failed"], json!([]), "{read}");

    let server = Served::start(&home, &packs());
    let usual = server.headers("usual");
    assert_eq!(usual["damaged"], false, "{usual}");
    assert_eq!(
        usual["signs"]["why"],
        "none of its signs reaches its threshold"
    );
    assert_eq!(usual["signs"]["read"], "at the end of a sort", "{usual}");
    assert_eq!(usual["signs"]["thresholds"], "pack", "{usual}");
    assert!(
        usual["signs"]["pack"].as_str().unwrap().starts_with("mri@"),
        "{usual}"
    );
    assert_eq!(usual["signs"]["unknown_words"]["named"], 2, "{usual}");
    assert_eq!(usual["signs"]["unknown_words"]["unread"], 0, "{usual}");

    let foreign = server.headers("foreign");
    let words = &foreign["signs"]["unknown_words"];
    assert_eq!(
        (words["named"].clone(), words["unread"].clone()),
        (json!(2), json!(1)),
        "{foreign}"
    );
    assert_eq!(words["share"], 0.5, "{foreign}");
    assert_eq!(words["flags"], true, "{foreign}");
    assert_eq!(foreign["signs"]["nameless"]["flags"], false, "{foreign}");
    assert!(
        foreign["signs"]["why"]
            .as_str()
            .unwrap()
            .starts_with("50 % of its named MR scans carry no word the rules know (flags at "),
        "{foreign}"
    );

    let stripped = server.headers("stripped");
    let nameless = &stripped["signs"]["nameless"];
    assert_eq!(
        (nameless["scans"].clone(), nameless["flags"].clone()),
        (json!(1), json!(true)),
        "{stripped}"
    );
    // the older readers' fields stay
    assert_eq!(stripped["signs"]["no_names"], 1, "{stripped}");
    assert_eq!(stripped["signs"]["share"], 0.5, "{stripped}");
    assert_eq!(stripped["signs"]["unknown_words"]["named"], 1, "{stripped}");

    let maker = server.headers("maker");
    let unseen = &maker["signs"]["unseen_maker"];
    assert_eq!(unseen["flags"], true, "{maker}");
    assert_eq!(unseen["makers"], json!({"ZETA IMAGING": 1}), "{maker}");
    assert!(
        maker["signs"]["why"].as_str().unwrap().contains(
            "by a maker the rules were not built on and no dataset brought in before it holds: ZETA IMAGING"
        ),
        "{maker}"
    );
    drop(server);

    // a second dataset of that maker: the registry saw it before this one
    // came in, and the first keeps its sign, though this sort, which judged
    // every dataset's scans again, read the first again too
    let _again = dataset(
        &home,
        "maker-again",
        "P-5",
        "1.2.826.0.1.3680043.8.497.31.5",
        &[(Some("t1_mprage_sag"), "ZETA IMAGING")],
    );
    ok(&home, &["fingerprint"]);
    let (_, said) = ok(
        &home,
        &["classify", "--pack-dir", packs().to_str().unwrap()],
    );
    let read = last_sort(&home)["headers"].clone();
    assert_eq!(by_name(&read, "maker-again")["damaged"], false, "{read}");
    assert!(said.contains("flagged as damaged"), "{said}");
    let server = Served::start(&home, &packs());
    let again = server.headers("maker-again");
    assert_eq!(again["signs"]["unseen_maker"]["scans"], 0, "{again}");
    let first = server.headers("maker");
    assert_eq!(
        first["damaged"], true,
        "the first of a new maker keeps its sign: {first}"
    );
    assert_eq!(first["signs"]["unseen_maker"]["flags"], true, "{first}");

    // a person's word wins, with a why, and the signs stay under it
    let (status, refused) = server.call(
        "PUT",
        "/api/datasets/foreign/headers",
        Some(json!({"damaged": false})),
    );
    assert_eq!(status, 400, "{refused}");
    let (status, set) = server.call(
        "PUT",
        "/api/datasets/foreign/headers",
        Some(json!({"damaged": false, "why": "our own site's research names"})),
    );
    assert_eq!(status, 200, "{set}");
    assert_eq!(set["damaged"], false, "{set}");
    assert_eq!(set["by"], "person", "{set}");
    assert_eq!(set["flag"]["why"], "our own site's research names", "{set}");
    assert_eq!(
        set["signs"]["damaged"], true,
        "the signs still say it: {set}"
    );
    drop(server);

    // a re-sort reads the signs again and keeps them, under the person's word
    let (_, said) = ok(
        &home,
        &[
            "classify",
            "--pack-dir",
            packs().to_str().unwrap(),
            "--scope",
            "dataset:foreign",
        ],
    );
    assert!(
        said.contains("headers: 1 dataset read, 0 flagged"),
        "{said}"
    );
    let read = last_sort(&home)["headers"].clone();
    assert_eq!(by_name(&read, "foreign")["by"], "person", "{read}");
    assert_eq!(by_name(&read, "foreign")["signs"], true, "{read}");
    let server = Served::start(&home, &packs());
    let kept = server.headers("foreign");
    assert_eq!(kept["damaged"], false, "{kept}");
    assert_eq!(kept["by"], "person", "{kept}");
    // the person lets the signs decide again
    let (status, back) = server.call(
        "PUT",
        "/api/datasets/foreign/headers",
        Some(json!({"damaged": null})),
    );
    assert_eq!(status, 200, "{back}");
    assert_eq!(back["damaged"], true, "{back}");
    assert_eq!(back["by"], "signs", "{back}");
    assert_eq!(back["flag"], Value::Null, "{back}");
    drop(server);

    // a pack before contract 11 states no thresholds: the nameless share
    // flags at a half, as before, and words and makers flag nothing
    let older = TempDir::new("dh-older-pack");
    copy_dir(&packs().join("mri"), &older.path().join("mri"));
    let manifest = std::fs::read_to_string(older.path().join("mri/pack.yml")).unwrap();
    let manifest: String = manifest
        .lines()
        .filter(|l| !l.starts_with("  damaged_headers:"))
        .map(|l| {
            if l.starts_with("contract:") {
                "contract: 10\n".to_string()
            } else if l.starts_with("version:") {
                "version: 1.2.3\n".to_string()
            } else {
                format!("{l}\n")
            }
        })
        .collect();
    std::fs::write(older.path().join("mri/pack.yml"), manifest).unwrap();
    ok(
        &home,
        &[
            "classify",
            "--pack-dir",
            older.path().to_str().unwrap(),
            "--scope",
            "dataset:foreign,stripped,maker",
        ],
    );
    let server = Served::start(&home, older.path());
    let foreign = server.headers("foreign");
    assert_eq!(foreign["damaged"], false, "{foreign}");
    assert_eq!(foreign["signs"]["thresholds"], "engine", "{foreign}");
    assert_eq!(
        foreign["signs"]["unknown_words"]["threshold"],
        Value::Null,
        "{foreign}"
    );
    assert_eq!(
        foreign["signs"]["unknown_words"]["unread"], 1,
        "read and shown: {foreign}"
    );
    assert_eq!(
        server.headers("stripped")["damaged"],
        true,
        "half its scans are nameless"
    );
    assert_eq!(
        server.headers("maker")["damaged"],
        false,
        "no threshold for a maker"
    );
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}
