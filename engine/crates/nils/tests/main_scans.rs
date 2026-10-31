// SPDX-License-Identifier: AGPL-3.0-only
//! Record 55, decision 6 (the main-scans study, sections 1 to 5), end to
//! end on a synthetic registry: a cohort reads the pack's picks until its
//! own rules are saved as version 2 at the door, which queues the cohort's
//! run; the run writes the cohort's own picks beside the pack's; the map,
//! subjects and strips doors answer from the axes; a person's pick of
//! nothing on a visit stands in the cohort's scope through a rerun; and a
//! release of the cohort's picked stacks records the rules it read. Record
//! 55 H3: Browse's doors read the whole registry as the dataset viewer's
//! read a dataset, and a cohort's picks door answers each member's sessions
//! with each role picked, doubted or missing. On SQLite always, and on
//! Postgres where a test DSN is set.

use std::io::Write as _;
use std::process::{Command, Stdio};

use nils_dicom::synth::TempDir;
use nils_registry::Store;
use serde_json::{Value, json};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../packs")
        .to_str()
        .unwrap()
        .to_string()
}

struct Home {
    dir: TempDir,
    /// The Postgres DSN and schema, when the registry lives there.
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

    /// A key and an empty registry, on Postgres when `pg` names one.
    fn init(name: &str, pg: Option<(String, String)>) -> Home {
        let home = Home {
            dir: TempDir::new(name),
            pg,
        };
        let (good, _, err) = home.run(&["key", "add", "k"], Some("a main scans test key\n"));
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
        home
    }

    fn count(&self, sql: &str) -> i64 {
        let mut store = self.store();
        let sql = sql.replace("{pick}", &store.qualified("pick"));
        store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
    }
}

const OPS: &str = "an-operator-token-of-length";
const CURATOR: &str = "a-curator-token-of-its-length";
const PLAIN: &str = "a-plain-token-of-its-length";
const READER: &str = "a-reader-token-of-its-length";
const SEALED: &str = "a-sealed-token-of-its-length";

