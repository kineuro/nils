// SPDX-License-Identifier: AGPL-3.0-only
//! Record 55, decision 6 of 2026-10-10 ("The data map (v0's heatmap, for
//! datasets and cohorts, with layers such as body part and post-contrast)
//! is where the trade-off is seen"), left out of the main-scans study's
//! first round and ruled in on 2026-10-13: body part and post-contrast as
//! layers on the Main scans map, end to end through `nils serve`.
//!
//! On a cohort of a synthetic registry, the map, its subjects and its
//! strips asked for a layer say what each visit's MR scans are, by the
//! body-part step's axes and by the post-contrast label, the axis speaking
//! for a stack no label reached; the value of the scan each visit takes,
//! under a draft beside the saved rules; each candidate's value; and
//! whether the step that answers the layer has run over the cohort, as the
//! cohort's own page says it. A sealed stack is in no count for a caller
//! who may not read it, and a layer writes nothing. On a dataset brought
//! in from a folder, the layer's step is the dataset's, as its Data page
//! carries it. On SQLite always, and on Postgres where a test DSN is set.

use std::collections::BTreeMap;
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
        let (good, _, err) = home.run(&["key", "add", "k"], Some("a map layers test key\n"));
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

    /// One number a statement answers, `{name}` standing for a table.
    fn count(&self, sql: &str) -> i64 {
        let mut store = self.store();
        let mut sql = sql.to_string();
        for t in [
            "stack",
            "series",
            "classification_axis",
            "contrast_label",
            "cohort",
            "cohort_member",
        ] {
            sql = sql.replace(&format!("{{{t}}}"), &store.qualified(t));
        }
        store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
    }

    /// Statements run in order, `{name}` standing for a table.
    fn execute(&self, statements: &[String]) {
        let mut store = self.store();
        for sql in statements {
            let mut sql = sql.clone();
            for t in [
                "stack",
                "series",
                "classification_axis",
                "contrast_label",
                "sealed_stack",
            ] {
                sql = sql.replace(&format!("{{{t}}}"), &store.qualified(t));
            }
            store.execute(&sql, &[]).unwrap();
        }
    }
}

const OPS: &str = "an-operator-token-of-length";
const SEALED: &str = "a-sealed-token-of-its-length";
const REVIEWER: &str = "a-review-only-token-of-length";

/// A `nils serve`, killed when dropped; with its worker where jobs must
/// run, without it where nothing else may write while a door is watched.
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
    fn start(home: &Home, ingest: &std::path::Path, worker: bool) -> Served {
        use std::io::BufRead as _;
        let tokens = [
            format!("{OPS}=ops@lab:operator"),
            format!("{SEALED}=sid@lab:operator,sealed:see"),
            format!("{REVIEWER}=rin@lab:review:see"),
        ]
        .join(",");
        let mut command = nils();
        command
            .arg("--registry")
            .arg(home.dir.path())
            .args(["serve", "--bind", "127.0.0.1:0"]);
        if worker {
            command.args(["--workers", "2", "--worker"]);
        }
        let child = command
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

    /// A map door's answer, which must be 200.
    fn post(&self, path: &str, body: Value, token: &str) -> Value {
        let (status, answer) = self.call("POST", path, Some(body), token);
        assert_eq!(status, 200, "{path}: {answer}");
        answer
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
/// stack, the pack's picks run, a cohort of four subjects who hold one,
/// and the post-contrast label of every session, as a sort writes it.
fn registry(pg: Option<(String, String)>) -> Home {
    let home = Home::init("map-layers-home", pg);
    home.ok(&["synth", "--seed", "11", "--subjects", "12"]);
    home.ok(&["session", "rebuild"]);
    home.execute(&[
        "INSERT INTO {classification_axis} (stack_id, axis, value, confidence, tier) \
         SELECT stack_id, 'role', 't1w', 1.0, 'rule' FROM {classification_axis} \
         WHERE axis = 'base' AND value = 'T1w'"
            .to_string(),
    ]);
    let mut store = home.store();
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
    add.extend(["--why", "a map layers test"]);
    home.ok(&add);
    home.ok(&["pick", "run", "--pack-dir", &packs(), "--json"]);
    home.ok(&["contrast", "label", "--pack-dir", &packs(), "--json"]);
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

/// What says whether anything was written: on SQLite the version of the
/// database another connection's commit moves (`PRAGMA data_version`), on
/// Postgres each table's rows and the newest transaction that wrote one.
fn written(home: &Home, watch: &mut Store) -> String {
    match &home.pg {
        None => watch.query("PRAGMA data_version", &[]).unwrap()[0]
            .int(0)
            .unwrap()
            .to_string(),
        Some((_, schema)) => {
            let tables: Vec<String> = watch
                .query(
                    &format!(
                        "SELECT table_name FROM information_schema.tables \
                         WHERE table_schema = '{schema}' AND table_type = 'BASE TABLE' \
                         ORDER BY table_name"
                    ),
                    &[],
                )
                .unwrap()
                .iter()
                .map(|r| r.text(0).unwrap().to_string())
                .collect();
            assert!(!tables.is_empty(), "the schema's tables");
            tables
                .iter()
                .map(|t| {
                    let r = &watch
                        .query(
                            &format!(
                                "SELECT CAST(COUNT(*) AS BIGINT), \
                                 CAST(COALESCE(MAX(CAST(CAST(xmin AS TEXT) AS BIGINT)), 0) AS BIGINT) \
                                 FROM \"{schema}\".\"{t}\""
                            ),
                            &[],
                        )
                        .unwrap()[0];
                    format!("{t}:{}:{}", r.int(0).unwrap(), r.int(1).unwrap())
                })
                .collect::<Vec<_>>()
                .join(",")
        }
    }
}

/// A visit's scans on a layer, by value.
fn scans_of(visit: &Value) -> BTreeMap<String, i64> {
    visit["layer"]["scans"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.clone(), v.as_i64().unwrap()))
        .collect()
}

/// The value most of a visit's scans carry, a tie to the first in `order`.
fn most(scans: &BTreeMap<String, i64>, order: &[&str]) -> Option<String> {
    let rank = |v: &str| order.iter().position(|o| *o == v).unwrap_or(order.len());
    scans
        .iter()
        .filter(|(_, n)| **n > 0)
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| rank(b.0).cmp(&rank(a.0))))
        .map(|(v, _)| v.clone())
}

