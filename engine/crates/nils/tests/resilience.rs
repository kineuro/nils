// SPDX-License-Identifier: AGPL-3.0-only
//! The engine that lost its port (2026-10-02): an engine out of descriptors
//! failed an accept, its HTTP library ended the accept loop and dropped the
//! listening socket, and the process lived on answering nobody. Here: an
//! engine whose descriptors run out keeps its port and serves again once
//! they come back, says so on its health door and on stderr; and under many
//! readers asking pictures at once its descriptors stay bounded, every
//! picture is answered, and the doors that are not pictures stay quick.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn ok(home: &TempDir, args: &[&str], stdin: Option<&str>) -> String {
    let mut child = nils()
        .arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "anna")
        .env("HOSTNAME", "ward-3")
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
    assert!(
        out.status.success(),
        "nils {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn packs() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

const READERS: [&str; 4] = [
    "a-first-reader-token-long",
    "a-second-reader-token-lng",
    "a-third-reader-token-long",
    "a-fourth-reader-token-lng",
];

/// `--auth token` with the four readers, each a reviewer (who opens pixels).
fn tokens() -> Vec<String> {
    let mut out = vec!["--auth".to_string(), "token".to_string()];
    for (i, t) in READERS.iter().enumerate() {
        out.push("--token".to_string());
        out.push(format!("{t}=reader{i}@lab:reviewer"));
    }
    out
}

/// An engine under a limit of open files (soft and hard, so the engine
/// cannot raise it), its stderr in a file, killed when dropped.
struct Engine {
    child: Child,
    port: u16,
    stderr: std::path::PathBuf,
}

impl Engine {
    fn start(home: &TempDir, files: u32, workers: u32, extra: &[String]) -> Engine {
        Self::start_with(home, files, workers, extra, &[])
    }

    fn start_with(
        home: &TempDir,
        files: u32,
        workers: u32,
        extra: &[String],
        envs: &[(&str, &str)],
    ) -> Engine {
        let stderr = home.path().join(format!("serve-{files}.err"));
        // `ulimit -n` in the shell sets both limits, then the shell becomes the engine
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(format!("ulimit -n {files} && exec \"$0\" \"$@\""))
            .arg(env!("CARGO_BIN_EXE_nils"))
            .arg("--registry")
            .arg(home.path())
            .args(["serve", "--bind", "127.0.0.1:0", "--workers"])
            .arg(workers.to_string())
            .args(["--pack-dir", packs().to_str().unwrap()])
            .args(extra)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .envs(envs.iter().copied())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        let first = BufReader::new(child.stdout.take().unwrap())
            .lines()
            .next()
            .and_then(Result::ok)
            .unwrap_or_else(|| {
                panic!(
                    "nils serve did not listen: {}",
                    std::fs::read_to_string(&stderr).unwrap_or_default()
                )
            });
        let addr = first.split_whitespace().nth(2).unwrap();
        let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
        Engine {
            child,
            port,
            stderr,
        }
    }

    fn get(&self, path: &str, token: Option<&str>) -> Result<(u16, Vec<u8>), String> {
        get(self.port, path, token)
    }

    fn health(&self) -> serde_json::Value {
        let (status, body) = self.get("/api/health", None).unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(status == 200 || status == 503, "{status} {doc}");
        doc
    }

    /// The engine's open descriptors, from /proc.
    fn open_files(&self) -> Option<usize> {
        std::fs::read_dir(format!("/proc/{}/fd", self.child.id()))
            .ok()
            .map(|d| d.count())
    }

    fn said(&self) -> String {
        std::fs::read_to_string(&self.stderr).unwrap_or_default()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One GET on a connection of its own: the status and the body.
fn get(port: u16, path: &str, token: Option<&str>) -> Result<(u16, Vec<u8>), String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(60))).ok();
    let auth = token.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{auth}\r\n")
                .as_bytes(),
        )
        .map_err(|e| e.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|e| e.to_string())?;
    let split = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no header block")?;
    let head = String::from_utf8_lossy(&response[..split]).to_string();
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or("no status")?;
    Ok((status, response[split + 4..].to_vec()))
}