/// A `nils serve` with its worker, killed when dropped.
struct Served {
    child: std::process::Child,
    port: u16,
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Served {
    fn start(home: &Home, ingest: &std::path::Path) -> Served {
        use std::io::BufRead as _;
        let tokens = [
            format!("{OPS}=ops@lab:operator"),
            format!("{CURATOR}=cleo@lab:reviewer,data:see"),
            format!("{PLAIN}=pat@lab:campaigns:see"),
            format!("{READER}=rita@lab:reader"),
            format!("{SEALED}=sid@lab:operator,sealed:see"),
        ]
        .join(",");
        let child = nils()
            .arg("--registry")
            .arg(home.dir.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "2",
                "--worker",
            ])
            .args(["--auth", "token", "--pack-dir", &packs()])
            .arg("--ingest-root")
            .arg(format!("ds={}", ingest.display()))
            .env("NILS_TOKENS", tokens)
            .env("NILS_PACK_DIR", packs())
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut held = Served { child, port: 0 };
        let stdout = held.child.stdout.take().unwrap();
        let mut lines = std::io::BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            panic!("nils serve did not listen");
        };
        let addr = first.split_whitespace().nth(2).unwrap();
        held.port = addr.rsplit(':').next().unwrap().parse().unwrap();
        std::thread::spawn(move || for _ in lines {});
        held
    }

    /// A dataset's summary with its counts read now (2026-10-31): the door
    /// answers the kept counts at once and reads them again apart when what
    /// they count moved, saying `counting`, so the summary is asked again
    /// until it no longer counts, a minute at most.
    fn summary(&self, path: &str, token: &str) -> (u16, Value) {
        let (mut status, mut doc) = self.call("GET", path, None, token);
        for _ in 0..600 {
            if status != 200 || doc["counting"] != true {
                return (status, doc);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            (status, doc) = self.call("GET", path, None, token);
        }
        panic!("{path} never stopped counting: {doc}");
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> (u16, Value) {
        use std::io::Read as _;
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
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

    /// A job once it is over, two minutes at most.
    fn over(&self, job: i64) -> Value {
        for _ in 0..1200 {
            let (_, j) = self.call("GET", &format!("/api/jobs/{job}"), None, OPS);
            if matches!(j["state"].as_str(), Some("done" | "failed" | "cancelled")) {
                return j;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("job {job} did not end");
    }
}

/// A synthetic registry with its sessions built, `role t1w` on every T1w
/// stack, the pack's picks run, and a cohort of four subjects who hold one.
fn registry(pg: Option<(String, String)>) -> Home {
    let home = Home::init("main-scans-home", pg);
    home.ok(&["synth", "--seed", "11", "--subjects", "12"]);
    home.ok(&["session", "rebuild"]);
    let mut store = home.store();
    let axis = store.qualified("classification_axis");
    store
        .execute(
            &format!(
                "INSERT INTO {axis} (stack_id, axis, value, confidence, tier) \
                 SELECT stack_id, 'role', 't1w', 1.0, 'rule' FROM {axis} \
                 WHERE axis = 'base' AND value = 'T1w'"
            ),
            &[],
        )
        .unwrap();
    let sql = format!(
        "SELECT DISTINCT su.code FROM {} ca JOIN {} f ON f.stack_id = ca.stack_id \
         JOIN {} su ON su.id = f.subject_id WHERE ca.axis = 'role' ORDER BY su.code",
        store.qualified("classification_axis"),
        store.qualified("stack_fingerprint"),
        store.qualified("subject"),
    );
    let codes: Vec<String> = store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect();
    drop(store);
    assert!(codes.len() > 5, "{codes:?}");
    home.ok(&["clinical", "cohort", "make", "quad"]);
    let mut add = vec!["clinical", "cohort", "add", "quad"];
    add.extend(codes.iter().take(4).map(String::as_str));
    add.extend(["--why", "a main scans test"]);
    home.ok(&add);
    home.ok(&["pick", "run", "--pack-dir", &packs(), "--json"]);
    home
}

/// The Postgres DSN of the tests, and a schema of this file's dropped
/// before and after `run`.
fn on_postgres(schema: &str, run: impl FnOnce(Option<(String, String)>)) {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        return;
    };
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    run(Some((dsn.clone(), schema.to_string())));
    drop();
}

#[test]
fn a_cohort_s_own_rules_make_its_picks_and_a_release_records_them() {
    cohort_rules(None);
}

/// 2026-10-11, the live check of dev.25: on Postgres the map with a draft
/// and the subjects door answered 500, a sum being NUMERIC there.
#[test]
fn a_cohort_s_own_rules_make_its_picks_on_postgres_too() {
    on_postgres("nils_main_scans_cohort", cohort_rules);
}

fn cohort_rules(pg: Option<(String, String)>) {
    let home = registry(pg);
    let p = packs();
    let ingest = TempDir::new("main-scans-ingest");
    let pack_picks = home.count("SELECT COUNT(*) FROM {pick} WHERE rules_scope_kind IS NULL");
    assert!(pack_picks > 0);
    let server = Served::start(&home, ingest.path());

    // nothing saved: version 1, the pack's defaults
    let (status, doc) = server.call("GET", "/api/picks/rules?cohort=quad", None, OPS);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["scope"]["name"], "quad", "{doc}");
    assert_eq!(doc["pack"]["name"], "mri", "{doc}");
    assert_eq!(doc["current"]["version"], 1, "{doc}");
    assert_eq!(doc["current"]["saved"], false, "{doc}");
    assert_eq!(doc["current"]["reason"], "the pack's defaults", "{doc}");
    assert_eq!(doc["versions"].as_array().unwrap().len(), 1, "{doc}");
    let t1 = &doc["current"]["rules"]["roles"]["t1w"];
    assert_eq!(t1["keep_alike"], "balanced", "{doc}");
    assert_eq!(t1["kinds_in_order"][0], "3D MP2RAGE", "{doc}");
    let (status, text) = server.call("GET", "/api/picks/rules/text?cohort=quad", None, OPS);
    assert_eq!(status, 200);
    let text = text.as_str().unwrap();
    assert!(text.contains("keep_alike: balanced"), "{text}");
    assert!(text.contains("# null means any"), "{text}");
    // who may read and save; what is not there
    let (status, _) = server.call("GET", "/api/picks/rules?cohort=quad", None, PLAIN);
    assert_eq!(status, 403);
    let (status, _) = server.call("GET", "/api/picks/rules?cohort=nobody", None, OPS);
    assert_eq!(status, 404);

    // a draft at the map door shows its effect before it is saved
    let mut rules = doc["current"]["rules"].clone();
    rules["roles"]["t1w"]["keep_alike"] = json!("across_the_data");
    rules["same_kind_in_one_visit"]["near_tie_within_percent"] = json!(2);
    let (status, map) = server.call(
        "POST",
        "/api/picks/map",
        Some(json!({"scope": {"cohort": "quad"}, "role": "t1w", "rules": rules})),
        OPS,
    );
    assert_eq!(status, 200, "{map}");
    assert_eq!(map["rules_version"], 1, "{map}");
    assert!(map["effect"]["visits_changed"].is_i64(), "{map}");
    assert!(
        map["effect"]["before"]["visits"].as_i64().unwrap() > 0,
        "{map}"
    );

    // refused: a field out of bounds names it, a save says why, a reader
    // may not save
    let mut bad = rules.clone();
    bad["same_kind_in_one_visit"]["weights"]["slice_count"] = json!(2);
    let (status, refused) = server.call(
        "POST",
        "/api/picks/rules?cohort=quad",
        Some(json!({"rules": bad, "reason": "too many slices", "based_on": 1})),
        OPS,
    );
    assert_eq!(status, 400, "{refused}");
    assert_eq!(
        refused["field"], "same_kind_in_one_visit.weights.slice_count",
        "{refused}"
    );
    let (status, _) = server.call(
        "POST",
        "/api/picks/rules?cohort=quad",
        Some(json!({"rules": rules, "reason": " ", "based_on": 1})),
        OPS,
    );
    assert_eq!(status, 422);
    let (status, _) = server.call(
        "POST",
        "/api/picks/rules?cohort=quad",
        Some(json!({"rules": rules, "reason": "mine", "based_on": 1})),
        CURATOR,
    );
    assert_eq!(status, 403);

    // saved as version 2, which queues the cohort's run
    let (status, saved) = server.call(
        "POST",
        "/api/picks/rules?cohort=quad",
        Some(json!({"rules": rules, "reason": "each visit its best kind", "based_on": 1})),
        OPS,
    );
    assert_eq!(status, 201, "{saved}");
    assert_eq!(saved["version"], 2, "{saved}");
    let (status, behind) = server.call(
        "POST",
        "/api/picks/rules?cohort=quad",
        Some(json!({"rules": rules, "reason": "again", "based_on": 1})),
        OPS,
    );
    assert_eq!(status, 409, "{behind}");
    assert_eq!(behind["latest"], 2, "{behind}");
    let job = server.over(saved["job"].as_i64().unwrap());
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(job["name"], "cohort:quad", "{job}");
    let report = &job["result"];
    assert_eq!(report["rules"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(report["rules"][0]["version"], 2, "{report}");
    assert_eq!(report["rules"][0]["scope"]["name"], "quad", "{report}");
    let own = home.count(
        "SELECT COUNT(*) FROM {pick} WHERE rules_scope_kind = 'cohort' AND rules_version = 2",
    );
    assert!(own > 0, "the cohort's own picks");
    assert_eq!(
        home.count("SELECT COUNT(*) FROM {pick} WHERE rules_scope_kind IS NULL"),
        pack_picks,
        "the pack's picks are the pack's run's"
    );
    let (_, doc) = server.call("GET", "/api/picks/rules?cohort=quad", None, OPS);
    assert_eq!(doc["current"]["version"], 2, "{doc}");
    assert_eq!(doc["current"]["saved"], true, "{doc}");
    assert_eq!(doc["current"]["author"], "ops@lab", "{doc}");
    assert_eq!(
        doc["current"]["rules"]["roles"]["t1w"]["keep_alike"],
        "across_the_data"
    );
    assert_eq!(doc["versions"][0]["version"], 2, "{doc}");
    assert_eq!(doc["versions"][1]["version"], 1, "{doc}");
    let (_, text) = server.call(
        "GET",
        "/api/picks/rules/text?cohort=quad&version=1",
        None,
        OPS,
    );
    assert!(text.as_str().unwrap().contains("keep_alike: balanced"));

    // the map, its subjects and its strips under the saved rules
    let (status, map) = server.call(
        "POST",
        "/api/picks/map",
        Some(json!({"scope": {"cohort": "quad"}, "role": "t1w", "columns": "scanner"})),
        OPS,
    );
    assert_eq!(status, 200, "{map}");
    assert_eq!(map["rules_version"], 2, "{map}");
    assert!(map.get("effect").is_none(), "{map}");
    let m = &map["metrics"];
    assert!(m["visits_taken"].as_i64().unwrap() > 0, "{map}");
    assert!(m["subjects"].as_i64().unwrap() <= 4, "{map}");
    assert!(!map["kinds"].as_array().unwrap().is_empty(), "{map}");
    let columns = map["columns"].as_array().unwrap();
    let (kind, column) = map["matrix"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|row| {
            let cells = row["cells"].as_array().unwrap();
            let i = cells.iter().position(|c| c.as_i64().unwrap() > 0)?;
            Some((row["kind"].clone(), columns[i]["key"].clone()))
        })
        .expect("a cell with a visit");
    let (status, page) = server.call(
        "POST",
        "/api/picks/map/subjects",
        Some(json!({
            "scope": {"cohort": "quad"}, "role": "t1w",
            "group": {"by": "cell", "kind": kind, "column": column},
            "order": "visits", "page": 0, "per_page": 24,
        })),
        OPS,
    );
    assert_eq!(status, 200, "{page}");
    assert!(page["total"].as_i64().unwrap() >= 1, "{page}");
    let subject = &page["subjects"][0];
    let visit = &subject["visits"][0];
    assert!(subject["subject"].is_string(), "{page}");
    assert_eq!(visit["visit"], 1, "{page}");
    assert!(visit["session_id"].is_i64(), "{page}");
    assert!(
        visit.get("session").is_none(),
        "no date leaves the door: {page}"
    );
    assert!(
        !visit["candidates"].as_array().unwrap().is_empty(),
        "{page}"
    );
    let (status, strips) = server.call(
        "POST",
        "/api/picks/map/strips",
        Some(json!({"scope": {"cohort": "quad"}, "role": "t1w"})),
        OPS,
    );
    assert_eq!(status, 200, "{strips}");
    assert_eq!(
        strips["subjects"].as_array().unwrap().len() as i64,
        m["subjects"].as_i64().unwrap(),
        "{strips}"
    );

    // a person's pick of nothing on a visit, in the cohort's scope
    let subject_id = subject["subject_id"].as_i64().unwrap();
    let session_id = visit["session_id"].as_i64().unwrap();
    // #453: a scheme the registry does not keep is not there (404, naming
    // it), at each door that takes one; nothing failed
    for (door, body, token) in [
        (
            "/api/picks/run",
            json!({"cohort": "quad", "scheme": "nope"}),
            OPS,
        ),
        (
            "/api/picks/map",
            json!({"scope": {"cohort": "quad"}, "role": "t1w", "scheme": "nope"}),
            OPS,
        ),
        (
            "/api/picks/map/subjects",
            json!({"scope": {"cohort": "quad"}, "role": "t1w", "scheme": "nope",
                   "group": {"by": "breaks"}}),
            OPS,
        ),
        (
            "/api/picks/map/strips",
            json!({"scope": {"cohort": "quad"}, "role": "t1w", "scheme": "nope"}),
            OPS,
        ),
        (
            "/api/picks",
            json!({
                "role": "t1w", "stacks": [], "why": "no usable T1w on this visit",
                "cohort": "quad", "subject_id": subject_id, "session_id": session_id,
                "scheme": "nope",
            }),
            CURATOR,
        ),
    ] {
        let (status, refused) = server.call("POST", door, Some(body), token);
        assert_eq!(status, 404, "{door}: {refused}");
        assert!(
            refused.to_string().contains("no scheme called nope"),
            "{door}: {refused}"
        );
    }
    let (status, person) = server.call(
        "POST",
        "/api/picks",
        Some(json!({
            "role": "t1w", "stacks": [], "why": "no usable T1w on this visit",
            "cohort": "quad", "subject_id": subject_id, "session_id": session_id,
        })),
        CURATOR,
    );
    assert_eq!(status, 201, "{person}");
    assert_eq!(person["scope"]["name"], "quad", "{person}");
    assert_eq!(person["scope"]["kind"], "cohort", "{person}");
    assert!(person.get("rules_scope").is_none(), "{person}");
    assert!(
        person.get("session_day").is_none(),
        "no date leaves the door: {person}"
    );
    let person_id = person["id"].as_i64().unwrap();
    // it stands through a rerun of the cohort, the pack's and its own
    let (status, started) = server.call(
        "POST",
        "/api/picks/run",
        Some(json!({"cohort": "quad"})),
        OPS,
    );
    assert_eq!(status, 202, "{started}");
    let done = server.over(started["job"].as_i64().unwrap());
    assert_eq!(done["state"], "done", "{done}");
    let scopes = done["result"]["rules"].as_array().unwrap();
    assert_eq!(scopes.len(), 2, "{done}");
    assert!(scopes[0]["scope"].is_null(), "{done}");
    assert_eq!(scopes[1]["standing"], 1, "{done}");
    assert_eq!(
        home.count(&format!(
            "SELECT COUNT(*) FROM {{pick}} WHERE id = {person_id} AND withdrawn_at IS NULL \
             AND rules_scope_kind = 'cohort'"
        )),
        1,
        "the person's pick stands in its scope"
    );
    let (_, page) = server.call(
        "POST",
        "/api/picks/map/subjects",
        Some(json!({
            "scope": {"cohort": "quad"}, "role": "t1w",
            "group": {"by": "cell", "kind": null, "column": column},
            "order": "visits",
        })),
        OPS,
    );
    let mine = page["subjects"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["visits"].as_array().unwrap().iter())
        .find(|v| v["pick"] == person_id)
        .unwrap_or_else(|| panic!("the person's visit: {page}"));
    assert_eq!(mine["by"], "person", "{mine}");
    assert!(mine["kind"].is_null(), "{mine}");
    assert_eq!(mine["why"], "no usable T1w on this visit", "{mine}");
    // withdrawn in the scope it belongs to, and not in another's name
    let (status, _) = server.call(
        "POST",
        &format!("/api/picks/{person_id}/withdraw"),
        Some(json!({"dataset": "nowhere"})),
        CURATOR,
    );
    assert_eq!(status, 404);
    drop(server);

    // a release of the cohort's picked stacks reads its picks and records
    // the rules
    let out = TempDir::new("main-scans-out");
    home.ok(&[
        "place",
        "add",
        "out",
        out.path().to_str().unwrap(),
        "--role",
        "export",
    ]);
    let tree = out.path().join("quad");
    let _ = home.run(
        &[
            "release",
            "--name",
            "quad-main",
            "--out",
            tree.to_str().unwrap(),
            "--picked",
            "--cohort",
            "quad",
            "--on-unknown",
            "write",
            "--pack-dir",
            &p,
        ],
        None,
    );
    let mut store = home.store();
    let row = store
        .query(
            &format!(
                "SELECT CAST(pick_rules AS TEXT) FROM {} ORDER BY id DESC LIMIT 1",
                store.qualified("release")
            ),
            &[],
        )
        .unwrap();
    assert_eq!(row.len(), 1, "the release has a row");
    let recorded: Value = serde_json::from_str(row[0].text(0).unwrap()).unwrap();
    assert_eq!(recorded["scope"]["name"], "quad", "{recorded}");
    assert_eq!(recorded["scope"]["kind"], "cohort", "{recorded}");
    assert_eq!(recorded["version"], 2, "{recorded}");
    assert!(recorded["digest"].is_string(), "{recorded}");
}

/// A run removes its own scope's run picks that no longer win: a visit
/// whose stacks lost the role (here, every stack of two subjects) keeps no
/// run pick, and a person's pick there stands.
#[test]
fn a_run_removes_its_picks_that_no_longer_win_and_leaves_a_person_s() {
    let home = registry(None);
    let p = packs();
    let mut store = home.store();
    let pick = store.qualified("pick");
    let pick_stack = store.qualified("pick_stack");
    let rows = store
        .query(
            &format!(
                "SELECT DISTINCT p.subject_id, ps.stack_id FROM {pick} p \
                 JOIN {pick_stack} ps ON ps.pick_id = p.id \
                 WHERE p.author_kind = 'agent' AND p.rules_scope_kind IS NULL ORDER BY p.subject_id"
            ),
            &[],
        )
        .unwrap();
    let mut subjects: Vec<(i64, i64)> = rows
        .iter()
        .map(|r| (r.int(0).unwrap(), r.int(1).unwrap()))
        .collect();
    subjects.dedup_by_key(|(s, _)| *s);
    assert!(subjects.len() >= 2, "{subjects:?}");
    let ((gone, _), (kept, kept_stack)) = (subjects[0], subjects[1]);
    // a person's pick on one of them
    let set: Value = serde_json::from_str(&home.ok(&[
        "pick",
        "set",
        "--pack-dir",
        &p,
        "--role",
        "t1w",
        "--stack",
        &kept_stack.to_string(),
        "--why",
        "this one, by eye",
        "--json",
    ]))
    .unwrap();
    let person = set["id"].as_i64().unwrap();
    // both subjects' stacks hold the role no more
    let axis = store.qualified("classification_axis");
    let fp = store.qualified("stack_fingerprint");
    store
        .execute(
            &format!(
                "DELETE FROM {axis} WHERE axis = 'role' AND stack_id IN \
                 (SELECT stack_id FROM {fp} WHERE subject_id IN ({gone}, {kept}))"
            ),
            &[],
        )
        .unwrap();
    let report: Value =
        serde_json::from_str(&home.ok(&["pick", "run", "--pack-dir", &p, "--json"])).unwrap();
    assert!(report["removed"].as_i64().unwrap() >= 2, "{report}");
    assert_eq!(
        home.count(&format!(
            "SELECT COUNT(*) FROM {{pick}} WHERE author_kind = 'agent' AND subject_id IN ({gone}, {kept})"
        )),
        0,
        "no run pick where nothing wins"
    );
    assert_eq!(
        home.count(&format!(
            "SELECT COUNT(*) FROM {{pick}} WHERE id = {person} AND withdrawn_at IS NULL"
        )),
        1,
        "the person's pick stands"
    );
}

/// 2026-10-11, the live check of dev.25: a dataset added from a root. The
/// root is no scope of the picks doors (the desk offered it as a dataset);
/// the map's columns by dataset are named after the dataset, not the root
/// that holds it; the map with a draft and the subjects door answer; and a
/// person's pick answers its scope and no date.
#[test]
fn a_dataset_under_a_root_is_mapped_as_itself() {
    under_a_root(None);
}

#[test]
fn a_dataset_under_a_root_is_mapped_as_itself_on_postgres_too() {
    on_postgres("nils_main_scans_root", under_a_root);
}

fn under_a_root(pg: Option<(String, String)>) {
    use dicom_core::VR;
    use dicom_dictionary_std::tags;
    use nils_dicom::synth::{self, MetaFields};
    let home = Home::init("main-scans-root-home", pg);
    // a root with one dataset folder: three subjects, two of them seen twice
    let root = TempDir::new("main-scans-root");
    let mut n = 0;
    for (s, patient) in ["S-ONE", "S-TWO", "S-THREE"].iter().enumerate() {
        let days: &[&str] = if s < 2 {
            &["20240131", "20250131"]
        } else {
            &["20240131"]
        };
        for (v, day) in days.iter().enumerate() {
            n += 1;
            let study = format!("1.2.826.0.1.3680043.8.498.{n}.1");
            let series = format!("{study}.1");
            for instance in 1..=3u32 {
                let sop = format!("{series}.{instance}");
                let mut e = synth::minimal_mr(&study, &series, &sop);
                e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
                e.push(synth::text(tags::STUDY_DATE, VR::DA, day));
                e.push(synth::text(tags::SERIES_NUMBER, VR::IS, "1"));
                e.push(synth::text(
                    tags::INSTANCE_NUMBER,
                    VR::IS,
                    &instance.to_string(),
                ));
                e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
                root.file(
                    &format!("fresh/derivatives/dcm-anon/sub-{s}/ses-{v}/IM_{instance:04}"),
                    &synth::part10(&MetaFields::mr(&sop), &e, true),
                );
            }
        }
    }
    home.ok(&[
        "place",
        "add",
        "data",
        root.path().to_str().unwrap(),
        "--role",
        "source",
    ]);
    let server = Served::start(&home, root.path());
    let (status, added) = server.call(
        "POST",
        "/api/places",
        Some(json!({
            "role": "source", "root": "data", "folder": "fresh",
            "patient_id": "id-type:patient-id", "subjects": "generated",
        })),
        OPS,
    );
    assert_eq!(status, 201, "{added}");
    assert_eq!(added["name"], "fresh", "{added}");
    // the bring-in: digest, fingerprint, classify and the dataset's pick run
    let (status, queued) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["bring-in", "@fresh", "--name", "first"]})),
        OPS,
    );
    assert_eq!(status, 202, "{queued}");
    let mut job = server.over(queued["job"].as_i64().unwrap());
    let mut kinds = Vec::new();
    loop {
        assert_eq!(job["state"], "done", "{job}");
        kinds.push(job["kind"].as_str().unwrap_or("").to_string());
        let mut next = job["chain"]["after"].as_i64();
        for _ in 0..50 {
            if next.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            let (_, j) = server.call("GET", &format!("/api/jobs/{}", job["id"]), None, OPS);
            next = j["chain"]["after"].as_i64();
        }
        match next {
            Some(n) => job = server.over(n),
            None => break,
        }
    }
    // record 55 E5: the score comes after the pick run, last
    assert_eq!(kinds[kinds.len() - 2..], ["pick", "score"], "{job}");
    assert!(
        home.count("SELECT COUNT(*) FROM {pick} WHERE withdrawn_at IS NULL") > 0,
        "the bring-in picked"
    );

    // the root is no scope: it holds datasets and is none
    let (status, refused) = server.call("GET", "/api/picks/rules?dataset=data", None, OPS);
    assert_eq!(status, 400, "{refused}");
    assert_eq!(refused["field"], "dataset", "{refused}");
    let (status, refused) = server.call(
        "POST",
        "/api/picks/map",
        Some(json!({"scope": {"dataset": "data"}, "role": "t1w"})),
        OPS,
    );
    assert_eq!(status, 400, "{refused}");
    assert_eq!(refused["field"], "dataset", "{refused}");

    // the columns by dataset are the dataset's, not the root's
    let (status, map) = server.call(
        "POST",
        "/api/picks/map",
        Some(json!({"scope": {"dataset": "fresh"}, "role": "t1w", "columns": "dataset"})),
        OPS,
    );
    assert_eq!(status, 200, "{map}");
    let columns = map["columns"].as_array().unwrap();
    assert!(!columns.is_empty(), "{map}");
    assert!(
        columns
            .iter()
            .all(|c| c["key"].as_str().unwrap().starts_with("fresh")),
        "{map}"
    );
    assert_eq!(map["metrics"]["subjects"], 3, "{map}");

    // a draft, its subjects and the strips
    let (status, rules) = server.call("GET", "/api/picks/rules?dataset=fresh", None, OPS);
    assert_eq!(status, 200, "{rules}");
    assert_eq!(rules["scope"]["name"], "fresh", "{rules}");
    let mut draft = rules["current"]["rules"].clone();
    draft["roles"]["t1w"]["keep_alike"] = json!("within_each_subject");
    draft["roles"]["t1w"]["dimension"] = json!("3d");
    let (status, map) = server.call(
        "POST",
        "/api/picks/map",
        Some(json!({"scope": {"dataset": "fresh"}, "role": "t1w", "rules": draft, "columns": "dataset"})),
        OPS,
    );
    assert_eq!(status, 200, "{map}");
    assert!(map["effect"]["visits_changed"].is_i64(), "{map}");
    for group in [
        json!({"by": "breaks"}),
        json!({"by": "series", "steps": []}),
    ] {
        let (status, page) = server.call(
            "POST",
            "/api/picks/map/subjects",
            Some(json!({
                "scope": {"dataset": "fresh"}, "role": "t1w", "rules": draft,
                "group": group, "order": "changes", "page": 0, "per_page": 24,
            })),
            OPS,
        );
        assert_eq!(status, 200, "{page}");
        assert!(page["total"].is_i64(), "{page}");
    }
    let column = columns[0]["key"].clone();
    let (status, page) = server.call(
        "POST",
        "/api/picks/map/subjects",
        Some(json!({
            "scope": {"dataset": "fresh"}, "role": "t1w", "columns": "dataset",
            "group": {"by": "cell", "kind": map["matrix"][0]["kind"], "column": column},
            "order": "visits",
        })),
        OPS,
    );
    assert_eq!(status, 200, "{page}");
    assert!(page["total"].as_i64().unwrap() >= 1, "{page}");
    let visit = &page["subjects"][0]["visits"][0];
    assert!(
        visit["column"].as_str().unwrap().starts_with("fresh"),
        "{page}"
    );
    let (status, strips) = server.call(
        "POST",
        "/api/picks/map/strips",
        Some(json!({"scope": {"dataset": "fresh"}, "role": "t1w", "rules": draft})),
        OPS,
    );
    assert_eq!(status, 200, "{strips}");
    assert_eq!(strips["subjects"].as_array().unwrap().len(), 3, "{strips}");

    // a person's pick made for the dataset before it has rules is the
    // pack's, and answers so, with no date
    let (status, person) = server.call(
        "POST",
        "/api/picks",
        Some(json!({
            "role": "t1w", "stacks": [], "why": "no usable T1w on this visit",
            "dataset": "fresh", "subject_id": page["subjects"][0]["subject_id"],
            "session_id": visit["session_id"],
        })),
        CURATOR,
    );
    assert_eq!(status, 201, "{person}");
    assert!(person["scope"].is_null(), "{person}");
    assert!(person.get("session_day").is_none(), "{person}");
}

