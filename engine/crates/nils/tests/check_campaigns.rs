// SPDX-License-Identifier: AGPL-3.0-only
//! Record 55 H3, job type 3 (check a result), the engine's second slice: a
//! check is made at the campaign door from a run's results, named results
//! or a selection's newest, previewed first and written nowhere; a body is
//! refused as section 8.3 says; each item shows the scan its result was
//! made on and the visit's main scans by role, frozen; unchecked leaves the
//! checked ones out, breached takes the units the run's checks flagged, and
//! a sample is the same for the same seed; the item door carries no day and
//! no subject code, and on a sealed scan no role; a rater opens a panel's
//! picture (and a scan of its frame, "+ scan") only while the check is
//! open; the close writes the statuses the list then narrows by, the ask's
//! measures carry them, and a direct check is refused while a check holds
//! the result, then withdraws the earlier decision. On SQLite and on
//! Postgres. Every file is synthetic. A last test times the preview of
//! 5,000 results.

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

    fn store(&self) -> Store {
        match &self.pg {
            Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
            None => Store::open_sqlite(&self.dir.path().join("registry.db")).unwrap(),
        }
    }

    fn sql(&self, sql: &str) -> String {
        let store = self.store();
        let mut sql = sql.to_string();
        for t in [
            "stack",
            "series",
            "place",
            "pipeline",
            "session_cache",
            "pick",
            "pick_stack",
            "decision",
            "derivative",
        ] {
            sql = sql.replace(&format!("{{{t}}}"), &store.qualified(t));
        }
        sql
    }

    fn int(&self, sql: &str) -> i64 {
        let sql = self.sql(sql);
        self.store().query(&sql, &[]).unwrap()[0].int(0).unwrap()
    }

    fn texts(&self, sql: &str) -> Vec<String> {
        let sql = self.sql(sql);
        self.store()
            .query(&sql, &[])
            .unwrap()
            .iter()
            .map(|r| r.text(0).unwrap().to_string())
            .collect()
    }

    fn exec(&self, sql: &str) {
        let sql = self.sql(sql);
        self.store().execute(&sql, &[]).unwrap();
    }
}

fn us(tag: dicom_core::Tag, v: u16) -> synth::Elem {
    synth::bytes(tag, VR::US, v.to_le_bytes().to_vec())
}

