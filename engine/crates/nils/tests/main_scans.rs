// SPDX-License-Identifier: AGPL-3.0-only
//! Record 55, decision 6 (the main-scans study, sections 1 to 5), end to
//! end on a synthetic registry: a cohort reads the pack's picks until its
//! own rules are saved as version 2 at the door, which queues the cohort's
//! run; the run writes the cohort's own picks beside the pack's; the map,
//! subjects and strips doors answer from the axes; a person's pick of
//! nothing on a visit stands in the cohort's scope through a rerun; and a
//! release of the cohort's picked stacks records the rules it read. On
//! SQLite always, and on Postgres where a test DSN is set.

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
    loop {
        assert_eq!(job["state"], "done", "{job}");
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
    assert_eq!(job["kind"], "pick", "{job}");
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
