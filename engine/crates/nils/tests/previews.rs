// SPDX-License-Identifier: AGPL-3.0-only
//! Record 55 H2 (E1, E2, E4): the preview a sort makes and the doors that
//! serve it. A classify run makes the previews of the stacks it judged
//! under the working place; `nils preview build` makes them for stacks
//! already sorted and makes none twice; the preview door answers the first
//! picture in one request from one file, gated as every picture is, with
//! an ETag and immutable caching when the digest is named; the frames door
//! answers a range of planes in one read; a page of scans carries its
//! pictures and its open questions; a sort queues its pyramids on the
//! pictures lane; and a worker beside the doors starts a queued job at
//! once.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn run(home: &TempDir, args: &[&str]) -> (bool, String, String) {
    let out = nils()
        .arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "anna")
        .env("HOSTNAME", "ward-3")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn ok(home: &TempDir, args: &[&str]) -> (String, String) {
    let (good, out, err) = run(home, args);
    assert!(good, "nils {args:?} failed: {err}");
    (out, err)
}

fn key(home: &TempDir) {
    let mut child = nils()
        .arg("--registry")
        .arg(home.path())
        .args(["key", "add", "k"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"a previews test key\n")
        .unwrap();
    assert!(child.wait().unwrap().success());
}

fn packs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

const NZ: u32 = 24;
const NY: u32 = 192;
const NX: u32 = 160;

fn us(tag: dicom_core::Tag, v: u16) -> synth::Elem {
    synth::bytes(tag, VR::US, v.to_le_bytes().to_vec())
}

/// A series of NZ planes of NY by NX uint16, a gradient of its own a plane.
fn series(dir: &TempDir, patient: &str, study: &str, series: &str, burned_in: bool) {
    for z in 0..NZ {
        let sop = format!("{series}.{}", z + 1);
        let mut e = synth::minimal_mr(study, series, &sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
        e.push(synth::text(tags::STUDY_DATE, VR::DA, "20260102"));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
        e.push(synth::text(
            tags::INSTANCE_NUMBER,
            VR::IS,
            &(z + 1).to_string(),
        ));
        e.push(synth::text(
            tags::IMAGE_POSITION_PATIENT,
            VR::DS,
            &format!("0\\0\\{}", z as f64 * 2.0),
        ));
        e.push(synth::text(
            tags::IMAGE_ORIENTATION_PATIENT,
            VR::DS,
            "1\\0\\0\\0\\1\\0",
        ));
        e.push(synth::text(tags::PIXEL_SPACING, VR::DS, "0.5\\0.5"));
        e.push(synth::text(tags::SLICE_THICKNESS, VR::DS, "2"));
        e.push(us(tags::SAMPLES_PER_PIXEL, 1));
        e.push(synth::text(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            "MONOCHROME2",
        ));
        e.push(us(tags::ROWS, NY as u16));
        e.push(us(tags::COLUMNS, NX as u16));
        e.push(us(tags::BITS_ALLOCATED, 16));
        e.push(us(tags::BITS_STORED, 16));
        e.push(us(tags::HIGH_BIT, 15));
        e.push(us(tags::PIXEL_REPRESENTATION, 0));
        if burned_in {
            e.push(synth::text(tags::BURNED_IN_ANNOTATION, VR::CS, "YES"));
        }
        let mut px = Vec::with_capacity((NY * NX * 2) as usize);
        for y in 0..NY {
            for x in 0..NX {
                let v: u16 = ((x * 7 + y * 3 + z * 101) % 4096) as u16;
                px.extend_from_slice(&v.to_le_bytes());
            }
        }
        e.push(synth::bytes(tags::PIXEL_DATA, VR::OW, px));
        dir.file(
            &format!("{study}/{sop}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
}

/// A registry with a dataset of two stacks with pixels, one carrying
/// burned-in annotation, a working place, and the dataset digested and
/// fingerprinted but not yet classified. On Postgres with a DSN.
struct Bed {
    home: TempDir,
    _src: TempDir,
    work: TempDir,
}

fn bed(name: &str, dsn: Option<&str>) -> Bed {
    let home = TempDir::new(&format!("previews-{name}-home"));
    let src = TempDir::new(&format!("previews-{name}-src"));
    let work = TempDir::new(&format!("previews-{name}-work"));
    series(&src, "P1", "1.2.3.A", "1.2.3.A.1", false);
    series(&src, "P2", "1.2.3.B", "1.2.3.B.1", true);
    key(&home);
    match dsn {
        Some(dsn) => {
            drop_schema(dsn, name);
            ok(
                &home,
                &[
                    "init",
                    "--backend",
                    "postgres",
                    "--dsn",
                    dsn,
                    "--schema",
                    &schema(name),
                    "--key",
                    "k",
                ],
            );
        }
        None => {
            ok(&home, &["init", "--key", "k"]);
        }
    }
    let tree = src.path().to_str().unwrap();
    // Wave 7a: the studies are anonymised, so they go into dcm-anon, whose
    // PatientID holds the patient id, and the subject code generator makes
    // each subject's code from it
    ok(
        &home,
        &[
            "place",
            "add",
            "incoming",
            tree,
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
        &home,
        &[
            "place",
            "add",
            "work",
            work.path().to_str().unwrap(),
            "--role",
            "working",
            "--fast",
        ],
    );
    ok(
        &home,
        &["digest", "--name", "a", "--no-private", "@incoming"],
    );
    ok(&home, &["fingerprint"]);
    Bed {
        home,
        _src: src,
        work,
    }
}

fn schema(name: &str) -> String {
    format!("nils_previews_{name}")
}

fn drop_schema(dsn: &str, name: &str) {
    let mut store = nils_registry::Store::connect_postgres(dsn, &schema(name)).unwrap();
    store
        .batch(&format!(
            "DROP SCHEMA IF EXISTS {s} CASCADE; DROP SCHEMA IF EXISTS {s}_linkage CASCADE",
            s = schema(name)
        ))
        .unwrap();
}

fn preview_file(work: &Path, stack: i64, held: bool) -> PathBuf {
    work.join("previews")
        .join(format!("{:03}", stack % 1000))
        .join(format!(
            "{stack}{}.preview",
            if held { ".held" } else { "" }
        ))
}

/// Classify, asking about every axis, so the stacks carry open questions.
fn classify(home: &TempDir) -> String {
    let (_, err) = ok(
        home,
        &[
            "classify",
            "--review-below",
            "1.0",
            "--pack-dir",
            packs().to_str().unwrap(),
        ],
    );
    err
}

fn json_line(out: &str) -> serde_json::Value {
    serde_json::from_str(out.trim().lines().last().unwrap()).unwrap()
}

fn mtime(p: &Path) -> std::time::SystemTime {
    std::fs::metadata(p).unwrap().modified().unwrap()
}

#[test]
fn the_sort_makes_previews_and_a_second_build_makes_none() {
    let b = bed("idem", None);
    let work = b.work.path();
    let err = classify(&b.home);
    assert!(
        err.contains("previews: 2 made, 0 current, 0 failed"),
        "{err}"
    );
    for stack in [1, 2] {
        assert!(preview_file(work, stack, false).exists(), "stack {stack}");
    }
    // the burned-in stack, whichever it is, has its held file too
    let held: Vec<i64> = [1, 2]
        .into_iter()
        .filter(|s| preview_file(work, *s, true).exists())
        .collect();
    assert_eq!(held.len(), 1, "{held:?}");
    let before: Vec<_> = [1, 2]
        .iter()
        .map(|s| mtime(&preview_file(work, *s, false)))
        .collect();

    // idempotent: made from the same files, nothing is made again
    let (out, _) = ok(&b.home, &["preview", "build", "--all"]);
    let doc = json_line(&out);
    assert_eq!(doc["built"], 0, "{doc}");
    assert_eq!(doc["current"], 2, "{doc}");
    assert_eq!(doc["failed"], 0, "{doc}");
    let after: Vec<_> = [1, 2]
        .iter()
        .map(|s| mtime(&preview_file(work, *s, false)))
        .collect();
    assert_eq!(before, after, "no preview was written again");
    let (out, _) = ok(&b.home, &["preview", "build", "--stack", "1"]);
    assert_eq!(json_line(&out)["current"], true, "{out}");

    // with --force it is made again, from the same files to the same digest
    let digest = json_line(&out)["digest"].clone();
    let (out, _) = ok(&b.home, &["preview", "build", "--stack", "1", "--force"]);
    let doc = json_line(&out);
    assert_eq!(doc["built"], true, "{doc}");
    assert_eq!(doc["digest"], digest, "{doc}");

    // resumable: a preview that went is made, the rest are current
    std::fs::remove_file(preview_file(work, 2, false)).unwrap();
    let (out, _) = ok(&b.home, &["preview", "build", "--dataset", "incoming"]);
    let doc = json_line(&out);
    assert_eq!(
        (doc["built"].as_i64(), doc["current"].as_i64()),
        (Some(1), Some(1)),
        "{doc}"
    );

    // a preview of a stack the registry does not hold, and a part file a
    // build that died left, go with --all
    let stray = preview_file(work, 999_999, false);
    std::fs::create_dir_all(stray.parent().unwrap()).unwrap();
    std::fs::write(&stray, b"x").unwrap();
    let (out, _) = ok(&b.home, &["preview", "build", "--all"]);
    let doc = json_line(&out);
    assert_eq!(doc["pruned"], 1, "{doc}");
    assert!(!stray.exists());

    // the run is a job of kind preview, with its counts as its result
    let (out, _) = ok(&b.home, &["jobs", "list", "--all", "--json"]);
    assert!(out.contains("\"preview\""), "{out}");

    // a classify that judges the stacks again makes no preview again
    let err = classify(&b.home);
    assert!(
        !err.contains("previews:") || err.contains("previews: 0 made"),
        "{err}"
    );

    // a stack that has no preview and is asked for it is told so
    let (good, _, err) = run(&b.home, &["preview", "build", "--stack", "77"]);
    assert!(!good);
    assert!(
        err.contains("stack 77: its preview was not made (no_files)"),
        "{err}"
    );
}

#[test]
fn a_classify_with_no_working_place_or_no_previews_makes_none() {
    let b = bed("none", None);
    let err = {
        let (_, err) = ok(
            &b.home,
            &[
                "classify",
                "--pack-dir",
                packs().to_str().unwrap(),
                "--no-previews",
            ],
        );
        err
    };
    assert!(!err.contains("previews:"), "{err}");
    assert!(!b.work.path().join("previews").exists());
    let (out, _) = ok(&b.home, &["preview", "build", "--all"]);
    assert_eq!(json_line(&out)["built"], 2, "{out}");
}

/// A server killed when the test is over: no request count to keep.
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
    fn start(home: &TempDir, extra: &[&str]) -> Server {
        let child = nils()
            .arg("--registry")
            .arg(home.path())
            .args(["serve", "--bind", "127.0.0.1:0", "--workers", "4"])
            .args(["--pack-dir", packs().to_str().unwrap()])
            .args(extra)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // held from here, so that a panic below kills it too
        let mut held = Server { child, port: 0 };
        let stdout = held.child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let first = lines.next().unwrap().unwrap();
        let addr = first.split_whitespace().nth(2).unwrap();
        held.port = addr.rsplit(':').next().unwrap().parse().unwrap();
        held
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        token: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> (u16, Vec<(String, String)>, Vec<u8>) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {token}\r\n"
        );
        for (k, v) in headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        if let Some(b) = body {
            head.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                b.len()
            ));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).unwrap();
        if let Some(b) = body {
            stream.write_all(b.as_bytes()).unwrap();
        }
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let split = response
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("a header block");
        let head = String::from_utf8_lossy(&response[..split]).to_string();
        let body = response[split + 4..].to_vec();
        let status: u16 = head
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = head
            .lines()
            .skip(1)
            .filter_map(|l| {
                l.split_once(": ")
                    .map(|(k, v)| (k.to_lowercase(), v.to_string()))
            })
            .collect();
        (status, headers, body)
    }

    fn get(&self, path: &str, token: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
        self.send("GET", path, token, &[], None)
    }

    fn json(&self, path: &str, token: &str) -> (u16, serde_json::Value) {
        let (status, _, body) = self.get(path, token);
        let text = String::from_utf8_lossy(&body).to_string();
        (
            status,
            serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)),
        )
    }
}