#[test]
fn an_engine_out_of_descriptors_keeps_its_port_and_serves_again() {
    if !Path::new("/proc/self/fd").exists() {
        eprintln!("no /proc: the descriptor count cannot be read here");
        return;
    }
    let home = TempDir::new("resilience-fd");
    ok(&home, &["key", "add", "k"], Some("a resilience test key\n"));
    ok(&home, &["init", "--key", "k"], None);
    let engine = Engine::start(&home, 64, 2, &tokens());
    let (status, _) = engine.get("/api/status", Some(READERS[0])).unwrap();
    assert_eq!(status, 200);
    let health = engine.health();
    assert_eq!(health["live"], true, "{health}");
    assert_eq!(health["files"]["limit"], 64, "{health}");
    assert!(
        engine
            .said()
            .contains("open files are limited to 64, and 2 handlers may want"),
        "{}",
        engine.said()
    );

    // connections that send nothing, until the engine has no descriptor
    // left for the next: each one it accepts costs it two
    let mut idle = Vec::new();
    for _ in 0..80 {
        if let Ok(s) = TcpStream::connect(("127.0.0.1", engine.port)) {
            idle.push(s);
        }
    }
    let full = Instant::now();
    while engine.open_files().unwrap_or(0) < 63 && full.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        engine.open_files().unwrap_or(0) >= 63,
        "the engine ran out of descriptors: {:?}",
        engine.open_files()
    );
    std::thread::sleep(Duration::from_millis(500));
    drop(idle);

    // the port stayed: once the descriptors are back the engine answers
    let back = Instant::now();
    let answered = loop {
        match engine.get("/api/status", Some(READERS[0])) {
            Ok((200, _)) => break true,
            _ if back.elapsed() > Duration::from_secs(20) => break false,
            _ => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    assert!(answered, "the engine no longer answers: {}", engine.said());
    let health = engine.health();
    assert_eq!(health["live"], true, "{health}");
    assert!(
        health["trouble"]["accept_errors"].as_u64().unwrap() > 0,
        "{health}"
    );
    // and it said so, on stderr, which is the journal
    assert!(
        engine.said().contains("a connection was not accepted"),
        "{}",
        engine.said()
    );
}

#[test]
fn a_handler_that_panics_answers_500_and_the_engine_serves_on() {
    let home = TempDir::new("resilience-panic");
    ok(&home, &["key", "add", "k"], Some("a resilience test key\n"));
    ok(&home, &["init", "--key", "k"], None);
    let engine = Engine::start_with(
        &home,
        1024,
        2,
        &tokens(),
        &[("NILS_SERVE_TEST_PANIC_ON", "/api/panic-for-the-test")],
    );
    // more panics than handlers: none is lost to one
    for _ in 0..5 {
        let (status, _) = engine
            .get("/api/panic-for-the-test", Some(READERS[0]))
            .unwrap();
        assert_eq!(status, 500);
    }
    let (status, _) = engine.get("/api/status", Some(READERS[0])).unwrap();
    assert_eq!(status, 200);
    // the answer reaches the caller just before its handler says it is done
    let asked = Instant::now();
    let mut health = engine.health();
    while health["handlers"]["busy"] != 0 && asked.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
        health = engine.health();
    }
    assert_eq!(health["live"], true, "{health}");
    assert_eq!(health["trouble"]["panics"], 5, "{health}");
    assert_eq!(health["handlers"]["busy"], 0, "{health}");
    assert!(
        engine
            .said()
            .contains("a request handler panicked on GET /api/panic-for-the-test"),
        "{}",
        engine.said()
    );
}

