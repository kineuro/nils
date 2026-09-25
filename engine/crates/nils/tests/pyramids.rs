// SPDX-License-Identifier: AGPL-3.0-only
//! Record 45 E1 and E2: the pyramids of a selection built as one job that
//! skips what is built, at the keyboard and through the jobs door, and the
//! manifest's geometry, which an oblique stack written here reports, and a
//! manifest from before reads as axial with the flag.

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

fn run(home: &TempDir, args: &[&str]) -> (bool, String, String) {
    let out = nils()
        .arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "anna")
        .env("HOSTNAME", "ward-3")
        .env_remove("NILS_JOB_ID")
        .env_remove("NILS_JOB_DETAIL")
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
    let (good, out, err) = run(home, args);
    assert!(good, "nils {args:?} failed: {err}");
    out
}

const NZ: u32 = 6;
const NY: u32 = 40;
const NX: u32 = 32;
/// The planes' spacing along their normal, millimetres.
const GAP: f64 = 2.5;

fn us(tag: dicom_core::Tag, v: u16) -> synth::Elem {
    synth::bytes(tag, VR::US, v.to_le_bytes().to_vec())
}

/// Twenty degrees about the patient's x axis: rows run along x, columns
/// along y tilted towards z.
fn oblique() -> [f64; 6] {
    let a = 20f64.to_radians();
    [1.0, 0.0, 0.0, 0.0, a.cos(), a.sin()]
}

fn cross(o: &[f64; 6]) -> [f64; 3] {
    [
        o[1] * o[5] - o[2] * o[4],
        o[2] * o[3] - o[0] * o[5],
        o[0] * o[4] - o[1] * o[3],
    ]
}

