// SPDX-License-Identifier: AGPL-3.0-only
//! Record 51 R3: no door decides or accepts an item a campaign holds. The
//! keyboard's `nils review apply` and `decide` (one value, several values of
//! a multi-valued answer, nothing, staged or not) and `nils review accept`
//! refuse a review item an open campaign asks, in the door's words, and
//! write nothing; a commit by filter or of everything leaves staged a
//! decision on an axis the campaign asks, until it closes. On SQLite
//! always, and on Postgres where a test DSN is set.

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

    fn count(&self, sql: &str) -> i64 {
        let mut store = self.store();
        let sql = sql
            .replace("{decision}", &store.qualified("decision"))
            .replace("{audit}", &store.qualified("audit"))
            .replace("{review_item}", &store.qualified("review_item"));
        store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
    }
}

/// A synthetic registry, classified, whose low confidences are review
/// items: grouped questions about each axis.
fn registry(pg: Option<(String, String)>) -> Home {
    let home = Home {
        dir: TempDir::new("held-home"),
        pg,
    };
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a held test key\n"));
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
    home.ok(&["classify", "--pack-dir", &packs(), "--review-below", "0.9"]);
    home
}

fn round(home: &Home) {
    let p = packs();
    let items = home.json(&[
        "review",
        "list",
        "--kind",
        "base:low_confidence",
        "--status",
        "open",
        "--json",
    ]);
    let item = &items["items"][0];
    assert_eq!(item["scope"], "group", "{items}");
    let id = item["id"].as_i64().unwrap();
    let ids = id.to_string();
    let member = {
        let mut store = home.store();
        let sql = format!(
            "SELECT stack_id FROM {} WHERE item_id = {id} ORDER BY stack_id",
            store.qualified("review_member")
        );
        store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
    };

    // a member decision staged before any campaign asked the item
    home.ok(&[
        "review",
        "apply",
        &ids,
        "--member",
        &member.to_string(),
        "--value",
        "T1w",
        "--stage",
    ]);
    let made = home.json(&[
        "campaign",
        "create",
        "held",
        "--axis",
        "base",
        "--review-kind",
        "base:low_confidence",
        "--pack-dir",
        &p,
        "--json",
    ]);
    let campaign = made["id"].as_i64().unwrap();
    let words = format!("is asked by campaign held ({campaign})");

    let decisions = || home.count("SELECT COUNT(*) FROM {decision}");
    let staged = || {
        home.count(
            "SELECT COUNT(*) FROM {decision} WHERE staged_at IS NOT NULL AND committed_at IS NULL AND withdrawn_at IS NULL",
        )
    };
    let in_force = || {
        home.count(
            "SELECT COUNT(*) FROM {decision} WHERE withdrawn_at IS NULL AND (staged_at IS NULL OR committed_at IS NOT NULL)",
        )
    };
    let audits = || home.count("SELECT COUNT(*) FROM {audit}");
    let before = (decisions(), staged(), audits());
    assert_eq!(before.1, 1);

    // every form at the keyboard is refused in the door's words
    let m = member.to_string();
    let forms: Vec<Vec<&str>> = vec![
        vec!["review", "apply", &ids, "--value", "T2w"],
        vec!["review", "apply", &ids, "--value", "T2w", "--stage"],
        vec!["review", "apply", &ids, "--nothing"],
        vec!["review", "apply", &ids, "--nothing", "--stage"],
        vec!["review", "apply", &ids, "--value", "T1w,T2w"],
        vec!["review", "apply", &ids, "--value", "T1w,T2w", "--stage"],
        vec!["review", "apply", &ids, "--member", &m, "--value", "T2w"],
        vec!["review", "decide", &ids, "--value", "T2w"],
        vec!["review", "decide", &ids, "--value", "T2w", "--stage"],
        vec!["review", "accept", &ids],
        vec!["review", "accept", &ids, "--why", "looked fine"],
    ];
    for form in &forms {
        let (good, out, err) = home.run(form, None);
        assert!(!good, "{form:?} was not refused: {out}");
        assert!(err.contains(&words), "{form:?}: {err}");
        assert!(err.contains("answer it there"), "{form:?}: {err}");
        assert_eq!(
            (decisions(), staged(), audits()),
            before,
            "{form:?} wrote something"
        );
    }
    let shown = home.json(&["review", "show", &ids, "--json"]);
    assert_eq!(shown["status"], "open", "{shown}");
    assert!(shown["accepted_by"].is_null(), "{shown}");

    // a commit by filter, or of everything, leaves the staged member
    // decision staged while the campaign asks its axis
    let force = in_force();
    let (good, out, err) = home.run(&["review", "commit", "--axis", "base"], None);
    assert!(good, "{err}");
    assert!(out.contains("committed 0 decision(s)"), "{out}");
    assert!(err.contains("open campaign asks"), "{err}");
    let (good, _, err) = home.run(&["review", "commit", "--all"], None);
    assert!(!good);
    assert!(err.contains("wait for their campaign"), "{err}");
    let named = home.count(
        "SELECT MAX(id) FROM {decision} WHERE staged_at IS NOT NULL AND committed_at IS NULL AND withdrawn_at IS NULL",
    );
    let (good, _, err) = home.run(&["review", "commit", &named.to_string()], None);
    assert!(!good);
    assert!(err.contains("campaign held asks"), "{err}");
    assert_eq!(staged(), 1);
    assert_eq!(in_force(), force, "nothing was put in force");

    // once the campaign closes, the keyboard decides the item again
    home.ok(&["campaign", "close", "held", "--pack-dir", &p]);
    let still = home.json(&["review", "show", &ids, "--json"]);
    assert_eq!(still["status"], "open", "{still}");
    home.ok(&["review", "apply", &ids, "--value", "T2w"]);
    let shown = home.json(&["review", "show", &ids, "--json"]);
    assert_eq!(shown["status"], "accepted", "{shown}");
    assert_eq!(decisions(), before.0 + 1);
}

#[test]
fn the_keyboard_does_not_decide_an_item_a_campaign_holds() {
    round(&registry(None));
}

#[test]
fn the_keyboard_does_not_decide_an_item_a_campaign_holds_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        return;
    };
    let schema = "nils_held_keyboard";
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
