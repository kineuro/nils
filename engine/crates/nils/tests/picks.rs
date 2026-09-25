// SPDX-License-Identifier: AGPL-3.0-only
//! Record 42 S3, a person's pick: it survives `nils pick run`, the ask sees
//! it, withdrawing it lets the run's pick apply again, and a session whose
//! pick is ambiguous is a `pick.border` item in `nils review list`. On
//! SQLite always, and on Postgres where a test DSN is set.

use std::io::Write as _;
use std::process::{Command, Stdio};

use nils_dicom::synth::TempDir;
use nils_registry::Store;

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

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let out = self.ok(args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }

    fn store(&self) -> Store {
        match &self.pg {
            Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
            None => Store::open_sqlite(&self.dir.path().join("registry.db")).unwrap(),
        }
    }
}

/// A synthetic registry with its sessions built, and `role t1w` on every
/// T1w stack, which is what a classify under the pack would say and the
/// generator does not write.
fn registry(pg: Option<(String, String)>) -> Home {
    let home = Home {
        dir: TempDir::new("picks-home"),
        pg,
    };
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a picks test key\n"));
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
    home
}

/// The stacks an ask over the picked T1w answers.
fn picked(home: &Home) -> Vec<i64> {
    let doc = home.dir.path().join("picked.ask.yml");
    std::fs::write(
        &doc,
        "ast_version: 1\nname: the picked t1w\nscheme: default\n\
         sets:\n  t1: {grain: stack, from: \"role:t1w\"}\n\
         keep: [t1]\nout: {set: t1, level: record}\n",
    )
    .unwrap();
    let out = home.ok(&[
        "ask",
        "run",
        "--file",
        doc.to_str().unwrap(),
        "--pack-dir",
        &packs(),
    ]);
    let fields: Vec<&str> = out.split_whitespace().collect();
    let at = fields.iter().position(|f| *f == "handle").unwrap();
    let rows = home.ok(&["ask", "handles", "export", "--handle", fields[at + 1]]);
    rows.lines()
        .skip(1)
        .map(|l| l.split(',').next().unwrap().parse().unwrap())
        .collect()
}

