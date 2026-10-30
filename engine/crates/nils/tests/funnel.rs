// SPDX-License-Identifier: AGPL-3.0-only

//! Record 58 P4 (Wave 7b spec section 15, gate E4), through the binary: the
//! yield funnel of a selection through a pipeline with a pack. Three
//! people, a T1w and a FLAIR each, one of the T1w 2D; a selection of the
//! T1w made by one maker; a pack whose hard need is a 3D stack. `nils funnel`
//! and `GET /api/funnel` answer the same document: a row for the stacks
//! the selection starts from, one for each of its filters, one for the
//! stacks in a unit, one for the units with their inputs and one for the
//! need, each counted by subjects, sessions and stacks and broken down by
//! year, base and 2D or 3D; the last line multiplies the certificate's base
//! bound by the stacks the base filter read. Counts only: no subject code,
//! no path.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

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

/// One person: the patient, the study's day, the UID root, and each
/// series as its number, description, protocol and acquisition type.
type Person<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a [(&'a str, &'a str, &'a str, &'a str)],
);

const PEOPLE: &[Person<'static>] = &[
    (
        "FUNNEL-P1",
        "20220115",
        "1.2.826.0.1.3680043.8.498.81",
        &[
            ("1", "t1_mprage_sag", "MPRAGE", "3D"),
            ("2", "t2_flair_sag", "FLAIR", "3D"),
        ],
    ),
    (
        "FUNNEL-P2",
        "20230310",
        "1.2.826.0.1.3680043.8.498.82",
        &[
            ("1", "t1_mprage_sag", "MPRAGE", "2D"),
            ("2", "t2_flair_sag", "FLAIR", "3D"),
        ],
    ),
    (
        "FUNNEL-P3",
        "20230411",
        "1.2.826.0.1.3680043.8.498.83",
        &[
            ("1", "t1_mprage_sag", "MPRAGE", "3D"),
            ("2", "t2_flair_sag", "FLAIR", "3D"),
        ],
    ),
];

fn tree() -> TempDir {
    let dir = TempDir::new("funnel-src");
    for &(patient, day, root, series) in PEOPLE {
        for &(n, description, protocol, dim) in series {
            let study = format!("{root}.1");
            let series = format!("{root}.1.{n}");
            for slice in 1..=12u32 {
                let sop = format!("{series}.{slice}");
                let mut e = synth::minimal_mr(&study, &series, &sop);
                let pixels: Vec<u8> = (0..32u32 * 32)
                    .flat_map(|i| (200 + (i % 32) as u16 + slice as u16).to_le_bytes())
                    .collect();
                e.extend([
                    synth::text(tags::PATIENT_ID, VR::LO, patient),
                    synth::text(tags::STUDY_DATE, VR::DA, day),
                    synth::text(tags::SERIES_TIME, VR::TM, &format!("0{n}1415")),
                    synth::text(tags::SERIES_DESCRIPTION, VR::LO, description),
                    synth::text(tags::PROTOCOL_NAME, VR::LO, protocol),
                    synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, dim),
                    synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
                    synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
                    synth::text(tags::BURNED_IN_ANNOTATION, VR::CS, "NO"),
                    synth::us(tags::ROWS, 32),
                    synth::us(tags::COLUMNS, 32),
                    synth::us(tags::BITS_ALLOCATED, 16),
                    synth::us(tags::BITS_STORED, 12),
                    synth::us(tags::HIGH_BIT, 11),
                    synth::us(tags::PIXEL_REPRESENTATION, 0),
                    synth::us(tags::SAMPLES_PER_PIXEL, 1),
                    synth::text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
                    synth::text(tags::PIXEL_SPACING, VR::DS, "8.0\\8.0"),
                    synth::text(tags::SLICE_THICKNESS, VR::DS, "1.0"),
                    synth::text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "0\\1\\0\\0\\0\\-1"),
                    synth::text(
                        tags::IMAGE_POSITION_PATIENT,
                        VR::DS,
                        &format!("{slice}\\0\\0"),
                    ),
                    synth::text(tags::INSTANCE_NUMBER, VR::IS, &slice.to_string()),
                    synth::bytes(tags::PIXEL_DATA, VR::OW, pixels),
                ]);
                dir.file(
                    &format!("{patient}/{n}/{slice}"),
                    &synth::part10(&MetaFields::mr(&sop), &e, true),
                );
            }
        }
    }
    dir
}