#[test]
fn a_cohort_s_map_carries_its_layers() {
    cohort_layers(None);
}

#[test]
fn a_cohort_s_map_carries_its_layers_on_postgres_too() {
    on_postgres("nils_map_layers_cohort", cohort_layers);
}

fn cohort_layers(pg: Option<(String, String)>) {
    let home = registry(pg);
    assert!(
        home.count("SELECT COUNT(*) FROM {contrast_label}") > 0,
        "the label is written"
    );
    // the body-part step's coarse mode on every T1w scan: the head
    home.execute(&[
        "DELETE FROM {classification_axis} WHERE axis = 'body_region'".to_string(),
        "INSERT INTO {classification_axis} (stack_id, axis, value, confidence, tier) \
         SELECT stack_id, 'body_region', 'head', 1.0, 'rule' FROM {classification_axis} \
         WHERE axis = 'role' AND value = 't1w'"
            .to_string(),
    ]);
    let ingest = TempDir::new("map-layers-ingest");
    let server = Served::start(&home, ingest.path(), false);
    let scope = json!({"cohort": "quad"});

    // no layer asked, none answered; a layer the map does not know is refused
    let map = server.post(
        "/api/picks/map",
        json!({"scope": scope, "role": "t1w"}),
        OPS,
    );
    assert!(map.get("layer").is_none(), "{map}");
    for wrong in [json!("contrast"), json!(3)] {
        let (status, refused) = server.call(
            "POST",
            "/api/picks/map",
            Some(json!({"scope": scope, "role": "t1w", "layer": wrong})),
            OPS,
        );
        assert_eq!(status, 400, "{refused}");
        assert_eq!(refused["field"], "layer", "{refused}");
    }

    // a page of subjects with a visit that takes a scan
    let columns = map["columns"].as_array().unwrap().clone();
    let (kind, column) = map["matrix"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|row| {
            let cells = row["cells"].as_array().unwrap();
            let i = cells.iter().position(|c| c.as_i64().unwrap() > 0)?;
            row["kind"]
                .is_string()
                .then(|| (row["kind"].clone(), columns[i]["key"].clone()))
        })
        .expect("a cell of a kind with a visit");
    let page_of = |layer: &str| {
        server.post(
            "/api/picks/map/subjects",
            json!({
                "scope": scope, "role": "t1w", "layer": layer,
                "group": {"by": "cell", "kind": kind, "column": column},
                "order": "visits", "page": 0, "per_page": 48,
            }),
            OPS,
        )
    };
    let page = page_of("post_contrast");
    let (si, vi) = page["subjects"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .find_map(|(si, s)| {
            s["visits"]
                .as_array()
                .unwrap()
                .iter()
                .position(|v| v["stack"].is_i64() && v["kind"].is_string())
                .map(|vi| (si, vi))
        })
        .expect("a visit that takes a scan");
    let at = |page: &Value| page["subjects"][si]["visits"][vi].clone();
    let subject_id = page["subjects"][si]["subject_id"].as_i64().unwrap();
    let visit = at(&page);
    let taken = visit["stack"].as_i64().unwrap();
    let before = scans_of(&visit);
    let before_flag = visit["layer"]["prior_72h"].as_i64().unwrap();
    assert!(
        before.values().sum::<i64>() + visit["layer"]["none"].as_i64().unwrap() > 0,
        "{visit}"
    );
    // another MR scan of the same study, which no role holds
    let other = home.count(&format!(
        "SELECT COALESCE(MIN(x.id), 0) FROM {{stack}} x JOIN {{series}} se ON se.id = x.series_id \
         WHERE x.modality = 'MR' AND x.id <> {taken} \
         AND se.study_id = (SELECT se2.study_id FROM {{stack}} x2 JOIN {{series}} se2 ON se2.id = x2.series_id WHERE x2.id = {taken}) \
         AND NOT EXISTS (SELECT 1 FROM {{classification_axis}} a WHERE a.stack_id = x.id AND a.axis = 'role')"
    ));
    assert!(other > 0, "another scan of the visit");
    let state = |stack: i64| {
        let mut store = home.store();
        let sql = format!(
            "SELECT contrast_state, contrast_prior_72h FROM {} WHERE stack_id = {stack}",
            store.qualified("contrast_label")
        );
        store
            .query(&sql, &[])
            .unwrap()
            .first()
            .map(|r| (r.text(0).unwrap().to_string(), r.text(1).unwrap() == "yes"))
    };
    let (taken_was, taken_flagged) = state(taken).expect("the scan taken is labelled");
    let (other_was, _) = state(other).expect("the other scan is labelled");

    // the scan taken: a spine on the body-part step's coarse mode; post
    // contrast by its label, the subject given contrast in the 72 hours
    // before. The other scan: no label, its axis saying contrast was given.
    home.execute(&[
        format!(
            "UPDATE {{classification_axis}} SET value = 'spine' WHERE axis = 'body_region' AND stack_id = {taken}"
        ),
        // beside it a weaker answer and a row of no value, which neither
        // backend reads in its place: a value first, then the confidence
        format!(
            "INSERT INTO {{classification_axis}} (stack_id, axis, value, confidence, tier) \
             VALUES ({taken}, 'body_region', 'chest', 0.1, 'model')"
        ),
        format!(
            "INSERT INTO {{classification_axis}} (stack_id, axis, value, confidence, tier) \
             VALUES ({taken}, 'body_region', NULL, 1.0, 'rule')"
        ),
        format!(
            "UPDATE {{contrast_label}} SET contrast_state = 'post', contrast_prior_72h = 'yes' WHERE stack_id = {taken}"
        ),
        format!("DELETE FROM {{contrast_label}} WHERE stack_id = {other}"),
        format!(
            "DELETE FROM {{classification_axis}} WHERE axis = 'post_contrast' AND stack_id = {other}"
        ),
        format!(
            "INSERT INTO {{classification_axis}} (stack_id, axis, value, confidence, tier) \
             VALUES ({other}, 'post_contrast', '1', 1.0, 'rule')"
        ),
    ]);

    // post-contrast: the label's state, the axis for the scan no label reached
    let page = page_of("post_contrast");
    assert_eq!(page["subjects"][si]["subject_id"], subject_id, "{page}");
    let visit = at(&page);
    assert_eq!(visit["stack"], taken, "{visit}");
    assert_eq!(visit["layer"]["taken"], "post", "{visit}");
    let after = scans_of(&visit);
    let was = |s: &str| i64::from(s == "post");
    assert_eq!(
        after.get("post").copied().unwrap_or(0),
        before.get("post").copied().unwrap_or(0) + 2 - was(&taken_was) - was(&other_was),
        "{visit}"
    );
    assert_eq!(
        after.values().sum::<i64>(),
        before.values().sum::<i64>(),
        "the same scans, every one with a state: {visit}"
    );
    assert_eq!(
        visit["layer"]["prior_72h"].as_i64().unwrap(),
        before_flag + i64::from(!taken_flagged),
        "{visit}"
    );
    for c in visit["candidates"].as_array().unwrap() {
        if c["stacks"].as_array().unwrap().contains(&json!(taken)) {
            assert_eq!(c["layer"], "post", "{visit}");
        }
    }
    assert_eq!(page["layer"]["layer"], "post_contrast", "{page}");
    assert_eq!(page["layer"]["operation"], "post_contrast", "{page}");
    let values: Vec<&str> = page["layer"]["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(values.contains(&"post"), "{page}");
    let order = ["pre", "post", "by_definition", "undetermined"];
    let ranks: Vec<usize> = values
        .iter()
        .map(|v| order.iter().position(|o| o == v).unwrap())
        .collect();
    assert!(ranks.windows(2).all(|w| w[0] < w[1]), "in order: {page}");

    let map = server.post(
        "/api/picks/map",
        json!({"scope": scope, "role": "t1w", "layer": "post_contrast"}),
        OPS,
    );
    let layer = &map["layer"];
    assert_eq!(layer["layer"], "post_contrast", "{map}");
    assert!(layer["prior_72h"].as_i64().unwrap() >= 1, "{map}");
    let step = &layer["step"];
    assert_eq!(step["step"], "post_contrast", "{map}");
    assert!(step["label"]["labelled"].as_i64().unwrap() > 0, "{map}");
    // the step's scans are the cohort's, as its own page counts them
    let (status, cohort) = server.call("GET", "/api/cohorts/quad", None, OPS);
    assert_eq!(status, 200, "{cohort}");
    let own = cohort["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["step"] == "post_contrast")
        .unwrap_or_else(|| panic!("the cohort's post-contrast step: {cohort}"))
        .clone();
    for key in ["state", "of", "answered", "served", "refusal", "label"] {
        assert_eq!(step[key], own[key], "{key}: {step} against {own}");
    }

    // body region: the scan taken a spine, every other T1w scan the head
    let page = page_of("body_region");
    let visit = at(&page);
    assert_eq!(visit["layer"]["taken"], "spine", "{visit}");
    let scans = scans_of(&visit);
    let candidates: i64 = visit["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["stacks"].as_array().unwrap().len() as i64)
        .sum();
    assert_eq!(scans.get("spine"), Some(&1), "{visit}");
    assert_eq!(
        scans.values().sum::<i64>(),
        candidates,
        "the T1w scans carry a region, the rest none: {visit}"
    );
    assert!(visit["layer"]["none"].as_i64().unwrap() > 0, "{visit}");
    assert!(
        visit["layer"].get("prior_72h").is_none(),
        "a flag of post-contrast's alone: {visit}"
    );
    for c in visit["candidates"].as_array().unwrap() {
        let want = if c["stacks"].as_array().unwrap().contains(&json!(taken)) {
            "spine"
        } else {
            "head"
        };
        assert_eq!(c["layer"], want, "{visit}");
    }

    let map = server.post(
        "/api/picks/map",
        json!({"scope": scope, "role": "t1w", "layer": "body_region"}),
        OPS,
    );
    let layer = &map["layer"];
    assert_eq!(layer["layer"], "body_region", "{map}");
    assert_eq!(layer["operation"], "body_part", "{map}");
    let values = layer["values"].as_array().unwrap();
    assert_eq!(values.len(), 2, "{map}");
    assert_eq!(values[0]["value"], "head", "{map}");
    assert_eq!(values[1]["value"], "spine", "{map}");
    assert_eq!(values[1]["visits_with"], 1, "{map}");
    assert_eq!(values[1]["visits_taken"], 1, "{map}");
    assert_eq!(values[1]["scans"], 1, "{map}");
    assert!(values[0].get("visits_taken_before").is_none(), "{map}");
    assert_eq!(layer["none"]["visits"], 0, "every visit holds a T1w: {map}");
    assert_eq!(
        values[0]["visits_taken"].as_i64().unwrap()
            + values[1]["visits_taken"].as_i64().unwrap()
            + layer["none"]["taken"].as_i64().unwrap(),
        map["metrics"]["visits_taken"].as_i64().unwrap(),
        "every visit that takes a scan is counted once: {map}"
    );
    let t1w = home.count(
        "SELECT COUNT(DISTINCT a.stack_id) FROM {classification_axis} a \
         JOIN {stack} x ON x.id = a.stack_id JOIN {series} se ON se.id = x.series_id \
         JOIN {cohort_member} m ON m.subject_id = se.subject_id JOIN {cohort} c ON c.id = m.cohort_id \
         WHERE a.axis = 'role' AND a.value = 't1w' AND x.modality = 'MR' \
         AND c.name = 'quad' AND m.left_at IS NULL",
    );
    assert_eq!(
        values[0]["scans"].as_i64().unwrap() + values[1]["scans"].as_i64().unwrap(),
        t1w,
        "{map}"
    );
    assert!(layer.get("prior_72h").is_none(), "{map}");
    let step = &layer["step"];
    assert_eq!(step["step"], "body_part", "{map}");
    assert_eq!(
        step["state"], "off",
        "no body-part model is served here: {map}"
    );
    assert!(step["refusal"]["reason"].is_string(), "{map}");
    // no answer waits to be put in force
    assert_eq!(step["staged"], 0, "{map}");
    // the step alone, for a page to ask while it runs: the map's, and read
    // by one who may read only the cohort's heatmap
    for token in [OPS, REVIEWER] {
        let (status, alone) = server.call(
            "GET",
            "/api/picks/layer-step?cohort=quad&layer=body_region",
            None,
            token,
        );
        assert_eq!(status, 200, "{alone}");
        assert_eq!(alone["operation"], "body_part", "{alone}");
        assert_eq!(alone["step"]["state"], step["state"], "{alone}");
        assert_eq!(alone["step"]["of"], step["of"], "{alone}");
    }
    let (status, refused) = server.call("GET", "/api/picks/layer-step?cohort=quad", None, OPS);
    assert_eq!(status, 400, "{refused}");
    assert_eq!(refused["field"], "layer", "{refused}");

    // a draft: what it takes, beside what the saved rules take
    let (_, rules) = server.call("GET", "/api/picks/rules?cohort=quad", None, OPS);
    let mut draft = rules["current"]["rules"].clone();
    draft["roles"]["t1w"]["keep_alike"] = json!("across_the_data");
    let map = server.post(
        "/api/picks/map",
        json!({"scope": scope, "role": "t1w", "rules": draft, "layer": "body_region"}),
        OPS,
    );
    let layer = &map["layer"];
    assert!(map["effect"].is_object(), "{map}");
    for v in layer["values"].as_array().unwrap() {
        assert!(v["visits_taken_before"].is_i64(), "{map}");
    }
    assert!(layer["none"]["taken_before"].is_i64(), "{map}");

    // the strips: by visit, the value taken and the value most scans carry
    let strips = server.post(
        "/api/picks/map/strips",
        json!({"scope": scope, "role": "t1w", "layer": "body_region"}),
        OPS,
    );
    let values: Vec<String> = strips["layer"]["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(values, ["head", "spine"], "{strips}");
    assert!(
        strips["layer"].get("step").is_none(),
        "the step is the map door's: {strips}"
    );
    let mine = strips["subjects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["subject_id"] == subject_id)
        .unwrap_or_else(|| panic!("the subject's strip: {strips}"));
    let index = |v: &str| values.iter().position(|x| x == v).unwrap() as i64;
    let page = page_of("body_region");
    let cards = page["subjects"][si]["visits"].as_array().unwrap();
    assert_eq!(
        mine["layer"]["taken"].as_array().unwrap().len(),
        cards.len()
    );
    for (j, card) in cards.iter().enumerate() {
        let taken = card["layer"]["taken"].as_str().map_or(-1, index);
        assert_eq!(mine["layer"]["taken"][j], taken, "{mine} {card}");
        let held = most(&scans_of(card), &["head", "spine", "chest", "other"])
            .as_deref()
            .map_or(-1, index);
        assert_eq!(mine["layer"]["held"][j], held, "{mine} {card}");
    }

    // the cohort's own heatmap (its picks door) carries the same layer: each
    // session's scans by value, the value of each role's scan picked, and
    // the step as the cohort's door says it
    let (status, refused) = server.call("GET", "/api/cohorts/quad/picks?layer=colour", None, OPS);
    assert_eq!(status, 400, "{refused}");
    assert_eq!(refused["field"], "layer", "{refused}");
    let (status, heat) = server.call("GET", "/api/cohorts/quad/picks", None, OPS);
    assert_eq!(status, 200, "{heat}");
    assert!(heat.get("layer").is_none(), "{heat}");
    let (status, heat) = server.call(
        "GET",
        "/api/cohorts/quad/picks?layer=body_region&limit=24",
        None,
        OPS,
    );
    assert_eq!(status, 200, "{heat}");
    assert_eq!(heat["layer"]["layer"], "body_region", "{heat}");
    assert_eq!(heat["layer"]["operation"], "body_part", "{heat}");
    assert_eq!(heat["layer"]["values"], json!(["head", "spine"]), "{heat}");
    assert_eq!(heat["layer"]["step"]["state"], "off", "{heat}");
    // the cohort's step is given with the first page alone
    let first = heat["subjects"][0]["id"].as_i64().unwrap();
    let (status, later) = server.call(
        "GET",
        &format!("/api/cohorts/quad/picks?layer=body_region&limit=1&after={first}"),
        None,
        OPS,
    );
    assert_eq!(status, 200, "{later}");
    assert!(later["layer"]["step"].is_null(), "{later}");
    assert!(later["layer"]["values"].is_array(), "{later}");
    let sessions: Vec<&Value> = heat["subjects"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["sessions"].as_array().unwrap().iter())
        .collect();
    let picked_spine: Vec<&&Value> = sessions
        .iter()
        .filter(|s| s["roles"]["t1w"]["stack"] == taken)
        .collect();
    assert_eq!(picked_spine.len(), 1, "{heat}");
    assert_eq!(picked_spine[0]["roles"]["t1w"]["layer"], "spine", "{heat}");
    assert_eq!(picked_spine[0]["layer"]["scans"]["spine"], 1, "{heat}");
    for s in &sessions {
        assert!(s["layer"]["none"].is_i64(), "{s}");
        if s["roles"]["t1w"]["stack"].is_i64() && s["roles"]["t1w"]["stack"] != taken {
            assert_eq!(s["roles"]["t1w"]["layer"], "head", "{s}");
        }
    }

    // a scan of a sample sealed now is in no count for a caller who may not
    // read it, and in every count for one who may
    home.execute(&[format!(
        "INSERT INTO {{sealed_stack}} (sample, stack_id, subject_id, sealed_by, sealed_at) \
         VALUES ('s@1', {taken}, {subject_id}, 'anna', '2026-10-14T10:00:00Z')"
    )]);
    let hidden = server.post(
        "/api/picks/map",
        json!({"scope": scope, "role": "t1w", "layer": "body_region"}),
        OPS,
    );
    assert!(
        hidden["layer"]["values"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["value"] != "spine"),
        "{hidden}"
    );
    let shown = server.post(
        "/api/picks/map",
        json!({"scope": scope, "role": "t1w", "layer": "body_region"}),
        SEALED,
    );
    assert_eq!(shown["layer"]["values"][1]["value"], "spine", "{shown}");
    assert_eq!(shown["layer"]["values"][1]["visits_taken"], 1, "{shown}");

    // a layer writes nothing: each door with each layer, once to warm, then
    // again watched
    let doors = |layer: &str| {
        for (path, extra) in [
            ("/api/picks/map", json!({})),
            ("/api/picks/map/strips", json!({})),
            (
                "/api/picks/map/subjects",
                json!({"group": {"by": "breaks"}, "order": "changes"}),
            ),
        ] {
            let mut body = json!({"scope": scope, "role": "t1w", "layer": layer});
            for (k, v) in extra.as_object().unwrap() {
                body[k] = v.clone();
            }
            server.post(path, body, OPS);
        }
        let (status, heat) = server.call(
            "GET",
            &format!("/api/cohorts/quad/picks?layer={layer}"),
            None,
            OPS,
        );
        assert_eq!(status, 200, "{heat}");
    };
    for layer in ["body_region", "body_part", "post_contrast"] {
        doors(layer);
    }
    let mut watch = home.store();
    let first = written(&home, &mut watch);
    for layer in ["body_region", "body_part", "post_contrast"] {
        doors(layer);
    }
    assert_eq!(
        written(&home, &mut watch),
        first,
        "a read with a layer wrote to the registry"
    );
}

#[test]
fn a_dataset_s_map_carries_its_layers() {
    dataset_layers(None);
}

#[test]
fn a_dataset_s_map_carries_its_layers_on_postgres_too() {
    on_postgres("nils_map_layers_dataset", dataset_layers);
}

/// A dataset brought in from a folder, as a site's first dataset is: the
/// layer's step is the dataset's, as its Data page carries it, and the
/// post-contrast label the sort wrote is on the map.
fn dataset_layers(pg: Option<(String, String)>) {
    use dicom_core::VR;
    use dicom_dictionary_std::tags;
    use nils_dicom::synth::{self, MetaFields};
    let home = Home::init("map-layers-dataset-home", pg);
    // a root with one dataset folder: two subjects, one seen twice
    let root = TempDir::new("map-layers-root");
    let mut n = 0;
    for (s, patient) in ["S-ONE", "S-TWO"].iter().enumerate() {
        let days: &[&str] = if s == 0 {
            &["20240131", "20250131"]
        } else {
            &["20240131"]
        };
        for (v, day) in days.iter().enumerate() {
            n += 1;
            let study = format!("1.2.826.0.1.3680043.8.498.{n}.1");
            for (number, description) in [(1, "t1 mprage"), (2, "t2 tse")] {
                let series = format!("{study}.{number}");
                for instance in 1..=3u32 {
                    let sop = format!("{series}.{instance}");
                    let mut e = synth::minimal_mr(&study, &series, &sop);
                    e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
                    e.push(synth::text(tags::STUDY_DATE, VR::DA, day));
                    e.push(synth::text(
                        tags::SERIES_NUMBER,
                        VR::IS,
                        &number.to_string(),
                    ));
                    e.push(synth::text(
                        tags::INSTANCE_NUMBER,
                        VR::IS,
                        &instance.to_string(),
                    ));
                    e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
                    root.file(
                        &format!(
                            "fresh/derivatives/dcm-anon/sub-{s}/ses-{v}/IM_{number}_{instance:04}"
                        ),
                        &synth::part10(&MetaFields::mr(&sop), &e, true),
                    );
                }
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
    let server = Served::start(&home, root.path(), true);
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
    // the bring-in: digest, fingerprint, classify (which labels contrast)
    // and the dataset's pick run
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
    let scope = json!({"dataset": "fresh"});
    let (status, summary) = server.call("GET", "/api/datasets/fresh/summary", None, OPS);
    assert_eq!(status, 200, "{summary}");
    let own = |step: &str| {
        summary["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["step"] == step)
            .unwrap_or_else(|| panic!("the dataset's {step} step: {summary}"))
            .clone()
    };
    for (layer, operation) in [
        ("body_region", "body_part"),
        ("body_part", "body_part"),
        ("post_contrast", "post_contrast"),
    ] {
        let map = server.post(
            "/api/picks/map",
            json!({"scope": scope, "role": "t1w", "layer": layer}),
            OPS,
        );
        assert_eq!(map["layer"]["layer"], layer, "{map}");
        assert_eq!(map["layer"]["operation"], operation, "{map}");
        let step = &map["layer"]["step"];
        let mine = own(operation);
        for key in [
            "step", "state", "of", "answered", "served", "refusal", "label",
        ] {
            assert_eq!(step[key], mine[key], "{layer} {key}: {step} against {mine}");
        }
        assert!(step["of"].as_i64().unwrap() > 0, "{step}");
        // the step alone, as a page asks it while it runs
        let (status, alone) = server.call(
            "GET",
            &format!("/api/picks/layer-step?dataset=fresh&layer={layer}"),
            None,
            OPS,
        );
        assert_eq!(status, 200, "{alone}");
        for key in ["step", "state", "of", "answered", "label"] {
            assert_eq!(alone["step"][key], mine[key], "{layer} {key}: {alone}");
        }
        // a dataset's step is read as its Data page is: review:see alone
        // reads a cohort's, never a dataset's
        let (status, refused) = server.call(
            "GET",
            &format!("/api/picks/layer-step?dataset=fresh&layer={layer}"),
            None,
            REVIEWER,
        );
        assert_eq!(status, 403, "{refused}");
        let strips = server.post(
            "/api/picks/map/strips",
            json!({"scope": scope, "role": "t1w", "layer": layer}),
            OPS,
        );
        assert_eq!(strips["subjects"].as_array().unwrap().len(), 2, "{strips}");
        for s in strips["subjects"].as_array().unwrap() {
            assert_eq!(
                s["layer"]["taken"].as_array().unwrap().len(),
                s["visits"].as_array().unwrap().len(),
                "{strips}"
            );
        }
    }
    // the label the sort wrote: every MR scan of the map's visits has a state
    let map = server.post(
        "/api/picks/map",
        json!({"scope": scope, "role": "t1w", "layer": "post_contrast"}),
        OPS,
    );
    let layer = &map["layer"];
    let labelled: i64 = layer["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["scans"].as_i64().unwrap())
        .sum();
    assert_eq!(labelled, 6, "two scans in each of three visits: {map}");
    assert_eq!(layer["none"]["scans"], 0, "{map}");
    assert_eq!(map["metrics"]["visits"], 3, "{map}");
}

/// How much time a layer adds to the map doors (record 55, decision 6:
/// they "should answer in about a second for a scope of 5,000 visits"), on
/// a synthetic registry of `NILS_LAYER_SUBJECTS` subjects (default 2,000),
/// each door asked nine times in pairs, without a layer and then with
/// one, the medians and the median of the pairs' differences printed. Run
/// by hand, in a release build:
/// `cargo test --release -p nils --test map_layers -- --ignored --nocapture`.
#[test]
#[ignore]
fn a_layer_adds_little_time() {
    let subjects: usize = std::env::var("NILS_LAYER_SUBJECTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    let home = Home::init("map-layers-time-home", None);
    home.ok(&["synth", "--seed", "7", "--subjects", &subjects.to_string()]);
    home.ok(&["session", "rebuild"]);
    home.execute(&[
        "INSERT INTO {classification_axis} (stack_id, axis, value, confidence, tier) \
         SELECT stack_id, 'role', 't1w', 1.0, 'rule' FROM {classification_axis} \
         WHERE axis = 'base' AND value = 'T1w'"
            .to_string(),
    ]);
    let mut store = home.store();
    let sql = format!(
        "SELECT DISTINCT su.code FROM {} su ORDER BY su.code",
        store.qualified("subject"),
    );
    let codes: Vec<String> = store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect();
    drop(store);
    home.ok(&["clinical", "cohort", "make", "all"]);
    for chunk in codes.chunks(200) {
        let mut add = vec!["clinical", "cohort", "add", "all"];
        add.extend(chunk.iter().map(String::as_str));
        add.extend(["--why", "the time of a layer"]);
        home.ok(&add);
    }
    home.ok(&["pick", "run", "--pack-dir", &packs(), "--json"]);
    home.ok(&["contrast", "label", "--pack-dir", &packs(), "--json"]);
    home.execute(&[
        "DELETE FROM {classification_axis} WHERE axis = 'body_region'".to_string(),
        "INSERT INTO {classification_axis} (stack_id, axis, value, confidence, tier) \
         SELECT stack_id, 'body_region', 'head', 1.0, 'rule' FROM {classification_axis} \
         WHERE axis = 'role' AND value = 't1w'"
            .to_string(),
    ]);
    let ingest = TempDir::new("map-layers-time-ingest");
    let server = Served::start(&home, ingest.path(), false);
    let scope = json!({"cohort": "all"});
    // each layer is timed against the door without one in pairs taken one
    // after the other, so that the machine's load, which drifts, falls on
    // both; the median of the pairs' differences is what the layer adds
    let body_of = |path: &str, layer: Option<&str>| {
        let mut body = json!({"scope": scope, "role": "t1w"});
        if path.ends_with("subjects") {
            body["group"] = json!({"by": "breaks"});
        }
        if let Some(l) = layer {
            body["layer"] = json!(l);
        }
        body
    };
    let timed = |path: &str, body: &Value| {
        let at = std::time::Instant::now();
        server.post(path, body.clone(), OPS);
        at.elapsed().as_secs_f64() * 1000.0
    };
    let median = |mut v: Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    let map = server.post(
        "/api/picks/map",
        json!({"scope": scope, "role": "t1w"}),
        OPS,
    );
    eprintln!(
        "{subjects} subjects, {} visits, {} taken",
        map["metrics"]["visits"], map["metrics"]["visits_taken"]
    );
    for path in [
        "/api/picks/map",
        "/api/picks/map/subjects",
        "/api/picks/map/strips",
    ] {
        let plain = body_of(path, None);
        let mut line = String::from(path);
        for layer in ["body_region", "body_part", "post_contrast"] {
            let with = body_of(path, Some(layer));
            // once each to warm
            timed(path, &plain);
            timed(path, &with);
            let (mut a, mut b, mut d) = (Vec::new(), Vec::new(), Vec::new());
            for _ in 0..9 {
                let x = timed(path, &plain);
                let y = timed(path, &with);
                a.push(x);
                b.push(y);
                d.push(y - x);
            }
            line.push_str(&format!(
                "; {layer}: without {:.0} ms, with {:.0} ms, added {:+.0} ms",
                median(a),
                median(b),
                median(d)
            ));
        }
        eprintln!("{line}");
    }
}