fn round(home: &Home) {
    let p = packs();
    let first = home.json(&["pick", "run", "--pack-dir", &p, "--json"]);
    let raised = first["raised"].as_i64().unwrap();
    assert!(
        raised > 0,
        "the synthetic registry has an ambiguous session: {first}"
    );
    assert_eq!(first["standing"], 0, "{first}");

    // Each ambiguous session is an open pick.border item in the review queue.
    let items = home.json(&["review", "list", "--kind", "pick.border", "--json"]);
    let open: Vec<&serde_json::Value> = items["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["status"] == "open")
        .collect();
    assert_eq!(open.len() as i64, raised, "{items}");
    let item = open[0];
    assert_eq!(item["scope"], "subject", "{item}");
    assert_eq!(item["ref"]["role"], "t1w", "{item}");
    let text = home.ok(&["review", "list"]);
    assert!(text.contains("pick.border"), "{text}");

    // The run's winner, and the candidate a person prefers.
    let considered = item["evidence"]["considered"].as_array().unwrap();
    assert!(considered.len() >= 2, "{item}");
    let run_pick = item["evidence"]["pick_id"].as_i64().unwrap();
    let mut store = home.store();
    let winner = store
        .query(
            &format!(
                "SELECT stack_id FROM {} WHERE pick_id = {run_pick}",
                store.qualified("pick_stack")
            ),
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    let other = considered
        .iter()
        .map(|c| c["stacks"][0].as_i64().unwrap())
        .find(|s| *s != winner)
        .unwrap();
    let before = picked(home);
    assert!(before.contains(&winner), "the run's pick applies: {winner}");
    assert!(!before.contains(&other));

    // A person picks the other one.
    let set = home.json(&[
        "pick",
        "set",
        "--pack-dir",
        &p,
        "--role",
        "t1w",
        "--stack",
        &other.to_string(),
        "--why",
        "the sharper of the two",
        "--json",
    ]);
    let person = set["id"].as_i64().unwrap();
    assert_eq!(set["overruled"].as_array().unwrap().len(), 1, "{set}");
    assert_eq!(set["answered"], 1, "{set}");
    assert_eq!(set["session_day"], item["ref"]["session_day"], "{set}");

    // It survives a run, which asks nothing more about that session.
    let again = home.json(&["pick", "run", "--pack-dir", &p, "--json"]);
    assert_eq!(again["standing"], 1, "{again}");
    assert_eq!(again["raised"].as_i64().unwrap(), raised - 1, "{again}");
    let listed = home.json(&["pick", "list", "--json"]);
    let row = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == person)
        .unwrap_or_else(|| panic!("the person's pick stands: {listed}"));
    assert_eq!(row["author_kind"], "person");

    // The ask sees the person's pick, and not the run's.
    let after = picked(home);
    assert!(after.contains(&other), "the person's pick applies");
    assert!(!after.contains(&winner), "the run's pick does not");
    assert_eq!(after.len(), before.len());

    // A run's pick is not withdrawn; a person's is, and the run's applies again.
    let agent = store
        .query(
            &format!(
                "SELECT id FROM {} WHERE overruled_by = {person}",
                store.qualified("pick")
            ),
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    let (good, _, err) = home.run(&["pick", "withdraw", &agent.to_string()], None);
    assert!(!good);
    assert!(
        err.contains("overrules it with a pick of their own"),
        "{err}"
    );
    let done = home.json(&["pick", "withdraw", &person.to_string(), "--json"]);
    assert_eq!(done["restored"], serde_json::json!([agent]), "{done}");
    let restored = picked(home);
    assert!(restored.contains(&winner));
    assert!(!restored.contains(&other));
    let (good, _, err) = home.run(&["pick", "withdraw", &person.to_string()], None);
    assert!(!good);
    assert!(err.contains("already withdrawn"), "{err}");

    // With the person's pick gone, the next run asks about the session again.
    let third = home.json(&["pick", "run", "--pack-dir", &p, "--json"]);
    assert_eq!(third["standing"], 0, "{third}");
    assert_eq!(third["raised"].as_i64().unwrap(), raised, "{third}");

    // Both acts are in the audit log.
    let audit = store
        .query(
            &format!(
                "SELECT action FROM {} WHERE action LIKE 'pick.%' ORDER BY id",
                store.qualified("audit")
            ),
            &[],
        )
        .unwrap();
    let actions: Vec<&str> = audit.iter().map(|r| r.text(0).unwrap()).collect();
    assert_eq!(actions, ["pick.set", "pick.withdraw"]);
}

#[test]
fn a_person_s_pick_survives_a_run_and_the_ask_sees_it() {
    round(&registry(None));
}

#[test]
fn a_person_s_pick_is_refused_what_it_cannot_name() {
    let home = registry(None);
    let p = packs();
    let (good, _, err) = home.run(
        &[
            "pick",
            "set",
            "--pack-dir",
            &p,
            "--role",
            "dwi",
            "--stack",
            "1",
            "--why",
            "x",
        ],
        None,
    );
    assert!(!good);
    assert!(err.contains("declares no pick for the role dwi"), "{err}");
    let (good, _, err) = home.run(
        &[
            "pick",
            "set",
            "--pack-dir",
            &p,
            "--role",
            "t1w",
            "--stack",
            "99999",
            "--why",
            "x",
        ],
        None,
    );
    assert!(!good);
    assert!(err.contains("no fingerprinted stack 99999"), "{err}");
    let (good, _, err) = home.run(
        &[
            "pick",
            "set",
            "--pack-dir",
            &p,
            "--role",
            "t1w",
            "--stack",
            "1",
            "--why",
            " ",
        ],
        None,
    );
    assert!(!good);
    assert!(err.contains("says why"), "{err}");
}

#[test]
fn a_person_s_pick_survives_a_run_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        return;
    };
    let schema = "nils_picks_round";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    round(&registry(Some((dsn.clone(), schema.to_string()))));
    drop();
}