/// Nima, 2026-10-12: a dataset is a folder that holds stacks, and a scan
/// copied into two datasets is one stack with a path in each (the
/// duplicate policy of 2026-10-10). The map by dataset shows it under
/// every dataset whose folder holds a file of it, as the one place that
/// says which folders hold a stack reads them; each of those columns
/// counts its visit, and the map says how many visits it shows under more
/// than one column; what is no column's (the visits, the subjects, the
/// strips) counts it once; and a cell's subjects are found under either
/// dataset.
#[test]
fn a_scan_two_datasets_hold_is_mapped_under_each() {
    held_twice(None);
}

#[test]
fn a_scan_two_datasets_hold_is_mapped_under_each_on_postgres_too() {
    on_postgres("nils_main_scans_twice", held_twice);
}

fn held_twice(pg: Option<(String, String)>) {
    use dicom_core::VR;
    use dicom_dictionary_std::tags;
    use nils_dicom::synth::{self, MetaFields};
    let home = Home::init("main-scans-twice-home", pg);
    // S-ONE seen twice in the first dataset, its first visit copied into
    // the second; S-TWO in the first alone, S-THREE in the second alone
    let first = TempDir::new("main-scans-first");
    let second = TempDir::new("main-scans-second");
    for (n, (patient, day, trees)) in [
        ("S-ONE", "20240131", &[&first, &second][..]),
        ("S-ONE", "20250131", &[&first][..]),
        ("S-TWO", "20240131", &[&first][..]),
        ("S-THREE", "20240131", &[&second][..]),
    ]
    .into_iter()
    .enumerate()
    {
        let study = format!("1.2.826.0.1.3680043.8.498.{}.1", n + 1);
        let series = format!("{study}.1");
        for instance in 1..=3u32 {
            let sop = format!("{series}.{instance}");
            let mut e = synth::minimal_mr(&study, &series, &sop);
            e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
            e.push(synth::text(tags::STUDY_DATE, VR::DA, day));
            e.push(synth::text(tags::SERIES_NUMBER, VR::IS, "1"));
            e.push(synth::text(
                tags::INSTANCE_NUMBER,
                VR::IS,
                &instance.to_string(),
            ));
            e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
            let bytes = synth::part10(&MetaFields::mr(&sop), &e, true);
            for tree in trees {
                tree.file(
                    &format!("derivatives/dcm-anon/{patient}/{n}/IM_{instance:04}"),
                    &bytes,
                );
            }
        }
    }
    for (name, tree) in [("first", &first), ("second", &second)] {
        home.ok(&[
            "place",
            "add",
            name,
            tree.path().to_str().unwrap(),
            "--role",
            "source",
            "--patient-id",
            "id-type:patient-id",
            "--subjects",
            "generated",
        ]);
        home.ok(&[
            "digest",
            "--name",
            name,
            "--no-private",
            &format!("@{name}"),
        ]);
    }
    home.ok(&["fingerprint"]);
    home.ok(&["classify", "--pack-dir", &packs()]);
    home.ok(&["pick", "run", "--pack-dir", &packs(), "--json"]);
    // the copied scan is one stack, with a file in each folder
    let mut store = home.store();
    let sql = format!("SELECT COUNT(*) FROM {}", store.qualified("stack"));
    let stacks = store.query(&sql, &[]).unwrap()[0].int(0).unwrap();
    assert_eq!(stacks, 4, "the copied scan is one stack");
    let sql = format!(
        "SELECT COUNT(*) FROM (SELECT stack_id FROM {} GROUP BY stack_id HAVING COUNT(*) = 2) t",
        store.qualified("source_stack")
    );
    let held = store.query(&sql, &[]).unwrap()[0].int(0).unwrap();
    assert_eq!(held, 1, "one stack has a file in each folder");
    drop(store);

    let ingest = TempDir::new("main-scans-twice-ingest");
    let server = Served::start(&home, ingest.path());
    let map_of = |scope: &str| -> Value {
        let (status, map) = server.call(
            "POST",
            "/api/picks/map",
            Some(json!({"scope": {"dataset": scope}, "role": "t1w", "columns": "dataset"})),
            OPS,
        );
        assert_eq!(status, 200, "{map}");
        map
    };
    // a column's key, by its dataset's name
    let key = |map: &Value, name: &str| -> (usize, Value) {
        let columns = map["columns"].as_array().unwrap();
        let i = columns
            .iter()
            .position(|c| c["key"].as_str().unwrap().starts_with(name))
            .unwrap_or_else(|| panic!("no column {name}: {map}"));
        (i, columns[i]["key"].clone())
    };

    // the first dataset's subjects, S-ONE and S-TWO, and their three visits:
    // S-ONE's first visit is in both columns, and each counts it
    let map = map_of("first");
    assert_eq!(map["metrics"]["subjects"], 2, "{map}");
    assert_eq!(map["metrics"]["visits"], 3, "{map}");
    let columns = map["columns"].as_array().unwrap();
    assert_eq!(columns.len(), 2, "{map}");
    let ((a, first_key), (b, second_key)) = (key(&map, "first"), key(&map, "second"));
    assert_eq!((a, b), (0, 1), "the column with more visits first: {map}");
    assert_eq!(columns[a]["visits"], 3, "{map}");
    assert_eq!(columns[a]["shared"], 1, "{map}");
    assert_eq!(columns[b]["visits"], 1, "{map}");
    assert_eq!(columns[b]["shared"], 1, "{map}");
    assert_eq!(map["shared_visits"], 1, "{map}");
    let cells: i64 = map["matrix"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r["cells"].as_array().unwrap().iter())
        .map(|c| c.as_i64().unwrap())
        .sum();
    assert_eq!(cells, 3 + 1, "the visits, and the shared one again: {map}");
    let by_visit: i64 = map["by_visit"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["visits"].as_i64().unwrap())
        .sum();
    assert_eq!(by_visit, 3, "each visit once: {map}");

    // a cell of the second dataset finds S-ONE through the copied scan,
    // whose visit names both columns, the first first
    let kind = map["matrix"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["cells"][b].as_i64().unwrap() > 0)
        .map(|r| r["kind"].clone())
        .unwrap_or_else(|| panic!("no cell under the second dataset: {map}"));
    let (status, page) = server.call(
        "POST",
        "/api/picks/map/subjects",
        Some(json!({
            "scope": {"dataset": "first"}, "role": "t1w", "columns": "dataset",
            "group": {"by": "cell", "kind": kind, "column": second_key},
            "order": "visits",
        })),
        OPS,
    );
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["total"], 1, "{page}");
    let visits = page["subjects"][0]["visits"].as_array().unwrap();
    assert_eq!(visits.len(), 2, "{page}");
    let copied = visits
        .iter()
        .find(|v| v["columns"].as_array().unwrap().len() == 2)
        .unwrap_or_else(|| panic!("no visit under both: {page}"));
    assert_eq!(copied["columns"], json!([first_key, second_key]), "{page}");
    assert_eq!(copied["column"], first_key, "{page}");
    assert!(
        visits.iter().any(|v| v["columns"] == json!([first_key])),
        "{page}"
    );

    // the strips draw each subject once
    let (status, strips) = server.call(
        "POST",
        "/api/picks/map/strips",
        Some(json!({"scope": {"dataset": "first"}, "role": "t1w", "columns": "dataset"})),
        OPS,
    );
    assert_eq!(status, 200, "{strips}");
    assert_eq!(strips["subjects"].as_array().unwrap().len(), 2, "{strips}");
    assert_eq!(strips["shared_visits"], 1, "{strips}");
    assert_eq!(strips["columns"], map["columns"], "{strips}");

    // the second dataset's subjects, S-ONE and S-THREE, on the stacks the
    // second holds alone (the second review of the map's layers,
    // 2026-10-15; Nima, 2026-10-12: a dataset holds stacks, never a
    // subject's membership): S-ONE's copied visit, under both datasets, and
    // S-THREE's, never S-ONE's visit the first holds alone
    let map = map_of("second");
    assert_eq!(map["metrics"]["subjects"], 2, "{map}");
    assert_eq!(map["metrics"]["visits"], 2, "{map}");
    let ((_, first_key), (_, second_key)) = (key(&map, "first"), key(&map, "second"));
    assert_eq!(
        map["columns"],
        json!([
            {"key": second_key, "visits": 2, "shared": 1},
            {"key": first_key, "visits": 1, "shared": 1},
        ]),
        "{map}"
    );
    assert_eq!(map["shared_visits"], 1, "{map}");

    // by scanner a visit is in one column
    let (status, map) = server.call(
        "POST",
        "/api/picks/map",
        Some(json!({"scope": {"dataset": "first"}, "role": "t1w", "columns": "scanner"})),
        OPS,
    );
    assert_eq!(status, 200, "{map}");
    assert_eq!(map["shared_visits"], 0, "{map}");
    let shown: i64 = map["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["visits"].as_i64().unwrap())
        .sum();
    assert_eq!(shown, 3, "{map}");

    // the third review of the map's layers (2026-10-15): a dataset's own
    // pick run chooses among the stacks it holds, as its map reads them, so
    // its saved picks name none of S-ONE's visit the first holds alone, and
    // the saved side agrees with a draft that says the rules again
    let (status, rules) = server.call("GET", "/api/picks/rules?dataset=second", None, OPS);
    assert_eq!(status, 200, "{rules}");
    let current = rules["current"]["rules"].clone();
    let (status, saved) = server.call(
        "POST",
        "/api/picks/rules?dataset=second",
        Some(json!({"rules": current, "reason": "the second's own picks", "based_on": 1})),
        OPS,
    );
    assert_eq!(status, 201, "{saved}");
    let job = server.over(saved["job"].as_i64().unwrap());
    assert_eq!(job["state"], "done", "{job}");
    let mut store = home.store();
    let sql = format!(
        "SELECT COUNT(*) FROM {pick} p JOIN {ps} s ON s.pick_id = p.id \
         WHERE p.rules_scope_kind = 'dataset' AND p.withdrawn_at IS NULL \
         AND s.stack_id NOT IN (SELECT ss.stack_id FROM {src} ss JOIN {source} so ON so.id = ss.source_id \
         WHERE so.root_canonical LIKE '{root}%')",
        root = second.path().canonicalize().unwrap().display(),
        pick = store.qualified("pick"),
        ps = store.qualified("pick_stack"),
        src = store.qualified("source_stack"),
        source = store.qualified("source"),
    );
    assert_eq!(
        store.query(&sql, &[]).unwrap()[0].int(0).unwrap(),
        0,
        "the second's picks name only its own stacks"
    );
    let own = format!(
        "SELECT COUNT(*) FROM {} WHERE rules_scope_kind = 'dataset' AND withdrawn_at IS NULL",
        store.qualified("pick")
    );
    assert_eq!(
        store.query(&own, &[]).unwrap()[0].int(0).unwrap(),
        2,
        "a pick a visit of the second"
    );
    drop(store);
    let (status, map) = server.call(
        "POST",
        "/api/picks/map",
        Some(json!({"scope": {"dataset": "second"}, "role": "t1w", "rules": current})),
        OPS,
    );
    assert_eq!(status, 200, "{map}");
    assert_eq!(map["metrics"]["visits_taken"], 2, "{map}");
    assert_eq!(map["effect"]["visits_changed"], 0, "{map}");
}