/// A series of NZ planes, each a gradient of its own, turned as `iop` says,
/// its planes GAP apart along their normal from `origin`. The instance
/// numbers run against the positions, so the order is the geometry's.
fn series(dir: &TempDir, study: &str, series: &str, iop: [f64; 6], origin: [f64; 3]) {
    let n = cross(&iop);
    for z in 0..NZ {
        let sop = format!("{series}.{}", z + 1);
        let mut e = synth::minimal_mr(study, series, &sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
        e.push(synth::text(
            tags::INSTANCE_NUMBER,
            VR::IS,
            &(NZ - z).to_string(),
        ));
        let at = |i: usize| origin[i] + z as f64 * GAP * n[i];
        e.push(synth::text(
            tags::IMAGE_POSITION_PATIENT,
            VR::DS,
            &format!("{:.6}\\{:.6}\\{:.6}", at(0), at(1), at(2)),
        ));
        e.push(synth::text(
            tags::IMAGE_ORIENTATION_PATIENT,
            VR::DS,
            &iop.iter()
                .map(|v| format!("{v:.6}"))
                .collect::<Vec<_>>()
                .join("\\"),
        ));
        e.push(synth::text(tags::PIXEL_SPACING, VR::DS, "0.9\\0.9"));
        e.push(synth::text(tags::SLICE_THICKNESS, VR::DS, "2.5"));
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
        let mut px = Vec::with_capacity((NY * NX * 2) as usize);
        for y in 0..NY {
            for x in 0..NX {
                px.extend_from_slice(&(((x + y * 3 + z * 50) % 4096) as u16).to_le_bytes());
            }
        }
        e.push(synth::bytes(tags::PIXEL_DATA, VR::OW, px));
        dir.file(
            &format!("{study}/{sop}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
}

const AXIAL: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

/// A registry of three stacks with pixels: an axial one, an oblique one,
/// and an axial one whose files are gone after the digest, so its pyramid
/// fails; a working place; and the selection of every stack.
struct Lab {
    home: TempDir,
    work: TempDir,
    _src: TempDir,
}

fn lab(name: &str) -> Lab {
    lab_over(name, |src| {
        series(src, "1.2.3.A", "1.2.3.A.1", AXIAL, [-14.0, -18.0, 30.0]);
        series(src, "1.2.3.B", "1.2.3.B.1", oblique(), [-14.0, -18.0, 12.0]);
        series(src, "1.2.3.C", "1.2.3.C.1", AXIAL, [0.0, 0.0, 0.0]);
    })
}

/// A lab over the files `fill` writes, digested, classified, with a
/// working place and the selection of every stack; the third series of
/// [`lab`] loses its files after the digest where there is one.
fn lab_over(name: &str, fill: impl FnOnce(&TempDir)) -> Lab {
    let home = TempDir::new(&format!("{name}-home"));
    let src = TempDir::new(&format!("{name}-src"));
    let work = TempDir::new(&format!("{name}-work"));
    fill(&src);
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
        .write_all(b"a pyramids test key\n")
        .unwrap();
    assert!(child.wait().unwrap().success());
    ok(&home, &["init", "--key", "k"]);
    ok(
        &home,
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            src.path().to_str().unwrap(),
        ],
    );
    ok(&home, &["fingerprint"]);
    let packs = packs();
    let packs = packs.to_str().unwrap();
    ok(&home, &["classify", "--pack-dir", packs]);
    // the third series' files go, so its pyramid cannot be read
    if src.path().join("1.2.3.C").exists() {
        std::fs::remove_dir_all(src.path().join("1.2.3.C")).unwrap();
    }
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
    );
    let doc = work.path().join("every.json");
    std::fs::write(
        &doc,
        json!({"ast_version": 1, "sets": {"every": {"grain": "stack"}}, "out": {"set": "every", "level": "record"}}).to_string(),
    )
    .unwrap();
    ok(
        &home,
        &[
            "ask",
            "selections",
            "save",
            "--name",
            "every",
            "--file",
            doc.to_str().unwrap(),
            "--pack-dir",
            packs,
        ],
    );
    std::fs::remove_file(&doc).unwrap();
    Lab {
        home,
        work,
        _src: src,
    }
}

impl Lab {
    fn manifest(&self, stack: i64) -> Option<Value> {
        std::fs::read_to_string(
            self.work
                .path()
                .join("pyramids")
                .join(stack.to_string())
                .join("manifest.json"),
        )
        .ok()
        .map(|t| serde_json::from_str(&t).unwrap())
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-4
}

/// E1 at the keyboard and E2: a selection's pyramids build once as one job,
/// a re-run skips them, a stack that cannot be read is counted with why;
/// the oblique stack reports its orientation, origin and frame, ordered
/// along its normal whatever its instance numbers say.
#[test]
fn a_selection_s_pyramids_build_once_and_an_oblique_stack_reports_its_orientation() {
    let lab = lab("pyramids-cli");
    let packs = packs();
    let packs = packs.to_str().unwrap();
    let args = [
        "pyramid",
        "build",
        "--select",
        "selection:every@1",
        "--pack-dir",
        packs,
        "--workers",
        "2",
    ];
    let first: Value = serde_json::from_str(ok(&lab.home, &args).trim()).unwrap();
    assert_eq!(first["stacks"], 3, "{first}");
    assert_eq!(first["built"], 2, "{first}");
    assert_eq!(first["skipped"], 0, "{first}");
    assert_eq!(first["failed"], 1, "{first}");
    // a failure names its stack and a reason class, never the reader's
    // words: a file's path can start with a subject code or hold a series
    // name, and the job's result is served at detail plain
    assert_eq!(first["failures"][0]["reason"], "unreadable", "{first}");
    assert!(first["failures"][0].get("why").is_none(), "{first}");
    let src = lab._src.path().to_str().unwrap().to_string();
    let text = first.to_string();
    assert!(!text.contains(&src), "{text}");
    assert!(!text.contains("1.2.3.C"), "{text}");
    let second: Value = serde_json::from_str(ok(&lab.home, &args).trim()).unwrap();
    assert_eq!(second["built"], 0, "{second}");
    assert_eq!(second["skipped"], 2, "{second}");
    assert_eq!(second["failed"], 1, "{second}");
    // each run is one job of kind pyramid, its result the counts
    let jobs: Value =
        serde_json::from_str(&ok(&lab.home, &["jobs", "list", "--all", "--json"])).unwrap();
    let list = jobs.as_array().or_else(|| jobs["jobs"].as_array()).unwrap();
    let pyramids: Vec<&Value> = list.iter().filter(|j| j["kind"] == "pyramid").collect();
    assert_eq!(pyramids.len(), 2, "{jobs}");
    assert!(pyramids.iter().all(|j| j["state"] == "done"), "{jobs}");
    assert!(
        pyramids
            .iter()
            .any(|j| j["result"]["skipped"] == 2 && j["result"]["built"] == 0),
        "{jobs}"
    );
    // one stack at the door's worker, whose error is what the verb printed:
    // the class again, never the path
    let (good, _, err) = run(
        &lab.home,
        &[
            "pyramid",
            "build",
            "--stack",
            &first["failures"][0]["stack"].to_string(),
        ],
    );
    assert!(!good);
    assert!(err.contains("unreadable"), "{err}");
    assert!(!err.contains(&src) && !err.contains("1.2.3.C"), "{err}");
    // no job's result names a path or a series
    for j in &pyramids {
        let text = j["result"].to_string();
        assert!(!text.contains(&src) && !text.contains("1.2.3.C"), "{text}");
    }

    // E2: which stack is which is read from the manifests
    let built: Vec<(i64, Value)> = (1..=3)
        .filter_map(|s| lab.manifest(s).map(|m| (s, m)))
        .collect();
    assert_eq!(built.len(), 2);
    let (_, straight) = built
        .iter()
        .find(|(_, m)| m["oblique"] == false)
        .expect("the axial stack");
    let (tilted_id, tilted) = built
        .iter()
        .find(|(_, m)| m["oblique"] == true)
        .expect("the oblique stack");
    assert_eq!(straight["orientation_known"], true);
    assert_eq!(straight["plane"], "axial");
    assert_eq!(straight["orientation"], json!(AXIAL));
    assert_eq!(straight["origin"], json!([-14.0, -18.0, 30.0]));
    let want = oblique();
    let got: Vec<f64> = tilted["orientation"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert!(
        got.iter().zip(want.iter()).all(|(a, b)| close(*a, *b)),
        "{tilted}"
    );
    assert_eq!(tilted["plane"], "axial", "twenty degrees off axial");
    assert_eq!(
        tilted["frame"],
        json!({"parallel": true, "evenly_spaced": true})
    );
    // the first plane is the lowest along the normal, which is the last
    // instance: the origin the series was written from
    let origin: Vec<f64> = tilted["origin"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert!(
        close(origin[0], -14.0) && close(origin[1], -18.0) && close(origin[2], 12.0),
        "{tilted}"
    );
    assert!(
        close(tilted["spacing"][0].as_f64().unwrap(), GAP),
        "{tilted}"
    );
    // a manifest from before record 45 is read as axial, with the flag
    let path = lab
        .work
        .path()
        .join("pyramids")
        .join(tilted_id.to_string())
        .join("manifest.json");
    let mut old = tilted.clone();
    for key in [
        "orientation",
        "origin",
        "frame",
        "orientation_known",
        "plane",
        "oblique",
    ] {
        old.as_object_mut().unwrap().remove(key);
    }
    std::fs::write(&path, old.to_string()).unwrap();
    let listed: Value =
        serde_json::from_str(&ok(&lab.home, &["pyramid", "list", "--json"])).unwrap();
    let read = &listed[tilted_id.to_string()];
    assert_eq!(read["orientation"], json!(AXIAL), "{listed}");
    assert_eq!(read["orientation_known"], false, "{listed}");
    assert_eq!(read["frame"], Value::Null, "{listed}");
}

/// Compressed stacks at the keyboard: the fixtures of
/// tests/fixtures/compressed (synthetic planes in every syntax the pyramid
/// decodes) build, a lossy stack's manifest says so, and the two stacks
/// that cannot be read, a twelve-bit JPEG extended plane and a stack with
/// one broken JPEG 2000 plane among good ones, are counted by reason.
#[test]
fn compressed_stacks_build_and_the_ones_that_cannot_are_counted_by_reason() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/compressed");
    let lab = lab_over("pyramids-compressed", |src| {
        for entry in std::fs::read_dir(&fixtures).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "dcm") {
                continue;
            }
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            let mut bytes = std::fs::read(&path).unwrap();
            if name == "f2.dcm" {
                // the JPEG 2000 codestream's start is gone
                let at = bytes
                    .windows(4)
                    .position(|w| w == [0xFF, 0x4F, 0xFF, 0x51])
                    .unwrap();
                bytes[at..at + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
            src.file(&name, &bytes);
        }
    });
    let packs = packs();
    let args = [
        "pyramid",
        "build",
        "--select",
        "selection:every@1",
        "--pack-dir",
        packs.to_str().unwrap(),
        "--workers",
        "2",
    ];
    let out: Value = serde_json::from_str(ok(&lab.home, &args).trim()).unwrap();
    assert_eq!(out["stacks"], 6, "{out}");
    assert_eq!(out["built"], 4, "{out}");
    assert_eq!(out["failed"], 2, "{out}");
    assert_eq!(
        out["failures_by_reason"],
        json!({"undecodable": 2}),
        "{out}"
    );
    let src = lab._src.path().to_str().unwrap().to_string();
    assert!(!out.to_string().contains(&src), "{out}");
    let manifests: Vec<Value> = (1..=6).filter_map(|s| lab.manifest(s)).collect();
    assert_eq!(manifests.len(), 4);
    let lossy: Vec<&Value> = manifests.iter().filter(|m| m["lossy"] == true).collect();
    assert_eq!(lossy.len(), 1, "{manifests:?}");
    assert_eq!(lossy[0]["dtype"], "uint16");
    assert!(
        manifests
            .iter()
            .any(|m| m["source_syntaxes"].as_array().unwrap().len() == 8),
        "{manifests:?}"
    );
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

const OPERATOR: &str = "an-operator-token-of-length";
const READER: &str = "a-reader-token-of-its-length";
const CURATOR: &str = "a-curator-token-of-its-length";

impl Server {
    fn start(home: &TempDir) -> Server {
        let tokens = [
            format!("{OPERATOR}=ops@lab:operator"),
            format!("{READER}=lou@lab:reader"),
            format!("{CURATOR}=cleo@lab:reviewer,campaigns:work"),
        ]
        .join(",");
        let mut child = nils()
            .arg("--registry")
            .arg(home.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "1",
                "--worker",
                "--auth",
                "token",
            ])
            .args(["--pack-dir", packs().to_str().unwrap()])
            .env("NILS_TOKENS", tokens)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            let _ = child.kill();
            panic!("nils serve did not listen");
        };
        let addr = first.split_whitespace().nth(2).unwrap();
        let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
        std::thread::spawn(move || for _ in lines {});
        Server { child, port }
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> (u16, Value) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
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

    /// A GET whose answer is bytes, such as the render door's JPEG.
    fn bytes(&self, path: &str, token: &str) -> (u16, Vec<u8>) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let head = format!(
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {token}\r\n\r\n"
        );
        stream.write_all(head.as_bytes()).unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let at = bytes
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("an HTTP answer");
        let headers = String::from_utf8_lossy(&bytes[..at]).to_string();
        let status: u16 = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, bytes[at + 4..].to_vec())
    }

    fn job(&self, command: Value) -> Value {
        let (status, doc) = self.call(
            "POST",
            "/api/jobs",
            Some(json!({"command": command})),
            OPERATOR,
        );
        assert_eq!(status, 202, "{doc}");
        let job = doc["job"].as_i64().unwrap();
        for _ in 0..600 {
            let (_, j) = self.call("GET", &format!("/api/jobs/{job}"), None, OPERATOR);
            if matches!(j["state"].as_str(), Some("done" | "failed" | "cancelled")) {
                return j;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("job {job} did not end");
    }
}

/// E1 through the door: the build over a selection is queued at the jobs
/// door as one job, the deployment's packs added and a caller's path
/// refused; a second run skips what the first built; the manifest door
/// names the geometry; a campaign made of the selection says how many of
/// its stacks have their picture.
#[test]
fn a_selection_s_pyramids_are_one_job_at_the_door_and_a_campaign_counts_its_pictures() {
    let lab = lab("pyramids-door");
    let server = Server::start(&lab.home);
    // a campaign of the selection before anything is built: none of its
    // stacks has its picture, and it says which job builds them
    let (status, made) = server.call(
        "POST",
        "/api/campaigns",
        Some(json!({"name": "look", "question": {"kind": "free"}, "source": {"selection": "every@1"}})),
        CURATOR,
    );
    assert_eq!(status, 201, "{made}");
    assert_eq!(made["pictures"]["stacks"], 3, "{made}");
    assert_eq!(made["pictures"]["have"], 0, "{made}");
    assert_eq!(made["pictures"]["missing"], 3, "{made}");
    let build = made["pictures"]["build"].clone();
    assert_eq!(build[2], "--handle", "{made}");
    // a path a caller composes never reaches the verb, nor a second source
    for bad in [
        json!([
            "pyramid",
            "build",
            "--select",
            "selection:every@1",
            "--pack-dir",
            "/tmp"
        ]),
        json!([
            "pyramid",
            "build",
            "--select",
            "selection:every@1",
            "--stack",
            "1"
        ]),
        json!(["pyramid", "rebuild"]),
    ] {
        let (status, doc) =
            server.call("POST", "/api/jobs", Some(json!({"command": bad})), OPERATOR);
        assert_eq!(status, 400, "{bad}: {doc}");
    }
    let (status, _) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["pyramid", "build", "--select", "selection:every@1"]})),
        READER,
    );
    assert_eq!(status, 403, "a reader holds no pipelines:work");
    let first = server.job(json!(["pyramid", "build", "--select", "selection:every@1"]));
    assert_eq!(first["state"], "done", "{first}");
    assert_eq!(first["kind"], "pyramid", "{first}");
    assert_eq!(first["result"]["built"], 2, "{first}");
    assert_eq!(first["result"]["failed"], 1, "{first}");
    // the campaign's own job, by the handle it pins, skips them all
    let second = server.job(build);
    assert_eq!(second["result"]["skipped"], 2, "{second}");
    assert_eq!(second["result"]["built"], 0, "{second}");
    let (status, again) = server.call(
        "POST",
        "/api/campaigns",
        Some(json!({"name": "look-again", "question": {"kind": "free"}, "source": {"selection": "every@1"}})),
        CURATOR,
    );
    assert_eq!(status, 201, "{again}");
    assert_eq!(again["pictures"]["have"], 2, "{again}");
    assert_eq!(again["pictures"]["missing"], 1, "{again}");
    // the manifest door names the geometry
    let tilted = (1..=3)
        .find(|s| lab.manifest(*s).is_some_and(|m| m["oblique"] == true))
        .unwrap();
    let (status, m) = server.call(
        "GET",
        &format!("/api/instances/{tilted}/manifest"),
        None,
        OPERATOR,
    );
    assert_eq!(status, 200, "{m}");
    assert_eq!(m["orientation_known"], true, "{m}");
    assert_eq!(m["frame"]["parallel"], true, "{m}");
    assert!(m["origin"].is_array() && m["orientation"].is_array(), "{m}");
}