/// A series of NZ axial planes of NY by NX, 0.5 by 0.5 mm, 2 mm apart, in
/// the frame of reference named, of a study on the day named.
fn series(dir: &TempDir, study: &str, series: &str, frame: &str, words: &str, day: &str) {
    for z in 0..NZ {
        let sop = format!("{series}.{}", z + 1);
        let mut e = synth::minimal_mr(study, series, &sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
        e.push(synth::text(tags::STUDY_DATE, VR::DA, day));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, words));
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
/// flipped): a block on plane 1, one voxel on plane 4, and `more` single
/// voxels on plane 5, label 99.
fn lesions(more: usize) -> Vec<u8> {
    let dims = [NX as usize, NY as usize, NZ as usize];
    let at = |i: usize, j: usize, k: usize| i + dims[0] * (j + dims[1] * k);
    let mut labels = vec![0u8; dims.iter().product()];
    for j in 20..24 {
        for i in 5..9 {
            labels[at(i, j, 1)] = 99;
        }
    }
    labels[at(30, 5, 4)] = 99;
    for m in 0..more {
        labels[at(2 + 3 * m, 30, 5)] = 99;
    }
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

    fn raw(
        &self,
        method: &str,
        path: &str,
        body: Option<(&str, &[u8])>,
        token: &str,
    ) -> (u16, Vec<u8>) {
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
        let status: u16 = head
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        (status, response[split + 4..].to_vec())
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> (u16, Value) {
        let text = body.map(|b| b.to_string()).unwrap_or_default();
        let (status, bytes) = self.raw(
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

    /// The status of a door, refused or not: 403 against anything else.
    fn opens(&self, path: &str, token: &str) -> bool {
        self.raw("GET", path, None, token).0 != 403
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
        - {key: structures, words: Structures, kind: labels}
"#;

fn init(home: &Home) {
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
}

/// A pipeline run's row, and a mask of it registered as a stack unit's.
fn run_of(home: &Home, pipeline: i64, place: i64) -> i64 {
    let mut store = home.store();
    nils_registry::pipeline::start(
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
    .unwrap()
}

fn mask_of(
    home: &Home,
    work: &Path,
    run: i64,
    place: i64,
    stack: i64,
    sub: &str,
    bytes: &[u8],
) -> i64 {
    let mut store = home.store();
    let rel = format!("derivatives/lesions-test/{run}/{sub}stack-{stack}/lesions.nii.gz");
    std::fs::create_dir_all(work.join(&rel).parent().unwrap()).unwrap();
    std::fs::write(work.join(&rel), bytes).unwrap();
    let belongs = nils_registry::derivative::belongs(&mut store, Some(stack), None, None, None)
        .unwrap_or_else(|_| panic!("stack {stack}"));
    nils_registry::derivative::insert_of_run(
        &mut store,
        &nils_registry::derivative::New {
            kind: "mask",
            belongs: &belongs,
            place_id: place,
            path: &rel,
            bytes: bytes.len() as i64,
            sha256: &sha256(bytes),
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
    .unwrap()
}

/// A person's pick of a stack for a role on a visit, as the pack's.
fn pick(home: &Home, role: &str, subject: i64, day: &str, stack: i64) {
    home.exec(&format!(
        "INSERT INTO {{pick}} (model, role, subject_id, session_day, scheme, reference, pack, pack_version, actor, author_kind, decided_at) \
         VALUES ('main', '{role}', {subject}, '{day}', 'default', 'registry', 'mri', '1', 'anna', 'person', '2026-10-11T09:00:00Z')"
    ));
    let id = home.int("SELECT MAX(id) FROM {pick}");
    home.exec(&format!(
        "INSERT INTO {{pick_stack}} (pick_id, stack_id) VALUES ({id}, {stack})"
    ));
}

fn check_body(name: &str, source: Value) -> Value {
    json!({
        "name": name,
        "question": {"kind": "check", "part_set": "lesions", "grain": "result",
                     "reasons": [
                        {"id": "under", "words": "Under-segmented"},
                        {"id": "not_lesion", "words": "Not a lesion"},
                        {"id": "missed", "words": "Missed lesion", "needs": "point"},
                        {"id": "other", "words": "Other", "needs": "note"}]},
        "source": source,
        "panels": ["made_on", "t1w", "flair"],
        "raters": ["anna@desk.example"],
        "raters_per_item": 1,
        "closes_into": "check",
    })
}

#[test]
fn a_check_is_made_read_and_closed_into_statuses() {
    checks(None);
}

#[test]
fn a_check_is_made_read_and_closed_into_statuses_on_postgres() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        return;
    };
    let schema = "nils_check_campaigns";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    checks(Some((dsn.clone(), schema.to_string())));
    drop();
}

fn checks(pg: Option<(String, String)>) {
    let home = Home {
        dir: TempDir::new("checks-home"),
        pg,
    };
    let src = TempDir::new("checks-src");
    // visit A: the FLAIR the masks are made on, a T1w and an extra scan in
    // its frame, and a scan of another frame; visit B: two scans in a frame
    series(
        &src,
        "1.2.3.A",
        "1.2.3.A.1",
        "1.2.3.F.1",
        "t2 flair",
        "20210203",
    );
    series(
        &src,
        "1.2.3.A",
        "1.2.3.A.2",
        "1.2.3.F.1",
        "t1 mprage",
        "20210203",
    );
    series(
        &src,
        "1.2.3.A",
        "1.2.3.A.3",
        "1.2.3.F.2",
        "t2 flair",
        "20210203",
    );
    series(
        &src,
        "1.2.3.A",
        "1.2.3.A.4",
        "1.2.3.F.1",
        "dwi trace",
        "20210203",
    );
    series(
        &src,
        "1.2.3.B",
        "1.2.3.B.1",
        "1.2.3.F.3",
        "t2 flair",
        "20220104",
    );
    series(
        &src,
        "1.2.3.B",
        "1.2.3.B.2",
        "1.2.3.F.3",
        "t1 mprage",
        "20220104",
    );
    init(&home);
    home.ok(&[
        "digest",
        "--name",
        "a",
        "--no-private",
        src.path().to_str().unwrap(),
    ]);
    home.ok(&["fingerprint"]);
    home.ok(&["classify", "--pack-dir", packs().to_str().unwrap()]);
    home.ok(&["session", "rebuild"]);
    let work = TempDir::new("checks-work");
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
    let (a, b, c, x, d, e) = (
        stack_of("1.2.3.A.1"),
        stack_of("1.2.3.A.2"),
        stack_of("1.2.3.A.3"),
        stack_of("1.2.3.A.4"),
        stack_of("1.2.3.B.1"),
        stack_of("1.2.3.B.2"),
    );
    let subject = home.int(&format!(
        "SELECT se.subject_id FROM {{stack}} st JOIN {{series}} se ON se.id = st.series_id WHERE st.id = {a}"
    ));
    let days = home.texts("SELECT first FROM {session_cache} ORDER BY first");
    assert_eq!(days.len(), 2, "two visits: {days:?}");
    let day = |i: usize| days[i].chars().take(10).collect::<String>();
    // the main scans: visit A's FLAIR is the made-on scan, its T1w another
    // of its frame; visit B has a T1w and no FLAIR
    pick(&home, "flair", subject, &day(0), a);
    pick(&home, "t1w", subject, &day(0), b);
    pick(&home, "t1w", subject, &day(1), e);
    // the made-on scans' pictures are built, so the masks tie to them voxel
    // for voxel as they are read
    for s in [a, d] {
        home.ok(&["pyramid", "build", "--stack", &s.to_string()]);
    }

    // three masks of one run: two on visit A's FLAIR, one on visit B's
    let yml = work.path().join("lesions-test.yml");
    std::fs::write(&yml, DESCRIPTOR).unwrap();
    home.ok(&["pipeline", "add", yml.to_str().unwrap()]);
    let pipeline = home.int("SELECT id FROM {pipeline} WHERE name = 'lesions-test'");
    let place = home.int("SELECT id FROM {place} WHERE name = 'scratch'");
    let run = run_of(&home, pipeline, place);
    let m1 = mask_of(&home, work.path(), run, place, a, "", &lesions(0));
    let m2 = mask_of(&home, work.path(), run, place, a, "again/", &lesions(1));
    let m3 = mask_of(&home, work.path(), run, place, d, "", &lesions(0));
    // a unit the run's declared checks flagged
    {
        let mut store = home.store();
        nils_registry::review::raise_pipeline_qc(
            &mut store,
            &nils_registry::review::PipelineQc {
                run_id: run,
                pipeline: "lesions-test@1",
                unit: &format!("stack-{d}"),
                stack_id: Some(d),
                subject_id: Some(subject),
                session_day: None,
                status: "breach",
                error: Some("holes <= 2"),
                metrics: &json!({}),
                job_id: None,
            },
            "2026-10-11T10:30:00Z",
        )
        .unwrap();
    }

    let server = Server::start(&home);
    let ops = token(
        "ops",
        &[
            "pipelines:see",
            "pipelines:work",
            "campaigns:see",
            "campaigns:work",
            "query:see",
            "query:work",
            "data:see",
            "review:see",
            "review:work",
        ],
        "quasi",
    );
    let anna = token("anna", &["campaigns:see", "campaigns:work"], "quasi");
    let bo = token("bo", &["campaigns:see", "campaigns:work"], "quasi");
    let maker = token("mo", &["campaigns:work", "campaigns:see"], "quasi");

    // the capabilities name the questions, the check's limits and its doors
    let caps = server.ok("GET", "/api/capabilities", None, &ops);
    assert!(
        caps["campaigns"]["questions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|q| q == "check"),
        "{caps}"
    );
    assert_eq!(caps["campaigns"]["check"]["panels_max"], 8);
    assert_eq!(
        caps["campaigns"]["check"]["presets"]["components"][5]["needs"],
        "point"
    );
    assert_eq!(caps["derivatives"]["check"]["grant"], "review:work");
    for door in [
        "POST /api/campaigns/preview",
        "GET /api/campaigns/{id}/items/{item}/check",
        "POST /api/derivatives/{id}/check",
        "POST /api/derivatives/{id}/check/withdraw",
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

    // the preview writes nothing: the parts of the masks are not read yet,
    // so it counts them and queues their job once
    let source = json!({"run": run, "output": "labels", "which": "unchecked"});
    let pending = server.ok(
        "POST",
        "/api/campaigns/preview",
        Some(check_body("x", source.clone())),
        &ops,
    );
    assert_eq!(pending["parts_pending"], 3, "{pending}");
    assert_eq!(
        pending["parts_jobs"].as_array().unwrap().len(),
        1,
        "{pending}"
    );
    let (status, refused) = server.call(
        "POST",
        "/api/campaigns",
        Some(check_body("too-early", source.clone())),
        &ops,
    );
    assert_eq!(status, 409, "{refused}");
    // the job reads the three masks
    let mut read = Value::Null;
    for _ in 0..600 {
        read = server.ok(
            "POST",
            "/api/campaigns/preview",
            Some(check_body("x", source.clone())),
            &ops,
        );
        if read["parts_pending"] == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(read["parts_pending"], 0, "{read}");
    assert_eq!(read["results"], 3, "{read}");
    assert_eq!(read["items"], 3, "{read}");
    assert_eq!(read["parts"], 7, "two, three and two lesions: {read}");
    assert_eq!(read["drawable"], 3, "{read}");
    assert_eq!(read["checked_already"], 0, "{read}");
    assert_eq!(read["panels_missing"], json!({"flair": 1}), "{read}");
    assert_eq!(read["panels"]["most"], 2, "{read}");
    assert_eq!(read["pictures_to_build"], 2, "{read}");
    assert_eq!(read["layers_to_build"], 6, "{read}");
    // a part grain reads every lesion
    let mut part = check_body("x", source.clone());
    part["question"]["grain"] = json!("part");
    let parts = server.ok("POST", "/api/campaigns/preview", Some(part), &ops);
    assert_eq!(parts["items"], 7, "{parts}");

    // refused as section 8.3 says, naming the field
    let refusal = |body: Value| {
        let (status, doc) = server.call("POST", "/api/campaigns/preview", Some(body), &ops);
        assert_eq!(status, 400, "{doc}");
        doc["error"].as_str().unwrap().to_string()
    };
    let mut b1 = check_body("x", source.clone());
    b1["question"]["part_set"] = json!("vessels");
    assert!(refusal(b1).contains("question.part_set"));
    let mut b2 = check_body("x", source.clone());
    b2["question"]["part_set"] = json!("structures");
    b2["question"]["grain"] = json!("part");
    assert!(refusal(b2).contains("question.grain"));
    let mut b3 = check_body("x", source.clone());
    b3["question"]["reasons"] = json!([]);
    assert!(refusal(b3).contains("question.reasons"));
    let mut b4 = check_body("x", source.clone());
    b4["question"]["reasons"] = json!(
        (0..8)
            .map(|i| json!({"id": format!("r{}", "x".repeat(i + 1)), "words": "W"}))
            .collect::<Vec<_>>()
    );
    assert!(refusal(b4).contains("question.reasons"));
    let mut b5 = check_body("x", source.clone());
    b5["panels"] = json!(["made_on", "t1w", "swi"]);
    assert!(refusal(b5).contains("panels"));
    let mut b6 = check_body("x", source.clone());
    b6["panels"] = json!(["made_on", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8"]);
    assert!(refusal(b6).contains("at most 8"));
    let many: Vec<i64> = (1..=5001).collect();
    assert!(refusal(check_body("x", json!({"derivatives": many}))).contains("source.derivatives"));
    let mut b7 = check_body("x", source.clone());
    b7["source"]["output"] = json!("volumes");
    assert!(refusal(b7).contains("source.output"));
    // making or previewing a check needs pipelines:see beside campaigns:work
    let (status, _) = server.call(
        "POST",
        "/api/campaigns/preview",
        Some(check_body("x", source.clone())),
        &maker,
    );
    assert_eq!(status, 403);

    // sources: breached takes the unit the run's checks flagged; named
    // results; a selection's newest result of the output for each scan
    let breached = server.ok(
        "POST",
        "/api/campaigns/preview",
        Some(check_body(
            "x",
            json!({"run": run, "output": "labels", "which": "breached"}),
        )),
        &ops,
    );
    assert_eq!(breached["items"], 1, "{breached}");
    let named = server.ok(
        "POST",
        "/api/campaigns/preview",
        Some(check_body("x", json!({"derivatives": [m1, m3]}))),
        &ops,
    );
    assert_eq!(named["items"], 2, "{named}");
    let selection = json!({"document": {
        "ast_version": 1,
        "params": {"ids": {"type": "list", "value": [a]}},
        "sets": {"s": {"grain": "stack", "where": [["in", {}, ["field", {}, "id"], ["param", {}, "ids"]]]}},
        "out": {"set": "s", "level": "record"},
    }});
    server.ok("PUT", "/api/ask/selections/visit-a", Some(selection), &ops);
    let newest = server.ok(
        "POST",
        "/api/campaigns/preview",
        Some(check_body(
            "x",
            json!({"selection": "visit-a@1", "pipeline": "lesions-test", "output": "labels"}),
        )),
        &ops,
    );
    assert_eq!(newest["items"], 1, "{newest}");
    assert_eq!(newest["parts"], 3, "the newer mask on the scan: {newest}");

    // a sample is the same for the same seed
    let sampled = |name: &str| -> i64 {
        let made = server.ok(
            "POST",
            "/api/campaigns",
            Some(check_body(
                name,
                json!({"run": run, "output": "labels", "which": "all", "sample": 1, "seed": "written first"}),
            )),
            &ops,
        );
        assert_eq!(made["source"]["seed"], "written first", "{made}");
        let id = made["id"].as_i64().unwrap();
        let item = made["items"][0]["derivative_id"].as_i64().unwrap();
        server.ok("POST", &format!("/api/campaigns/{id}/close"), None, &ops);
        item
    };
    assert_eq!(sampled("sample-1"), sampled("sample-2"));

    // a direct check of one result, then the check leaves it out
    let direct = server.ok(
        "POST",
        &format!("/api/derivatives/{m2}/check"),
        Some(json!({"value": "accepted"})),
        &ops,
    );
    assert_eq!(direct["check"], "accepted", "{direct}");
    let (status, _) = server.call(
        "POST",
        &format!("/api/derivatives/{m2}/check"),
        Some(json!({"value": "accepted"})),
        &anna,
    );
    assert_eq!(status, 403, "a direct check needs review:work");
    let after = server.ok(
        "POST",
        "/api/campaigns/preview",
        Some(check_body("x", source.clone())),
        &ops,
    );
    assert_eq!(after["checked_already"], 1, "{after}");
    assert_eq!(
        after["items"], 2,
        "unchecked leaves the checked out: {after}"
    );

    // the check itself: all three results, read by anna
    let made = server.ok(
        "POST",
        "/api/campaigns",
        Some(check_body(
            "lesions run",
            json!({"run": run, "output": "labels", "which": "all"}),
        )),
        &ops,
    );
    let check = made["id"].as_i64().unwrap();
    assert_eq!(made["closes_into"], "check", "{made}");
    assert_eq!(made["grain"], "result", "{made}");
    assert!(made["content_hash"].is_string(), "{made}");
    assert_eq!(made["items"].as_array().unwrap().len(), 3, "{made}");
    assert!(made["layers"]["job"].is_i64(), "{made}");
    // the check keeps no list of the roles its panels show, which a rater
    // could read a sealed panel's role from (the review of E2, item 9)
    let doc = server.ok("GET", &format!("/api/campaigns/{check}"), None, &anna);
    assert!(doc["source"]["counts"]["panels"].is_null(), "{doc}");
    assert!(doc["source"]["counts"]["panels_missing"].is_null(), "{doc}");
    assert!(!doc.to_string().contains("t1w"), "{doc}");
    assert_eq!(made["pictures"]["stacks"], 4, "{made}");
    // an open check holds its results: no direct check, no second check
    let (status, held) = server.call(
        "POST",
        &format!("/api/derivatives/{m1}/check"),
        Some(json!({"value": "accepted"})),
        &ops,
    );
    assert_eq!(status, 409, "{held}");
    let again = server.ok(
        "POST",
        "/api/campaigns/preview",
        Some(check_body(
            "x",
            json!({"run": run, "output": "labels", "which": "all"}),
        )),
        &ops,
    );
    assert_eq!(again["held"], 3, "{again}");

    // a rater opens a panel's picture, and a scan of the made-on scan's
    // frame, while the check is open; never another frame's; nobody else
    assert!(server.opens(&format!("/api/instances/{b}/manifest"), &anna));
    assert!(server.opens(&format!("/api/instances/{x}/manifest"), &anna));
    assert!(!server.opens(&format!("/api/instances/{c}/manifest"), &anna));
    assert!(!server.opens(&format!("/api/instances/{b}/manifest"), &bo));
    assert!(server.opens(&format!("/api/derivatives/{m1}/parts"), &anna));
    assert!(server.opens(
        &format!("/api/derivatives/{m1}/layers/{b}/lesions/manifest"),
        &anna
    ));

    // visit B's T1w is of a sample sealed now
    let sealed = json!({"document": {
        "ast_version": 1,
        "params": {"ids": {"type": "list", "value": [e]}},
        "sets": {"s": {"grain": "stack", "where": [["in", {}, ["field", {}, "id"], ["param", {}, "ids"]]]}},
        "out": {"set": "s", "level": "record"},
    }});
    server.ok("PUT", "/api/ask/selections/sealed-e", Some(sealed), &ops);
    home.ok(&[
        "labels",
        "seal",
        "--select",
        "selection:sealed-e@1",
        "--pack-dir",
        packs().to_str().unwrap(),
    ]);

    // the reader: claim, read the item, answer
    let items: Vec<(i64, i64)> = made["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["id"].as_i64().unwrap(),
                i["derivative_id"].as_i64().unwrap(),
            )
        })
        .collect();
    for _ in 0..3 {
        let claimed = server.ok(
            "POST",
            &format!("/api/campaigns/{check}/claim"),
            Some(json!({})),
            &anna,
        );
        let item = claimed["item"]["id"].as_i64().unwrap();
        let assignment = claimed["assignment"]["id"].as_i64().unwrap();
        // each item ahead names its panels' scans and the result's layer on
        // each, so the reader warms the next masks with their pictures
        for a in claimed["ahead"].as_array().unwrap() {
            let stacks = a["stacks"].as_array().unwrap();
            let layers = a["layers"].as_array().unwrap();
            assert_eq!(stacks.len(), layers.len(), "{claimed}");
            for (s, l) in stacks.iter().zip(layers) {
                assert_eq!(&l["stack"], s, "{claimed}");
                assert!(l["derivative"].is_i64(), "{claimed}");
                assert_eq!(l["set"], "lesions", "{claimed}");
            }
        }
        let doc = server.ok(
            "GET",
            &format!("/api/campaigns/{check}/items/{item}/check"),
            None,
            &anna,
        );
        let d = items.iter().find(|(i, _)| *i == item).unwrap().1;
        assert_eq!(doc["result"]["id"], d, "{doc}");
        assert_eq!(doc["result"]["pipeline"], "lesions-test", "{doc}");
        assert_eq!(doc["result"]["output"], "labels", "{doc}");
        assert_eq!(doc["set"]["kind"], "components", "{doc}");
        assert_eq!(doc["of"], 3, "{doc}");
        assert!(doc["mine"].is_null(), "{doc}");
        // no day, no subject code, no study time
        let text = doc.to_string();
        for never in ["2021", "2022", "\"subject", "session_day", "P1"] {
            assert!(!text.contains(never), "{never} in {doc}");
        }
        let shows: Vec<&str> = doc["panels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["shows"].as_str().unwrap())
            .collect();
        if d == m3 {
            // visit B's made-on scan has no role picked, and its T1w is
            // sealed: each shows its series description, the T1w no role
            assert_eq!(doc["panels"][1]["stack"], e, "{doc}");
            assert_eq!(shows, ["t2 flair", "t1 mprage"], "{doc}");
            assert!(doc["panels"][1]["role"].is_null(), "{doc}");
            assert_eq!(doc["parts"].as_array().unwrap().len(), 2, "{doc}");
        } else {
            assert_eq!(shows, ["FLAIR", "T1w"], "{doc}");
            assert_eq!(doc["panels"][0]["made_on"], true, "{doc}");
            assert_eq!(doc["panels"][0]["link"], "same", "{doc}");
            assert_eq!(doc["panels"][1]["link"], "world", "{doc}");
            assert_eq!(doc["more"][0]["stack"], x, "{doc}");
        }
        // the flag on the first result, the second accepted, the third
        // rejected with a reason that needs its note
        let body = if d == m1 {
            json!({"value": "flagged", "form": {"parts": [{"label": 1, "reason": "under"}], "seen": [1, 2], "shown": [x]}})
        } else if d == m2 {
            json!({"value": "accepted", "form": {"seen": [1, 2, 3]}})
        } else {
            json!({"value": "rejected", "form": {"reasons": ["other"]}, "why": "the wrong scan"})
        };
        server.ok(
            "POST",
            &format!("/api/campaigns/{check}/assignments/{assignment}/answer"),
            Some(body),
            &anna,
        );
        let mine = server.ok(
            "GET",
            &format!("/api/campaigns/{check}/items/{item}/check"),
            None,
            &anna,
        );
        assert!(mine["mine"]["seconds"].is_number(), "{mine}");
    }
    // the item door is the check's raters' and adjudicators' alone: a
    // reviewer who rates in it does not read it (the review of E2, item 17)
    let (status, _) = server.call(
        "GET",
        &format!("/api/campaigns/{check}/items/{}/check", items[0].0),
        None,
        &ops,
    );
    assert_eq!(status, 403, "a reviewer is no rater of the check");
    assert!(
        made["source"]["warm_job"].is_i64() || made["layers"]["job"].is_null(),
        "{made}"
    );
    // the item door is a rater's: bo reads none of it
    let (status, _) = server.call(
        "GET",
        &format!("/api/campaigns/{check}/items/{}/check", items[0].0),
        None,
        &bo,
    );
    assert!(status == 403 || status == 404, "{status}");
    // stats count verdicts and reasons
    let stats = server.ok("GET", &format!("/api/campaigns/{check}/stats"), None, &ops);
    assert_eq!(stats["all"]["verdicts"]["flagged"], 1, "{stats}");
    assert_eq!(stats["all"]["verdicts"]["rejected"], 1, "{stats}");
    assert_eq!(stats["all"]["reasons"]["under"], 1, "{stats}");

    // the close writes the statuses, which the list narrows by
    let closed = server.ok("POST", &format!("/api/campaigns/{check}/close"), None, &ops);
    assert_eq!(closed["resolved"], 3, "{closed}");
    let by = |c: &str| -> Vec<i64> {
        server.ok("GET", &format!("/api/derivatives?check={c}"), None, &ops)["derivatives"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["id"].as_i64().unwrap())
            .collect()
    };
    assert_eq!(by("flagged"), vec![m1]);
    assert_eq!(by("accepted"), vec![m2]);
    assert_eq!(by("rejected"), vec![m3]);
    let parts = server.ok("GET", &format!("/api/derivatives/{m1}/parts"), None, &ops);
    assert_eq!(parts["parts"][0]["check"], "flagged", "{parts}");
    assert_eq!(parts["parts"][0]["check_reason"], "under", "{parts}");
    assert_eq!(parts["parts"][1]["check"], "accepted", "{parts}");
    // the direct check of m2 was withdrawn by the close, never deleted
    assert_eq!(
        home.int(&format!(
            "SELECT COUNT(*) FROM {{decision}} WHERE scope = 'derivative' AND ref = '{m2}' AND withdrawn_at IS NOT NULL"
        )),
        1
    );
    // the check is closed: its rater opens no panel any more
    assert!(!server.opens(&format!("/api/instances/{b}/manifest"), &anna));

    // the ask's measures carry what the check said of the run's results
    {
        let mut store = home.store();
        nils_registry::measure::insert(
            &mut store,
            &[nils_registry::measure::New {
                run_id: run,
                pipeline_id: pipeline,
                pipeline: "lesions-test",
                derivative_id: None,
                source: "labels",
                scope: "stack",
                subject_id: Some(subject),
                session_day: None,
                stack_id: Some(a),
                unit_id: &format!("stack-{a}"),
                name: "volume",
                ty: "number",
                number: Some(8.0),
                text: None,
                unit: None,
                created_at: "2026-10-11T10:00:00Z",
            }],
        )
        .unwrap();
    }
    let asked = server.ok(
        "POST",
        "/api/ask/run",
        Some(json!({"document": {
            "ast_version": 1,
            "params": {"ids": {"type": "list", "value": [a]}},
            "sets": {"s": {"grain": "stack", "where": [["in", {}, ["field", {}, "id"], ["param", {}, "ids"]]]}},
            "out": {"set": "s", "level": "record", "columns": [
                ["field", {}, "id"],
                ["field", {}, "measure.lesions-test.volume"],
                ["field", {}, "measure.lesions-test.checked"],
            ]},
        }, "fresh": true})),
        &ops,
    );
    let row = asked["rows"][0].to_string();
    assert!(
        row.contains("flagged"),
        "the worst of the scan's results, flagged and accepted: {asked}"
    );

    // a direct check withdraws the close's decision; its withdrawal leaves
    // the result unchecked
    server.ok(
        "POST",
        &format!("/api/derivatives/{m3}/check"),
        Some(json!({"value": "flagged", "reasons": ["misplaced"], "why": "on the next scan"})),
        &ops,
    );
    assert_eq!(by("flagged"), vec![m3, m1]);
    let gone = server.ok(
        "POST",
        &format!("/api/derivatives/{m3}/check/withdraw"),
        Some(json!({"why": "checked by mistake"})),
        &ops,
    );
    assert!(gone["check"].is_null(), "{gone}");
    assert_eq!(by("unchecked"), vec![m3]);
    let (status, _) = server.call(
        "POST",
        &format!("/api/derivatives/{m3}/check/withdraw"),
        None,
        &ops,
    );
    assert_eq!(status, 409);

    // the warming job built every panel's layer beside the scans' pictures,
    // inside the test's own working place
    for _ in 0..600 {
        let built = [(m1, a), (m1, b), (m3, d), (m3, e)].iter().all(|(m, s)| {
            let sha =
                home.texts(&format!("SELECT sha256 FROM {{derivative}} WHERE id = {m}"))[0].clone();
            work.path()
                .join("pyramids")
                .join(s.to_string())
                .join("layers")
                .join(&sha[..16])
                .join("lesions")
                .join("manifest.json")
                .is_file()
        });
        if built {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("the check's layers were not built within a minute");
}

/// Section 8.3: a preview answers in about a second for 5,000 results,
/// read from the stored parts and rows. A synthetic registry's stacks each
/// carry masks with two parts; the time is printed and held to a bound a
/// debug build meets.
#[test]
fn a_preview_of_five_thousand_results_is_quick() {
    let home = Home {
        dir: TempDir::new("checks-speed"),
        pg: None,
    };
    init(&home);
    home.ok(&["synth", "--seed", "7", "--subjects", "300"]);
    home.ok(&["session", "rebuild"]);
    let work = TempDir::new("checks-speed-work");
    home.ok(&[
        "place",
        "add",
        "scratch",
        work.path().to_str().unwrap(),
        "--role",
        "working",
    ]);
    let yml = work.path().join("lesions-test.yml");
    std::fs::write(&yml, DESCRIPTOR).unwrap();
    home.ok(&["pipeline", "add", yml.to_str().unwrap()]);
    let pipeline = home.int("SELECT id FROM {pipeline} WHERE name = 'lesions-test'");
    let place = home.int("SELECT id FROM {place} WHERE name = 'scratch'");
    let run = run_of(&home, pipeline, place);
    let stacks: Vec<i64> = home
        .texts("SELECT CAST(id AS TEXT) FROM {stack} ORDER BY id")
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
    assert!(stacks.len() > 500, "{}", stacks.len());
    // a T1w and a FLAIR picked on every visit, so the panels are looked up
    for (role, which) in [("t1w", "MIN"), ("flair", "MAX")] {
        home.exec(&format!(
            "INSERT INTO {{pick}} (model, role, subject_id, session_day, scheme, reference, pack, pack_version, actor, author_kind, decided_at) \
             SELECT 'main', '{role}', sc.subject_id, sc.first, 'default', 'registry', 'mri', '1', 'anna', 'person', '2026-10-11T09:00:00Z' \
             FROM {{session_cache}} sc"
        ));
        home.exec(&format!(
            "INSERT INTO {{pick_stack}} (pick_id, stack_id) \
             SELECT p.id, (SELECT {which}(st.id) FROM {{stack}} st JOIN {{series}} se ON se.id = st.series_id \
                           WHERE se.subject_id = p.subject_id) \
             FROM {{pick}} p WHERE p.role = '{role}'"
        ));
    }
    let picked = home.int("SELECT COUNT(*) FROM {pick_stack}");
    assert!(picked > 100, "{picked}");
    let mut store = home.store();
    let part = |label: i64| nils_registry::derivative::Part {
        label,
        name: None,
        voxels: 4,
        volume_mm3: 2.0,
        anchor_mm: [0.0, 0.0, label as f64],
        box_mm: [[0.0; 3], [1.0; 3]],
        position: label - 1,
        method: "26-connected".into(),
        check_status: None,
        check_reason: None,
        check_decision_id: None,
    };
    let sets = json!([
        {"key": "lesions", "words": "Lesions", "kind": "components", "labels": [99], "count": 2},
        {"key": "structures", "words": "Structures", "kind": "labels", "labels": "nonzero", "count": 0},
    ]);
    store.begin().unwrap();
    for n in 0..5000usize {
        let stack = stacks[n % stacks.len()];
        let belongs =
            nils_registry::derivative::belongs(&mut store, Some(stack), None, None, None).unwrap();
        let rel = format!("derivatives/lesions-test/{run}/{n}/stack-{stack}/lesions.nii.gz");
        let id = nils_registry::derivative::insert_of_run(
            &mut store,
            &nils_registry::derivative::New {
                kind: "mask",
                belongs: &belongs,
                place_id: place,
                path: &rel,
                bytes: 1,
                sha256: &format!("{n:064x}"),
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
        nils_registry::derivative::set_grid(
            &mut store,
            id,
            &json!({"dims": [4, 4, 4]}),
            Some(stack),
            "same",
        )
        .unwrap();
        let t = store.qualified("derivative");
        store
            .execute(
                &format!("UPDATE {t} SET part_sets = '{}' WHERE id = {id}", sets),
                &[],
            )
            .unwrap();
        let p = store.qualified("derivative_part");
        for l in [1i64, 2] {
            let pa = part(l);
            store
                .execute(
                    &format!(
                        "INSERT INTO {p} (derivative_id, part_set, label, voxels, volume_mm3, anchor_mm, box_mm, position, method) \
                         VALUES ({id}, 'lesions', {l}, {}, {}, '[0,0,{l}]', '[[0,0,0],[1,1,1]]', {}, '26-connected')",
                        pa.voxels, pa.volume_mm3, pa.position
                    ),
                    &[],
                )
                .unwrap();
        }
    }
    store.commit().unwrap();
    drop(store);
    let server = Server::start(&home);
    let ops = token(
        "ops",
        &[
            "pipelines:see",
            "campaigns:see",
            "campaigns:work",
            "review:work",
        ],
        "quasi",
    );
    let body = check_body(
        "speed",
        json!({"run": run, "output": "labels", "which": "all"}),
    );
    // the first read warms the engine's caches, as a desk's first change does
    server.ok("POST", "/api/campaigns/preview", Some(body.clone()), &ops);
    let started = std::time::Instant::now();
    let p = server.ok("POST", "/api/campaigns/preview", Some(body), &ops);
    let took = started.elapsed();
    eprintln!(
        "preview of {} results over {} stacks: {:.3} s at the door, {} s inside; {p}",
        p["results"],
        stacks.len().min(5000),
        took.as_secs_f64(),
        p["seconds"]
    );
    assert_eq!(p["results"], 5000, "{p}");
    assert_eq!(p["items"], 5000, "{p}");
    assert_eq!(p["parts"], 10_000, "{p}");
    assert!(took.as_secs_f64() < 20.0, "{took:?}");
}

/// A small registry for the review's tests: one visit of two scans in one
/// frame, a working place, the lesions pipeline and one run of it. Answers
/// the home, the working place, the scans, the pipeline's and the run's
/// ids and the place's.
fn small(name: &str) -> (Home, TempDir, i64, i64, i64, i64, i64) {
    let home = Home {
        dir: TempDir::new(name),
        pg: None,
    };
    let src = TempDir::new(&format!("{name}-src"));
    series(
        &src,
        "1.2.3.A",
        "1.2.3.A.1",
        "1.2.3.F.1",
        "t2 flair",
        "20210203",
    );
    series(
        &src,
        "1.2.3.A",
        "1.2.3.A.2",
        "1.2.3.F.1",
        "t1 mprage",
        "20210203",
    );
    init(&home);
    home.ok(&[
        "digest",
        "--name",
        "a",
        "--no-private",
        src.path().to_str().unwrap(),
    ]);
    home.ok(&["fingerprint"]);
    home.ok(&["classify", "--pack-dir", packs().to_str().unwrap()]);
    home.ok(&["session", "rebuild"]);
    let work = TempDir::new(&format!("{name}-work"));
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
    let (a, b) = (stack_of("1.2.3.A.1"), stack_of("1.2.3.A.2"));
    home.ok(&["pyramid", "build", "--stack", &a.to_string()]);
    let yml = work.path().join("lesions-test.yml");
    std::fs::write(&yml, DESCRIPTOR).unwrap();
    home.ok(&["pipeline", "add", yml.to_str().unwrap()]);
    let pipeline = home.int("SELECT id FROM {pipeline} WHERE name = 'lesions-test'");
    let place = home.int("SELECT id FROM {place} WHERE name = 'scratch'");
    let run = run_of(&home, pipeline, place);
    // the sources stay on disk while the test reads them
    std::mem::forget(src);
    (home, work, a, b, pipeline, run, place)
}

fn ops_token() -> String {
    token(
        "ops",
        &[
            "pipelines:see",
            "pipelines:work",
            "campaigns:see",
            "campaigns:work",
            "query:see",
            "query:work",
            "review:see",
            "review:work",
        ],
        "quasi",
    )
}

/// Preview until the parts of every result are read.
fn read_preview(server: &Server, body: &Value, token: &str) -> Value {
    for _ in 0..600 {
        let p = server.ok("POST", "/api/campaigns/preview", Some(body.clone()), token);
        if p["parts_pending"] == 0 {
            return p;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("the parts were still pending after a minute");
}

/// Review of E1, E2, item 2: a run with a mask that cannot be read (its
/// file gone) and a withdrawn one is still checked: the parts job reads the
/// others, the unreadable one is counted as one that cannot be drawn, with
/// why, and the check is made without it; a layer whose build failed is
/// answered 422 again, never queued again in turn.
#[test]
fn a_mask_that_cannot_be_read_is_counted_and_left_out() {
    let (home, work, a, _b, _pipeline, run, place) = small("checks-unread");
    let good = mask_of(&home, work.path(), run, place, a, "", &lesions(0));
    let gone = mask_of(&home, work.path(), run, place, a, "gone/", &lesions(1));
    let old = mask_of(&home, work.path(), run, place, a, "old/", &lesions(2));
    let spare = mask_of(&home, work.path(), run, place, a, "spare/", &lesions(3));
    for m in [gone, old] {
        let path =
            home.texts(&format!("SELECT path FROM {{derivative}} WHERE id = {m}"))[0].clone();
        std::fs::remove_file(work.path().join(path)).unwrap();
    }
    home.exec(&format!(
        "UPDATE {{derivative}} SET withdrawn_at = '2026-10-11T11:00:00Z' WHERE id = {old}"
    ));
    let server = Server::start(&home);
    let ops = ops_token();
    let body = check_body(
        "all of it",
        json!({"run": run, "output": "labels", "which": "all"}),
    );
    let p = read_preview(&server, &body, &ops);
    assert_eq!(p["results"], 3, "the withdrawn one is no result: {p}");
    assert_eq!(p["items"], 2, "{p}");
    let why = p["not_drawable"].to_string();
    assert!(why.contains("not in"), "{p}");
    // the check is made without the one that cannot be read
    let named = check_body("named", json!({"derivatives": [good, gone]}));
    let made = server.ok("POST", "/api/campaigns", Some(named), &ops);
    let held: Vec<i64> = made["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["derivative_id"].as_i64().unwrap())
        .collect();
    assert_eq!(held, vec![good], "{made}");
    // a layer that cannot be built: its file went after it was read
    let path = home.texts(&format!(
        "SELECT path FROM {{derivative}} WHERE id = {spare}"
    ))[0]
        .clone();
    std::fs::remove_file(work.path().join(path)).unwrap();
    let door = format!("/api/derivatives/{spare}/layers/{a}/lesions/manifest");
    let mut first = 0;
    for _ in 0..600 {
        first = server.raw("GET", &door, None, &ops).0;
        if first != 202 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(first, 422);
    for _ in 0..3 {
        assert_eq!(
            server.raw("GET", &door, None, &ops).0,
            422,
            "not queued again"
        );
    }
}

/// Review of E1, E2, item 3: a layer whose scan's picture cannot be read
/// says why by its class, never by the archive's file path.
#[test]
fn a_layer_whose_scan_cannot_be_read_names_no_file() {
    let (home, work, a, b, _pipeline, run, place) = small("checks-unread-scan");
    let m = mask_of(&home, work.path(), run, place, a, "", &lesions(0));
    home.ok(&["derivative", "parts", &m.to_string()]);
    // the scan's files go from the archive
    let sql = home.sql(&format!(
        "SELECT so.root, f.path FROM {} i JOIN {} f ON f.id = i.source_file_id \
         JOIN {} so ON so.id = f.source_id WHERE i.stack_id = {b}",
        home.store().qualified("instance"),
        home.store().qualified("source_file"),
        home.store().qualified("source"),
    ));
    let files: Vec<(String, String)> = home
        .store()
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.text(0).unwrap().to_string(),
                r.text(1).unwrap().to_string(),
            )
        })
        .collect();
    assert!(!files.is_empty());
    let root = files[0].0.clone();
    for (r, p) in &files {
        std::fs::remove_file(Path::new(r).join(p)).unwrap();
    }
    let (good, out, err) = home.run(
        &[
            "derivative",
            "layer",
            &m.to_string(),
            "--stack",
            &b.to_string(),
        ],
        None,
    );
    assert!(!good, "{out}");
    let said = format!("{out}{err}");
    assert!(said.contains("could not be built"), "{said}");
    assert!(
        !said.contains(&root),
        "a file's path is the archive's: {said}"
    );
    assert!(!said.contains("1.2.3.A"), "{said}");
}

/// Review of E1, item 15: a mask whose header declares more than a mask
/// holds is refused at the door before it is registered: nothing is
/// registered and its bytes are gone; one that is no NIfTI mask is
/// registered, as a file an outside app answered with, and read as one
/// that cannot be drawn.
#[test]
fn a_mask_too_large_to_read_is_never_registered() {
    let (home, work, a, _b, _pipeline, _run, _place) = small("checks-huge");
    let server = Server::start(&home);
    let ops = ops_token();
    let mut huge = lesions(0)[..352].to_vec();
    for (i, d) in [3i16, 30000, 30000, 30000].iter().enumerate() {
        huge[40 + 2 * i..42 + 2 * i].copy_from_slice(&d.to_le_bytes());
    }
    let sha = sha256(&huge);
    let (status, body) = server.raw(
        "POST",
        &format!("/api/derivatives?kind=mask&stack={a}&grid_stack={a}&sha256={sha}&name=h.nii"),
        Some(("application/x-nifti", huge.as_slice())),
        &ops,
    );
    assert_eq!(status, 413, "{}", String::from_utf8_lossy(&body));
    let listed = server.ok("GET", "/api/derivatives?kind=mask", None, &ops);
    assert!(
        listed["derivatives"].as_array().unwrap().is_empty(),
        "{listed}"
    );
    let kept = walk(&work.path().join("derivatives"));
    assert!(
        !kept.iter().any(|p| p.to_string_lossy().contains(&sha)),
        "{kept:?}"
    );
    // a file an outside app answered with is registered all the same
    let other = b"a segmentation in another format".to_vec();
    let (status, body) = server.raw(
        "POST",
        &format!(
            "/api/derivatives?kind=mask&stack={a}&sha256={}&name=s.seg",
            sha256(&other)
        ),
        Some(("application/octet-stream", other.as_slice())),
        &ops,
    );
    let up: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{up}");
    assert_eq!(up["grid_link"], "none", "{up}");
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}
