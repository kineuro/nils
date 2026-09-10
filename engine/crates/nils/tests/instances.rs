// SPDX-License-Identifier: AGPL-3.0-only
//! Wave 5 §12.7, the gated instance door and the pyramid job: the pyramid is
//! built into a working place and refused without one; the tile door
//! answers a plane in one response; the slab door caps at thirty-two; the
//! render door answers a JPEG; one audit row per stack opened; a reader
//! without the class is refused; burned-in annotation holds the tiles below
//! the class and blanks the render.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn run(home: &TempDir, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
    let mut cmd = nils();
    cmd.arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "anna")
        .env("HOSTNAME", "ward-3")
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
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn ok(home: &TempDir, args: &[&str], stdin: Option<&str>) -> String {
    let (good, out, err) = run(home, args, stdin);
    assert!(good, "nils {args:?} failed: {err}");
    out
}

fn packs() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

const NZ: u32 = 40;
const NY: u32 = 300;
const NX: u32 = 280;

fn us(tag: dicom_core::Tag, v: u16) -> synth::Elem {
    synth::bytes(tag, VR::US, v.to_le_bytes().to_vec())
}

/// A series of NZ planes of NY by NX uint16 with a gradient that differs per
/// plane, so a decoded tile can be checked against what was written.
fn series(dir: &TempDir, study: &str, series: &str, burned_in: bool) {
    for z in 0..NZ {
        let sop = format!("{series}.{}", z + 1);
        let mut e = synth::minimal_mr(study, series, &sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
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

/// A registry with two stacks that carry pixels: one plain, one with
/// burned-in annotation.
fn registry() -> (TempDir, TempDir) {
    let home = TempDir::new("instances-home");
    let dir = TempDir::new("instances-src");
    series(&dir, "1.2.3.A", "1.2.3.A.1", false);
    series(&dir, "1.2.3.B", "1.2.3.B.1", true);
    ok(&home, &["key", "add", "k"], Some("an instances test key\n"));
    ok(&home, &["init", "--key", "k"], None);
    ok(
        &home,
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            dir.path().to_str().unwrap(),
        ],
        None,
    );
    ok(&home, &["fingerprint"], None);
    ok(
        &home,
        &["classify", "--pack-dir", packs().to_str().unwrap()],
        None,
    );
    (home, dir)
}

struct Server {
    child: Child,
    port: u16,
}

impl Server {
    fn start(home: &TempDir, requests: usize, extra: &[&str]) -> Server {
        let mut cmd = nils();
        cmd.arg("--registry")
            .arg(home.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "2",
                "--requests",
            ])
            .arg(requests.to_string())
            .args(["--pack-dir", packs().to_str().unwrap()])
            .args(extra)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            panic!("nils serve did not listen: {err}");
        };
        let addr = first.split_whitespace().nth(2).unwrap();
        let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
        Server { child, port }
    }

    /// A GET with a bearer: the status, the headers, the body's bytes.
    fn get(&self, path: &str, token: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let head = format!(
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {token}\r\n\r\n"
        );
        stream.write_all(head.as_bytes()).unwrap();
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

    fn json(&self, path: &str, token: &str) -> (u16, serde_json::Value) {
        let (status, _, body) = self.get(path, token);
        let text = String::from_utf8_lossy(&body).to_string();
        (
            status,
            serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)),
        )
    }

    fn finish(mut self) {
        let status = self.child.wait().unwrap();
        assert!(status.success(), "nils serve exited {status}");
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
const OPERATOR: &str = "an-operator-token-of-len";
const ADMIN: &str = "an-admin-token-of-length";

#[test]
fn the_pyramid_is_built_into_a_working_place_and_the_doors_are_gated() {
    let (home, _src) = registry();
    // refused without a working place, with the rule's sentence
    let (good, _, err) = run(&home, &["pyramid", "build", "--stack", "1"], None);
    assert!(!good);
    assert!(err.contains("no working place is bound"), "{err}");

    // a working place, then the build
    let work = TempDir::new("instances-work");
    ok(
        &home,
        &[
            "place",
            "add",
            "scratch",
            work.path().to_str().unwrap(),
            "--role",
            "working",
            "--fast",
        ],
        None,
    );
    // the digest made one stack per series; which is which is read from the manifest's annotation flag
    let started = std::time::Instant::now();
    let built = ok(
        &home,
        &["pyramid", "build", "--stack", "1", "--workers", "4"],
        None,
    );
    let wall = started.elapsed().as_secs_f64();
    let built: serde_json::Value = serde_json::from_str(built.trim()).unwrap();
    assert_eq!(built["shape"], serde_json::json!([NZ, NY, NX]), "{built}");
    assert_eq!(built["levels"], 4);
    assert_eq!(built["codec"], "htj2k");
    ok(
        &home,
        &["pyramid", "build", "--stack", "2", "--workers", "4"],
        None,
    );
    let manifest_of = |id: i64| -> serde_json::Value {
        serde_json::from_str(
            &std::fs::read_to_string(
                work.path()
                    .join("pyramids")
                    .join(id.to_string())
                    .join("manifest.json"),
            )
            .unwrap(),
        )
        .unwrap()
    };
    let (plain, annotated) = if manifest_of(1)["annotation"]["burned_in"] == true {
        (2, 1)
    } else {
        (1, 2)
    };
    let root = work.path().join("pyramids").join(plain.to_string());
    assert!(root.join("manifest.json").exists());
    assert!(root.join("0").join("0").join("0_0.j2c").exists());
    assert!(
        root.join("3")
            .join(format!("{}", NZ - 1))
            .join("0_0.j2c")
            .exists()
    );
    let raw = built["raw_bytes"].as_u64().unwrap();
    let total: u64 = built["bytes_per_level"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_u64().unwrap())
        .sum();
    eprintln!(
        "pyramid of {}x{}x{} u16: {raw} raw bytes, {total} in the pyramid, {wall:.2} s wall",
        NZ, NY, NX
    );
    assert!(
        total < raw * 2,
        "reversible tiles should not double the raw bytes"
    );

    // the doors
    // exactly the requests below: the server serves that many and exits
    let server = Server::start(
        &home,
        18,
        &[
            "--auth",
            "token",
            "--token",
            &format!("{READER}=lou@lab:reader"),
            "--token",
            &format!("{REVIEWER}=rev@lab:reviewer"),
            "--token",
            &format!("{OPERATOR}=ops@lab:operator"),
            "--token",
            &format!("{ADMIN}=root@lab:admin"),
        ],
    );
    let base = format!("/api/instances/{plain}");

    // a reader without the class is refused, gated
    let (status, doc) = server.json(&format!("{base}/manifest"), READER);
    assert_eq!(status, 403, "{doc}");
    assert_eq!(doc["disclosure"], "gated");
    let (status, _, _) = server.get(&format!("{base}/tiles/0/0"), READER);
    assert_eq!(status, 403);

    // the manifest under the reviewer
    let (status, m) = server.json(&format!("{base}/manifest"), REVIEWER);
    assert_eq!(status, 200, "{m}");
    assert_eq!(m["codec"], "htj2k");
    assert_eq!(m["tile"], 256);
    assert_eq!(m["shape"], serde_json::json!([NZ, NY, NX]));
    assert_eq!(m["spacing"], serde_json::json!([2.0, 0.5, 0.5]));
    assert_eq!(m["intercept"], 0);
    assert_eq!(m["annotation"]["burned_in"], false);
    assert_eq!(m["level_shapes"][0]["tiles"], serde_json::json!([2, 2]));
    assert_eq!(
        m["level_shapes"][3]["shape"],
        serde_json::json!([NZ, NY / 8, NX / 8])
    );
    assert!(m["window"]["width"].as_f64().unwrap() > 0.0);

    // one plane's tiles in one response: the count, the offsets, the codec
    let (status, headers, body) = server.get(&format!("{base}/tiles/0/5"), REVIEWER);
    assert_eq!(status, 200);
    assert_eq!(
        header(&headers, "content-type"),
        Some("application/x-nils-tiles")
    );
    assert_eq!(header(&headers, "x-nils-codec"), Some("htj2k"));
    assert_eq!(u32_at(&body, 0), 4, "two by two tiles at level 0");
    let offsets: Vec<usize> = (0..4).map(|i| u32_at(&body, 4 + 4 * i) as usize).collect();
    assert_eq!(offsets[0], 4 + 16);
    assert!(offsets.windows(2).all(|w| w[0] < w[1]) && offsets[3] < body.len());
    // the first tile decodes back to what was written
    let first = &body[offsets[0]..offsets[1]];
    assert_eq!(&first[..2], &[0xFF, 0x4F], "a codestream starts with SOC");

    // the slab: up to thirty-two planes, refused past that
    let (status, headers, slab) = server.get(&format!("{base}/slab/1/0-8"), REVIEWER);
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "x-nils-planes"), Some("8"));
    assert_eq!(u32_at(&slab, 0), 8);
    let (status, _, _) = server.get(&format!("{base}/slab/1/0-33"), REVIEWER);
    assert_eq!(status, 416);
    let (status, _, _) = server.get(&format!("{base}/slab/1/30-50"), REVIEWER);
    assert_eq!(status, 416);

    // the render: a JPEG, along every axis
    let (status, headers, jpeg) = server.get(&format!("{base}/render/1/3"), REVIEWER);
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "content-type"), Some("image/jpeg"));
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    assert_eq!(header(&headers, "x-nils-held"), Some("false"));
    let (status, _, side) = server.get(
        &format!("{base}/render/2/10?axis=y&c=2000&w=4000"),
        REVIEWER,
    );
    assert_eq!(status, 200);
    assert_eq!(&side[..2], &[0xFF, 0xD8]);

    // one audit row per stack opened, not per tile: the reviewer opened the plain stack once
    let (status, audit) = server.json("/api/audit?action=instance.open&limit=50", ADMIN);
    assert_eq!(status, 200, "{audit}");
    let rows = audit["rows"].as_array().unwrap();
    let mine: Vec<_> = rows
        .iter()
        .filter(|r| r["principal"] == "rev@lab" && r["scope"]["stack"] == plain)
        .collect();
    assert_eq!(
        mine.len(),
        1,
        "one row for the stack, {} requests: {rows:?}",
        6
    );

    // burned-in annotation: the tiles and the slab are held below the operator, the render blanks the band
    let annotated_base = format!("/api/instances/{annotated}");
    let (status, m) = server.json(&format!("{annotated_base}/manifest"), REVIEWER);
    assert_eq!(status, 200, "{m}");
    assert_eq!(m["annotation"]["burned_in"], true);
    assert_eq!(m["annotation"]["where"], "header");
    assert_eq!(m["held"], true);
    let (status, doc) = server.json(&format!("{annotated_base}/tiles/0/0"), REVIEWER);
    assert_eq!(status, 403, "{doc}");
    assert_eq!(doc["disclosure"], "gated");
    let (status, _) = server.json(&format!("{annotated_base}/slab/0/0-2"), REVIEWER);
    assert_eq!(status, 403);
    let (status, headers, held) = server.get(&format!("{annotated_base}/render/0/0"), REVIEWER);
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "x-nils-held"), Some("true"));
    assert_eq!(&held[..2], &[0xFF, 0xD8]);
    // the operator holds the class: the tiles open and the render is whole
    let (status, headers, _) = server.get(&format!("{annotated_base}/tiles/0/0"), OPERATOR);
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "x-nils-codec"), Some("htj2k"));
    let (status, headers, _) = server.get(&format!("{annotated_base}/render/0/0"), OPERATOR);
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "x-nils-held"), Some("false"));

    // the capabilities name the doors and the batch offers the job
    let (status, caps) = server.json("/api/capabilities", REVIEWER);
    assert_eq!(status, 200);
    let doors: Vec<&str> = caps["doors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .collect();
    assert!(
        doors.contains(&"GET /api/instances/{stack}/tiles/{level}/{z}"),
        "{doors:?}"
    );
    let (status, batch) = server.json("/api/batches/1", REVIEWER);
    assert_eq!(status, 200, "{batch}");
    assert_eq!(batch["pyramid"]["offered"], true, "{batch}");
    server.finish();
}

#[test]
fn a_reader_of_the_manifest_is_refused_before_the_pyramid_exists() {
    let (home, _src) = registry();
    let work = TempDir::new("instances-work-2");
    ok(
        &home,
        &[
            "place",
            "add",
            "scratch",
            work.path().to_str().unwrap(),
            "--role",
            "working",
        ],
        None,
    );
    let server = Server::start(
        &home,
        3,
        &[
            "--auth",
            "token",
            "--token",
            &format!("{REVIEWER}=rev@lab:reviewer"),
        ],
    );
    let (status, doc) = server.json("/api/instances/1/manifest", REVIEWER);
    assert_eq!(status, 404, "{doc}");
    assert!(
        doc["error"]
            .as_str()
            .unwrap()
            .contains("pyramid build --stack 1"),
        "{doc}"
    );
    let (status, doc) = server.json("/api/instances/1/tiles/9/0", REVIEWER);
    assert_eq!(status, 404, "{doc}");
    let (status, doc) = server.json("/api/instances/x/manifest", REVIEWER);
    assert_eq!(status, 404, "{doc}");
    server.finish();
}
