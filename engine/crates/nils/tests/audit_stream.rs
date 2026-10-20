// SPDX-License-Identifier: AGPL-3.0-only

//! Record 58 P2 (Wave 7b spec §13), preparing gate E2 on a lab registry,
//! through the doors: a draw by count takes 150 of the stacks accepted
//! since a time, the same 150 for the same seed twice, and refuses a draw
//! since the last draw when nothing was accepted after it; two synthetic
//! raters answer through the campaign doors, blind; the settle waits for
//! both, then holds exactly every stack where a rater differs from the
//! accepted answer and a seeded 10 % of the agreements, in a verify
//! campaign whose proposal is the accepted answer and whose A and B are it
//! and a differing answer, unlabelled; the grade reads the settled answers
//! and lets the rest stand, counts by vendor and acquisition year, holds
//! the rule `{max_wrong: 1, passed}` and opens a rule review once when the
//! round is over it; rounds pool; and no read of the audit or of its settle
//! is blind training data. SQLite, and Postgres where
//! `NILS_TEST_POSTGRES_DSN` is set.

use std::collections::{BTreeMap, BTreeSet};
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

/// How many stacks the lab registry holds: more than the month's 150.
const STACKS: usize = 172;

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/oidc")
}

/// A lab registry of `STACKS` one-file T1 stacks of two makers and two
/// years, classified, made by the command line, on SQLite or on a Postgres
/// schema.
fn registry(pg: Option<&(String, String)>) -> (TempDir, TempDir) {
    let home = TempDir::new("audit-home");
    let dir = TempDir::new("audit-src");
    for i in 0..STACKS {
        let study = format!("1.2.826.0.1.3680043.8.498.{}", 1000 + i);
        let series = format!("{study}.1");
        let sop = format!("{series}.1");
        let mut e = synth::minimal_mr(&study, &series, &sop);
        e.push(synth::text(
            tags::PATIENT_ID,
            VR::LO,
            &format!("P{}", i / 2),
        ));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
        let maker = if i % 3 == 0 {
            "GE MEDICAL SYSTEMS"
        } else {
            "SIEMENS"
        };
        e.push(synth::text(tags::MANUFACTURER, VR::LO, maker));
        let day = if i % 2 == 0 { "20190304" } else { "20240611" };
        e.push(synth::text(tags::STUDY_DATE, VR::DA, day));
        e.push(synth::text(tags::ACQUISITION_DATE, VR::DA, day));
        dir.file(
            &format!("{study}/{sop}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
    let run = |args: &[&str], stdin: Option<&str>| {
        let mut cmd = nils();
        cmd.arg("--registry")
            .arg(home.path())
            .args(args)
            .env("USER", "cleo")
            .env("HOSTNAME", "lab")
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
        assert!(
            out.status.success(),
            "{}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["key", "add", "k"], Some("an audit stream test key\n"));
    match pg {
        Some((dsn, schema)) => run(
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
        ),
        None => run(&["init", "--key", "k"], None),
    }
    run(
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            dir.path().to_str().unwrap(),
        ],
        None,
    );
    run(&["fingerprint"], None);
    run(&["classify", "--pack-dir", packs().to_str().unwrap()], None);
    (home, dir)
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
        // held from here, so that a panic below kills it too
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

    fn ok(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> Value {
        let (status, doc) = self.call(method, path, body, token);
        assert!(
            (200..300).contains(&status),
            "{method} {path}: {status} {doc}"
        );
        doc
    }
}

/// A token the desk's issuer could have minted: the subject, the account's
/// name, its grants and detail.
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

/// Claim and answer every item a token is given in a campaign, the answer
/// made from the item's stack. Answers how many it answered.
fn answer_all(
    server: &Server,
    campaign: i64,
    token: &str,
    mut answer: impl FnMut(i64, &Value) -> Value,
) -> usize {
    let mut n = 0;
    loop {
        let claimed = server.ok(
            "POST",
            &format!("/api/campaigns/{campaign}/claim"),
            Some(json!({"role": "rater"})),
            token,
        );
        if claimed["assignment"].is_null() {
            return n;
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
        n += 1;
    }
}

/// A draw's stacks by id: their DICOM identity and the base accepted.
fn drawn(server: &Server, draw: i64, token: &str) -> Vec<(i64, String, String)> {
    server.ok("GET", &format!("/api/audit/draws/{draw}"), None, token)["stacks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["stack_id"].as_i64().unwrap(),
                format!("{}|{}|{}", s["study_uid"], s["series_uid"], s["stack_key"]),
                s["accepted"]["base"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

fn stream(pg: Option<(String, String)>) {
    let (home, _src) = registry(pg.as_ref());
    let server = Server::start(&home);
    let cleo = token(
        "cleo",
        "Cleo Curator",
        &[
            "review:see",
            "review:work",
            "campaigns:see",
            "campaigns:work",
        ],
        "quasi",
    );
    let rater = |sub: &str| token(sub, sub, &["campaigns:see", "campaigns:work"], "quasi");
    let (rater_a, rater_b, nora) = (rater("rater-a"), rater("rater-b"), rater("nora"));
    let principal = |t: &str| {
        server.ok("GET", "/api/capabilities", None, t)["principal"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (a_p, b_p, nora_p) = (principal(&rater_a), principal(&rater_b), principal(&nora));
    let since = "2000-01-01T00:00:00Z";

    // ------------------------------------------------ the draw by count
    let (status, doc) = server.call(
        "POST",
        "/api/audit/draws",
        Some(json!({"count": 150, "share": 0.5, "axes": ["base"]})),
        &cleo,
    );
    assert_eq!(status, 400, "count and share at once: {doc}");
    let body = json!({"count": 150, "since": since, "seed": "month-1", "axes": ["base"],
                      "raters": [a_p, b_p]});
    let first = server.ok("POST", "/api/audit/draws", Some(body.clone()), &cleo);
    let d1 = first["draw"]["id"].as_i64().unwrap();
    assert_eq!(first["draw"]["count"], 150, "{first}");
    assert_eq!(first["draw"]["drawn"], 150, "{first}");
    assert_eq!(first["draw"]["since"], since, "{first}");
    let frame = first["draw"]["frame"]["stacks"].as_i64().unwrap();
    assert!(frame > 150, "a real draw from a larger frame: {first}");
    assert_eq!(first["campaign"]["purpose"], "audit", "{first}");
    assert_eq!(
        first["campaign"]["adjudication"]["when"], "never",
        "{first}"
    );
    assert_eq!(first["campaign"]["trains"], false, "{first}");
    let raters_campaign = first["campaign"]["id"].as_i64().unwrap();
    let one = drawn(&server, d1, &cleo);
    assert_eq!(one.len(), 150);
    // the same seed draws the same 150 stacks again
    let again = server.ok("POST", "/api/audit/draws", Some(body), &cleo);
    let d2 = again["draw"]["id"].as_i64().unwrap();
    assert_eq!(
        again["draw"]["frame"]["digest"], first["draw"]["frame"]["digest"],
        "{again}"
    );
    let two = drawn(&server, d2, &cleo);
    let idents = |v: &[(i64, String, String)]| v.iter().map(|x| x.1.clone()).collect::<Vec<_>>();
    assert_eq!(idents(&one), idents(&two));
    // since the last draw, nothing more was accepted
    let (status, doc) = server.call(
        "POST",
        "/api/audit/draws",
        Some(json!({"count": 150, "axes": ["base"]})),
        &cleo,
    );
    assert_eq!(status, 409, "{doc}");
    assert!(doc.to_string().contains("accepted since"), "{doc}");
    // vendor and year are frozen on the drawn stacks
    let shown = server.ok("GET", &format!("/api/audit/draws/{d1}"), None, &cleo);
    let vendors: BTreeSet<String> = shown["stacks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["vendor"].as_str().unwrap_or("none").to_string())
        .collect();
    assert_eq!(
        vendors,
        ["ge", "siemens"].iter().map(|s| s.to_string()).collect(),
        "{shown}"
    );
    let accepted: BTreeMap<i64, String> = one.iter().map(|x| (x.0, x.2.clone())).collect();
    assert!(accepted.values().all(|b| !b.is_empty()), "{shown}");

    // ---------------------------------- the raters, blind, through the doors
    let ids: Vec<i64> = one.iter().map(|x| x.0).collect();
    // the first rater differs on stacks 0 and 1; the second rater on 1 and 2 and 3
    let other = |b: &str| if b == "PDw" { "T2w" } else { "PDw" }.to_string();
    let a_wrong: BTreeSet<i64> = [ids[0], ids[1]].into();
    let b_wrong: BTreeSet<i64> = [ids[1], ids[2], ids[3]].into();
    let n = answer_all(&server, raters_campaign, &rater_a, |stack, why| {
        assert_eq!(why["blind"], true, "an audit is read blind: {why}");
        let b = &accepted[&stack];
        json!({"base": if a_wrong.contains(&stack) { other(b) } else { b.clone() }})
    });
    assert_eq!(n, 150);
    let (status, doc) = server.call(
        "POST",
        &format!("/api/audit/draws/{d1}/grade"),
        Some(json!({})),
        &cleo,
    );
    assert_eq!(
        status, 409,
        "a draw by count is graded from its settle: {doc}"
    );
    let (status, doc) = server.call(
        "POST",
        &format!("/api/audit/draws/{d1}/settle"),
        Some(json!({"settlers": [nora_p]})),
        &cleo,
    );
    assert_eq!(status, 409, "the settle waits for both raters: {doc}");
    assert!(doc.to_string().contains("wait on a rater"), "{doc}");
    let n = answer_all(&server, raters_campaign, &rater_b, |stack, _| {
        let b = &accepted[&stack];
        json!({"base": if b_wrong.contains(&stack) { other(b) } else { b.clone() }})
    });
    assert_eq!(n, 150);

    // ------------------------------------------------------ the settle
    let settled = server.ok(
        "POST",
        &format!("/api/audit/draws/{d1}/settle"),
        Some(json!({"settlers": [nora_p]})),
        &cleo,
    );
    let s = &settled["settle"];
    assert_eq!(s["differences"], 4, "{settled}");
    assert_eq!(s["agreements"], 146, "{settled}");
    assert_eq!(
        s["agreements_settled"], 15,
        "10 % of 146 rounded up: {settled}"
    );
    assert_eq!(s["items"], 19, "{settled}");
    assert_eq!(s["seed"], "month-1", "the draw's seed: {settled}");
    assert_eq!(s["campaign"]["purpose"], "verify", "{settled}");
    for p in s["proposals"].as_array().unwrap() {
        assert_eq!(p["refused_values"], 0, "{settled}");
    }
    let settle = s["campaign"]["id"].as_i64().unwrap();
    assert_eq!(
        settled["draw"]["campaigns"],
        json!({"raters": raters_campaign, "settle": settle, "rule_review": null}),
        "{settled}"
    );
    let c = server.ok("GET", &format!("/api/campaigns/{settle}"), None, &cleo);
    assert_eq!(c["purpose"], "verify", "{c}");
    assert_eq!(c["planted"]["count"], 2, "10 % of 19 planted: {c}");
    let held: BTreeSet<i64> = c["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| it["stack_id"].as_i64().unwrap())
        .collect();
    let differences: BTreeSet<i64> = a_wrong.union(&b_wrong).copied().collect();
    assert!(differences.is_subset(&held), "every difference is settled");
    assert_eq!(held.len(), 19);
    // the same draw is settled once
    let (status, _) = server.call(
        "POST",
        &format!("/api/audit/draws/{d1}/settle"),
        Some(json!({})),
        &cleo,
    );
    assert_eq!(status, 409);
    // the person: the accepted answer is the proposal; where a rater
    // differs, A and B are it and the differing answer, unlabelled; the
    // first two stacks' raters were right, so the rules were wrong there
    let mut ab_seen = 0;
    let n = answer_all(&server, settle, &nora, |stack, why| {
        assert_eq!(why["purpose"], "verify", "{why}");
        assert_eq!(why["blind"], false, "{why}");
        let b = accepted[&stack].clone();
        if differences.contains(&stack) && !why["ab"].is_null() {
            ab_seen += 1;
            let sides: BTreeSet<String> = why["ab"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x["base"].as_str().unwrap().to_string())
                .collect();
            assert_eq!(sides, [b.clone(), other(&b)].into(), "{why}");
            assert_eq!(why["suggested"]["base"], json!(b), "{why}");
            let by = why["suggested_by"].as_str().unwrap_or_default();
            assert!(by.starts_with("source-"), "unlabelled: {why}");
        }
        if !differences.contains(&stack) && why["ab"].is_null() {
            // an agreement shows no A and B
        }
        json!({"base": if stack == ids[0] || stack == ids[1] { other(&b) } else { b }})
    });
    assert_eq!(n, 19);
    assert!(ab_seen >= 2, "A and B on the unplanted differences");
    server.ok(
        "POST",
        &format!("/api/campaigns/{settle}/close"),
        Some(json!({})),
        &cleo,
    );

    // ------------------------------------------------------- the grade
    let graded = server.ok(
        "POST",
        &format!("/api/audit/draws/{d1}/grade"),
        Some(json!({"reviewers": [nora_p]})),
        &cleo,
    );
    let g = &graded["grade"];
    assert_eq!(g["read"], 150, "{graded}");
    assert_eq!(g["wrong"], 2, "{graded}");
    assert_eq!(g["wrong_by_axis"]["base"], 2, "{graded}");
    assert_eq!(g["not_settled"], 0, "{graded}");
    assert_eq!(g["settle"]["settled"], 19, "{graded}");
    assert_eq!(g["settle"]["stood"], 131, "{graded}");
    assert_eq!(
        g["rule"],
        json!({"max_wrong": 1, "passed": false}),
        "{graded}"
    );
    assert_eq!(g["trains"], false, "{graded}");
    let exact = g["bound"].as_f64().unwrap();
    assert!(exact > 2.0 / 150.0 && exact < 0.05, "{graded}");
    let sum = |by: &Value, f: &str| {
        by.as_object()
            .unwrap()
            .values()
            .map(|v| v[f].as_u64().unwrap())
            .sum::<u64>()
    };
    assert_eq!(sum(&g["by_vendor"], "read"), 150, "{graded}");
    assert_eq!(sum(&g["by_vendor"], "wrong"), 2, "{graded}");
    assert_eq!(sum(&g["by_year"], "read"), 150, "{graded}");
    let years: BTreeSet<&String> = g["by_year"].as_object().unwrap().keys().collect();
    assert_eq!(
        years,
        [&"2019".to_string(), &"2024".to_string()].into(),
        "{graded}"
    );
    // the month over the rule opens a rule review over its wrong stacks
    let review = &graded["rule_review"];
    assert_eq!(review["opened"], true, "{graded}");
    let review_id = review["campaign_id"].as_i64().unwrap();
    let rc = server.ok("GET", &format!("/api/campaigns/{review_id}"), None, &cleo);
    let review_stacks: BTreeSet<i64> = rc["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| it["stack_id"].as_i64().unwrap())
        .collect();
    assert_eq!(review_stacks, [ids[0], ids[1]].into(), "{rc}");
    assert_eq!(rc["suggest"], "rules", "{rc}");
    assert_eq!(rc["closes_into"], "none", "{rc}");
    let shown = server.ok("GET", &format!("/api/audit/draws/{d1}"), None, &cleo);
    assert_eq!(shown["campaigns"]["rule_review"], review_id, "{shown}");
    // graded again, the review is the same
    let regraded = server.ok(
        "POST",
        &format!("/api/audit/draws/{d1}/grade"),
        Some(json!({})),
        &cleo,
    );
    assert_eq!(regraded["rule_review"]["opened"], false, "{regraded}");
    assert_eq!(
        regraded["rule_review"]["campaign_id"], review_id,
        "{regraded}"
    );

    // ------------------------------------- a second round, and the pool:
    // ten stacks on the header certificate's five axes, the answers whole
    let small = server.ok(
        "POST",
        "/api/audit/draws",
        Some(
            json!({"count": 10, "since": "2000-01-01", "seed": "month-2",
                    "raters": [a_p, b_p]}),
        ),
        &cleo,
    );
    let d3 = small["draw"]["id"].as_i64().unwrap();
    let rc3 = small["campaign"]["id"].as_i64().unwrap();
    let shown3 = server.ok("GET", &format!("/api/audit/draws/{d3}"), None, &cleo);
    let whole: BTreeMap<i64, Value> = shown3["stacks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|st| {
            // null says the axis has no value, as the rules said
            (st["stack_id"].as_i64().unwrap(), st["accepted"].clone())
        })
        .collect();
    assert_eq!(whole.len(), 10, "{shown3}");
    let odd = *whole.keys().next().unwrap();
    // the first rater cannot tell one stack's technique; the person keeps the accepted answer
    let differs: BTreeSet<i64> = whole
        .iter()
        .filter(|(s, _)| **s == odd)
        .map(|(s, _)| *s)
        .collect();
    answer_all(&server, rc3, &rater_a, |stack, _| {
        let mut a = whole[&stack].clone();
        if stack == odd {
            a["technique"] = json!("cant_tell");
        }
        a
    });
    answer_all(&server, rc3, &rater_b, |stack, _| whole[&stack].clone());
    let s3 = server.ok(
        "POST",
        &format!("/api/audit/draws/{d3}/settle"),
        Some(json!({"settlers": [nora_p], "planted": {"share": 0.0}})),
        &cleo,
    );
    assert_eq!(s3["settle"]["differences"], differs.len(), "{s3}");
    for p in s3["settle"]["proposals"].as_array().unwrap() {
        assert_eq!(p["refused_values"], 0, "the joints are taken: {s3}");
    }
    let c3 = s3["settle"]["campaign"]["id"].as_i64().unwrap();
    answer_all(&server, c3, &nora, |stack, why| {
        if stack == odd {
            // a can't tell is a difference, never an alternative: the
            // accepted answer is shown alone on that axis
            assert!(why["ab"].is_null(), "{why}");
        }
        whole[&stack].clone()
    });
    server.ok(
        "POST",
        &format!("/api/campaigns/{c3}/close"),
        Some(json!({})),
        &cleo,
    );
    let g3 = server.ok("POST", &format!("/api/audit/draws/{d3}/grade"), None, &cleo);
    assert_eq!(g3["grade"]["wrong"], 0, "{g3}");
    let read3 = g3["grade"]["read"].as_u64().unwrap();
    assert_eq!(
        read3 + g3["grade"]["cant_tell"].as_u64().unwrap(),
        10,
        "{g3}"
    );
    assert_eq!(g3["grade"]["rule"]["passed"], true, "{g3}");
    assert_eq!(g3["rule_review"]["campaign_id"], Value::Null, "{g3}");
    let pooled = server.ok(
        "GET",
        &format!("/api/audit/draws/pooled?ids={d1},{d3}"),
        None,
        &cleo,
    );
    let p = &pooled["pooled"];
    assert_eq!(p["read"], 150 + read3, "{pooled}");
    assert_eq!(p["wrong"], 2, "{pooled}");
    assert_eq!(p["rule"]["rounds_over"], json!([d1]), "{pooled}");
    assert_eq!(sum(&p["by_vendor"], "read"), 150 + read3, "{pooled}");
    let (status, doc) = server.call(
        "GET",
        &format!("/api/audit/draws/pooled?ids={d1},{d2}"),
        None,
        &cleo,
    );
    assert_eq!(status, 409, "a draw not graded is not pooled: {doc}");

    // --------------------------------------- these reads never train
    drop(server);
    let mut store = match &pg {
        Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
        None => Store::open_sqlite(&home.path().join("registry.db")).unwrap(),
    };
    for (campaign, purpose) in [(raters_campaign, "audit"), (settle, "verify")] {
        assert_eq!(
            nils_registry::blind::purpose(&mut store, campaign).unwrap(),
            purpose
        );
        assert!(nils_registry::blind::never_trains(purpose));
    }
    let rows = store
        .query(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE campaign_id = {settle} AND (blind IS NULL OR blind <> 0)",
                store.qualified("campaign_answer")
            ),
            &[],
        )
        .unwrap();
    assert_eq!(
        rows[0].int(0).unwrap(),
        0,
        "every settle answer is not blind"
    );
}

/// The edges of the settle and of the default frame: an accepted answer
/// with no value on the asked axis, a can't tell on one axis, a settle with
/// nothing in it, and the default `since` of a round.
fn edges(pg: Option<(String, String)>) {
    let (home, _src) = registry(pg.as_ref());
    let open = || match &pg {
        Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
        None => Store::open_sqlite(&home.path().join("registry.db")).unwrap(),
    };
    let tokens = |server: &Server| {
        let rater = |sub: &str| token(sub, sub, &["campaigns:see", "campaigns:work"], "quasi");
        let (a, b, nora) = (rater("rater-a"), rater("rater-b"), rater("nora"));
        let p = |t: &str| {
            server.ok("GET", "/api/capabilities", None, t)["principal"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let ps = (p(&a), p(&b), p(&nora));
        (a, b, nora, ps)
    };
    let cleo = token(
        "cleo",
        "Cleo Curator",
        &[
            "review:see",
            "review:work",
            "campaigns:see",
            "campaigns:work",
        ],
        "quasi",
    );
    let mut server = Server::start(&home);
    let (rater_a, rater_b, nora, (a_p, b_p, nora_p)) = tokens(&server);
    let since = "2000-01-01T00:00:00Z";
    let draw = |server: &Server, count: i64, seed: &str| {
        server.ok(
            "POST",
            "/api/audit/draws",
            Some(
                json!({"count": count, "since": since, "seed": seed, "axes": ["base"],
                        "raters": [a_p, b_p]}),
            ),
            &cleo,
        )
    };

    // ---------- 1 and 2: a null accepted answer, and a can't tell on one axis
    let made = draw(&server, 4, "edge-1");
    let made1_frame = made["draw"]["frame"]["stacks"].as_i64().unwrap();
    let d1 = made["draw"]["id"].as_i64().unwrap();
    let rc1 = made["campaign"]["id"].as_i64().unwrap();
    let ids: Vec<i64> = drawn(&server, d1, &cleo).iter().map(|x| x.0).collect();
    let accepted: BTreeMap<i64, String> = drawn(&server, d1, &cleo)
        .into_iter()
        .map(|x| (x.0, x.2))
        .collect();
    drop(server);
    {
        // the first stack has no value for its base in the frame
        let mut store = open();
        let d = store.dialect();
        store
            .execute(
                &format!(
                    "UPDATE {} SET accepted = {} WHERE draw_id = {} AND stack_id = {}",
                    store.qualified("audit_draw_stack"),
                    d.param(1, nils_registry::schema::Type::Json),
                    d1,
                    ids[0]
                ),
                &[nils_registry::store::Param::from(r#"{"base":null}"#)],
            )
            .unwrap();
    }
    server = Server::start(&home);
    let other = |b: &str| if b == "PDw" { "T2w" } else { "PDw" }.to_string();
    // the first rater names a base where the frame has none, cannot tell the
    // second stack's, and differs on the third; the second rater agrees
    answer_all(&server, rc1, &rater_a, |stack, _| {
        if stack == ids[0] {
            json!({"base": "PDw"})
        } else if stack == ids[1] {
            json!({"base": "cant_tell"})
        } else if stack == ids[2] {
            json!({"base": other(&accepted[&stack])})
        } else {
            json!({"base": accepted[&stack]})
        }
    });
    answer_all(&server, rc1, &rater_b, |stack, _| {
        if stack == ids[0] {
            json!({"base": null})
        } else {
            json!({"base": accepted[&stack]})
        }
    });
    let s1 = server.ok(
        "POST",
        &format!("/api/audit/draws/{d1}/settle"),
        Some(json!({"settlers": [nora_p], "planted": {"share": 0.0}, "share": 0.0})),
        &cleo,
    );
    assert_eq!(s1["settle"]["differences"], 3, "{s1}");
    for p in s1["settle"]["proposals"].as_array().unwrap() {
        assert_eq!(p["refused_values"], 0, "nothing is refused silently: {s1}");
    }
    let c1 = s1["settle"]["campaign"]["id"].as_i64().unwrap();
    let mut seen = 0;
    answer_all(&server, c1, &nora, |stack, why| {
        seen += 1;
        let text = why.to_string();
        if stack == ids[0] {
            // no value accepted: no proposal on the axis, never the word null
            // the differing answer is what is shown
            assert!(!text.contains("\"base\":\"null\""), "{why}");
            assert_eq!(why["suggested"]["base"], "PDw", "{why}");
        }
        if stack == ids[1] {
            // a can't tell offers no alternative
            assert!(why["ab"].is_null(), "{why}");
            assert_eq!(why["suggested"]["base"], accepted[&stack], "{why}");
        }
        if stack == ids[2] {
            assert!(!why["ab"].is_null(), "a real difference is shown: {why}");
        }
        json!({"base": accepted[&stack]})
    });
    assert_eq!(seen, 3);

    // ------------------------------------------- 4: a settle with nothing in it
    let made = draw(&server, 3, "edge-4");
    let d4 = made["draw"]["id"].as_i64().unwrap();
    let rc4 = made["campaign"]["id"].as_i64().unwrap();
    let acc4: BTreeMap<i64, String> = drawn(&server, d4, &cleo)
        .into_iter()
        .map(|x| (x.0, x.2))
        .collect();
    for t in [&rater_a, &rater_b] {
        answer_all(&server, rc4, t, |stack, _| json!({"base": acc4[&stack]}));
    }
    let s4 = server.ok(
        "POST",
        &format!("/api/audit/draws/{d4}/settle"),
        Some(json!({"settlers": [nora_p], "share": 0.0})),
        &cleo,
    );
    assert_eq!(s4["settle"]["items"], 0, "{s4}");
    assert_eq!(s4["settle"]["campaign"], Value::Null, "{s4}");
    assert!(s4["settle"]["note"].as_str().is_some(), "{s4}");
    let (status, doc) = server.call(
        "POST",
        &format!("/api/audit/draws/{d4}/settle"),
        Some(json!({"settlers": [nora_p]})),
        &cleo,
    );
    assert_eq!(status, 409, "settled once: {doc}");
    let g4 = server.ok("POST", &format!("/api/audit/draws/{d4}/grade"), None, &cleo);
    assert_eq!(g4["grade"]["read"], 3, "every stack stands: {g4}");
    assert_eq!(g4["grade"]["wrong"], 0, "{g4}");
    assert_eq!(g4["grade"]["rule"]["passed"], true, "{g4}");
    assert_eq!(g4["grade"]["settle"]["stood"], 3, "{g4}");

    // ------- 3 and 6: the default since is the round's last draw by count
    // a stack is sorted after the last count draw; a draw by share then comes
    // later than that sort, and does not move the default
    let a_id = d4;
    drop(server);
    {
        let mut store = open();
        let d = store.dialect();
        let ts = nils_registry::schema::Type::Timestamp;
        // the count draw was made a minute ago; one stack was sorted
        // half a minute after it
        let minute = |secs: i64| {
            nils_registry::time::iso_of(
                (nils_registry::time::now_secs() as i64 + secs).max(0) as u64
            )
        };
        store
            .execute(
                &format!(
                    "UPDATE {} SET created_at = {} WHERE id = {a_id}",
                    store.qualified("audit_draw"),
                    d.param(1, ts)
                ),
                &[nils_registry::store::Param::from(minute(-60).as_str())],
            )
            .unwrap();
        // every classification was finished half a minute after the draw
        store
            .execute(
                &format!(
                    "UPDATE {} SET finished_at = {}",
                    store.qualified("job"),
                    d.param(1, ts)
                ),
                &[nils_registry::store::Param::from(minute(-30).as_str())],
            )
            .unwrap();
    }
    let server = Server::start(&home);
    // a draw by share, later than the sort, of the same round
    let share = server.ok(
        "POST",
        "/api/audit/draws",
        Some(json!({"share": 0.5, "axes": ["base"], "seed": "edge-share", "raters": [a_p, b_p]})),
        &cleo,
    );
    assert!(share["draw"]["count"].is_null(), "{share}");
    let next = server.ok(
        "POST",
        "/api/audit/draws",
        Some(json!({"count": 10, "axes": ["base"], "seed": "edge-next", "raters": [a_p, b_p]})),
        &cleo,
    );
    assert_eq!(
        next["draw"]["frame"]["stacks"], made1_frame,
        "the share draw did not cut the frame: {next}"
    );
    // and two frames in a row are disjoint: nothing was sorted since
    let (status, doc) = server.call(
        "POST",
        "/api/audit/draws",
        Some(json!({"count": 10, "axes": ["base"], "seed": "edge-again", "raters": [a_p, b_p]})),
        &cleo,
    );
    assert_eq!(status, 409, "the frame after a draw is empty: {doc}");
}

#[test]
fn the_monthly_audit_draws_by_count_is_settled_against_the_accepted_answer_and_pools() {
    stream(None);
}

#[test]
fn the_monthly_audit_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    let schema = "nils_audit_stream";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    stream(Some((dsn.clone(), schema.to_string())));
    drop();
}

#[test]
fn the_audit_edges_settle_and_frame() {
    edges(None);
}

#[test]
fn the_audit_edges_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    let schema = "nils_audit_edges";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    edges(Some((dsn.clone(), schema.to_string())));
    drop();
}