/// Under systemd's watchdog the engine says it is ready once it listens and
/// feeds the watchdog while its own health door answers.
#[test]
fn under_a_watchdog_the_engine_says_ready_and_feeds_it() {
    let home = TempDir::new("resilience-notify");
    ok(&home, &["key", "add", "k"], Some("a resilience test key\n"));
    ok(&home, &["init", "--key", "k"], None);
    let socket = home.path().join("notify");
    let listen = std::os::unix::net::UnixDatagram::bind(&socket).unwrap();
    listen
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let _engine = Engine::start_with(
        &home,
        1024,
        2,
        &tokens(),
        &[
            ("NOTIFY_SOCKET", socket.to_str().unwrap()),
            ("WATCHDOG_USEC", "300000"),
        ],
    );
    // while it starts the watchdog is fed as it is; once it listens it says
    // READY=1 and goes on feeding it on its health door's live answer
    let mut heard: Vec<String> = Vec::new();
    let mut fed_since_ready = 0;
    let mut buf = [0u8; 256];
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(30) && fed_since_ready < 3 {
        let n = listen.recv(&mut buf).unwrap();
        let said = String::from_utf8_lossy(&buf[..n]).to_string();
        let ready = heard.iter().any(|m| m.starts_with("READY=1"));
        if ready && said == "WATCHDOG=1" {
            fed_since_ready += 1;
        }
        heard.push(said);
    }
    assert!(
        heard
            .iter()
            .any(|m| m.starts_with("READY=1\nSTATUS=serving 127.0.0.1:")),
        "{heard:?}"
    );
    assert_eq!(fed_since_ready, 3, "{heard:?}");
}

const NZ: u32 = 64;
const NY: u32 = 300;
const NX: u32 = 280;

fn us(tag: dicom_core::Tag, v: u16) -> synth::Elem {
    synth::bytes(tag, VR::US, v.to_le_bytes().to_vec())
}

