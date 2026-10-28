// SPDX-License-Identifier: AGPL-3.0-only
//! Record 55, ruling 35 (c), Nima 2026-10-16: each handover has a password
//! of its own, made by NILS or typed by the person handing over, or is not
//! encrypted for a reason given; and who may show it is a grant
//! (`handover-password:see`, in the admin's set) or the release's maker.
//! Through the doors and the command line: a typed password goes in sealed
//! and comes back only to the maker, a holder of the grant or an admin; an
//! operator without either is refused; an unencrypted handover needs a why;
//! every show is audited by the handover's id; and the typed password and
//! the made one appear in no file of the registry and in nothing the engine
//! printed. A stand-in 7z keeps a digest of the password it was given, never
//! the password, so the archives are checked too.

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

fn packs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

const TYPED: &str = "a typed handover password 31";
const ALSO_TYPED: &str = "another one, typed at a terminal";

const ADMIN: &str = "an-admin-token-of-length";
const MAKER: &str = "the-makers-token-of-leng";
const OPERATOR: &str = "an-operator-token-of-len";
const GRANTED: &str = "a-granted-token-of-lengt";
const AGENT: &str = "X-Nils-Actor: {\"kind\": \"agent\", \"name\": \"helper\"}\r\n";

/// A 7z for the test: it says it is 7-Zip, packs nothing but a digest of
/// the password it was given (never the password), and reads an archive
/// back with that digest, exiting 2 on a wrong password as 7z does.
const FAKE_7Z: &str = r#"#!/bin/sh
verb="$1"
case "$verb" in
  "") echo "7-Zip (a) 23.01 a stand-in for the tests"; exit 0 ;;
  a)
    shift; enc=no; archive=""
    for x in "$@"; do
      case "$x" in
        -p) enc=yes ;;
        -*) ;;
        *) archive="$x" ;;
      esac
    done
    if [ "$enc" = yes ]; then read -r pw; read -r again; else pw=""; fi
    printf '%s' "$pw" | sha256sum | cut -c1-64 > "$archive"
    echo "enc=$enc" >> "$archive"
    exit 0 ;;
  t)
    for x in "$@"; do archive="$x"; done
    if grep -q "enc=yes" "$archive"; then
      read -r pw
      [ "$(printf '%s' "$pw" | sha256sum | cut -c1-64)" = "$(head -n1 "$archive")" ] || exit 2
    fi
    exit 0 ;;
esac
exit 1
"#;

fn run(home: &Path, args: &[&str], stdin: Option<&str>, path: &str) -> String {
    let mut child = nils()
        .arg("--registry")
        .arg(home)
        .args(args)
        .env("USER", "anna")
        .env("HOSTNAME", "ward-3")
        .env("PATH", path)
        .env_remove("NILS_DSN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    match stdin {
        Some(text) => child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap(),
        None => drop(child.stdin.take()),
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        // only the child this test started, by its own handle
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Answer {
    status: u16,
    headers: String,
    body: serde_json::Value,
    text: String,
}

impl Server {
    fn start(home: &Path, log: &Path, path: &str) -> Server {
        let err = std::fs::File::create(log).unwrap();
        let child = nils()
            .arg("--registry")
            .arg(home)
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "1",
                "--worker",
            ])
            .args(["--pack-dir", packs().to_str().unwrap()])
            .args([
                "--auth",
                "token",
                "--token",
                "an-admin-token-of-length=chief@lab:admin",
                "--token",
                "the-makers-token-of-leng=anna@ward-3:operator",
                "--token",
                "an-operator-token-of-len=olle@lab:operator",
                "--token",
                "a-granted-token-of-lengt=gerd@lab:release:see,handover-password:see",
            ])
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env("PATH", path)
            .env_remove("NILS_DSN")
            .stdout(Stdio::piped())
            .stderr(Stdio::from(err))
            .spawn()
            .unwrap();
        let mut held = Server { child, port: 0 };
        let stdout = held.child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let first = lines.next().unwrap().unwrap();
        let addr = first.split_whitespace().nth(2).unwrap();
        held.port = addr.rsplit(':').next().unwrap().parse().unwrap();
        let out = log.with_extension("out");
        std::thread::spawn(move || {
            let mut f = std::fs::File::create(out).unwrap();
            for l in lines.map_while(Result::ok) {
                let _ = writeln!(f, "{l}");
            }
        });
        held
    }

    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
        token: &str,
        extra: &str,
    ) -> Answer {
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAuthorization: Bearer {token}\r\n{extra}\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (headers, text) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status = headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        Answer {
            status,
            headers: headers.to_ascii_lowercase(),
            body: serde_json::from_str(text).unwrap_or(serde_json::Value::Null),
            text: text.to_string(),
        }
    }

    fn post(&self, path: &str, body: serde_json::Value, token: &str) -> Answer {
        self.call("POST", path, Some(body), token, "")
    }

    fn get(&self, path: &str, token: &str) -> Answer {
        self.call("GET", path, None, token, "")
    }

    /// Until the job is over, then its state.
    fn over(&self, job: i64) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            let a = self.get(&format!("/api/jobs/{job}"), ADMIN);
            let state = a.body["state"].as_str().unwrap_or("").to_string();
            if matches!(state.as_str(), "done" | "failed" | "cancelled") {
                return state;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "job {job}: {}",
                a.text
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}