/// Record 42 S6 on S3: a pick campaign closes through the person's pick
/// writer. A rater answers an occasion's stacks, the close writes a
/// person's pick naming the campaign, the run's pick on the occasion stops
/// applying and points at it, and the next `nils pick run` leaves it
/// standing. A pick of stacks on another occasion than the item's is
/// refused and the item stays unresolved.
#[test]
fn a_pick_campaign_closes_through_the_person_s_pick_writer() {
    let home = registry(None);
    let p = packs();
    home.ok(&["pick", "run", "--pack-dir", &p, "--json"]);
    let doc = home.dir.path().join("visits.json");
    std::fs::write(
        &doc,
        serde_json::json!({
            "ast_version": 1,
            "sets": {"visits": {"grain": "session"}},
            "out": {"set": "visits", "level": "record"},
        })
        .to_string(),
    )
    .unwrap();
    home.ok(&[
        "ask",
        "selections",
        "save",
        "--name",
        "visits",
        "--file",
        doc.to_str().unwrap(),
        "--pack-dir",
        &p,
    ]);
    let made = home.json(&[
        "campaign",
        "create",
        "visits",
        "--pick-role",
        "t1w",
        "--select",
        "selection:visits@1",
        "--closes-into",
        "pick",
        "--pack-dir",
        &p,
        "--json",
    ]);
    let campaign = made["id"].as_i64().unwrap();
    let items = made["items"].as_array().unwrap();
    assert!(items.len() >= 2, "{made}");

    // The run's standing pick on an occasion, by its subject and day.
    let mut store = home.store();
    let run_pick = |store: &mut Store, subject: i64, day: &str| -> Option<(i64, Vec<i64>)> {
        let rows = store
            .query(
                &format!(
                    "SELECT id FROM {} WHERE role = 't1w' AND subject_id = {subject} \
                     AND session_day = '{day}' AND withdrawn_at IS NULL",
                    store.qualified("pick")
                ),
                &[],
            )
            .unwrap();
        let id = rows.first()?.int(0).unwrap();
        let stacks = store
            .query(
                &format!(
                    "SELECT stack_id FROM {} WHERE pick_id = {id} ORDER BY stack_id",
                    store.qualified("pick_stack")
                ),
                &[],
            )
            .unwrap()
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect();
        Some((id, stacks))
    };

    // The first item: the rater picks the stacks the run picked, which is a
    // person's judgement all the same. The second: stacks of the first
    // item's occasion, which are not the second's.
    let mut answered: Vec<(i64, i64, String, i64, Vec<i64>)> = Vec::new();
    for _ in 0..2 {
        let claimed = home.json(&["campaign", "claim", "visits", "--json"]);
        let item = &claimed["item"];
        let subject = item["subject_id"].as_i64().unwrap();
        let day = item["session_day"].as_str().unwrap()[..10].to_string();
        let a = claimed["assignment"]["id"].as_i64().unwrap();
        let stacks = match answered.first() {
            None => {
                let (id, stacks) = run_pick(&mut store, subject, &day)
                    .unwrap_or_else(|| panic!("a run's pick on {subject} {day}"));
                answered.push((a, subject, day.clone(), id, stacks.clone()));
                stacks
            }
            Some((_, first_subject, first_day, _, stacks)) => {
                assert!(
                    (*first_subject, first_day.as_str()) != (subject, day.as_str()),
                    "two items of one occasion"
                );
                stacks.clone()
            }
        };
        let value = stacks
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        home.ok(&["campaign", "answer", &a.to_string(), "--value", &value]);
    }
    let closed = home.json(&["campaign", "close", "visits", "--pack-dir", &p, "--json"]);
    let picks = closed["picks"].as_array().unwrap();
    assert_eq!(picks.len(), 1, "{closed}");
    let person = picks[0].as_i64().unwrap();
    let refused = closed["refused"].as_array().unwrap();
    assert_eq!(refused.len(), 1, "{closed}");
    assert!(
        refused[0]["why"]
            .as_str()
            .unwrap()
            .contains("the pick was asked of subject"),
        "{closed}"
    );

    // The pick is a person's, of the closer, naming its campaign, and the
    // run's pick it overruled points at it.
    let (_, subject, day, run, stacks) = &answered[0];
    let row = store
        .query(
            &format!(
                "SELECT author_kind, actor, campaign_id, why, subject_id FROM {} WHERE id = {person}",
                store.qualified("pick")
            ),
            &[],
        )
        .unwrap();
    assert_eq!(row[0].text(0).unwrap(), "person");
    assert_eq!(row[0].text(1).unwrap(), "anna@ward-3");
    assert_eq!(row[0].opt_int(2).unwrap(), Some(campaign));
    assert!(row[0].text(3).unwrap().contains("campaign visits"));
    assert_eq!(row[0].int(4).unwrap(), *subject);
    let over = store
        .query(
            &format!(
                "SELECT overruled_by, withdrawn_at IS NOT NULL FROM {} WHERE id = {run}",
                store.qualified("pick")
            ),
            &[],
        )
        .unwrap();
    assert_eq!(over[0].opt_int(0).unwrap(), Some(person));
    assert_eq!(over[0].int(1).unwrap(), 1);
    assert_eq!(run_pick(&mut store, *subject, day).unwrap().1, *stacks);

    // A run leaves it standing.
    let again = home.json(&["pick", "run", "--pack-dir", &p, "--json"]);
    assert_eq!(again["standing"], 1, "{again}");
    let still = run_pick(&mut store, *subject, day).unwrap();
    assert_eq!(still.0, person, "the person's pick still applies");
}

/// A server on a home, its first line read for the port.
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

const CURATOR: &str = "a-curator-token-of-its-length";
const PLAIN: &str = "a-plain-campaigns-token-long";
const RATER: &str = "a-rater-of-campaigns-token";