const ECHO: &str = r#"# SPDX-License-Identifier: AGPL-3.0-only
name: funnel-echo
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/funnel-echo@sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
inputs:
  - id: mode
    name: What to do
    type: String
    value-key: "[MODE]"
    default-value: all
    value-choices: [all, silent]
command-line: echo [Manifest] [OutputLocation] [Inputs] [MODE]
x-nils:
  analysis-level: stack
  input: {layout: stacks}
  units: apart
  outputs:
    - id: echo
      kind: output
      path-template: "stack-{stack}/out.txt"
      media-type: text/plain
  needs: {gpu: none}
"#;

fn echo_pack(digest: &str) -> String {
    format!(
        r#"nils_pack: 1
pipeline: {{name: funnel-echo, descriptor_sha256: "{digest}", version: "0.1"}}
needs:
  stack:
    hard:
      - {{when: "dimension == '3D'", words: "the stack is not 3D", source: policy}}
knobs:
  mode: {{comparability: none, set_by: person}}
"#
    )
}

struct Lab {
    home: TempDir,
    work: TempDir,
    _src: TempDir,
}

impl Lab {
    fn new() -> Lab {
        let lab = Lab {
            home: TempDir::new("funnel-home"),
            work: TempDir::new("funnel-work"),
            _src: tree(),
        };
        lab.ok(&["key", "add", "k"], Some("a funnel test key\n"));
        lab.ok(&["init", "--key", "k"], None);
        let src = lab._src.path().to_str().unwrap().to_string();
        lab.ok(&["digest", "--name", "a", "--no-private", &src], None);
        lab.ok(&["fingerprint"], None);
        let packs = packs();
        let packs = packs.to_str().unwrap();
        lab.ok(&["classify", "--pack-dir", packs], None);
        lab.ok(&["pick", "run", "--pack-dir", packs], None);
        let work = lab.work.path().to_str().unwrap().to_string();
        lab.ok(
            &[
                "place", "add", "scratch", &work, "--role", "working", "--fast",
            ],
            None,
        );
        let doc = lab.work.path().join("t1.json");
        std::fs::write(
            &doc,
            json!({
                "ast_version": 1,
                "sets": {"t1": {"grain": "stack", "where": [
                    ["=", {}, ["axis", {}, "base"], "T1w"],
                    ["=", {}, ["field", {}, "manufacturer"], "SYNTHETIC"]
                ]}},
                "out": {"set": "t1", "level": "record"}
            })
            .to_string(),
        )
        .unwrap();
        lab.ok(
            &[
                "ask",
                "selections",
                "save",
                "--name",
                "t1",
                "--file",
                doc.to_str().unwrap(),
                "--pack-dir",
                packs,
            ],
            None,
        );
        let digest = nils_pipeline::descriptor::parse(ECHO).unwrap().digest();
        let dir = lab.work.path().join("pair");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("nils.job.yml"), ECHO).unwrap();
        std::fs::write(dir.join("nils-pack.yaml"), echo_pack(&digest)).unwrap();
        lab.ok(&["pipeline", "add", dir.to_str().unwrap(), "--json"], None);
        lab
    }

    fn command(&self) -> Command {
        let mut c = nils();
        c.arg("--registry")
            .arg(self.home.path())
            .env("USER", "anna")
            .env("NILS_PACK_DIR", packs())
            .env_remove("NILS_DSN")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL");
        c
    }

    fn run(&self, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
        let mut child = self
            .command()
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        if let Some(text) = stdin {
            input.write_all(text.as_bytes()).unwrap();
        }
        drop(input);
        let out = child.wait_with_output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }

    fn ok(&self, args: &[&str], stdin: Option<&str>) -> String {
        let (good, out, err) = self.run(args, stdin);
        assert!(good, "nils {}: {err}", args.join(" "));
        out
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.ok(args, None);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    }

    fn store(&self) -> nils_registry::Store {
        nils_registry::Store::open_sqlite(&self.home.path().join("registry.db")).unwrap()
    }

    /// A certificate whose graded result bounds base on its own.
    fn certify(&self) -> Value {
        let result = nils_registry::labels::graded(&json!({
            "bar": 0.02, "n": 5000, "errors": 6,
            "axes": {"base": {"n": 5000, "errors": 2}}
        }))
        .unwrap();
        let mut store = self.store();
        let t = nils_registry::schema::table("certificate");
        let cols = ["sample", "model_ids", "result", "created_by", "created_at"];
        store
            .insert(
                &nils_registry::Insert::new(t, &cols),
                &[vec![
                    nils_registry::Param::from("evidence:run-1"),
                    nils_registry::Param::from("[]"),
                    nils_registry::Param::from(result.to_string()),
                    nils_registry::Param::from("cleo@lab"),
                    nils_registry::Param::from("2026-10-30T00:00:00Z"),
                ]],
            )
            .unwrap();
        result
    }
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

