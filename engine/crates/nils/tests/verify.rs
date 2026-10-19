// SPDX-License-Identifier: AGPL-3.0-only

//! Record 58 P1 (Wave 7b spec §12), gate E1 through the doors: a verify
//! campaign shows its proposal first on every item, filled in on every
//! asked axis, with a planted value on a seeded share of its items; a
//! measuring or a training campaign shows nothing before the answer and a
//! deciding one shows at once, as before; every verify answer is recorded
//! not blind and its label set never trains; Results report the catch
//! against the bar and name each reader by their account's name; the export
//! for a paper holds counts and digests and no header text, path or
//! subject code.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use nils_dicom::synth::{self, MetaFields, TempDir};
use serde_json::{Value, json};

const ISSUER: &str = "https://desk.example/";

/// The series descriptions the files carry: header text that no export
/// for a paper may hold.
const DESCRIPTIONS: [&str; 3] = ["t1 mprage", "t2 flair", "t2 spine"];

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/oidc")
}

/// A registry of four stacks of two people, classified, made by the
/// command line. Answers the home and the folder the files are in.
fn registry() -> (TempDir, TempDir) {
    let home = TempDir::new("verify-home");
    let dir = TempDir::new("verify-src");
    for (patient, study, sop, description) in [
        ("P1", "1.2.3.A", "1.2.3.A.1.1", DESCRIPTIONS[0]),
        ("P1", "1.2.3.B", "1.2.3.B.1.1", DESCRIPTIONS[1]),
        ("P2", "1.2.3.C", "1.2.3.C.1.1", DESCRIPTIONS[0]),
        ("P2", "1.2.3.D", "1.2.3.D.1.1", DESCRIPTIONS[2]),
    ] {
        let mut e = synth::minimal_mr(study, &format!("{study}.1"), sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
        dir.file(
            &format!("{study}/{sop}"),
            &synth::part10(&MetaFields::mr(sop), &e, true),
        );
    }
    let run = |args: &[&str], stdin: Option<&str>| {
        let mut cmd = nils();
        cmd.arg("--registry")
            .arg(home.path())
            .args(args)
            .env("USER", "cleo")
            .env("HOSTNAME", "lab")
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
    run(&["key", "add", "k"], Some("a verify test key\n"));
    run(&["init", "--key", "k"], None);
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

fn every_stack() -> Value {
    json!({"document": {
        "ast_version": 1,
        "sets": {"every": {"grain": "stack"}},
        "out": {"set": "every", "level": "record"},
    }})
}

/// A campaign's items by id, with their stacks.
fn items_of(campaign: &Value) -> Vec<(i64, i64)> {
    campaign["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| (it["id"].as_i64().unwrap(), it["stack_id"].as_i64().unwrap()))
        .collect()
}

/// Every string anywhere in a document.
fn strings_in(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| strings_in(x, out)),
        Value::Object(m) => m.values().for_each(|x| strings_in(x, out)),
        _ => {}
    }
}

#[test]
fn a_verify_campaign_shows_the_proposal_first_plants_values_and_exports_counts() {
    let (home, src) = registry();
    let server = Server::start(&home);
    let cleo = token(
        "cleo",
        "Cleo Curator",
        &[
            "query:work",
            "data:see",
            "review:work",
            "campaigns:work",
            "places:work",
        ],
        "quasi",
    );
    let anna = token(
        "anna",
        "Anna Rater",
        &["campaigns:see", "campaigns:work"],
        "quasi",
    );
    let bo = token(
        "bo",
        "Bo Rater",
        &["campaigns:see", "campaigns:work"],
        "quasi",
    );
    let principal = |t: &str| {
        server.ok("GET", "/api/capabilities", None, t)["principal"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (anna_p, bo_p) = (principal(&anna), principal(&bo));
    server.ok(
        "PUT",
        "/api/ask/selections/every-stack",
        Some(every_stack()),
        &cleo,
    );
    let question = json!({"kind": "axes", "axes": ["base", "technique"]});
    let make = |body: Value| server.call("POST", "/api/campaigns", Some(body), &cleo);

    // ------------------------------------------------- what verify takes
    let (status, doc) = make(json!({
        "name": "no-proposal", "purpose": "verify", "question": question,
        "source": {"selection": "every-stack@1"}, "raters": [anna_p],
    }));
    assert_eq!(status, 400, "{doc}");
    assert!(
        doc.to_string().contains("suggest rules or imported"),
        "{doc}"
    );
    let (status, doc) = make(json!({
        "name": "a-form", "purpose": "verify", "suggest": "rules",
        "question": {"kind": "form", "schema": {"properties": {"ok": {"type": "boolean"}}, "required": ["ok"]}},
        "source": {"selection": "every-stack@1"}, "raters": [anna_p],
    }));
    assert_eq!(status, 400, "{doc}");
    assert!(
        doc.to_string().contains("axis or an axes question"),
        "{doc}"
    );
    let (status, doc) = make(json!({
        "name": "decide-planted", "question": question, "suggest": "rules",
        "source": {"selection": "every-stack@1"}, "raters": [anna_p], "planted": {"share": 0.1},
    }));
    assert_eq!(status, 400, "{doc}");
    assert!(doc.to_string().contains("only a verify campaign"), "{doc}");
    let (status, doc) = make(json!({
        "name": "identities", "purpose": "verify", "question": question, "suggest": "rules",
        "source": {"selection": "every-stack@1"}, "raters": [anna_p],
        "planted": {"from": [{"true": "T1w", "wrong": "T2w", "count": 3, "stack": 7}]},
    }));
    assert_eq!(status, 400, "{doc}");
    assert!(doc.to_string().contains("no stack identity"), "{doc}");

    // ------------------------------- the truth: a deciding twin shows the
    // rules' proposal at once, before any answer, as before
    let twin = server.ok(
        "POST",
        "/api/campaigns",
        Some(json!({
            "name": "twin", "question": question, "suggest": "rules",
            "source": {"selection": "every-stack@1"}, "raters": [bo_p],
        })),
        &cleo,
    );
    assert_eq!(twin["purpose"], "decide", "{twin}");
    assert!(twin["planted"].is_null(), "{twin}");
    let twin_id = twin["id"].as_i64().unwrap();
    let mut truth: BTreeMap<i64, Value> = BTreeMap::new();
    for (item, stack) in items_of(&twin) {
        let why = server.ok(
            "GET",
            &format!("/api/campaigns/{twin_id}/items/{item}/why"),
            None,
            &bo,
        );
        assert_eq!(why["blind"], false, "{why}");
        assert_eq!(why["purpose"], "decide", "{why}");
        assert!(why.get("ab").is_none(), "{why}");
        truth.insert(stack, why["suggested"].clone());
    }
    assert!(
        truth.values().any(|v| v.is_object()),
        "the rules propose something for the fixture stacks: {truth:?}"
    );

    // ------------------------------- the verify campaign, every item planted
    // (the 10 % share is proved on 200 items in the registry's own test; here
    // every item is planted so each proposal the rules make is one)
    let made = server.ok(
        "POST",
        "/api/campaigns",
        Some(json!({
            "name": "verify", "purpose": "verify", "question": question, "suggest": "rules",
            "source": {"selection": "every-stack@1"}, "raters": [anna_p],
            "closes_into": "decision",
            "planted": {"share": 1.0, "seed": "paper", "from": [{"true": "T1w", "wrong": "T2w", "count": 4}]},
        })),
        &cleo,
    );
    let id = made["id"].as_i64().unwrap();
    assert_eq!(made["purpose"], "verify", "{made}");
    assert_eq!(
        made["planted"],
        json!({"share": 1.0, "count": 4, "opened": false}),
        "{made}"
    );
    // the reader is told that planted values exist, never where or the seed
    let seen = server.ok("GET", &format!("/api/campaigns/{id}"), None, &anna);
    assert_eq!(
        seen["planted"],
        json!({"share": 1.0, "count": 4, "opened": false}),
        "{seen}"
    );
    assert!(!seen.to_string().contains("paper"), "{seen}");

    // every item shows its proposal first, filled in on every asked axis,
    // before any answer; a planted item differs from the truth on one axis
    let items = items_of(&made);
    let mut shown_of: BTreeMap<i64, Value> = BTreeMap::new();
    let mut planted: BTreeSet<i64> = BTreeSet::new();
    for (item, stack) in &items {
        let why = server.ok(
            "GET",
            &format!("/api/campaigns/{id}/items/{item}/why"),
            None,
            &anna,
        );
        assert_eq!(why["blind"], false, "{why}");
        assert_eq!(why["purpose"], "verify", "{why}");
        assert_eq!(why["suggest"], "rules", "{why}");
        assert!(why.get("ab").is_some(), "{why}");
        assert_eq!(why["suggestions"], json!([]), "{why}");
        assert!(why["worth"].is_null(), "{why}");
        // the file's text is shown, the rules' own values are not
        assert!(why.get("header_door").is_some(), "{why}");
        let shown = why["suggested"].clone();
        let true_one = &truth[stack];
        if true_one.is_object() {
            let s = shown.as_object().unwrap_or_else(|| panic!("{why}"));
            assert!(
                s.contains_key("base") && s.contains_key("technique"),
                "{why}"
            );
            if shown != *true_one {
                let differing = s
                    .iter()
                    .filter(|(k, v)| true_one[k.as_str()] != **v)
                    .count();
                assert_eq!(differing, 1, "{shown} against {true_one}");
                planted.insert(*item);
            }
        }
        shown_of.insert(*item, shown);
    }
    // every item the rules propose for is planted
    let proposed = truth.values().filter(|v| v.is_object()).count();
    assert_eq!(
        planted.len(),
        proposed,
        "planted among the items shown: {planted:?}"
    );
    assert!(planted.len() >= 2, "{truth:?}");

    // one stack per screen: no batch and no gallery
    for door in ["batches", "gallery"] {
        let (status, doc) = server.call("GET", &format!("/api/campaigns/{id}/{door}"), None, &anna);
        assert_eq!(status, 409, "{door}: {doc}");
        assert!(
            doc.to_string().contains("one stack per screen"),
            "{door}: {doc}"
        );
    }

    // --------------------- a measuring and a training campaign show nothing
    // before the answer, and the claim door carries no suggestion anywhere
    for purpose in ["measure", "train"] {
        let c = server.ok(
            "POST",
            "/api/campaigns",
            Some(json!({
                "name": purpose, "purpose": purpose, "question": question, "suggest": "rules",
                "source": {"selection": "every-stack@1"}, "raters": [bo_p],
            })),
            &cleo,
        );
        assert!(c["planted"].is_null(), "{c}");
        let cid = c["id"].as_i64().unwrap();
        let claimed = server.ok(
            "POST",
            &format!("/api/campaigns/{cid}/claim"),
            Some(json!({})),
            &bo,
        );
        assert!(claimed["item"].get("suggested").is_none(), "{claimed}");
        let item = claimed["item"]["id"].as_i64().unwrap();
        let a = claimed["assignment"]["id"].as_i64().unwrap();
        let why = server.ok(
            "GET",
            &format!("/api/campaigns/{cid}/items/{item}/why"),
            None,
            &bo,
        );
        assert_eq!(why["blind"], true, "{purpose}: {why}");
        assert!(why["suggested"].is_null(), "{purpose}: {why}");
        assert_eq!(why["purpose"], purpose, "{why}");
        server.ok(
            "POST",
            &format!("/api/campaigns/{cid}/assignments/{a}/answer"),
            Some(json!({"value": {"base": "T1w", "technique": "MPRAGE"}})),
            &bo,
        );
        let why = server.ok(
            "GET",
            &format!("/api/campaigns/{cid}/items/{item}/why"),
            None,
            &bo,
        );
        assert_eq!(why["blind"], false, "{purpose}: {why}");
        let stack = claimed["item"]["stack_id"].as_i64().unwrap();
        assert_eq!(why["suggested"], truth[&stack], "{purpose}: {why}");
    }
    let claimed = server.ok(
        "POST",
        &format!("/api/campaigns/{twin_id}/claim"),
        Some(json!({})),
        &bo,
    );
    assert!(claimed["item"].get("suggested").is_none(), "{claimed}");

    // ------------------------------------ the reader answers the campaign:
    // she keeps the first planted proposal and catches the rest
    let mut kept_planted = false;
    for _ in 0..items.len() {
        let claimed = server.ok(
            "POST",
            &format!("/api/campaigns/{id}/claim"),
            Some(json!({})),
            &anna,
        );
        let item = claimed["item"]["id"].as_i64().unwrap();
        let stack = claimed["item"]["stack_id"].as_i64().unwrap();
        let a = claimed["assignment"]["id"].as_i64().unwrap();
        assert!(claimed["item"].get("suggested").is_none(), "{claimed}");
        let value = if planted.contains(&item) && !kept_planted {
            kept_planted = true;
            shown_of[&item].clone()
        } else if truth[&stack].is_object() {
            truth[&stack].clone()
        } else {
            json!({"base": "T1w", "technique": "MPRAGE"})
        };
        let done = server.ok(
            "POST",
            &format!("/api/campaigns/{id}/assignments/{a}/answer"),
            Some(json!({"value": value})),
            &anna,
        );
        assert!(done["answer"].is_i64(), "{done}");
    }
    // every verify answer is recorded not blind, with the proposal shown
    let all = server.ok("GET", &format!("/api/campaigns/{id}/answers"), None, &cleo);
    let answers = all["answers"].as_array().unwrap();
    assert_eq!(answers.len(), items.len(), "{all}");
    for a in answers {
        assert_eq!(a["blind"], false, "{a}");
        let item = a["item_id"].as_i64().unwrap();
        if shown_of[&item].is_object() {
            // the answer keeps the proposal as the one text an axes answer is
            let kept: Value = serde_json::from_str(a["suggested"].as_str().unwrap()).unwrap();
            assert_eq!(kept, shown_of[&item], "{a}");
            assert_eq!(a["suggested_by"], "rules", "{a}");
        }
    }
    let changed = answers.iter().filter(|a| a["changed"] == true).count();
    assert_eq!(changed, planted.len() - 1, "{all}");

    // ------------------------------------------------- Results and Pace
    let results = server.ok("GET", &format!("/api/campaigns/{id}/results"), None, &cleo);
    assert_eq!(results["purpose"], "verify", "{results}");
    let catch = &results["catch"];
    assert_eq!(catch["planted"], planted.len(), "{results}");
    assert_eq!(catch["caught"], planted.len() - 1, "{results}");
    assert_eq!(catch["items"], 4, "{results}");
    assert_eq!(catch["shown"], planted.len(), "{results}");
    assert_eq!(catch["unread"], 0, "{results}");
    assert_eq!(catch["bar"], 0.9, "{results}");
    assert_eq!(catch["below_bar"], true, "{results}");
    assert_eq!(results["below_bar"], true, "{results}");
    assert_eq!(
        catch["per_rater"],
        json!([{"rater": anna_p, "name": "Anna Rater", "planted": planted.len(),
                "caught": planted.len() - 1, "rate": (planted.len() - 1) as f64 / planted.len() as f64}]),
        "{results}"
    );
    let raters = results["raters"].as_array().unwrap();
    assert_eq!(raters.len(), 1, "{results}");
    assert_eq!(raters[0]["rater"], anna_p, "{results}");
    assert_eq!(raters[0]["name"], "Anna Rater", "{results}");
    assert!(
        results["items"][0].get("planted").is_none(),
        "the plants are opened by the close, not before: {results}"
    );
    for a in results["items"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|it| it["answers"].as_array().unwrap())
    {
        assert_eq!(a["name"], "Anna Rater", "{a}");
    }
    let pace = server.ok("GET", &format!("/api/campaigns/{id}/stats"), None, &cleo);
    assert_eq!(pace["raters"][0]["principal"], anna_p, "{pace}");
    assert_eq!(pace["raters"][0]["name"], "Anna Rater", "{pace}");
    let mine = server.ok("GET", &format!("/api/campaigns/{id}/stats"), None, &anna);
    assert_eq!(mine["raters"][0]["name"], "Anna Rater", "{mine}");

    // ------------------------------------- the export for a paper, blind
    // until the close to a rater, open to one who reads every answer
    let out = TempDir::new("verify-out");
    server.ok(
        "POST",
        "/api/places",
        Some(json!({"name": "paper-out", "role": "export", "path": out.path().to_str().unwrap()})),
        &cleo,
    );
    let (status, doc) = server.call(
        "POST",
        &format!("/api/campaigns/{id}/export"),
        Some(json!({"of": "counts", "name": "peek"})),
        &anna,
    );
    assert_eq!(status, 403, "{doc}");
    let (status, doc) = server.call(
        "POST",
        &format!("/api/campaigns/{id}/export"),
        Some(json!({"of": "rows"})),
        &cleo,
    );
    assert_eq!(status, 400, "{doc}");
    assert!(
        doc.to_string().contains("outcomes, answers or counts"),
        "{doc}"
    );

    // ------------------------------------------------------- the close
    let closed = server.ok(
        "POST",
        &format!("/api/campaigns/{id}/close"),
        Some(json!({})),
        &cleo,
    );
    assert_eq!(closed["resolved"], items.len(), "{closed}");
    // the one plant she kept leaves its axis out of the decisions: a planted
    // value is known to be wrong and is never written as one (spec 12.4)
    assert_eq!(closed["planted_skipped"], 1, "{closed}");
    assert_eq!(
        closed["decisions"].as_array().unwrap().len(),
        items.len() * 2 - 1,
        "{closed}"
    );
    let after = server.ok("GET", &format!("/api/campaigns/{id}"), None, &anna);
    assert_eq!(after["planted"]["opened"], true, "{after}");
    assert_eq!(after["planted"]["seed"], "paper", "{after}");
    assert_eq!(after["planted"]["count"], 4, "{after}");
    let list = after["planted"]["list"].as_array().unwrap();
    assert_eq!(list.len(), 4, "{after}");
    let fixed: Vec<&Value> = list.iter().filter(|r| !r["wrong"].is_null()).collect();
    assert_eq!(fixed.len(), planted.len(), "{after}");
    for r in &fixed {
        assert!(planted.contains(&r["item"].as_i64().unwrap()), "{r}");
        assert!(r["axis"] == "base" || r["axis"] == "technique", "{r}");
    }
    // once closed, the reader's own results carry the plants per item and
    // her own name
    let results = server.ok("GET", &format!("/api/campaigns/{id}/results"), None, &anna);
    let rows = results["items"].as_array().unwrap();
    // every item was drawn; a plant was shown on those the rules proposed for
    let drawn = rows.iter().filter(|it| it["planted"].is_object()).count();
    assert_eq!(drawn, 4, "{results}");
    let marked = rows
        .iter()
        .filter(|it| it["planted"]["axis"].is_string())
        .count();
    assert_eq!(marked, planted.len(), "{results}");
    assert_eq!(
        results["catch"]["per_rater"][0]["name"], "Anna Rater",
        "{results}"
    );
    // the item whose plant she kept says which axis the close left out, and
    // its decisions are the other axis alone; every other item is whole
    let left_out: Vec<&Value> = rows
        .iter()
        .filter(|it| it["outcome"]["planted_skipped"].is_array())
        .collect();
    assert_eq!(left_out.len(), 1, "{results}");
    assert_eq!(
        left_out[0]["outcome"]["planted_skipped"],
        json!([left_out[0]["planted"]["axis"]]),
        "{results}"
    );
    assert_eq!(
        left_out[0]["outcome"]["decisions"]
            .as_object()
            .unwrap()
            .len(),
        1,
        "{results}"
    );
    for it in rows.iter().filter(|it| it["outcome"].is_object()) {
        if it["id"] != left_out[0]["id"] {
            assert_eq!(
                it["outcome"]["decisions"].as_object().map(|m| m.len()),
                Some(2),
                "{it}"
            );
        }
    }

    // a label set of the campaign never trains
    let set = server.ok(
        "POST",
        &format!("/api/campaigns/{id}/export"),
        Some(json!({"of": "outcomes", "name": "verify-labels"})),
        &cleo,
    );
    let set_id = set["id"].as_i64().unwrap();
    let seen = server.ok("GET", &format!("/api/label-sets/{set_id}"), None, &cleo);
    let training = seen["training"].as_str().unwrap_or_default();
    assert!(
        training.starts_with("refused:")
            && training.contains("verify campaign, whose reads never train"),
        "{seen}"
    );

    // the export for a paper: counts, digests and rates, nothing else
    let exported = server.ok(
        "POST",
        &format!("/api/campaigns/{id}/export"),
        Some(json!({"of": "counts", "name": "paper"})),
        &cleo,
    );
    assert_eq!(exported["of"], "counts", "{exported}");
    assert_eq!(exported["version"], 1, "{exported}");
    let dir = Path::new(exported["path"].as_str().unwrap());
    assert!(dir.starts_with(out.path()), "{exported}");
    let files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, ["counts.json"], "{files:?}");
    let text = std::fs::read_to_string(dir.join("counts.json")).unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["counts"]["items"], items.len(), "{text}");
    assert_eq!(doc["counts"]["answers"], items.len(), "{text}");
    assert_eq!(doc["counts"]["planted"], 4, "{text}");
    assert_eq!(doc["counts"]["caught"], planted.len() - 1, "{text}");
    assert_eq!(
        doc["counts"]["answers_by_rater"],
        json!([{"rater": "rater 1", "answers": items.len()}]),
        "{text}"
    );
    assert_eq!(doc["catch"]["per_rater"][0]["rater"], "rater 1", "{text}");
    assert_eq!(doc["catch"]["bar"], 0.9, "{text}");
    assert!(doc["catch"]["rate"].is_f64(), "{text}");
    assert!(doc["agreement"].is_object(), "{text}");
    assert_eq!(
        doc["digests"]["frame"]["sha256"].as_str().unwrap().len(),
        64,
        "{text}"
    );
    assert_eq!(
        doc["digests"]["planted"]["sha256"].as_str().unwrap().len(),
        64,
        "{text}"
    );
    assert_eq!(doc["campaign"]["purpose"], "verify", "{text}");
    // the scan: no header text, no path, no subject code, no account
    let lower = text.to_lowercase();
    for word in DESCRIPTIONS {
        assert!(!lower.contains(word), "{word} in the export: {text}");
    }
    for forbidden in [
        "mprage", "flair", "spine", "p1", "p2", "anna", "cleo", "bo@",
    ] {
        assert!(
            !lower.contains(forbidden),
            "{forbidden} in the export: {text}"
        );
    }
    for account in [&anna_p, &bo_p] {
        assert!(
            !text.contains(account.as_str()),
            "an account in the export: {text}"
        );
    }
    for path in [src.path(), home.path(), out.path()] {
        assert!(
            !text.contains(path.to_str().unwrap()),
            "a path in the export: {text}"
        );
    }
    assert!(!text.contains('/'), "a path in the export: {text}");
    let subjects = server.ok("GET", "/api/subjects", None, &cleo);
    let mut codes = Vec::new();
    for s in subjects["subjects"].as_array().unwrap() {
        strings_in(&s["code"], &mut codes);
    }
    assert!(!codes.is_empty(), "{subjects}");
    for code in codes {
        assert!(!text.contains(&code), "subject code in the export: {text}");
    }
    // a second export is the name's next version, beside the first
    let again = server.ok(
        "POST",
        &format!("/api/campaigns/{id}/export"),
        Some(json!({"of": "counts", "name": "paper"})),
        &cleo,
    );
    assert_eq!(again["version"], 2, "{again}");
    assert!(
        Path::new(again["path"].as_str().unwrap())
            .join("counts.json")
            .exists()
    );
}
