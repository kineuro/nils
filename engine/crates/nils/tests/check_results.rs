// SPDX-License-Identifier: AGPL-3.0-only
//! Record 55 H3, job type 3 (check a result), the engine's first slice: a
//! mask registered at the door with the scan its grid is on is tied to it
//! voxel for voxel; a mask a run made is read by `nils derivative parts`
//! into the parts its descriptor declares; the parts door pages them; the
//! layer doors answer 202 with the job that builds the layer, then the
//! manifest, the tiles and the slab as the instance doors answer a picture,
//! on the scan and on another scan of its frame, never on a scan of
//! another frame; detail plain is refused; a rater reads them through an
//! open campaign that holds the result, and not once it is closed; every
//! opening is audited naming the result; the list narrows by check, run
//! and output; and the jobs door queues the two verbs and no other
//! derivative verb. On SQLite and on Postgres. Every file is synthetic.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_registry::Store;
use serde_json::{Value, json};

const ISSUER: &str = "https://desk.example/";
const NZ: u32 = 6;
const NY: u32 = 40;
const NX: u32 = 36;

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/oidc")
}

struct Home {
    dir: TempDir,
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

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(self.ok(args).trim()).unwrap()
    }

    fn store(&self) -> Store {
        match &self.pg {
            Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
            None => Store::open_sqlite(&self.dir.path().join("registry.db")).unwrap(),
        }
    }

    fn int(&self, sql: &str) -> i64 {
        let mut store = self.store();
        let mut sql = sql.to_string();
        for t in ["stack", "series", "place", "pipeline"] {
            sql = sql.replace(&format!("{{{t}}}"), &store.qualified(t));
        }
        store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
    }
}

fn us(tag: dicom_core::Tag, v: u16) -> synth::Elem {
    synth::bytes(tag, VR::US, v.to_le_bytes().to_vec())
}

