// SPDX-License-Identifier: AGPL-3.0-only
//! Review round 1, end to end (record 56 §6.3; the design's slices ER1 to
//! ER3): two datasets brought in through the worker, each with scans the
//! rules find no base for. `GET /api/review/questions` gathers the sort's
//! waiting questions of a dataset into one plain question with its
//! distinct scans and the two answers its keys give; `GET
//! /api/review/questions/{key}/scans` pages its scans, each with its
//! picture, NILS's answer and the rules' reason; and `POST
//! /api/review/questions/{key}/answer` answers the scans a person saw with
//! one value and an exception in one transaction, closes their questions
//! and re-sorts exactly those scans at once, leaving the scan left out, the
//! other dataset and every other stack as the sort wrote them. A blind-first
//! question is shown without NILS's answers and its answer is marked blind;
//! a stale page, a value the axis does not have and an exception outside the
//! answer are refused with nothing written. On SQLite always, and on
//! Postgres where a test DSN is set.

use std::io::Write as _;
use std::process::{Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_registry::Store;
use nils_registry::store::Cell;
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

    /// The rows of a query over the registry, `{table}` named as the store
    /// qualifies it.
    fn rows(&self, sql: &str) -> Vec<Vec<Value>> {
        let mut store = self.store();
        let mut text = sql.to_string();
        for t in [
            "classification_axis",
            "classification",
            "stack_fingerprint",
            "review_item",
            "review_member",
            "decision",
            "job",
        ] {
            text = text.replace(&format!("{{{t}}}"), &store.qualified(t));
        }
        store
            .query(&text, &[])
            .unwrap()
            .iter()
            .map(|r| {
                r.0.iter()
                    .map(|c| match c {
                        Cell::Int(n) => json!(n),
                        Cell::Bool(b) => json!(i64::from(*b)),
                        Cell::Double(d) => json!(d),
                        Cell::Text(t) => json!(t),
                        Cell::Null | Cell::Bytes(_) => Value::Null,
                    })
                    .collect()
            })
            .collect()
    }
}