/// A series of NZ planes, NY by NX: two by two tiles at the finest level.
fn series(dir: &TempDir, study: &str, series: &str) {
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
        let mut px = Vec::with_capacity((NY * NX * 2) as usize);
        for y in 0..NY {
            for x in 0..NX {
                let v: u16 = ((x * 5 + y * 3 + z * 97) % 4096) as u16;
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

#[test]
fn many_readers_warming_pictures_leave_the_engine_bounded_and_answering() {
    if !Path::new("/proc/self/fd").exists() {
        eprintln!("no /proc: the descriptor count cannot be read here");
        return;
    }
    let home = TempDir::new("resilience-load");
    let src = TempDir::new("resilience-load-src");
    let work = TempDir::new("resilience-load-work");
    series(&src, "1.2.9.A", "1.2.9.A.1");
    series(&src, "1.2.9.B", "1.2.9.B.1");
    ok(&home, &["key", "add", "k"], Some("a resilience test key\n"));
    ok(&home, &["init", "--key", "k"], None);
    ok(
        &home,
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            src.path().to_str().unwrap(),
        ],
        None,
    );
    ok(&home, &["fingerprint"], None);
    ok(
        &home,
        &["classify", "--pack-dir", packs().to_str().unwrap()],
        None,
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
        None,
    );
    for stack in ["1", "2"] {
        ok(
            &home,
            &["pyramid", "build", "--stack", stack, "--workers", "4"],
            None,
        );
    }
    // 16 handlers under 512 open files: the readers' connections, the
    // handlers' registries and the tile reads must all fit, and they do
    // only while the tile reads are bounded
    let workers = 16;
    let engine = Engine::start(&home, 512, workers, &tokens());
    let (status, _) = engine
        .get("/api/instances/1/slab/0/0-32", Some(READERS[0]))
        .unwrap();
    assert_eq!(status, 200);
    let idle_files = engine.open_files().unwrap();

    // four readers with twelve connections each, asking slabs of both stacks
    // at every level as fast as they come back, for four seconds: a page's
    // viewer, its volume and the warmer of the next items together
    let until = Instant::now() + Duration::from_secs(4);
    let port = engine.port;
    let mut askers = Vec::new();
    for (r, token) in READERS.iter().enumerate() {
        for c in 0..12usize {
            askers.push(std::thread::spawn(move || {
                let mut answered = std::collections::BTreeMap::<u16, usize>::new();
                let mut n = r * 31 + c * 7;
                while Instant::now() < until {
                    n += 1;
                    let stack = 1 + n % 2;
                    let level = [0, 0, 1, 2, 3][n % 5];
                    let z0 = (n % 2) * 32;
                    // slabs, which the slab cache keeps, and single planes'
                    // tiles, which are read from their files every time
                    let path = if n % 3 == 0 {
                        format!("/api/instances/{stack}/tiles/{level}/{}", n % NZ as usize)
                    } else {
                        format!("/api/instances/{stack}/slab/{level}/{z0}-{}", z0 + 32)
                    };
                    let status = get(port, &path, Some(token)).map_or(0, |(s, _)| s);
                    *answered.entry(status).or_default() += 1;
                }
                answered
            }));
        }
    }
    // beside them, the doors that are not pictures, and the descriptors
    let mut slowest = Duration::ZERO;
    let mut most_files = 0;
    while Instant::now() < until {
        let asked = Instant::now();
        let (status, _) = engine.get("/api/status", Some(READERS[1])).unwrap();
        assert_eq!(status, 200);
        slowest = slowest.max(asked.elapsed());
        most_files = most_files.max(engine.open_files().unwrap_or(0));
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut answered = std::collections::BTreeMap::<u16, usize>::new();
    for a in askers {
        for (status, n) in a.join().unwrap() {
            *answered.entry(status).or_default() += n;
        }
    }
    let health = engine.health();
    eprintln!(
        "answered {answered:?}; descriptors {idle_files} idle, {most_files} at most; slowest status {slowest:?}; {health}"
    );
    // every picture answered whole, nothing turned away, nothing refused
    assert_eq!(
        answered.keys().copied().collect::<Vec<_>>(),
        vec![200],
        "{answered:?}"
    );
    assert!(answered[&200] > 48, "{answered:?}");
    assert!(
        health["tile_reads"]["total"].as_u64().unwrap() > 1000,
        "the tiles were read from their files: {health}"
    );
    // the descriptors stayed within the readers' 48 connections (two each),
    // the engine's turns at tile files and the handlers' registries
    let bound = idle_files + 48 * 2 + 64 + workers as usize * 4;
    assert!(
        most_files <= bound,
        "{most_files} descriptors, more than {bound}"
    );
    assert!(most_files < 512, "{most_files}");
    assert_eq!(health["live"], true, "{health}");
    assert_eq!(health["trouble"]["accept_errors"], 0, "{health}");
    assert_eq!(health["trouble"]["panics"], 0, "{health}");
    assert!(
        health["tile_reads"]["peak"].as_u64().unwrap()
            <= health["tile_reads"]["at_once"].as_u64().unwrap(),
        "{health}"
    );
    // a claim or a status is not stuck behind the pictures
    assert!(slowest < Duration::from_secs(3), "{slowest:?}");
}

/// `--requests N` (for tests) ends the engine as it always did: each
/// handler ends after it serves a request at or past N, or when it finds
/// nothing to take for 250 ms once N are served, and the engine exits when
/// they all have. A caller that keeps asking past N is answered by a handler
/// still there or turned away at once, never left waiting, and the engine
/// ends by itself with success.
#[test]
fn a_server_told_its_requests_ends_by_itself_while_a_caller_keeps_asking() {
    let home = TempDir::new("resilience-requests");
    ok(&home, &["key", "add", "k"], Some("a resilience test key\n"));
    ok(&home, &["init", "--key", "k"], None);
    let mut extra = tokens();
    extra.extend(["--requests".to_string(), "3".to_string()]);
    let mut engine = Engine::start(&home, 1024, 2, &extra);
    for i in 0..3 {
        let (status, body) = engine.get("/api/status", Some(READERS[0])).unwrap();
        assert_eq!(status, 200, "ask {i}: {}", String::from_utf8_lossy(&body));
    }
    // past N, every 50 ms: each ask comes back at once, whatever it says
    let asking = Instant::now();
    let ended = loop {
        if let Some(status) = engine.child.try_wait().unwrap() {
            break Some(status);
        }
        if asking.elapsed() > Duration::from_secs(20) {
            break None;
        }
        let asked = Instant::now();
        let _ = engine.get("/api/status", Some(READERS[0]));
        assert!(
            asked.elapsed() < Duration::from_secs(5),
            "an ask past N waited"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        ended.is_some_and(|s| s.success()),
        "{ended:?}: {}",
        engine.said()
    );
}