fn header<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

const READER: &str = "a-reader-token-of-length";
const REVIEWER: &str = "a-reviewer-token-of-len";
const ADMIN: &str = "an-admin-token-of-length";

fn tokens() -> Vec<String> {
    vec![
        "--auth".into(),
        "token".into(),
        "--token".into(),
        format!("{READER}=lou@lab:reader"),
        "--token".into(),
        format!("{REVIEWER}=rev@lab:reader,reviewer"),
        "--token".into(),
        format!("{ADMIN}=root@lab:admin"),
    ]
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

#[test]
fn the_preview_doors_answer_the_first_picture_in_one_request() {
    doors_sweep("doors", None);
}

#[test]
fn the_preview_doors_and_a_page_of_pictures_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    doors_sweep("pg", Some(&dsn));
    drop_schema(&dsn, "pg");
}

fn doors_sweep(name: &str, dsn: Option<&str>) {
    let b = bed(name, dsn);
    classify(&b.home);
    let work = b.work.path();
    let annotated = if preview_file(work, 1, true).exists() {
        1
    } else {
        2
    };
    let plain = 3 - annotated;
    let previews_queued = |home: &TempDir| {
        jobs(home)
            .iter()
            .filter(|j| words(j).contains("preview"))
            .count()
    };
    let jobs_before = previews_queued(&b.home);
    let extra = tokens();
    let extra: Vec<&str> = extra.iter().map(String::as_str).collect();
    let server = Server::start(&b.home, &extra);
    let base = format!("/api/instances/{plain}/preview");

    // gated as every picture is: below detail quasi, refused
    let (status, doc) = server.json(&base, READER);
    assert_eq!(status, 403, "{doc}");
    assert_eq!(doc["disclosure"], "gated", "{doc}");

    // the first picture in one request: the header and the three middle
    // planes, each a JPEG data URL
    let (status, headers, body) = server.get(&base, REVIEWER);
    assert_eq!(status, 200);
    let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(doc["shape"], serde_json::json!([NZ, NY, NX]), "{doc}");
    assert_eq!(doc["codec"], "jpeg");
    assert_eq!(doc["held"], false);
    assert_eq!(doc["frames"]["count"], NZ);
    assert_eq!(doc["frames"]["width"], NX);
    for plane in ["axial", "coronal", "sagittal"] {
        let m = &doc["middle"][plane];
        assert!(
            m["data"]
                .as_str()
                .unwrap()
                .starts_with("data:image/jpeg;base64,/9j/"),
            "{plane}: {m}"
        );
        assert_eq!(
            m["width"]
                .as_u64()
                .unwrap()
                .max(m["height"].as_u64().unwrap()),
            256,
            "{plane}"
        );
    }
    let digest = doc["digest"].as_str().unwrap().to_string();
    assert_eq!(digest.len(), 32);
    let etag = header(&headers, "etag").unwrap().to_string();
    assert!(etag.contains(&digest), "{etag}");
    assert_eq!(header(&headers, "cache-control"), Some("private, no-cache"));
    // named by its digest, it is immutable
    let (_, headers, _) = server.get(&format!("{base}?v={digest}"), REVIEWER);
    assert_eq!(
        header(&headers, "cache-control"),
        Some("private, max-age=31536000, immutable")
    );
    // asked again with its ETag: 304 and no body
    let (status, headers, body) =
        server.send("GET", &base, REVIEWER, &[("If-None-Match", &etag)], None);
    assert_eq!(status, 304);
    assert!(body.is_empty());
    assert_eq!(header(&headers, "etag"), Some(etag.as_str()));

    // one middle plane as an image
    let (status, headers, jpeg) = server.get(&format!("{base}?plane=coronal"), REVIEWER);
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "content-type"), Some("image/jpeg"));
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    let (status, _, _) = server.get(&format!("{base}?plane=oblique"), REVIEWER);
    assert_eq!(status, 404);

    // a range of planes, one read: from, count, width, height, the offsets
    let (status, headers, frames) = server.get(&format!("{base}/planes?from=4&to=10"), REVIEWER);
    assert_eq!(status, 200);
    assert_eq!(
        header(&headers, "content-type"),
        Some("application/x-nils-frames")
    );
    assert_eq!(header(&headers, "x-nils-planes"), Some("6"));
    assert_eq!(
        (
            u32_at(&frames, 0),
            u32_at(&frames, 4),
            u32_at(&frames, 8),
            u32_at(&frames, 12)
        ),
        (4, 6, NX, NY)
    );
    let offsets: Vec<usize> = (0..7)
        .map(|i| u32_at(&frames, 16 + 4 * i) as usize)
        .collect();
    assert_eq!(offsets[0], 16 + 4 * 7);
    assert_eq!(offsets[6], frames.len());
    for w in offsets.windows(2) {
        assert_eq!(&frames[w[0]..w[0] + 2], &[0xFF, 0xD8]);
    }
    // every plane by default
    let (_, headers, _) = server.get(&format!("{base}/planes"), REVIEWER);
    assert_eq!(
        header(&headers, "x-nils-planes"),
        Some(NZ.to_string().as_str())
    );
    let (status, _, _) = server.get(&format!("{base}/planes?from=0&to={}", NZ + 1), REVIEWER);
    assert_eq!(status, 416);

    // burned-in annotation: the band held below detail sensitive
    let held = format!("/api/instances/{annotated}/preview");
    let (_, doc) = server.json(&held, REVIEWER);
    assert_eq!(doc["held"], true, "{doc}");
    assert_eq!(doc["burned_in"], true, "{doc}");
    let (_, doc) = server.json(&held, ADMIN);
    assert_eq!(doc["held"], false, "{doc}");

    // one audit row for the stack opened, whatever was asked
    let (status, doc) = server.json("/api/audit?action=instance.open&principal=rev@lab", ADMIN);
    if status == 200 {
        let rows = doc["rows"]
            .as_array()
            .or(doc.as_array())
            .cloned()
            .unwrap_or_default();
        let opened: Vec<_> = rows
            .iter()
            .filter(|r| r["scope"]["stack"] == plain && r["scope"]["purpose"] == "preview")
            .collect();
        assert_eq!(opened.len(), 1, "{doc}");
    }

    // warm, the door answers well inside the first picture's budget
    let mut doc_ms = Vec::new();
    let mut frames_ms = Vec::new();
    for _ in 0..30 {
        let t = std::time::Instant::now();
        let (status, _, _) = server.get(&format!("{base}?v={digest}"), REVIEWER);
        assert_eq!(status, 200);
        doc_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = std::time::Instant::now();
        let (status, _, _) = server.get(&format!("{base}/planes"), REVIEWER);
        assert_eq!(status, 200);
        frames_ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let (d, f) = (median(doc_ms), median(frames_ms));
    eprintln!("preview door warm p50 {d:.1} ms; all {NZ} planes p50 {f:.1} ms");
    assert!(d < 100.0, "the preview door took {d:.1} ms warm");
    assert!(f < 100.0, "the planes door took {f:.1} ms warm");

    // a page of scans with their pictures and their questions, one request
    let (status, doc) = server.json("/api/datasets/incoming/scans?pictures=1", REVIEWER);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["pictures"]["shown"], true, "{doc}");
    assert_eq!(doc["pictures"]["missing"], 0, "{doc}");
    let scans = doc["scans"].as_array().unwrap();
    assert_eq!(scans.len(), 2);
    for s in scans {
        let p = &s["picture"];
        assert!(
            p["data"]
                .as_str()
                .unwrap()
                .starts_with("data:image/jpeg;base64,"),
            "{s}"
        );
        assert_eq!(p["held"], s["stack"] == annotated, "{s}");
        assert!(s["questions"].is_array(), "{s}");
    }
    // the questions are the open review items' kinds on each stack
    let (_, review) = server.json("/api/review?status=open&limit=500", ADMIN);
    let kinds: std::collections::BTreeSet<String> = scans
        .iter()
        .flat_map(|s| s["questions"].as_array().unwrap().clone())
        .map(|k| k.as_str().unwrap().to_string())
        .collect();
    let open: std::collections::BTreeSet<String> = review["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|i| i["kind"].as_str().map(str::to_string))
        .collect();
    assert!(!kinds.is_empty(), "{doc}");
    assert!(kinds.is_subset(&open), "{kinds:?} not in {open:?}");
    // without the pixels' grant and detail, no picture, and why
    let (status, doc) = server.json("/api/datasets/incoming/scans?pictures=1", READER);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["pictures"]["shown"], false, "{doc}");
    assert!(doc["scans"][0]["picture"].is_null(), "{doc}");
    // and without pictures=1 the page is as it was
    let (_, doc) = server.json("/api/datasets/incoming/scans", REVIEWER);
    assert!(doc["scans"][0].get("picture").is_none(), "{doc}");
    let t = std::time::Instant::now();
    for _ in 0..10 {
        let (status, _) = server.json("/api/datasets/incoming/scans?pictures=1", REVIEWER);
        assert_eq!(status, 200);
    }
    eprintln!(
        "a page of 2 scans with pictures: {:.1} ms a request",
        t.elapsed().as_secs_f64() * 100.0
    );

    // a stack with no preview yet: its first picture at once, the middle
    // plane decoded from its one file, partial and never kept; the preview
    // is made after on a thread of the engine, never queued
    std::fs::remove_file(preview_file(work, plain, false)).unwrap();
    // the doors trust an open file for two seconds before they look again
    let recheck = std::time::Duration::from_millis(2200);
    std::thread::sleep(recheck);
    let (status, headers, body) = server.get(&base, REVIEWER);
    assert_eq!(status, 200);
    let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(doc["partial"], true, "{doc}");
    assert_eq!(doc["digest"], digest, "{doc}");
    assert_eq!(doc["shape"], serde_json::json!([NZ, NY, NX]), "{doc}");
    assert_eq!(doc["frames"]["count"], NZ, "{doc}");
    assert_eq!(doc["frames"]["width"], NX, "{doc}");
    assert!(
        doc["middle"]["axial"]["data"]
            .as_str()
            .unwrap()
            .starts_with("data:image/jpeg;base64,/9j/"),
        "{doc}"
    );
    assert_eq!(header(&headers, "cache-control"), Some("no-store"));
    assert_eq!(header(&headers, "x-nils-partial"), Some("true"));
    assert!(header(&headers, "etag").is_none());
    // a short range of planes at once, a long one when the preview is made
    let (status, _, frames) = server.get(&format!("{base}/planes?from=4&to=10"), REVIEWER);
    assert_eq!(status, 200);
    assert_eq!(
        (
            u32_at(&frames, 0),
            u32_at(&frames, 4),
            u32_at(&frames, 8),
            u32_at(&frames, 12)
        ),
        (4, 6, NX, NY)
    );
    let (status, headers, _) = server.get(&format!("{base}/planes"), REVIEWER);
    assert_eq!(status, 200);
    assert_eq!(
        header(&headers, "x-nils-planes"),
        Some(NZ.to_string().as_str())
    );
    let mut whole = serde_json::Value::Null;
    for _ in 0..100 {
        let (_, doc) = server.json(&base, REVIEWER);
        whole = doc;
        if whole["partial"] == false {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(whole["partial"], false, "{whole}");
    assert_eq!(whole["digest"], digest, "{whole}");
    assert!(preview_file(work, plain, false).exists());
    assert_eq!(previews_queued(&b.home), jobs_before, "no job was queued");
    // burned-in annotation: the first picture holds the band too; and a
    // page of scans with no previews has each a picture from its one file
    for s in [plain, annotated] {
        for h in [false, true] {
            let _ = std::fs::remove_file(preview_file(work, s, h));
        }
    }
    std::thread::sleep(recheck);
    let (_, doc) = server.json(&held, REVIEWER);
    assert_eq!(doc["partial"], true, "{doc}");
    assert_eq!(doc["held"], true, "{doc}");
    assert_eq!(doc["burned_in"], true, "{doc}");
    // a page takes the stills decoded within its budget (100 ms) and counts
    // the rest missing, whose threads go on and keep them for the page
    // asked next: under load the first page may lack one, so the page is
    // asked again until each scan has its picture, in a generous time
    let asked = std::time::Instant::now();
    let doc = loop {
        let (status, doc) = server.json("/api/datasets/incoming/scans?pictures=1", REVIEWER);
        assert_eq!(status, 200, "{doc}");
        let scans = doc["scans"].as_array().unwrap();
        let lacking = scans.iter().filter(|s| s["picture"].is_null()).count();
        assert_eq!(
            doc["pictures"]["missing"], lacking,
            "a scan without its picture yet is counted: {doc}"
        );
        if lacking == 0 {
            break doc;
        }
        assert!(
            asked.elapsed() < std::time::Duration::from_secs(30),
            "the stills were never kept for the next page: {doc}"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    for s in doc["scans"].as_array().unwrap() {
        assert!(
            s["picture"]["data"]
                .as_str()
                .is_some_and(|d| d.starts_with("data:image/jpeg;base64,")),
            "{s}"
        );
        assert_eq!(s["picture"]["held"], s["stack"] == annotated, "{s}");
    }
    let (status, doc) = server.json("/api/instances/4242/preview", REVIEWER);
    assert_eq!(status, 404, "{doc}");
    let (status, _) = server.json(&format!("{base}/nothing"), REVIEWER);
    assert_eq!(status, 404);
}

fn jobs(home: &TempDir) -> Vec<serde_json::Value> {
    let (out, _) = ok(home, &["jobs", "list", "--all", "--json"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    doc.as_array()
        .cloned()
        .or_else(|| doc["jobs"].as_array().cloned())
        .unwrap_or_default()
}

fn words(j: &serde_json::Value) -> String {
    let w = &j["args"]["queued"];
    let w = if w.is_array() { w } else { &j["args"]["argv"] };
    w.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

#[test]
fn a_sort_queues_its_pyramids_on_the_pictures_lane() {
    let b = bed("lane", None);
    ok(
        &b.home,
        &[
            "jobs",
            "enqueue",
            "--",
            "classify",
            "--pack-dir",
            packs().to_str().unwrap(),
        ],
    );
    // the main lane runs the sort and its previews, and leaves the pyramids
    ok(&b.home, &["jobs", "work", "--once", "--lane", "main"]);
    let list = jobs(&b.home);
    let classify = list
        .iter()
        .find(|j| j["kind"] == "classify")
        .expect("the classify job");
    assert_eq!(classify["state"], "done", "{classify}");
    assert_eq!(classify["result"]["previews"]["built"], 2, "{classify}");
    let pyramid = list
        .iter()
        .find(|j| j["kind"] == "pyramid")
        .expect("a pyramid job queued after the sort");
    assert_eq!(pyramid["state"], "queued", "{pyramid}");
    assert_eq!(
        words(pyramid),
        format!("pyramid build --classified {} --place work", classify["id"]),
        "{pyramid}"
    );
    assert!(!b.work.path().join("pyramids").exists());
    // a reader's single stack, queued after it, runs first on its lane
    ok(
        &b.home,
        &["jobs", "enqueue", "--", "pyramid", "build", "--stack", "2"],
    );
    ok(&b.home, &["jobs", "work", "--once", "--lane", "pictures"]);
    let list = jobs(&b.home);
    let built: Vec<&serde_json::Value> = list.iter().filter(|j| j["kind"] == "pyramid").collect();
    assert_eq!(built.len(), 2, "{list:?}");
    assert!(built.iter().all(|j| j["state"] == "done"), "{built:?}");
    let single = built
        .iter()
        .find(|j| words(j).contains("--stack 2"))
        .unwrap();
    let background = built
        .iter()
        .find(|j| words(j).contains("--classified"))
        .unwrap();
    assert!(
        single["finished_at"].as_str() <= background["started_at"].as_str(),
        "the reader's stack ran first: {single} {background}"
    );
    assert_eq!(background["result"]["built"], 1, "{background}");
    assert_eq!(background["result"]["skipped"], 1, "{background}");
    for s in [1, 2] {
        assert!(
            b.work
                .path()
                .join("pyramids")
                .join(s.to_string())
                .join("manifest.json")
                .exists()
        );
    }
}

#[test]
fn a_worker_beside_the_doors_starts_a_queued_job_at_once() {
    let b = bed("wake", None);
    let extra = tokens();
    let mut extra: Vec<&str> = extra.iter().map(String::as_str).collect();
    extra.push("--worker");
    let server = Server::start(&b.home, &extra);
    // let the workers reach their first wait on an empty queue
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let t = std::time::Instant::now();
    let (status, _, body) = server.send(
        "POST",
        "/api/jobs",
        ADMIN,
        &[],
        Some(r#"{"command": ["fingerprint"], "name": "woken"}"#),
    );
    assert!(status < 300, "{status}: {}", String::from_utf8_lossy(&body));
    let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let id = doc["job"]
        .as_i64()
        .or_else(|| doc["id"].as_i64())
        .unwrap_or_else(|| panic!("{doc}"));
    let done = loop {
        let (_, j) = server.json(&format!("/api/jobs/{id}"), ADMIN);
        let state = j["state"].as_str().unwrap_or_default().to_string();
        if state == "done" || state == "failed" {
            break state;
        }
        assert!(
            t.elapsed() < std::time::Duration::from_secs(20),
            "job {id} is {state}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let took = t.elapsed().as_secs_f64();
    eprintln!("queued to {done} in {took:.2} s");
    assert_eq!(done, "done");
    // the look every five seconds alone took up to five; woken, the job
    // runs as soon as it is queued
    assert!(took < 3.0, "the job took {took:.2} s to run");
}