impl Served {
    fn start(home: &Home) -> Served {
        use std::io::BufRead as _;
        let tokens = [
            format!("{CURATOR}=cleo@lab:reviewer,campaigns:work"),
            format!("{PLAIN}=pat@lab:campaigns:see"),
            format!("{RATER}=rae@lab:campaigns:work"),
        ]
        .join(",");
        let mut child = nils()
            .arg("--registry")
            .arg(home.dir.path())
            .args(["serve", "--bind", "127.0.0.1:0", "--auth", "token"])
            .args(["--pack-dir", &packs()])
            .env("NILS_TOKENS", tokens)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_DSN")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut lines = std::io::BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            let _ = child.kill();
            panic!("nils serve did not listen");
        };
        let addr = first.split_whitespace().nth(2).unwrap();
        let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
        Served { child, port }
    }

    fn get(&self, path: &str, token: &str) -> (u16, serde_json::Value) {
        use std::io::Read as _;
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let head = format!(
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {token}\r\n\r\n"
        );
        stream.write_all(head.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (headers, text) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status: u16 = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        (
            status,
            serde_json::from_str(text).unwrap_or(serde_json::Value::String(text.to_string())),
        )
    }
}

/// Record 45: a pick campaign's session item names its candidates at the
/// door, the session's stacks with what the classifier says of each and the
/// run's pick among them, at detail quasi, since a session is named by its
/// subject and its day. On SQLite, and on Postgres where a test DSN is set.
fn candidates_round(home: &Home) {
    let p = packs();
    home.ok(&["pick", "run", "--pack-dir", &p, "--json"]);
    let doc = home.dir.path().join("visits.json");
    std::fs::write(
        &doc,
        serde_json::json!({
            "ast_version": 1,
            "sets": {"visits": {"grain": "session"}},
            "out": {"set": "visits", "level": "record"},
        })
        .to_string(),
    )
    .unwrap();
    home.ok(&[
        "ask",
        "selections",
        "save",
        "--name",
        "visits",
        "--file",
        doc.to_str().unwrap(),
        "--pack-dir",
        &p,
    ]);
    let made = home.json(&[
        "campaign",
        "create",
        "visits",
        "--pick-role",
        "t1w",
        "--select",
        "selection:visits@1",
        "--closes-into",
        "pick",
        "--rater",
        "rae@lab",
        "--rater",
        "pat@lab",
        "--pack-dir",
        &p,
        "--json",
    ]);
    let campaign = made["id"].as_i64().unwrap();
    let items = made["items"].as_array().unwrap();
    let server = Served::start(home);
    let mut with_a_pick = 0;
    for it in items {
        let item = it["id"].as_i64().unwrap();
        let path = format!("/api/campaigns/{campaign}/items/{item}/candidates");
        let (status, doc) = server.get(&path, PLAIN);
        assert_eq!(status, 403, "a session opens at quasi: {doc}");
        let (status, doc) = server.get(&path, CURATOR);
        assert_eq!(status, 200, "{doc}");
        assert_eq!(doc["role"], "t1w", "{doc}");
        let candidates = doc["candidates"].as_array().unwrap();
        assert!(!candidates.is_empty(), "every session holds a stack: {doc}");
        for c in candidates {
            assert!(c["stack_id"].is_i64(), "{doc}");
            assert!(c["axes"].is_object(), "{doc}");
        }
        // the run's pick names stacks among the candidates
        for pick in doc["picks"].as_array().unwrap() {
            with_a_pick += 1;
            for s in pick["stacks"].as_array().unwrap() {
                let c = candidates
                    .iter()
                    .find(|c| c["stack_id"] == *s)
                    .unwrap_or_else(|| panic!("a picked stack is a candidate: {doc}"));
                assert!(
                    c["picked_by"].as_array().unwrap().contains(&pick["id"]),
                    "{doc}"
                );
            }
        }
    }
    assert!(with_a_pick > 0, "the run picked on some occasion");
    // record 48: a rater without query:see reaches the pictures of a
    // session's stacks through the open campaign that names them and asks
    // the session (here
    // as far as the working place, which this registry has none of), and of
    // no other stack
    let item = items[0]["id"].as_i64().unwrap();
    let (_, doc) = server.get(
        &format!("/api/campaigns/{campaign}/items/{item}/candidates"),
        CURATOR,
    );
    let stack = doc["candidates"][0]["stack_id"].as_i64().unwrap();
    let (status, doc) = server.get(&format!("/api/instances/{stack}/manifest"), RATER);
    assert_eq!(status, 409, "{doc}");
    let (status, doc) = server.get("/api/instances/999999/manifest", RATER);
    assert_eq!(status, 403, "{doc}");
    let (status, doc) = server.get(&format!("/api/instances/{stack}/manifest"), PLAIN);
    assert_eq!(status, 403, "{doc}");
}

#[test]
fn a_pick_campaign_s_item_names_its_candidates_at_the_door() {
    candidates_round(&registry(None));
}

#[test]
fn a_pick_campaign_s_item_names_its_candidates_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        return;
    };
    let schema = "nils_picks_candidates";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    candidates_round(&registry(Some((dsn.clone(), schema.to_string()))));
    drop();
}