fn every_file(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            every_file(&p, out);
        } else if let Ok(bytes) = std::fs::read(&p) {
            out.push((p, bytes));
        }
    }
}

fn holds(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// A registry with one release, `r`, made by anna@ward-3.
fn released(home: &Path, tree: &Path, out: &Path, path: &str) {
    for (study, sop) in [("1.2.3.A", "1.2.3.A.1.1"), ("1.2.3.B", "1.2.3.B.1.1")] {
        let mut e = synth::minimal_mr(study, &format!("{study}.1"), sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
        let file = tree.join(study).join(sop);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, synth::part10(&MetaFields::mr(sop), &e, true)).unwrap();
    }
    run(
        home,
        &["key", "add", "k"],
        Some("a handover test key\n"),
        path,
    );
    run(home, &["init", "--key", "k"], None, path);
    run(
        home,
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            tree.to_str().unwrap(),
        ],
        None,
        path,
    );
    run(
        home,
        &[
            "release",
            "--name",
            "r",
            "--on-unknown",
            "write",
            "--pack-dir",
            packs().to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        None,
        path,
    );
}

#[test]
fn each_handover_has_its_own_password_and_only_its_maker_and_the_grant_see_it() {
    let home = TempDir::new("handover-pw-home");
    let work = TempDir::new("handover-pw-work");
    let bin = work.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let seven = bin.join("7z");
    std::fs::write(&seven, FAKE_7Z).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&seven, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let tree = work.path().join("tree");
    let release_out = work.path().join("release");
    let ship = work.path().join("ship");
    released(home.path(), &tree, &release_out, &path);
    let log = work.path().join("serve.err");
    let server = Server::start(home.path(), &log, &path);
    let out = |name: &str| ship.join(name).display().to_string();

    // --- a folder a release is writing, or died in, is not handed over
    // (Wave 7b, the maintenance track): the door refuses before it queues
    let lock = nils_release::lock::path(&release_out);
    std::fs::write(
        &lock,
        r#"{"job": 77, "host": "a-node", "pid": 1, "started_at": "2026-10-28T10:00:00Z"}"#,
    )
    .unwrap();
    let a = server.post(
        "/api/handovers",
        serde_json::json!({ "release": "r", "out": out("locked") }),
        ADMIN,
    );
    assert_eq!(a.status, 409, "{}", a.text);
    assert!(
        a.text
            .contains("a release (job 77 on a-node) is writing it, or did not finish")
            && a.text.contains("before handing it over"),
        "{}",
        a.text
    );
    std::fs::remove_file(&lock).unwrap();

    // --- typing a password: the maker, never an operator without the grant
    let typed = serde_json::json!({ "release": "r", "out": out("typed"), "password": "typed", "typed": TYPED });
    let a = server.post("/api/handovers", typed.clone(), OPERATOR);
    assert_eq!(a.status, 403, "{}", a.text);
    assert!(!a.text.contains(TYPED));
    let a = server.call("POST", "/api/handovers", Some(typed.clone()), MAKER, AGENT);
    assert_eq!(a.status, 403, "{}", a.text);
    let a = server.post(
        "/api/handovers",
        serde_json::json!({ "release": "r", "out": out("short"), "password": "typed", "typed": "short" }),
        MAKER,
    );
    assert_eq!(a.status, 400, "{}", a.text);
    let a = server.post("/api/handovers", typed.clone(), MAKER);
    assert_eq!(a.status, 202, "{}", a.text);
    assert!(
        !a.text.contains(TYPED),
        "the typed password came back: {}",
        a.text
    );
    assert!(a.text.contains("(sealed)"), "{}", a.text);
    assert!(
        a.headers.contains("cache-control: no-store"),
        "{}",
        a.headers
    );
    assert_eq!(server.over(a.body["job"].as_i64().unwrap()), "done");

    // --- unencrypted needs a why ----------------------------------------
    let a = server.post(
        "/api/handovers",
        serde_json::json!({ "release": "r", "out": out("plain"), "encrypt": false }),
        OPERATOR,
    );
    assert_eq!(a.status, 400, "{}", a.text);
    let a = server.post(
        "/api/handovers",
        serde_json::json!({ "release": "r", "out": out("plain"), "encrypt": false, "why": "the partner's own encrypted drive" }),
        OPERATOR,
    );
    assert_eq!(a.status, 202, "{}", a.text);
    assert_eq!(server.over(a.body["job"].as_i64().unwrap()), "done");

    // --- made by NILS, the default ----------------------------------------
    let a = server.post(
        "/api/handovers",
        serde_json::json!({ "release": "r", "out": out("made") }),
        OPERATOR,
    );
    assert_eq!(a.status, 202, "{}", a.text);
    assert_eq!(server.over(a.body["job"].as_i64().unwrap()), "done");

    // --- the list: how each is kept closed, and who may show it ----------
    let a = server.get("/api/handovers?release=r", OPERATOR);
    assert_eq!(a.status, 200, "{}", a.text);
    let rows = a.body["handovers"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 3, "{}", a.text);
    let id_of = |kind: &str| {
        rows.iter()
            .find(|r| r["password"] == kind)
            .unwrap_or_else(|| panic!("no {kind} handover"))["id"]
            .as_i64()
            .unwrap()
    };
    let (typed_id, plain_id, made_id) = (id_of("typed"), id_of("none"), id_of("made"));
    assert!(rows.iter().all(|r| r["may_show"] == false), "{}", a.text);
    assert!(
        rows.iter().all(|r| r["maker"] == "anna@ward-3"),
        "{}",
        a.text
    );
    let plain = rows.iter().find(|r| r["id"] == plain_id).unwrap();
    assert_eq!(plain["encrypted"], false);
    assert_eq!(
        plain["unencrypted_why"],
        "the partner's own encrypted drive"
    );
    let a = server.get("/api/handovers?release=r", MAKER);
    let mine = a.body["handovers"].as_array().unwrap();
    assert!(
        mine.iter().all(|r| r["may_show"] == (r["id"] != plain_id)),
        "{}",
        a.text
    );

    // --- who may show it --------------------------------------------------
    let show = |id: i64, token: &str| {
        server.post(
            &format!("/api/handovers/{id}/password"),
            serde_json::json!({}),
            token,
        )
    };
    let a = show(typed_id, OPERATOR);
    assert_eq!(
        a.status, 403,
        "another operator, without the grant: {}",
        a.text
    );
    assert!(!a.text.contains(TYPED));
    let a = server.call(
        "POST",
        &format!("/api/handovers/{typed_id}/password"),
        Some(serde_json::json!({})),
        ADMIN,
        AGENT,
    );
    assert_eq!(a.status, 403, "{}", a.text);
    let mut shows = 0;
    for token in [MAKER, GRANTED, ADMIN] {
        let a = show(typed_id, token);
        assert_eq!(a.status, 200, "{token}: {}", a.text);
        assert_eq!(a.body["password"], TYPED);
        assert_eq!(a.body["kind"], "typed");
        assert!(
            a.headers.contains("cache-control: no-store"),
            "{}",
            a.headers
        );
        shows += 1;
    }
    let a = show(made_id, MAKER);
    assert_eq!(a.status, 200, "{}", a.text);
    shows += 1;
    let made_password = a.body["password"].as_str().unwrap().to_string();
    assert_eq!(made_password.len(), 64);
    let a = show(plain_id, ADMIN);
    assert_eq!(a.status, 409, "{}", a.text);
    assert!(a.text.contains("unencrypted"), "{}", a.text);

    // --- the jobs door queues no password ---------------------------------
    for command in [
        serde_json::json!(["handover", "password", "--handover", typed_id.to_string()]),
        serde_json::json!([
            "handover",
            "run",
            "--release",
            "r",
            "--out",
            out("x"),
            "--sealed-password",
            "00"
        ]),
    ] {
        let a = server.post(
            "/api/jobs",
            serde_json::json!({ "command": command }),
            ADMIN,
        );
        assert_eq!(a.status, 400, "{}", a.text);
    }

    // --- the audit: each show by the handover's id, never a password -----
    let a = server.get("/api/audit?limit=500", ADMIN);
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(
        a.text.matches("handover.password").count(),
        shows,
        "{}",
        a.text
    );
    assert!(
        a.text.contains("the partner's own encrypted drive"),
        "{}",
        a.text
    );
    assert!(
        !a.text.contains(TYPED) && !a.text.contains(&made_password),
        "{}",
        a.text
    );
    drop(server);

    // --- the command line: the same passwords, and one typed at a terminal
    let cli = run(
        home.path(),
        &["handover", "password", "--handover", &made_id.to_string()],
        None,
        &path,
    );
    assert_eq!(cli.trim(), made_password);
    let old = run(
        home.path(),
        &["handover", "password", "--key", "k"],
        None,
        &path,
    );
    assert_ne!(old.trim(), made_password, "never the key's shared one");
    run(
        home.path(),
        &[
            "handover",
            "run",
            "--release",
            "r",
            "--out",
            &out("terminal"),
            "--password-stdin",
        ],
        Some(&format!("{ALSO_TYPED}\n")),
        &path,
    );
    let list = run(home.path(), &["handover", "list", "--json"], None, &path);
    let list: serde_json::Value = serde_json::from_str(&list).unwrap();
    let terminal = list
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["id"].as_i64().unwrap())
        .max()
        .unwrap();
    let cli = run(
        home.path(),
        &["handover", "password", "--handover", &terminal.to_string()],
        None,
        &path,
    );
    assert_eq!(cli.trim(), ALSO_TYPED);
    let verified = run(
        home.path(),
        &["handover", "verify", "--handover", &typed_id.to_string()],
        None,
        &path,
    );
    assert!(verified.contains("typed"), "{verified}");
    let text = run(home.path(), &["handover", "list"], None, &path);
    assert!(
        text.contains("not encrypted: the partner's own encrypted drive"),
        "{text}"
    );
    assert!(
        text.contains("a password of its own made by NILS"),
        "{text}"
    );

    // --- no password anywhere: the registry, the archives, what was printed
    let mut files = Vec::new();
    every_file(home.path(), &mut files);
    every_file(work.path(), &mut files);
    assert!(files.iter().any(|(p, _)| p.ends_with("registry.db")));
    assert!(files.iter().any(|(p, _)| p == &log));
    for (p, bytes) in &files {
        for secret in [TYPED, ALSO_TYPED, made_password.as_str()] {
            assert!(!holds(bytes, secret), "{} holds a password", p.display());
        }
    }
}