/// 2026-10-12 (Nima on the development build): a dataset's run of its main
/// scans finished at once, its log said "Main scans picked for 10
/// subjects", and nothing was picked. A pick run brings the session cache
/// up to date before it reads visits, and says what it wrote: the visits
/// and subjects it picked on, or, where it wrote nothing, why (no scan of a
/// main role, no visit). The Main scans map of a dataset read after the
/// cache was built shows its visits, and the dataset's summary says no
/// count of visits while the cache lacks some of its studies, rather than
/// 0.
#[test]
fn a_pick_run_and_the_map_read_visits_the_cache_did_not_hold() {
    uncached(None);
}

#[test]
fn a_pick_run_and_the_map_read_visits_the_cache_did_not_hold_on_postgres_too() {
    on_postgres("nils_main_scans_uncached", uncached);
}

fn uncached(pg: Option<(String, String)>) {
    use dicom_core::VR;
    use dicom_dictionary_std::tags;
    use nils_dicom::synth::{self, MetaFields};
    let home = Home::init("main-scans-uncached-home", pg);
    let root = TempDir::new("main-scans-uncached");
    // four datasets: `fresh`, a T1 on two days of one subject and one of
    // another; `nameless`, one subject whose only series no rule reads;
    // `undated`, one subject's T1 in a study with no date; and `later`, two
    // subjects' T1s read after the cache was built
    let mut n = 0;
    let mut write = |folder: &str, patient: &str, day: Option<&str>, description: &str| {
        n += 1;
        let study = format!("1.2.826.0.1.3680043.8.497.{n}.1");
        let series = format!("{study}.1");
        for instance in 1..=3u32 {
            let sop = format!("{series}.{instance}");
            let mut e = synth::minimal_mr(&study, &series, &sop);
            e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
            if let Some(day) = day {
                e.push(synth::text(tags::STUDY_DATE, VR::DA, day));
            }
            e.push(synth::text(tags::SERIES_NUMBER, VR::IS, "1"));
            e.push(synth::text(
                tags::INSTANCE_NUMBER,
                VR::IS,
                &instance.to_string(),
            ));
            e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
            root.file(
                &format!("{folder}/derivatives/dcm-anon/{patient}/{n}/IM_{instance:04}"),
                &synth::part10(&MetaFields::mr(&sop), &e, true),
            );
        }
    };
    write("fresh", "S-ONE", Some("20240131"), "t1 mprage");
    write("fresh", "S-ONE", Some("20250131"), "t1 mprage");
    write("fresh", "S-TWO", Some("20240131"), "t1 mprage");
    write("nameless", "S-THREE", Some("20240131"), "zzqx");
    write("undated", "S-FOUR", None, "t1 mprage");
    write("later", "S-FIVE", Some("20240131"), "t1 mprage");
    write("later", "S-SIX", Some("20240131"), "t1 mprage");
    let packs = packs();
    let tree = |name: &str| {
        root.path()
            .join(name)
            .join("derivatives/dcm-anon")
            .to_str()
            .unwrap()
            .to_string()
    };
    for name in ["fresh", "nameless", "undated", "later"] {
        home.ok(&[
            "place",
            "add",
            name,
            root.path().join(name).to_str().unwrap(),
            "--role",
            "source",
            "--patient-id",
            "id-type:patient-id",
            "--subjects",
            "generated",
        ]);
    }
    let sort = |home: &Home| {
        home.ok(&["fingerprint"]);
        home.ok(&["classify", "--pack-dir", &packs]);
    };
    // read and sorted from the keyboard, so no pick run follows the sort
    // and nothing builds the cache
    for name in ["fresh", "nameless", "undated"] {
        home.ok(&["digest", &tree(name), "--name", name]);
    }
    sort(&home);
    let cached = |home: &Home| {
        let mut store = home.store();
        let sql = format!(
            "SELECT COUNT(*) FROM {}",
            store.qualified("session_cache_study")
        );
        store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
    };
    assert_eq!(cached(&home), 0, "nothing has built the cache");

    // the pick run builds it, and picks each visit's T1
    let run = |dataset: &str| -> Value {
        let out = home.ok(&[
            "pick",
            "run",
            "--dataset",
            dataset,
            "--pack-dir",
            &packs,
            "--json",
        ]);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    };
    let fresh = run("fresh");
    assert_eq!(fresh["subjects"], 2, "{fresh}");
    assert_eq!(fresh["written"], 3, "{fresh}");
    assert_eq!(fresh["visits"], 3, "{fresh}");
    assert_eq!(fresh["visits_picked"], 3, "{fresh}");
    assert_eq!(fresh["subjects_picked"], 2, "{fresh}");
    assert!(fresh.get("nothing").is_none(), "{fresh}");
    assert_eq!(
        fresh["said"], "Main scans picked for 3 visits of 2 subjects",
        "{fresh}"
    );
    assert!(cached(&home) > 0, "the run built the cache");
    let listed = home.ok(&["pick", "list", "--dataset", "fresh", "--json"]);
    let listed: Value = serde_json::from_str(&listed).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 3, "{listed}");
    // the job says it too
    let job = fresh["job"].as_i64().unwrap().to_string();
    let shown = home.ok(&["jobs", "show", &job]);
    assert!(
        shown.contains("said        Main scans picked for 3 visits of 2 subjects"),
        "{shown}"
    );
    // and a run that writes nothing says why
    let nameless = run("nameless");
    assert_eq!(nameless["written"], 0, "{nameless}");
    assert_eq!(nameless["visits_picked"], 0, "{nameless}");
    assert_eq!(
        nameless["nothing"], "its one subject has no scan of a main role",
        "{nameless}"
    );
    assert_eq!(
        nameless["said"], "No main scans picked: its one subject has no scan of a main role",
        "{nameless}"
    );
    let undated = run("undated");
    assert_eq!(undated["written"], 0, "{undated}");
    assert_eq!(
        undated["nothing"], "no visits found for its one subject, whose studies carry no date",
        "{undated}"
    );
    let text = home.ok(&["pick", "run", "--dataset", "undated", "--pack-dir", &packs]);
    assert!(
        text.contains("No main scans picked: no visits found for its one subject"),
        "{text}"
    );

    // a dataset read after the cache was built: its summary counts no
    // visits until they are in it, and its map shows them
    home.ok(&["digest", &tree("later"), "--name", "later"]);
    sort(&home);
    let server = Served::start(&home, root.path());
    let (status, summary) = server.summary("/api/datasets/later/summary", OPS);
    assert_eq!(status, 200, "{summary}");
    assert_eq!(summary["subjects"], 2, "{summary}");
    assert!(
        summary["sessions"].is_null(),
        "the cache holds none of its visits yet: {summary}"
    );
    let (status, map) = server.call(
        "POST",
        "/api/picks/map",
        Some(json!({"scope": {"dataset": "later"}, "role": "t1w"})),
        OPS,
    );
    assert_eq!(status, 200, "{map}");
    assert_eq!(map["metrics"]["subjects"], 2, "{map}");
    assert_eq!(map["metrics"]["visits"], 2, "{map}");
    let (status, strips) = server.call(
        "POST",
        "/api/picks/map/strips",
        Some(json!({"scope": {"dataset": "later"}, "role": "t1w"})),
        OPS,
    );
    assert_eq!(status, 200, "{strips}");
    assert_eq!(strips["subjects"].as_array().unwrap().len(), 2, "{strips}");
    let (_, summary) = server.summary("/api/datasets/later/summary", OPS);
    assert_eq!(summary["sessions"], 2, "{summary}");
    let (_, summary) = server.summary("/api/datasets/fresh/summary", OPS);
    assert_eq!(summary["sessions"], 3, "{summary}");
}