/// A series of NZ axial planes of NY by NX, 0.5 by 0.5 mm, 2 mm apart,
/// in the frame of reference named.
fn series(dir: &TempDir, study: &str, series: &str, frame: &str) {
    for z in 0..NZ {
        let sop = format!("{series}.{}", z + 1);
        let mut e = synth::minimal_mr(study, series, &sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t2 flair"));
        e.push(synth::text(tags::FRAME_OF_REFERENCE_UID, VR::UI, frame));
        e.push(synth::text(
            tags::INSTANCE_NUMBER,
            VR::IS,
            &(z + 1).to_string(),
        ));
        e.push(synth::text(
            tags::IMAGE_POSITION_PATIENT,
            VR::DS,
            &format!("0\\0\\{}", z * 2),
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

/// A NIfTI-1 mask of u8 labels with an sform.
fn nifti(dims: [usize; 3], affine: [[f64; 4]; 3], labels: &[u8]) -> Vec<u8> {
    let mut h = vec![0u8; 352];
    h[0..4].copy_from_slice(&348i32.to_le_bytes());
    let dim: [i16; 8] = [
        3,
        dims[0] as i16,
        dims[1] as i16,
        dims[2] as i16,
        1,
        1,
        1,
        1,
    ];
    for (i, d) in dim.iter().enumerate() {
        h[40 + 2 * i..42 + 2 * i].copy_from_slice(&d.to_le_bytes());
    }
    h[70..72].copy_from_slice(&2i16.to_le_bytes());
    h[72..74].copy_from_slice(&8i16.to_le_bytes());
    for (i, v) in [1.0f32, 0.5, 0.5, 2.0].iter().enumerate() {
        h[76 + 4 * i..80 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    h[108..112].copy_from_slice(&352f32.to_le_bytes());
    h[254..256].copy_from_slice(&1i16.to_le_bytes());
    for (r, at) in [280usize, 296, 312].iter().enumerate() {
        for c in 0..4 {
            h[at + 4 * c..at + 4 * c + 4].copy_from_slice(&(affine[r][c] as f32).to_le_bytes());
        }
    }
    h[344..348].copy_from_slice(b"n+1\0");
    h.extend_from_slice(labels);
    h
}

/// A lesion mask on the scan's own grid, as dcm2niix lays it out (rows
/// flipped): a block of 4 by 4 voxels on plane 1 and one voxel on plane
/// 4, label 99.
fn lesions() -> Vec<u8> {
    let dims = [NX as usize, NY as usize, NZ as usize];
    let at = |i: usize, j: usize, k: usize| i + dims[0] * (j + dims[1] * k);
    let mut labels = vec![0u8; dims.iter().product()];
    for j in 20..24 {
        for i in 5..9 {
            labels[at(i, j, 1)] = 99;
        }
    }
    labels[at(30, 5, 4)] = 99;
    let affine = [
        [-0.5, 0.0, 0.0, 0.0],
        [0.0, 0.5, 0.0, -0.5 * (NY - 1) as f64],
        [0.0, 0.0, 2.0, 0.0],
    ];
    nifti(dims, affine, &labels)
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
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
    fn start(home: &Home) -> Server {
        let trust = format!(
            "issuer={ISSUER},audience=nils,jwks={}",
            fixtures().join("jwks.json").display()
        );
        let child = nils()
            .arg("--registry")
            .arg(home.dir.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "4",
                "--worker",
                "--auth",
                "oidc",
                "--oidc-trust",
                &trust,
            ])
            .args(["--pack-dir", packs().to_str().unwrap()])
            .env("USER", "cleo")
            .env("HOSTNAME", "lab")
            .env_remove("NILS_DSN")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL")
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
        std::thread::spawn(move || for _ in lines {});
        held
    }

    /// A call: the status, the headers, the body's bytes.
    fn raw(
        &self,
        method: &str,
        path: &str,
        body: Option<(&str, &[u8])>,
        token: &str,
    ) -> (u16, Vec<(String, String)>, Vec<u8>) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let (kind, bytes) = body.unwrap_or(("application/json", b""));
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: {kind}\r\nAuthorization: Bearer {token}\r\n\r\n",
            bytes.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(bytes).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let split = response
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("a header block");
        let head = String::from_utf8_lossy(&response[..split]).to_string();
        let mut lines = head.lines();
        let status: u16 = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = lines
            .filter_map(|l| l.split_once(": "))
            .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
            .collect();
        (status, headers, response[split + 4..].to_vec())
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> (u16, Value) {
        let text = body.map(|b| b.to_string()).unwrap_or_default();
        let (status, _, bytes) = self.raw(
            method,
            path,
            Some(("application/json", text.as_bytes())),
            token,
        );
        let doc = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({"bytes": bytes.len()}));
        (status, doc)
    }

    fn ok(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> Value {
        let (status, doc) = self.call(method, path, body, token);
        assert!(
            (200..300).contains(&status),
            "{method} {path}: {status} {doc}"
        );
        doc
    }

    /// Ask until the door answers something other than 202.
    fn after_building(&self, path: &str, token: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
        for _ in 0..600 {
            let r = self.raw("GET", path, None, token);
            if r.0 != 202 {
                return r;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("{path} was still building after a minute");
    }
}

/// A token the desk would sign: the subject, its grants and its detail.
fn token(sub: &str, grants: &[&str], detail: &str) -> String {
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
        &json!({"iss": ISSUER, "aud": "nils", "sub": sub, "grants": grants, "detail": detail,
                "exp": now + 3600, "iat": now}),
        &key,
    )
    .unwrap()
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

const DESCRIPTOR: &str = r#"name: lesions-test
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/lesions@sha256:59c45f54a1f1dc69134f63bec91a726e41c71c64a16cc21cda0b54526910a3c3"
x-nils:
  analysis-level: stack
  input: {layout: stacks}
  outputs:
    - id: labels
      kind: mask
      path-template: "stack-{stack}/lesions.nii.gz"
      parts:
        - {key: lesions, words: Lesions, kind: components, labels: [99]}
"#;

#[test]
fn a_result_is_read_into_its_parts_and_drawn_on_its_scans() {
    checked(None);
}

#[test]
fn a_result_is_read_into_its_parts_and_drawn_on_its_scans_on_postgres() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        return;
    };
    let schema = "nils_check_results";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    checked(Some((dsn.clone(), schema.to_string())));
    drop();
}

fn checked(pg: Option<(String, String)>) {
    let home = Home {
        dir: TempDir::new("check-home"),
        pg,
    };
    let src = TempDir::new("check-src");
    // two scans in one frame, one in another
    series(&src, "1.2.3.A", "1.2.3.A.1", "1.2.3.F.1");
    series(&src, "1.2.3.A", "1.2.3.A.2", "1.2.3.F.1");
    series(&src, "1.2.3.A", "1.2.3.A.3", "1.2.3.F.2");
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a check test key\n"));
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
    home.ok(&[
        "digest",
        "--name",
        "a",
        "--no-private",
        src.path().to_str().unwrap(),
    ]);
    home.ok(&["fingerprint"]);
    home.ok(&["classify", "--pack-dir", packs().to_str().unwrap()]);
    let work = TempDir::new("check-work");
    home.ok(&[
        "place",
        "add",
        "scratch",
        work.path().to_str().unwrap(),
        "--role",
        "working",
    ]);
    let stack_of = |uid: &str| {
        home.int(&format!(
            "SELECT st.id FROM {{stack}} st JOIN {{series}} se ON se.id = st.series_id WHERE se.series_instance_uid = '{uid}'"
        ))
    };
    let (a, b, c) = (
        stack_of("1.2.3.A.1"),
        stack_of("1.2.3.A.2"),
        stack_of("1.2.3.A.3"),
    );
    home.ok(&["pyramid", "build", "--stack", &a.to_string()]);

    // a mask a run made, as a stack unit's: the run's row and its file
    let yml = work.path().join("lesions-test.yml");
    std::fs::write(&yml, DESCRIPTOR).unwrap();
    home.ok(&["pipeline", "add", yml.to_str().unwrap()]);
    let pipeline = home.int("SELECT id FROM {pipeline} WHERE name = 'lesions-test'");
    let place = home.int("SELECT id FROM {place} WHERE name = 'scratch'");
    let mask = lesions();
    let sha = sha256(&mask);
    let (run, made) = {
        let mut store = home.store();
        let run = nils_registry::pipeline::start(
            &mut store,
            &nils_registry::pipeline::NewRun {
                pipeline_id: pipeline,
                job_id: None,
                handle_id: None,
                selection: None,
                params: &json!({}),
                runtime: "podman",
                runtime_version: "test",
                host: "lab",
                device: "cpu",
                model_ids: &[],
                label_set_id: None,
                place_id: Some(place),
                scratch_place_id: None,
                principal: "ops@lab",
                actor: None,
                started_at: "2026-10-11T10:00:00Z",
            },
        )
        .unwrap();
        let rel = format!("derivatives/lesions-test/{run}/stack-{a}/lesions.nii.gz");
        std::fs::create_dir_all(work.path().join(&rel).parent().unwrap()).unwrap();
        std::fs::write(work.path().join(&rel), &mask).unwrap();
        let belongs = nils_registry::derivative::belongs(&mut store, Some(a), None, None, None)
            .unwrap_or_else(|_| panic!("stack {a}"));
        let id = nils_registry::derivative::insert_of_run(
            &mut store,
            &nils_registry::derivative::New {
                kind: "mask",
                belongs: &belongs,
                place_id: place,
                path: &rel,
                bytes: mask.len() as i64,
                sha256: &sha,
                media_type: "application/x-nifti",
                registered_by: "ops@lab",
                actor: None,
                model_id: None,
                run_id: None,
                preprocess_version: None,
                supersedes_id: None,
                created_at: "2026-10-11T10:00:00Z",
            },
            run,
        )
        .unwrap();
        (run, id)
    };

    // the job a check queues for a result registered before: its grid,
    // its scan, its parts as the descriptor declares them
    let read = home.json(&["derivative", "parts", &made.to_string(), "--json"]);
    let one = &read["results"][0];
    assert_eq!(one["grid_link"], "same", "{read}");
    assert_eq!(one["grid_stack"], a, "{read}");
    assert_eq!(
        one["sets"][0],
        json!({"set": "lesions", "parts": 2}),
        "{read}"
    );
    let again = home.json(&["derivative", "parts", "--run", &run.to_string(), "--json"]);
    assert_eq!(again["results"][0]["skipped"], "made already", "{again}");
    let shown = home.json(&["derivative", "show", &made.to_string(), "--json"]);
    assert_eq!(shown["grid_link"], "same");
    assert_eq!(shown["grid_stack_id"], a);
    assert_eq!(shown["part_sets"][0]["key"], "lesions");
    assert_eq!(shown["part_sets"][0]["count"], 2);
    assert_eq!(shown["check"], Value::Null);

    let server = Server::start(&home);
    let ops = token(
        "ops",
        &[
            "pipelines:see",
            "pipelines:work",
            "campaigns:work",
            "query:work",
            "data:see",
            "review:work",
        ],
        "quasi",
    );
    let plain = token("pat", &["pipelines:see"], "plain");
    let anna = token("anna", &["campaigns:work"], "quasi");
    let bo = token("bo", &["campaigns:work"], "quasi");

    // the doors are listed, each with its policy row
    let caps = server.ok("GET", "/api/capabilities", None, &ops);
    for door in [
        "GET /api/derivatives/{id}/parts",
        "GET /api/derivatives/{id}/layers/{stack}/{set}/manifest",
        "GET /api/derivatives/{id}/layers/{stack}/{set}/tiles/{level}/{z}",
        "GET /api/derivatives/{id}/layers/{stack}/{set}/slab/{level}/{z0}-{z1}",
    ] {
        assert!(
            caps["doors"].as_array().unwrap().iter().any(|d| d == door),
            "{door}"
        );
        assert!(
            caps["policy"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["door"] == door),
            "{door}"
        );
    }
    assert_eq!(caps["derivatives"]["layers"]["detail"], "quasi");
    assert_eq!(caps["derivatives"]["parts"]["set_max"], 10_000);

    // a mask registered at the door with the scan its grid is on
    let path = format!(
        "/api/derivatives?kind=mask&stack={a}&grid_stack={a}&name=whole.nii.gz&sha256={sha}"
    );
    let (status, _, body) = server.raw(
        "POST",
        &path,
        Some(("application/x-nifti", mask.as_slice())),
        &ops,
    );
    let up: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{up}");
    assert_eq!(up["grid_link"], "same", "{up}");
    assert_eq!(up["grid_stack_id"], a, "{up}");
    assert_eq!(up["grid"]["dims"], json!([NX, NY, NZ]), "{up}");
    assert_eq!(up["part_sets"][0]["key"], "whole", "{up}");
    let upload = up["id"].as_i64().unwrap();
    // a grid stack is a mask's, and a stack the registry holds
    let (status, _, _) = server.raw(
        "POST",
        &format!("/api/derivatives?kind=output&stack={a}&grid_stack={a}&sha256={sha}"),
        Some(("application/octet-stream", mask.as_slice())),
        &ops,
    );
    assert_eq!(status, 400);
    let (status, _, _) = server.raw(
        "POST",
        &format!("/api/derivatives?kind=mask&stack={a}&grid_stack=999&sha256={sha}"),
        Some(("application/octet-stream", mask.as_slice())),
        &ops,
    );
    assert_eq!(status, 400);

    // the parts door: a page at a time, in order or largest first
    let parts = server.ok("GET", &format!("/api/derivatives/{made}/parts"), None, &ops);
    assert_eq!(parts["set"], "lesions", "{parts}");
    assert_eq!(parts["kind"], "components", "{parts}");
    assert_eq!(parts["count"], 2, "{parts}");
    let list = parts["parts"].as_array().unwrap();
    // bottom to top: the block on plane 1, then the voxel on plane 4
    assert_eq!(list[0]["label"], 1, "{parts}");
    assert_eq!(list[0]["voxels"], 16, "{parts}");
    assert_eq!(list[0]["volume_mm3"], 8.0, "{parts}");
    assert_eq!(list[1]["voxels"], 1, "{parts}");
    assert_eq!(list[1]["anchor_mm"], json!([15.0, 17.0, 8.0]), "{parts}");
    assert_eq!(list[1]["check"], Value::Null, "{parts}");
    let page = server.ok(
        "GET",
        &format!("/api/derivatives/{made}/parts?order=size&per_page=1&page=1"),
        None,
        &ops,
    );
    assert_eq!(page["parts"].as_array().unwrap().len(), 1, "{page}");
    assert_eq!(page["parts"][0]["voxels"], 1, "{page}");
    let (status, _) = server.call(
        "GET",
        &format!("/api/derivatives/{made}/parts?set=structures"),
        None,
        &ops,
    );
    assert_eq!(status, 404);
    let (status, _) = server.call(
        "GET",
        &format!("/api/derivatives/{made}/parts?order=biggest"),
        None,
        &ops,
    );
    assert_eq!(status, 400);
    // plain detail reads the parts, which are numbers and positions
    server.ok(
        "GET",
        &format!("/api/derivatives/{made}/parts"),
        None,
        &plain,
    );
    let whole = server.ok(
        "GET",
        &format!("/api/derivatives/{upload}/parts"),
        None,
        &ops,
    );
    assert_eq!(
        (&whole["set"], &whole["count"]),
        (&json!("whole"), &json!(0))
    );

    // the layer: 202 with the job that builds it, then as a picture's doors
    let layer =
        |stack: i64, rest: &str| format!("/api/derivatives/{made}/layers/{stack}/lesions/{rest}");
    let (status, _, body) = server.raw("GET", &layer(a, "tiles/0/4"), None, &ops);
    let queued: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 202, "{queued}");
    assert!(queued["job"].as_i64().is_some(), "{queued}");
    let (status, _, body) = server.after_building(&layer(a, "manifest"), &ops);
    let m: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{m}");
    let picture: Value = serde_json::from_str(
        &std::fs::read_to_string(
            work.path()
                .join("pyramids")
                .join(a.to_string())
                .join("manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(m["shape"], picture["shape"], "{m}");
    assert_eq!(m["levels"], picture["levels"], "{m}");
    for (l, p) in m["level_shapes"]
        .as_array()
        .unwrap()
        .iter()
        .zip(picture["level_shapes"].as_array().unwrap())
    {
        assert_eq!((&l["shape"], &l["tiles"]), (&p["shape"], &p["tiles"]));
    }
    assert_eq!(m["link"], "same", "{m}");
    assert_eq!(m["values"], "part", "{m}");
    assert_eq!(m["voxels"], 17, "{m}");
    let (status, headers, body) = server.raw("GET", &layer(a, "tiles/0/4"), None, &ops);
    assert_eq!(status, 200);
    assert!(
        headers
            .iter()
            .any(|(k, v)| k == "x-nils-derivative" && *v == made.to_string())
    );
    assert_eq!(u32_at(&body, 0), 1, "one tile a plane of 40 by 36");
    let (status, _, body) = server.raw("GET", &layer(a, "slab/0/0-6"), None, &ops);
    assert_eq!(status, 200);
    assert_eq!(u32_at(&body, 0), NZ, "six planes");
    for (rest, want) in [
        ("slab/0/0-7", 416),
        ("slab/0/0-40", 416),
        ("slab/0/3-3", 400),
        ("tiles/9/0", 404),
        ("render/0/0", 404),
    ] {
        let (status, _, _) = server.raw("GET", &layer(a, rest), None, &ops);
        assert_eq!(status, want, "{rest}");
    }
    let (status, _, _) = server.raw(
        "GET",
        &format!("/api/derivatives/{made}/layers/{a}/structures/manifest"),
        None,
        &ops,
    );
    assert_eq!(status, 404);
    // on another scan of its frame, by world position; never on another frame's
    let (status, _, body) = server.after_building(&layer(b, "manifest"), &ops);
    let mb: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{mb}");
    assert_eq!(mb["link"], "world", "{mb}");
    assert_eq!(mb["voxels"], 17, "{mb}");
    let (status, _, body) = server.raw("GET", &layer(c, "manifest"), None, &ops);
    assert_eq!(status, 409, "{}", String::from_utf8_lossy(&body));
    // a mask read whole draws whole
    let (status, _, body) = server.after_building(
        &format!("/api/derivatives/{upload}/layers/{a}/whole/manifest"),
        &ops,
    );
    let mw: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        (status, &mw["values"], &mw["voxels"]),
        (200, &json!("mask"), &json!(17))
    );
    // a mask follows anatomy as the pixels do: detail plain is refused
    let (status, _, _) = server.raw("GET", &layer(a, "manifest"), None, &plain);
    assert_eq!(status, 403);

    // a rater reads them through an open campaign that holds the result,
    // and nobody else does
    let selection = json!({"document": {
        "ast_version": 1,
        "params": {"ids": {"type": "list", "value": [a]}},
        "sets": {"s": {"grain": "stack", "where": [["in", {}, ["field", {}, "id"], ["param", {}, "ids"]]]}},
        "out": {"set": "s", "level": "record"},
    }});
    server.ok("PUT", "/api/ask/selections/one", Some(selection), &ops);
    let mut inputs = serde_json::Map::new();
    inputs.insert(format!("stack:{a}"), json!([made]));
    server.ok(
        "POST",
        "/api/campaigns",
        Some(json!({
            "name": "check-one",
            "question": {"kind": "axis", "axis": "base"},
            "source": {"selection": "one@1"},
            "raters": ["anna@desk.example"],
            "raters_per_item": 1,
            "adjudication": {"when": "never"},
            "closes_into": "none",
            "inputs": inputs,
        })),
        &ops,
    );
    server.ok(
        "GET",
        &format!("/api/derivatives/{made}/parts"),
        None,
        &anna,
    );
    let (status, _, _) = server.raw("GET", &layer(a, "manifest"), None, &anna);
    assert_eq!(status, 200);
    let (status, _, _) = server.raw("GET", &layer(a, "slab/0/0-6"), None, &anna);
    assert_eq!(status, 200);
    // not on a scan the campaign does not show, not a result it does not
    // hold: to a caller without pipelines:see, a result not theirs and one
    // not there are one answer, 404 (the review of E1, item 16)
    let (status, _, _) = server.raw("GET", &layer(b, "manifest"), None, &anna);
    assert_eq!(status, 404);
    let (status, held) = server.call(
        "GET",
        &format!("/api/derivatives/{upload}/parts"),
        None,
        &anna,
    );
    assert_eq!(status, 404);
    let (status, absent) = server.call("GET", "/api/derivatives/99999/parts", None, &anna);
    assert_eq!(status, 404);
    assert_eq!(
        held["error"]
            .as_str()
            .unwrap()
            .replace(&upload.to_string(), "N"),
        absent["error"].as_str().unwrap().replace("99999", "N"),
        "not theirs and not there read alike"
    );
    for path in [
        format!("/api/derivatives/{made}/parts"),
        layer(a, "manifest"),
    ] {
        let (status, _, _) = server.raw("GET", &path, None, &bo);
        assert_eq!(status, 404, "{path}");
    }
    home.ok(&["campaign", "close", "check-one"]);
    let (status, _, _) = server.raw("GET", &layer(a, "manifest"), None, &anna);
    assert_eq!(status, 404, "a closed campaign opens nothing");

    // every opening names the result in the audit, once in the window
    let audit = home.json(&[
        "audit",
        "list",
        "--action",
        "instance.open",
        "--limit",
        "1000",
        "--json",
    ]);
    let rows: Vec<&Value> = audit
        .as_array()
        .or_else(|| audit["rows"].as_array())
        .unwrap()
        .iter()
        .filter(|r| r["scope"]["derivative"] == made)
        .collect();
    let who = |p: &str, s: i64| {
        rows.iter()
            .filter(|r| r["principal"] == p && r["scope"]["stack"] == s)
            .count()
    };
    assert_eq!(who("ops@desk.example", a), 1, "{rows:?}");
    assert_eq!(who("ops@desk.example", b), 1, "{rows:?}");
    assert_eq!(who("anna@desk.example", a), 1, "{rows:?}");
    assert!(
        rows.iter()
            .any(|r| r["principal"] == "anna@desk.example" && r["scope"]["campaign"].is_i64())
    );

    // the list narrows by check, run and output; each row says its check
    // and its tie
    let listed = server.ok(
        "GET",
        "/api/derivatives?kind=mask&check=unchecked",
        None,
        &ops,
    );
    let rows = listed["derivatives"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{listed}");
    assert!(
        rows.iter()
            .all(|r| r["grid_link"] == "same" && r["check"].is_null())
    );
    let none = server.ok("GET", "/api/derivatives?check=accepted", None, &ops);
    assert!(none["derivatives"].as_array().unwrap().is_empty());
    let (status, _) = server.call("GET", "/api/derivatives?check=maybe", None, &ops);
    assert_eq!(status, 400);
    let by_output = server.ok(
        "GET",
        &format!("/api/derivatives?run={run}&output=labels"),
        None,
        &ops,
    );
    assert_eq!(by_output["derivatives"][0]["id"], made, "{by_output}");
    let other = server.ok(
        "GET",
        &format!("/api/derivatives?run={run}&output=volumes"),
        None,
        &ops,
    );
    assert!(other["derivatives"].as_array().unwrap().is_empty());
    let (status, _) = server.call("GET", "/api/derivatives?output=labels", None, &ops);
    assert_eq!(status, 400);

    // the jobs door queues a result's parts and layer, and no other
    // derivative verb
    let (status, doc) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["derivative", "add", "/etc/hostname", "--kind", "mask"]})),
        &ops,
    );
    assert_eq!(status, 400, "{doc}");
    let (status, doc) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["derivative", "parts", made.to_string(), "--force"]})),
        &ops,
    );
    assert!((200..300).contains(&status), "{status} {doc}");

    // nothing was written outside the test's own folders: the layers lie
    // beside the scans' pictures in the working place
    let layers = work
        .path()
        .join("pyramids")
        .join(a.to_string())
        .join("layers")
        .join(&sha[..16]);
    assert!(layers.join("lesions").join("manifest.json").is_file());
    assert!(layers.join("whole").join("manifest.json").is_file());
}