const SEER: &str = "a-pipelines-seer-token-long";
const READER: &str = "a-reader-token-of-its-length";

impl Server {
    fn start(lab: &Lab) -> Server {
        let tokens = [
            format!("{SEER}=pat@lab:pipelines:see"),
            format!("{READER}=lou@lab:review:see"),
        ]
        .join(",");
        let child = lab
            .command()
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

    fn get(&self, path: &str, token: &str) -> (u16, Value) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let head = format!(
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {token}\r\n\r\n"
        );
        stream.write_all(head.as_bytes()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&response[..split]).to_string();
        let status: u16 = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        let text = String::from_utf8_lossy(&response[split + 4..]).to_string();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }
}

fn n(v: &Value, k: &str) -> i64 {
    v[k].as_i64().unwrap_or_else(|| panic!("{k} in {v}"))
}

fn counts(row: &Value) -> (i64, i64, i64) {
    (n(row, "subjects"), n(row, "sessions"), n(row, "stacks"))
}

fn by(row: &Value, axis: &str) -> BTreeMap<String, i64> {
    row[axis]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| (g["value"].as_str().unwrap().to_string(), n(g, "stacks")))
        .collect()
}

#[test]
fn the_funnel_counts_each_filter_and_need_and_the_same_at_the_door() {
    let lab = Lab::new();
    let doc = lab.json(&["funnel", "t1", "funnel-echo", "--json"]);
    assert_eq!(doc["selection"], "selection:t1@1", "{doc}");
    assert_eq!(doc["unit"], "stack", "{doc}");
    let rows = doc["rows"].as_array().unwrap();
    let kinds: Vec<&str> = rows.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        ["selection", "filter", "filter", "units", "inputs", "need"],
        "{doc}"
    );
    // six stacks of three people; the T1w filter keeps three; all of
    // one maker; each has its input; the 3D need drops the 2D one
    assert_eq!(counts(&rows[0]), (3, 3, 6), "{}", rows[0]);
    assert_eq!(counts(&rows[1]), (3, 3, 3), "{}", rows[1]);
    assert_eq!(rows[1]["axes"], json!(["base"]), "{}", rows[1]);
    assert_eq!(rows[1]["what"], "base = T1w", "{}", rows[1]);
    assert_eq!(rows[2]["axes"], json!([]), "{}", rows[2]);
    assert_eq!(counts(&rows[2]), (3, 3, 3), "{}", rows[2]);
    assert_eq!(counts(&rows[3]), (3, 3, 3), "{}", rows[3]);
    assert_eq!(n(&rows[4], "units"), 3, "{}", rows[4]);
    assert_eq!(counts(&rows[5]), (2, 2, 2), "{}", rows[5]);
    assert_eq!(rows[5]["need"], "stack: dimension == '3D'", "{}", rows[5]);
    assert_eq!(rows[5]["what"], "the stack is not 3D", "{}", rows[5]);
    assert_eq!(n(&rows[5]["removed"], "stacks"), 1, "{}", rows[5]);
    assert_eq!(doc["monotone"], true, "{doc}");
    // each row is the one before less what it removed, and its breakdowns
    // add up to it
    for k in 1..rows.len() {
        assert_eq!(
            n(&rows[k - 1], "stacks") - n(&rows[k]["removed"], "stacks"),
            n(&rows[k], "stacks"),
            "{}",
            rows[k]
        );
    }
    for r in rows {
        for b in ["by_year", "by_base", "by_dimension"] {
            assert_eq!(by(r, b).values().sum::<i64>(), n(r, "stacks"), "{b}: {r}");
        }
    }
    assert_eq!(
        by(&rows[0], "by_year"),
        [("2022".to_string(), 2), ("2023".to_string(), 4)].into(),
        "{}",
        rows[0]
    );
    assert_eq!(
        by(&rows[1], "by_dimension"),
        [("2D".to_string(), 1), ("3D".to_string(), 2)].into(),
        "{}",
        rows[1]
    );
    assert_eq!(
        by(&rows[5], "by_base"),
        [("T1w".to_string(), 2)].into(),
        "{}",
        rows[5]
    );
    // no certificate yet: the line says so and carries no number
    assert!(doc["budget"]["at_most"].is_null(), "{doc}");
    assert!(
        doc["budget"]["words"]
            .as_str()
            .unwrap()
            .contains("no certificate"),
        "{doc}"
    );
    // counts only: no patient, no path, no file
    let text = doc.to_string();
    assert!(!text.contains("FUNNEL-P"), "{text}");
    assert!(!text.contains(lab._src.path().to_str().unwrap()), "{text}");

    // a certificate: the base filter read the six stacks of row 0
    let result = lab.certify();
    let doc = lab.json(&["funnel", "selection:t1@1", "funnel-echo", "--json"]);
    let bound = result["axes"]["base"]["bound"].as_f64().unwrap();
    let want = (bound * 6.0).ceil() as u64;
    assert_eq!(doc["budget"]["at_most"], want, "{doc}");
    assert_eq!(doc["budget"]["terms"][0]["stacks"], 6, "{doc}");
    assert_eq!(doc["budget"]["terms"][0]["bound_from"], "axis", "{doc}");
    assert_eq!(
        doc["budget"]["words"],
        format!(
            "at most {want} {} misplaced by label error",
            if want == 1 { "stack" } else { "stacks" }
        )
    );
    // the words name every row and end with the line
    let words = lab.ok(&["funnel", "t1", "funnel-echo"], None);
    assert!(words.contains("base = T1w"), "{words}");
    assert!(
        words.trim_end().ends_with("misplaced by label error"),
        "{words}"
    );
    // refusals say what is missing
    let (good, _, err) = lab.run(&["funnel", "nothing", "funnel-echo"], None);
    assert!(!good && err.contains("no selection nothing"), "{err}");
    let (good, _, err) = lab.run(&["funnel", "t1", "no-such-pipeline"], None);
    assert!(
        !good && err.contains("no pipeline no-such-pipeline"),
        "{err}"
    );

    // the door answers the same document, under pipelines:see
    let server = Server::start(&lab);
    let (status, at_door) = server.get("/api/funnel?selection=t1%401&pipeline=funnel-echo", SEER);
    assert_eq!(status, 200, "{at_door}");
    assert_eq!(at_door, doc, "the door and the command differ");
    let (status, _) = server.get("/api/funnel?selection=t1&pipeline=funnel-echo", READER);
    assert_eq!(status, 403);
    let (status, said) = server.get("/api/funnel?selection=t1", SEER);
    assert_eq!(status, 400, "{said}");
    assert!(said.to_string().contains("pipeline is missing"), "{said}");
    let (status, _) = server.get("/api/funnel?selection=nothing&pipeline=funnel-echo", SEER);
    assert_eq!(status, 404);
    let (status, _) = server.get(
        "/api/funnel?selection=t1&pipeline=funnel-echo&certificate=99",
        SEER,
    );
    assert_eq!(status, 404);
    let (status, caps) = server.get("/api/capabilities", SEER);
    assert_eq!(status, 200);
    assert!(caps.to_string().contains("GET /api/funnel"), "{caps}");
}