#[test]
fn browse_reads_the_registry_and_a_cohort_s_picks_each_session() {
    browse_and_picks(None);
}

#[test]
fn browse_reads_the_registry_and_a_cohort_s_picks_each_session_on_postgres_too() {
    on_postgres("nils_main_scans_browse", browse_and_picks);
}

/// Whether a text is a shape: digits as 9, letters as a or A.
fn shaped(text: &str) -> bool {
    text.chars()
        .all(|c| !c.is_ascii_alphanumeric() || matches!(c, '9' | 'a' | 'A'))
}

fn browse_and_picks(pg: Option<(String, String)>) {
    let home = registry(pg);
    let ingest = TempDir::new("main-scans-browse-ingest");
    let server = Served::start(&home, ingest.path());
    let ok = |path: &str, token: &str| {
        let (status, doc) = server.call("GET", path, None, token);
        assert_eq!(status, 200, "{path}: {doc}");
        doc
    };
    let mut store = home.store();
    let stacks = store
        .query(
            &format!("SELECT COUNT(*) FROM {}", store.qualified("stack")),
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    let subjects = store
        .query(
            &format!(
                "SELECT COUNT(DISTINCT subject_id) FROM {}",
                store.qualified("series")
            ),
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    drop(store);

    // ------------------------------------------------ Browse
    let all = ok("/api/subjects", OPS);
    assert_eq!(all["scope"], json!({"kind": "registry"}), "{all}");
    assert_eq!(all["totals"]["subjects"], subjects, "{}", all["totals"]);
    assert_eq!(all["totals"]["scans"], stacks, "{all}");
    let first = ok("/api/subjects?limit=5&order=code", OPS);
    assert_eq!(first["count"], 5, "{first}");
    let next = first["next"].as_i64().unwrap();
    let second = ok(
        &format!("/api/subjects?limit=5&order=code&after={next}"),
        OPS,
    );
    assert_ne!(first["subjects"][0]["id"], second["subjects"][0]["id"]);
    // a reader is at plain: the codes are their shapes
    let plain = ok("/api/subjects", READER);
    assert_eq!(plain["detail"], "plain", "{plain}");
    for s in plain["subjects"].as_array().unwrap() {
        assert!(shaped(s["code"].as_str().unwrap()), "{s}");
    }
    let (status, _) = server.call("GET", "/api/subjects", None, PLAIN);
    assert_eq!(status, 403, "Browse is data:see's");
    // one subject's sessions and one session's stacks
    let subject = all["subjects"][0]["id"].as_i64().unwrap();
    let scans = all["subjects"][0]["scans"].as_i64().unwrap();
    let visits = ok(&format!("/api/subjects/{subject}/sessions"), OPS);
    assert_eq!(visits["scope"], json!({"kind": "registry"}), "{visits}");
    let listed: i64 = visits["visits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["scans"].as_i64().unwrap())
        .sum();
    assert_eq!(listed, scans, "{visits}");
    let mut seen = 0;
    for v in visits["visits"].as_array().unwrap() {
        let page = match v["session"].as_i64() {
            Some(id) => ok(&format!("/api/sessions/{id}/stacks?limit=200"), OPS),
            None => {
                let studies: Vec<String> = v["studies"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                ok(
                    &format!("/api/sessions/none/stacks?studies={}", studies.join(",")),
                    OPS,
                )
            }
        };
        assert_eq!(page["scope"], json!({"kind": "registry"}), "{page}");
        assert_eq!(page["count"], v["scans"], "{v} {page}");
        seen += page["count"].as_i64().unwrap();
    }
    assert_eq!(seen, scans);
    let (status, _) = server.call("GET", "/api/sessions/none/stacks", None, OPS);
    assert_eq!(status, 400);
    let (status, _) = server.call("GET", "/api/subjects/999999/sessions", None, OPS);
    assert_eq!(status, 404);
    // every scan a page at a time
    let page = ok("/api/scans?limit=7", OPS);
    assert_eq!(page["total"], stacks, "{page}");
    assert_eq!(page["count"], 7, "{page}");
    let after = page["next"].as_i64().unwrap();
    let more = ok(&format!("/api/scans?limit=7&after={after}"), OPS);
    assert!(
        more["scans"][0]["stack"].as_i64().unwrap() != page["scans"][0]["stack"].as_i64().unwrap()
    );
    // a stack of a sample sealed now is in no number but to sealed:see
    let sealed = page["scans"][0]["stack"].as_i64().unwrap();
    let mut store = home.store();
    let owner = store
        .query(
            &format!(
                "SELECT se.subject_id FROM {} st JOIN {} se ON se.id = st.series_id WHERE st.id = {sealed}",
                store.qualified("stack"),
                store.qualified("series")
            ),
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    store
        .execute(
            &format!(
                "INSERT INTO {} (sample, stack_id, subject_id, sealed_by, sealed_at) \
                 VALUES ('a browse test', {sealed}, {owner}, 'anna@ward-3', '2026-10-13T00:00:00Z')",
                store.qualified("sealed_stack")
            ),
            &[],
        )
        .unwrap();
    drop(store);
    assert_eq!(ok("/api/scans?limit=1", OPS)["total"], stacks - 1);
    assert_eq!(ok("/api/scans?limit=1", SEALED)["total"], stacks);
    assert_eq!(ok("/api/subjects", OPS)["totals"]["scans"], stacks - 1);

    // ------------------------------------------------ a cohort's picks
    let doc = ok("/api/cohorts/quad/picks", CURATOR);
    assert_eq!(doc["cohort"]["name"], "quad", "{doc}");
    assert_eq!(doc["total"], 4, "{doc}");
    assert!(
        doc["roles"].as_array().unwrap().iter().any(|r| r == "t1w"),
        "{doc}"
    );
    let picked = doc["totals"]["t1w"]["picked"].as_i64().unwrap();
    assert!(picked > 0, "{doc}");
    // every picked role names its stack and the rules, every missing one nothing
    let mut counted = 0;
    let mut a_pick: Option<(i64, String, i64)> = None;
    for s in doc["subjects"].as_array().unwrap() {
        for (i, ses) in s["sessions"].as_array().unwrap().iter().enumerate() {
            assert_eq!(ses["number"], i + 1, "{ses}");
            assert!(ses["label"].as_str().unwrap().starts_with("ses-"), "{ses}");
            let t1 = &ses["roles"]["t1w"];
            match t1["state"].as_str().unwrap() {
                "picked" => {
                    counted += 1;
                    assert!(t1["stack"].is_i64() || t1["withheld"] == true, "{t1}");
                    assert_eq!(t1["by"], "rules", "{t1}");
                    if a_pick.is_none() {
                        let day = ses["label"].as_str().unwrap()[4..].to_string();
                        a_pick = Some((
                            s["id"].as_i64().unwrap(),
                            day,
                            ses["session"].as_i64().unwrap(),
                        ));
                    }
                }
                "missing" => assert!(t1["stack"].is_null() && t1["by"].is_null(), "{t1}"),
                other => panic!("{other}"),
            }
        }
    }
    assert_eq!(counted, picked, "the page holds the whole cohort: {doc}");
    // agrees with the picks summary, which counts the same picks
    let summary = ok("/api/picks/summary?cohort=quad", OPS);
    assert_eq!(summary["roles"]["t1w"]["picked"], picked, "{summary}");
    // paged by subject id
    let one = ok("/api/cohorts/quad/picks?limit=3", CURATOR);
    assert_eq!(one["count"], 3, "{one}");
    let next = one["next"].as_i64().unwrap();
    let two = ok(
        &format!("/api/cohorts/quad/picks?limit=3&after={next}"),
        CURATOR,
    );
    assert_eq!(two["count"], 1, "{two}");
    assert!(two["next"].is_null(), "{two}");
    // an open border doubts the role on that session
    let (subject, day, _) = a_pick.unwrap();
    let day = format!("{}-{}-{}", &day[..4], &day[4..6], &day[6..8]);
    let mut store = home.store();
    nils_registry::review::raise_pick_border(
        &mut store,
        &nils_registry::review::PickBorder {
            model: "mri-default",
            role: "t1w",
            subject_id: subject,
            day: &day,
            borders: &["too_close"],
            pick_id: None,
            score: None,
            margin: None,
            runner_up_score: None,
            considered: &json!([]),
            notes: &json!({}),
            job_id: None,
            scheme: &json!({}),
            pack: "mri",
            pack_version: "1",
        },
        "2026-10-13T00:00:00Z",
    )
    .unwrap();
    drop(store);
    let doc = ok("/api/cohorts/quad/picks", CURATOR);
    assert_eq!(doc["totals"]["t1w"]["doubted"], 1, "{doc}");
    assert_eq!(doc["totals"]["t1w"]["picked"], picked - 1, "{doc}");
    // a reader at plain: shapes; no grant: refused; no cohort: 404
    let plain = ok("/api/cohorts/quad/picks", READER);
    for s in plain["subjects"].as_array().unwrap() {
        assert!(shaped(s["code"].as_str().unwrap()), "{s}");
        for ses in s["sessions"].as_array().unwrap() {
            assert!(shaped(ses["label"].as_str().unwrap()), "{ses}");
        }
    }
    let (status, _) = server.call("GET", "/api/cohorts/quad/picks", None, PLAIN);
    assert_eq!(status, 403);
    let (status, _) = server.call("GET", "/api/cohorts/nobody/picks", None, CURATOR);
    assert_eq!(status, 404);
}