/// Multi-frame objects: every frame is a plane. The fixtures in
/// `tests/fixtures/multiframe` (see its `make.sh`) are digested as they
/// come: an enhanced MR of five frames in five syntaxes, one whose frames'
/// positions run out of order, a classic multi-frame object with no
/// position per frame, a stack of two enhanced files and a single-frame
/// one, and a file whose frames the digest splits into an axial and a
/// sagittal stack. Each stack's pyramid holds its own frames, and the
/// render door draws all three planes with the stack's slice count.
#[test]
fn multi_frame_stacks_build_every_frame_and_render_on_every_axis() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multiframe");
    let lab = lab_over("pyramids-multiframe", |src| {
        for entry in std::fs::read_dir(&fixtures).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "dcm") {
                let name = path.file_name().unwrap().to_str().unwrap().to_string();
                src.file(&name, &std::fs::read(&path).unwrap());
            }
        }
    });
    let packs = packs();
    let args = [
        "pyramid",
        "build",
        "--select",
        "selection:every@1",
        "--pack-dir",
        packs.to_str().unwrap(),
        "--workers",
        "2",
    ];
    let out: Value = serde_json::from_str(ok(&lab.home, &args).trim()).unwrap();
    // g0-g4, h0, i0, the j stack and k0's two
    assert_eq!(out["stacks"], 10, "{out}");
    assert_eq!(out["built"], 10, "{out}");
    let manifests: Vec<(i64, Value)> = (1..=10)
        .map(|s| (s, lab.manifest(s).expect("every stack is built")))
        .collect();
    let mut seen: Vec<(u64, String, String)> = manifests
        .iter()
        .map(|(_, m)| {
            (
                m["shape"][0].as_u64().unwrap(),
                m["order"].as_str().unwrap().to_string(),
                m["plane"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    seen.sort();
    let want = |n: u64, order: &str, plane: &str| (n, order.to_string(), plane.to_string());
    let mut expected = vec![
        want(3, "position", "axial"),
        want(3, "position", "sagittal"),
        want(4, "frames", "axial"),
        want(7, "position", "axial"),
    ];
    expected.extend((0..6).map(|_| want(5, "position", "axial")));
    expected.sort();
    assert_eq!(seen, expected);
    for (_, m) in &manifests {
        let classic = m["order"] == "frames";
        assert_eq!(m["orientation_known"], !classic, "{m}");
        let spacing = m["spacing"][0].as_f64().unwrap();
        assert!(close(spacing, if classic { 3.0 } else { 2.0 }), "{m}");
        assert!(m["multiframe_files"].as_u64().unwrap() >= 1, "{m}");
    }
    // the render door, as the viewer asks for it: a plane of each axis,
    // the slice count the stack's
    let server = Server::start(&lab.home);
    for (stack, m) in &manifests {
        let nz = m["shape"][0].as_u64().unwrap() as u32;
        for (axis, width, height) in [("z", 64, 48), ("y", 64, nz), ("x", 48, nz)] {
            let (status, jpeg) = server.bytes(
                &format!("/api/instances/{stack}/render/0/1?axis={axis}"),
                OPERATOR,
            );
            assert_eq!(status, 200, "stack {stack} axis {axis}");
            let img = image::load_from_memory(&jpeg).unwrap();
            assert_eq!(
                (img.width(), img.height()),
                (width, height),
                "stack {stack} axis {axis}"
            );
        }
    }
}

/// A registry of `n` small stacks with their pyramids built, as a grid of
/// the viewer asks for them.
fn grid(n: usize) -> (TempDir, TempDir, TempDir) {
    let home = TempDir::new("pyramids-grid-home");
    let src = TempDir::new("pyramids-grid-src");
    let work = TempDir::new("pyramids-grid-work");
    for i in 0..n {
        let study = format!("1.2.9.{i}");
        series(
            &src,
            &study,
            &format!("{study}.1"),
            AXIAL,
            [0.0, 0.0, i as f64],
        );
    }
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
        .write_all(b"a grid key\n")
        .unwrap();
    assert!(child.wait().unwrap().success());
    ok(&home, &["init", "--key", "k"]);
    ok(
        &home,
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            src.path().to_str().unwrap(),
        ],
    );
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
    );
    for s in 1..=n {
        ok(
            &home,
            &[
                "pyramid",
                "build",
                "--stack",
                &s.to_string(),
                "--workers",
                "1",
            ],
        );
    }
    (home, src, work)
}

/// One picture of each of `n` stacks, twice: the grid, then a synced step.
/// Answers the second pass's times in milliseconds, sorted.
fn open_twice(server: &Server, n: usize) -> Vec<f64> {
    let mut times = Vec::new();
    for pass in 0..2 {
        for s in 1..=n {
            let t = std::time::Instant::now();
            let (status, _) = server.call(
                "GET",
                &format!("/api/instances/{s}/render/3/0"),
                None,
                OPERATOR,
            );
            assert_eq!(status, 200);
            if pass == 1 {
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    times
}

fn audit_rows(home: &TempDir) -> usize {
    let audit: Value = serde_json::from_str(&ok(
        home,
        &[
            "audit",
            "list",
            "--action",
            "instance.open",
            "--limit",
            "1000",
            "--json",
        ],
    ))
    .unwrap();
    audit
        .as_array()
        .or_else(|| audit["rows"].as_array())
        .map_or(0, Vec::len)
}

/// Record 45: a grid of more stacks than the audit's old window of 200 rows
/// writes one row per stack opened, and no second one on its next step.
#[test]
fn a_grid_of_many_stacks_writes_one_audit_row_per_stack() {
    const GRID: usize = 230;
    let (home, _src, _work) = grid(GRID);
    let server = Server::start(&home);
    open_twice(&server, GRID);
    assert_eq!(audit_rows(&home), GRID);
}

/// Record 45, the viewer's grid: how long the render door takes per tile
/// when a grid of 250 stacks asks for one picture of each, twice. Run by
/// hand (`--ignored`); it prints the second pass's median, 95th percentile
/// and total.
#[test]
#[ignore]
fn the_render_door_s_time_per_tile() {
    const GRID: usize = 250;
    let (home, _src, _work) = grid(GRID);
    let server = Server::start(&home);
    let times = open_twice(&server, GRID);
    eprintln!(
        "render door, {GRID} tiles: median {:.2} ms, p95 {:.2} ms, total {:.0} ms; {} audit rows",
        times[GRID / 2],
        times[GRID * 95 / 100],
        times.iter().sum::<f64>(),
        audit_rows(&home)
    );
}