const OPS: &str = "an-operator-token-of-length";
/// Review work at detail plain, and no pipelines work: a pick run is not
/// theirs to queue, and pictures, which are pixels, are not shown them.
const REVIEWS: &str = "a-reviewer-token-of-length";
/// Reading data, and no review grant.
const READS: &str = "a-reader-token-of-its-length";
const ADMIN: &str = "an-administrator-token-long";

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
            format!("{REVIEWS}=rae@lab:review:see,review:work"),
            format!("{READS}=lou@lab:reader"),
            format!("{ADMIN}=adm@lab:admin"),
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
            .args(["--auth", "token", "--pack-dir", &packs()])
            .env("NILS_TOKENS", tokens)
            .env("NILS_PACK_DIR", packs())
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_DSN")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
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

    /// Every job over, with at least `sorts` sorts and `pyramids` picture
    /// builds among them, the last step a bring-in queues: two minutes at
    /// most.
    fn settled(&self, sorts: usize, pyramids: usize) -> Vec<Value> {
        let mut last = Value::Null;
        for _ in 0..1200 {
            let doc = self.get("/api/jobs?all=1&limit=200", OPS);
            last = doc.clone();
            let jobs = doc["jobs"].as_array().unwrap().clone();
            let over = jobs
                .iter()
                // the worker's own rows run as long as it does
                .filter(|j| !j["kind"].as_str().unwrap_or_default().ends_with("worker"))
                .all(|j| matches!(j["state"].as_str(), Some("done" | "failed" | "cancelled")));
            let of = |k: &str| jobs.iter().filter(|j| j["kind"] == k).count();
            if over && of("classify") >= sorts && of("pyramid") >= pyramids {
                return jobs;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let seen: Vec<String> = last["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|j| format!("{} {} {}", j["kind"], j["state"], j["error"]))
            .collect();
        panic!("the jobs did not settle: {seen:?}");
    }
}

/// One series of three images with pixels, so the sort makes its picture.
fn series(
    dir: &TempDir,
    at: &str,
    patient: &str,
    study: &str,
    n: usize,
    description: &str,
    extra: &[(dicom_core::Tag, VR, &str)],
) {
    let series = format!("{study}.{n}");
    for instance in 1..=3u32 {
        let sop = format!("{series}.{instance}");
        let mut e = synth::minimal_mr(study, &series, &sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
        e.push(synth::text(tags::STUDY_DATE, VR::DA, "20240131"));
        e.push(synth::text(tags::SERIES_NUMBER, VR::IS, &n.to_string()));
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
        e.push(synth::text(tags::PIXEL_SPACING, VR::DS, "16\\16"));
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
        for (tag, vr, value) in extra {
            e.push(synth::text(*tag, *vr, value));
        }
        let px: Vec<u8> = (0..256u32)
            .flat_map(|i| (((i * 7 + instance * 101 + n as u32 * 13) % 4096) as u16).to_le_bytes())
            .collect();
        e.push(synth::bytes(tags::PIXEL_DATA, VR::OW, px));
        dir.file(
            &format!("{at}/s{n}/IM_{instance:04}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
}

/// The stack a series description names.
fn stack_named(home: &Home, description: &str) -> i64 {
    home.rows(&format!(
        "SELECT stack_id FROM {{stack_fingerprint}} WHERE text_series_description = '{description}'"
    ))[0][0]
        .as_i64()
        .unwrap()
}

/// The axis's value and tier on a stack, as stored.
fn base_of(home: &Home, stack: i64) -> Option<(String, String)> {
    home.rows(&format!(
        "SELECT COALESCE(value, ''), tier FROM {{classification_axis}} WHERE stack_id = {stack} AND axis = 'base'"
    ))
    .first()
    .map(|r| {
        (
            r[0].as_str().unwrap_or_default().to_string(),
            r[1].as_str().unwrap_or_default().to_string(),
        )
    })
}

/// The job that wrote a stack's classification.
fn sorted_by(home: &Home, stack: i64) -> i64 {
    home.rows(&format!(
        "SELECT job_id FROM {{classification}} WHERE stack_id = {stack}"
    ))[0][0]
        .as_i64()
        .unwrap()
}

fn key(text: &str) -> String {
    text.replace(':', "%3A")
        .replace('|', "%7C")
        .replace('?', "%3F")
}

fn round(pg: Option<(String, String)>) {
    let home = Home {
        dir: TempDir::new("review-questions-home"),
        pg,
    };
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a review questions test key\n"));
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
    // a dataset of two subjects: a T1 the rules know, and three scans no
    // rule weights, whose base is missing
    let mprage: &[(dicom_core::Tag, VR, &str)] = &[
        (tags::SCANNING_SEQUENCE, VR::CS, "GR"),
        (tags::SEQUENCE_VARIANT, VR::CS, "SK\\SP\\MP"),
        (tags::SEQUENCE_NAME, VR::SH, "*tfl3d1_16"),
        (tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
    ];
    let ds = TempDir::new("review-questions-ds");
    series(
        &ds,
        "sub-0",
        "S-ONE",
        "1.2.826.0.1.3680043.8.499.1.1",
        1,
        "t1 mprage",
        mprage,
    );
    series(
        &ds,
        "sub-0",
        "S-ONE",
        "1.2.826.0.1.3680043.8.499.1.1",
        2,
        "zzqx a",
        &[],
    );
    series(
        &ds,
        "sub-0",
        "S-ONE",
        "1.2.826.0.1.3680043.8.499.1.1",
        3,
        "zzqx b",
        &[],
    );
    series(
        &ds,
        "sub-1",
        "S-TWO",
        "1.2.826.0.1.3680043.8.499.2.1",
        1,
        "zzqx c",
        &[],
    );
    home.ok(&[
        "place",
        "add",
        "ds",
        ds.path().to_str().unwrap(),
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
    // another dataset, with one such scan of its own
    let other = TempDir::new("review-questions-other");
    series(
        &other,
        "sub-2",
        "S-THREE",
        "1.2.826.0.1.3680043.8.499.3.1",
        1,
        "zzqx d",
        &[],
    );
    home.ok(&[
        "place",
        "add",
        "other",
        other.path().to_str().unwrap(),
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
    let work = TempDir::new("review-questions-work");
    home.ok(&[
        "place",
        "add",
        "work",
        work.path().to_str().unwrap(),
        "--role",
        "working",
    ]);

    let server = Worked::start(&home);
    // a share of none drawn blind first, until the blind check below
    let (status, said) = server.call(
        "PUT",
        "/api/settings",
        Some(json!({"review": {"blind_share": 0}})),
        ADMIN,
    );
    assert_eq!(status, 200, "{said}");
    for (n, (name, dataset)) in [("first", "ds"), ("second", "other")].iter().enumerate() {
        let (status, queued) = server.call(
            "POST",
            "/api/jobs",
            Some(json!({"command": ["bring-in", format!("@{dataset}"), "--name", name]})),
            OPS,
        );
        assert_eq!(status, 202, "{queued}");
        let jobs = server.settled(n + 1, n + 1);
        for j in jobs.iter().filter(|j| {
            ["digest", "fingerprint", "classify"].contains(&j["kind"].as_str().unwrap_or_default())
        }) {
            assert_eq!(j["state"], "done", "{j}");
        }
    }
    let (t1, a, b, c, d) = (
        stack_named(&home, "t1 mprage"),
        stack_named(&home, "zzqx a"),
        stack_named(&home, "zzqx b"),
        stack_named(&home, "zzqx c"),
        stack_named(&home, "zzqx d"),
    );
    assert_eq!(
        base_of(&home, t1).unwrap().0,
        "T1w",
        "the rules know the T1"
    );
    for s in [a, b, c, d] {
        assert!(
            base_of(&home, s).is_none_or(|(v, _)| v.is_empty()),
            "no rule weights stack {s}"
        );
    }

    // --- ER1: the questions, in plain words, with distinct scans
    let (status, _) = server.call("GET", "/api/review/questions?dataset=ds", None, READS);
    assert_eq!(status, 403, "the questions are Review's");
    let list = server.get("/api/review/questions?dataset=ds", REVIEWS);
    assert_eq!(list["scope"]["kind"], "dataset", "{list}");
    assert_eq!(list["scans"], 3, "{list}");
    let questions = list["questions"].as_array().unwrap();
    assert_eq!(questions.len(), 1, "{list}");
    let q = &questions[0];
    assert_eq!(q["key"], "base:missing", "{q}");
    assert_eq!(q["axis"], "base", "{q}");
    assert_eq!(q["shape"], "missing", "{q}");
    assert_eq!(q["scans"], 3, "{q}");
    assert_eq!(q["staged"], 0, "{q}");
    assert_eq!(q["words"], "Which base?", "{q}");
    assert_eq!(
        q["reason"], "The rules found no base for these scans.",
        "{q}"
    );
    assert_eq!(q["kinds"]["base:missing"], 3, "{q}");
    let values: Vec<String> = q["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(values.len(), 2, "{q}");
    // the value the dataset holds most often is the first key
    assert_eq!(values[0], "T1w", "{q}");
    let vocabulary = list["vocabularies"]["base"].as_array().unwrap();
    assert!(
        vocabulary.iter().any(|v| v["value"] == "T2w")
            && vocabulary
                .iter()
                .any(|v| v["value"] == "T2*w" && v["name"] == "T2starw"),
        "the stored form, its name and its label: {vocabulary:?}"
    );
    assert_eq!(list["elsewhere"], json!([]), "{list}");
    assert_eq!(list["held"]["scans"], 0, "{list}");
    // the other dataset's, and the registry's
    let theirs = server.get("/api/review/questions?dataset=other", REVIEWS);
    assert_eq!(theirs["questions"][0]["scans"], 1, "{theirs}");
    let all = server.get("/api/review/questions", REVIEWS);
    assert_eq!(all["scope"]["kind"], "registry", "{all}");
    assert_eq!(all["questions"][0]["scans"], 4, "{all}");
    // a scan the rules know asks nothing; one they do not asks this
    let none = server.get(
        &format!("/api/review/questions?dataset=ds&stack={t1}"),
        REVIEWS,
    );
    assert_eq!(none["questions"], json!([]), "{none}");
    let one = server.get(
        &format!("/api/review/questions?dataset=ds&stack={a}"),
        REVIEWS,
    );
    assert_eq!(one["questions"][0]["key"], "base:missing", "{one}");
    let (status, _) = server.call(
        "GET",
        "/api/review/questions?dataset=ds&cohort=x",
        None,
        REVIEWS,
    );
    assert_eq!(status, 400);
    let (status, _) = server.call(
        "GET",
        "/api/review/questions?dataset=nowhere",
        None,
        REVIEWS,
    );
    assert_eq!(status, 404);
    // the one count, the Data card's need a look, says the same
    let summary = server.get("/api/datasets/ds/summary", OPS);
    assert_eq!(summary["need_a_look"], 3, "{summary}");

    // --- ER2: a page of the question's scans
    let path = format!(
        "/api/review/questions/{}/scans?dataset=ds",
        key("base:missing")
    );
    let page = server.get(&path, OPS);
    assert_eq!(page["total"], 3, "{page}");
    assert_eq!(page["pages"], 1, "{page}");
    assert_eq!(page["words"], "Which base?", "{page}");
    assert_eq!(page["values"], q["values"], "{page}");
    assert!(!page["vocabulary"].as_array().unwrap().is_empty(), "{page}");
    assert_eq!(page["system1"]["status"], "no_model", "{page}");
    let tiles = page["scans"].as_array().unwrap();
    let mut shown: Vec<i64> = tiles.iter().map(|t| t["stack"].as_i64().unwrap()).collect();
    shown.sort_unstable();
    let mut three = vec![a, b, c];
    three.sort_unstable();
    assert_eq!(shown, three, "{page}");
    for t in tiles {
        assert_eq!(t["status"], "open", "{t}");
        assert_eq!(t["mode"], "routine", "{t}");
        assert_eq!(t["blind"], false, "{t}");
        assert!(t["nils"].is_null(), "no base yet: {t}");
        assert!(t["other"].is_null(), "{t}");
        assert!(t["rules"].is_object(), "the rules' reason: {t}");
        assert!(t["rules"]["line"].is_string(), "{t}");
        assert!(t["system1"].is_null(), "{t}");
        assert!(t["staged"].is_null(), "{t}");
        assert!(t["items"].as_array().is_some_and(|i| !i.is_empty()), "{t}");
        assert!(
            t["series_description"]
                .as_str()
                .unwrap()
                .starts_with("zzqx"),
            "{t}"
        );
        assert!(t["subject"]["id"].is_i64(), "{t}");
        assert!(
            t["picture"]["data"]
                .as_str()
                .is_some_and(|d| d.starts_with("data:image/jpeg")),
            "a picture inline: {t}"
        );
    }
    let small = server.get(&format!("{path}&limit=2&page=2"), OPS);
    assert_eq!(small["pages"], 2, "{small}");
    assert_eq!(small["scans"].as_array().unwrap().len(), 1, "{small}");
    let (status, _) = server.call(
        "GET",
        &format!(
            "/api/review/questions/{}/scans?dataset=ds",
            key("base:T1w|T2w")
        ),
        None,
        OPS,
    );
    assert_eq!(status, 404, "a question nothing asks here");
    let (status, _) = server.call(
        "GET",
        "/api/review/questions/nonsense/scans?dataset=ds",
        None,
        OPS,
    );
    assert_eq!(status, 400);
    // pictures are pixels: below detail quasi there are none, and the page
    // says why
    let plain = server.get(&path, REVIEWS);
    assert!(
        plain["scans"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["picture"].is_null()),
        "{plain}"
    );
    assert_eq!(plain["pictures"]["shown"], false, "{plain}");

    // --- ER3: refusals write nothing
    let answer = format!("/api/review/questions/{}/answer", key("base:missing"));
    let (status, said) = server.call(
        "POST",
        &answer,
        Some(json!({"dataset": "ds", "value": "T2w", "stacks": [a]})),
        READS,
    );
    assert_eq!(status, 403, "{said}");
    for (body, want) in [
        (
            json!({"dataset": "ds", "value": "NotABase", "stacks": [a]}),
            400,
        ),
        (json!({"dataset": "ds", "stacks": [a]}), 400),
        (
            json!({"dataset": "ds", "value": "T2w", "nothing": true, "stacks": [a]}),
            400,
        ),
        (
            json!({"dataset": "ds", "value": "T2w", "stacks": [a], "except": [{"stack": b, "value": "PDw"}]}),
            400,
        ),
        (
            json!({"dataset": "ds", "value": "T2w", "stacks": [a, t1]}),
            409,
        ),
        (json!({"dataset": "ds", "value": "T2w", "stacks": [d]}), 409),
    ] {
        let (status, said) = server.call("POST", &answer, Some(body.clone()), REVIEWS);
        assert_eq!(status, want, "{body}: {said}");
    }
    assert_eq!(
        home.rows("SELECT COUNT(*) FROM {decision}")[0][0],
        0,
        "nothing written"
    );

    // --- ER3: the page a person saw, answered T2w with one exception, PDw;
    // the third scan of the question left for later
    let jobs_before: Vec<(i64, i64)> = [t1, c, d]
        .iter()
        .map(|s| (*s, sorted_by(&home, *s)))
        .collect();
    let (status, done) = server.call(
        "POST",
        &answer,
        Some(json!({
            "dataset": "ds", "value": "T2w", "stacks": [a, b],
            "except": [{"stack": b, "value": "PDw"}],
        })),
        OPS,
    );
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["key"], "base:missing", "{done}");
    assert_eq!(done["answered"], 2, "{done}");
    assert_eq!(done["staged"], false, "{done}");
    assert_eq!(done["left"], 1, "the third waits: {done}");
    let decided: Vec<(i64, String)> = done["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            (
                x["stack"].as_i64().unwrap(),
                x["value"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(decided.contains(&(a, "T2w".to_string())), "{done}");
    assert!(decided.contains(&(b, "PDw".to_string())), "{done}");
    // one decision a scan, at the stack's scope, a person's, in force
    let rows = home.rows(
        "SELECT scope, ref, value, author_kind, actor, rules_in_view FROM {decision} WHERE withdrawn_at IS NULL ORDER BY id",
    );
    assert_eq!(rows.len(), 2, "{rows:?}");
    for r in &rows {
        assert_eq!(r[0], "stack", "{rows:?}");
        assert_eq!(r[3], "person", "{rows:?}");
        assert_eq!(r[4], "ops@lab", "{rows:?}");
        // the rules' answer the person saw: none, which a sort compares with
        assert_eq!(r[5], "", "{rows:?}");
    }
    // re-sorted at once: their base is the person's, by a classify job over
    // those two stacks alone
    let resort = &done["resort"];
    assert_eq!(resort["state"], "done", "{done}");
    assert_eq!(resort["stacks"], 2, "{done}");
    let job = resort["job"].as_i64().unwrap();
    assert_eq!(
        base_of(&home, a).unwrap(),
        ("T2w".to_string(), "decision".to_string())
    );
    assert_eq!(
        base_of(&home, b).unwrap(),
        ("PDw".to_string(), "decision".to_string())
    );
    assert_eq!(sorted_by(&home, a), job);
    assert_eq!(sorted_by(&home, b), job);
    for (s, was) in &jobs_before {
        assert_eq!(
            sorted_by(&home, *s),
            *was,
            "stack {s} is its own sort's still"
        );
    }
    let kind = home.rows(&format!(
        "SELECT kind, state, CAST(args AS TEXT) FROM {{job}} WHERE id = {job}"
    ));
    assert_eq!(kind[0][0], "classify", "{kind:?}");
    assert_eq!(kind[0][1], "done", "{kind:?}");
    let args: Value = serde_json::from_str(kind[0][2].as_str().unwrap()).unwrap();
    assert_eq!(args["scope"].as_array().map(Vec::len), Some(1), "{args}");
    assert!(
        args["scope"][0].as_str().unwrap().ends_with("2 scans"),
        "{args}"
    );
    assert!(
        base_of(&home, c).is_none_or(|(v, _)| v.is_empty()),
        "the third is not decided"
    );
    // the pick run a sort is followed by, queued for their subject
    assert!(done["picks"]["job"].is_i64(), "{done}");

    // the question asks the third scan alone now, everywhere
    let list = server.get("/api/review/questions?dataset=ds", REVIEWS);
    assert_eq!(list["questions"][0]["scans"], 1, "{list}");
    assert_eq!(list["scans"], 1, "{list}");
    let summary = server.get("/api/datasets/ds/summary", OPS);
    assert_eq!(summary["need_a_look"], 1, "{summary}");
    let theirs = server.get("/api/review/questions?dataset=other", REVIEWS);
    assert_eq!(
        theirs["questions"][0]["scans"], 1,
        "the other dataset's waits as it did: {theirs}"
    );
    // answered again from a stale page: refused, nothing written
    let (status, said) = server.call(
        "POST",
        &answer,
        Some(json!({"dataset": "ds", "value": "T1w", "stacks": [a]})),
        OPS,
    );
    assert_eq!(status, 409, "{said}");
    assert_eq!(
        home.rows("SELECT COUNT(*) FROM {decision} WHERE withdrawn_at IS NULL")[0][0],
        2
    );

    // a reviewer answers the rest, nothing on the axis; no pick run is theirs
    let (status, done) = server.call(
        "POST",
        &answer,
        Some(json!({"dataset": "ds", "nothing": true})),
        REVIEWS,
    );
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["answered"], 1, "{done}");
    assert_eq!(done["left"], 0, "{done}");
    assert!(done["picks"]["why"].is_string(), "{done}");
    assert_eq!(
        base_of(&home, c).unwrap().1,
        "decision",
        "no value, by a person"
    );
    let list = server.get("/api/review/questions?dataset=ds", REVIEWS);
    assert_eq!(list["questions"], json!([]), "{list}");
    assert_eq!(list["scans"], 0, "{list}");

    // --- blind first: every question drawn blind shows none of NILS's
    // answers, and its answer, given before any showed, is marked blind
    let (status, said) = server.call(
        "PUT",
        "/api/settings",
        Some(json!({"review": {"blind_share": 1}})),
        ADMIN,
    );
    assert_eq!(status, 200, "{said}");
    // the scan of the other dataset waits on what is left of the one
    // grouped question the second sort asked of all four, whose mode was
    // drawn routine when ds's page was first read and kept through the
    // split; it is drawn again here, as a question first read now is
    {
        let mut store = home.store();
        let t = store.qualified("review_item");
        store
            .execute(
                &format!(
                    "UPDATE {t} SET mode = NULL WHERE status = 'open' AND kind = 'base:missing'"
                ),
                &[],
            )
            .unwrap();
    }
    let path = format!(
        "/api/review/questions/{}/scans?dataset=other",
        key("base:missing")
    );
    let page = server.get(&path, OPS);
    let tile = &page["scans"][0];
    assert_eq!(tile["stack"], d, "{page}");
    assert_eq!(tile["mode"], "blind_first", "{tile}");
    assert_eq!(tile["blind"], true, "{tile}");
    for k in ["nils", "rules", "system1", "staged", "name", "axes"] {
        assert!(tile[k].is_null(), "{k}: {tile}");
    }
    assert!(
        tile["series_description"].is_string(),
        "the header text a reader needs stays: {tile}"
    );
    let (status, done) = server.call(
        "POST",
        &format!("/api/review/questions/{}/answer", key("base:missing")),
        // a value named by the pack's name for it, stored as the axis stores it
        Some(json!({"dataset": "other", "value": "T2starw"})),
        OPS,
    );
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["blind"], 1, "{done}");
    assert_eq!(done["decisions"][0]["value"], "T2*w", "{done}");
    let marked = home.rows(&format!(
        "SELECT blind, mode, value, rules_in_view FROM {{decision}} WHERE ref = '{d}' AND withdrawn_at IS NULL"
    ));
    assert!(
        marked[0][3].is_null(),
        "the rules' answer was hidden: {marked:?}"
    );
    assert_eq!(marked[0][0], 1, "{marked:?}");
    assert_eq!(marked[0][1], "blind_first", "{marked:?}");
    assert_eq!(marked[0][2], "T2*w", "{marked:?}");

    // the doors are listed, each with its grant
    let caps = server.get("/api/capabilities", REVIEWS);
    for door in [
        "GET /api/review/questions",
        "GET /api/review/questions/{key}/scans",
        "POST /api/review/questions/{key}/answer",
    ] {
        assert!(
            caps["doors"].as_array().unwrap().iter().any(|x| x == door),
            "{door}"
        );
        let row = caps["policy"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["door"] == door)
            .unwrap_or_else(|| panic!("{door} has no policy row"));
        let want = if door.starts_with("POST") {
            "review:work"
        } else {
            "review:see"
        };
        assert_eq!(row["grant"], want, "{row}");
    }
    server.settled(0, 0);
    drop(server);
    drop(work);
    drop(other);
    drop(ds);
}

#[test]
fn a_question_is_answered_as_a_grid_and_only_its_scans_are_re_sorted() {
    round(None);
}

#[test]
fn a_question_is_answered_as_a_grid_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    let schema = "nils_review_questions";
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
