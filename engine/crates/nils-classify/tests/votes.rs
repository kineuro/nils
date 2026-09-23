// SPDX-License-Identifier: AGPL-3.0-only

//! Record 41, S2: a run that hears every rule writes the same verdicts as
//! one that does not, and its vote matrix covers every axis a rule decided.
//! Over the synthetic registry, on both backends.

use std::env;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use nils_classify::classify::classify;
use nils_classify::votes::{self, Filter, HEADER};
use nils_dicom::synth::TempDir;
use nils_digest::Cancel;
use nils_registry::home::{Home, InitOptions};
use nils_registry::{Backend, Registry, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_classify_votes";

fn postgres_dsn() -> Option<String> {
    env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
}

struct Lab {
    name: &'static str,
    registry: Registry,
    _dir: TempDir,
    _guard: Option<MutexGuard<'static, ()>>,
}

impl Drop for Lab {
    fn drop(&mut self) {
        if let Some(dsn) = postgres_dsn().filter(|_| self._guard.is_some()) {
            let mut store = Store::connect_postgres(&dsn, SCHEMA).expect("connect");
            store
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
                ))
                .ok();
        }
    }
}

fn lab(name: &'static str, backend: Backend, dsn: Option<String>) -> Lab {
    let dir = TempDir::new("votes-home");
    let home = Home::new(dir.path());
    home.keys(None)
        .add("k", b"nils-classify-votes-key")
        .unwrap();
    let registry = home
        .init(&InitOptions {
            backend,
            dsn,
            schema: (backend == Backend::Postgres).then(|| SCHEMA.to_string()),
            scheme: Scheme::DEFAULT,
            key: "k".to_string(),
            display_length: 12,
            session_scheme: None,
        })
        .unwrap();
    Lab {
        name,
        registry,
        _dir: dir,
        _guard: None,
    }
}

fn labs() -> Vec<Lab> {
    let mut out = vec![lab("sqlite", Backend::Sqlite, None)];
    if let Some(dsn) = postgres_dsn() {
        let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = Store::connect_postgres(&dsn, SCHEMA).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
            ))
            .expect("drop");
        let mut l = lab("postgres", Backend::Postgres, Some(dsn));
        l._guard = Some(guard);
        out.push(l);
    }
    out
}

fn mri() -> nils_pack::Pack {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
    nils_pack::load(&dir, None).expect("the MRI pack loads")
}

/// Every row of a table the verdict lives in, as text, in a fixed order.
fn snapshot(reg: &mut Registry, table: &str, columns: &str) -> Vec<String> {
    let store = reg.store();
    let sql = format!(
        "SELECT {columns} FROM {} ORDER BY {columns}",
        store.qualified(table)
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| {
            (0..columns.split(',').count())
                .map(|i| match r.get(i) {
                    nils_registry::store::Cell::Null => "-".to_string(),
                    nils_registry::store::Cell::Text(t) => t.clone(),
                    nils_registry::store::Cell::Int(i) => i.to_string(),
                    nils_registry::store::Cell::Double(d) => format!("{d}"),
                    other => format!("{other:?}"),
                })
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect()
}

const AXIS: &str = "stack_id, axis, value, confidence, tier";
const EVIDENCE: &str =
    "stack_id, axis, value, tier, confidence, rule_set, rule, source, matched, pass, author";

fn count(reg: &mut Registry, table: &str, filter: &str) -> i64 {
    let store = reg.store();
    let sql = format!("SELECT COUNT(*) FROM {}{filter}", store.qualified(table));
    store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
}

fn settings(votes: bool) -> nils_classify::Settings {
    nils_classify::Settings {
        votes,
        ..nils_classify::Settings::default()
    }
}

/// The matrix as lines, split into their seven columns, header checked.
fn matrix(reg: &mut Registry, filter: &Filter) -> (Vec<Vec<String>>, votes::Written) {
    let mut out: Vec<u8> = Vec::new();
    let written = votes::write(reg.store(), filter, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some(HEADER));
    let rows: Vec<Vec<String>> = lines
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect();
    for r in &rows {
        assert_eq!(r.len(), 7, "{r:?}");
    }
    (rows, written)
}

#[test]
fn hearing_every_rule_changes_no_verdict_and_the_matrix_covers_every_decided_axis() {
    let pack = mri();
    for mut lab in labs() {
        let name = lab.name;
        let reg = &mut lab.registry;
        let manifest = nils_synth::build(
            reg,
            &nils_synth::Plan {
                seed: 7,
                subjects: 40,
            },
        )
        .unwrap();
        assert!(
            manifest.counts.stacks > 100,
            "{name}: {:?}",
            manifest.counts
        );

        // Without votes, as the engine judged before record 41.
        let plain = classify(reg, &pack, &settings(false), &Cancel::new()).unwrap();
        let axes = snapshot(reg, "classification_axis", AXIS);
        let evidence = snapshot(reg, "classification_evidence", EVIDENCE);
        assert_eq!(count(reg, "classification_vote", ""), 0, "{name}");

        // With them: the same rows, and a vote list per stack and phase.
        let heard = classify(reg, &pack, &settings(true), &Cancel::new()).unwrap();
        assert_eq!(plain.written, heard.written, "{name}");
        assert_eq!(plain.by_tier, heard.by_tier, "{name}");
        assert_eq!(plain.review_items, heard.review_items, "{name}");
        assert_eq!(axes, snapshot(reg, "classification_axis", AXIS), "{name}");
        assert_eq!(
            evidence,
            snapshot(reg, "classification_evidence", EVIDENCE),
            "{name}"
        );
        assert_eq!(
            count(reg, "classification_vote", " WHERE phase = 'class'"),
            heard.written,
            "{name}: one class list per classified stack"
        );
        assert_eq!(
            count(reg, "classification_vote", " WHERE phase = 'disposition'"),
            heard.disposed,
            "{name}: one disposition list per disposed stack"
        );
        // The voters are numbered once per pack version: a second run with
        // votes adds none.
        let voters = count(reg, "classification_voter", "");
        assert_eq!(voters as usize, nils_pack::voters(&pack).len(), "{name}");
        classify(reg, &pack, &settings(true), &Cancel::new()).unwrap();
        assert_eq!(count(reg, "classification_voter", ""), voters, "{name}");

        // Every axis a rule decided has its decider in the matrix: the same
        // stack, axis, rule and value, at the tier the evidence cites.
        let (rows, written) = matrix(reg, &Filter::default());
        assert_eq!(written.votes as usize, rows.len(), "{name}");
        assert_eq!(written.stacks, heard.written, "{name}");
        let heard_set: std::collections::HashSet<(String, String, String, String, String, String)> =
            rows.iter()
                .map(|r| {
                    (
                        r[0].clone(),
                        r[1].clone(),
                        r[2].clone(),
                        r[3].clone(),
                        r[5].clone(),
                        r[6].clone(),
                    )
                })
                .collect();
        let store = reg.store();
        let sql = format!(
            "SELECT stack_id, axis, rule_set, rule, value, tier FROM {} \
             WHERE pass IS NULL AND author IS NULL AND tier <> 'default'",
            store.qualified("classification_evidence")
        );
        let decided = store.query(&sql, &[]).unwrap();
        assert!(decided.len() > 100, "{name}: {} decided", decided.len());
        let mut axes_decided = std::collections::BTreeSet::new();
        for r in &decided {
            let key = (
                r.int(0).unwrap().to_string(),
                r.text(1).unwrap().to_string(),
                r.text(2).unwrap().to_string(),
                r.text(3).unwrap().to_string(),
                r.text(4).unwrap().to_string(),
                r.text(5).unwrap().to_string(),
            );
            axes_decided.insert(key.1.clone());
            assert!(heard_set.contains(&key), "{name}: {key:?} has no vote");
        }
        for a in &axes_decided {
            assert!(written.axes.contains(a), "{name}: {a} is not in the matrix");
        }
        // Nothing but identities and the pack's vocabulary: every rule and
        // value a line names is the pack's own.
        for r in &rows {
            let set = pack
                .rule_sets
                .iter()
                .find(|s| s.name == r[2])
                .unwrap_or_else(|| panic!("{name}: {r:?} names no rule set"));
            assert!(set.rules.iter().any(|x| x.id == r[3]), "{name}: {r:?}");
            let axis = &pack.axes[pack.axis_index(&r[1]).expect("an axis")];
            assert!(
                r[5].is_empty() || axis.id_of_stored(&r[5]).is_some(),
                "{name}: {r:?} is no value of {}",
                axis.name
            );
        }
        // One axis alone.
        let (only, _) = matrix(
            reg,
            &Filter {
                axis: Some("base".into()),
            },
        );
        assert!(
            !only.is_empty() && only.iter().all(|r| r[1] == "base"),
            "{name}"
        );

        // A run without votes leaves none behind to outlive its verdict.
        classify(reg, &pack, &settings(false), &Cancel::new()).unwrap();
        assert_eq!(count(reg, "classification_vote", ""), 0, "{name}");
    }
}

/// What hearing every rule costs, on the synthetic registry: a run without
/// votes, then one with, three times each, on SQLite. Not a gate; run it by
/// name with `--ignored --nocapture` and read the numbers.
#[test]
#[ignore]
fn what_the_votes_cost() {
    let pack = mri();
    let subjects: usize = env::var("NILS_VOTES_SUBJECTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(400);
    let mut lab = lab("sqlite", Backend::Sqlite, None);
    let reg = &mut lab.registry;
    let manifest = nils_synth::build(reg, &nils_synth::Plan { seed: 3, subjects }).unwrap();
    let mut times = [Vec::new(), Vec::new()];
    for _ in 0..3 {
        for (i, votes) in [false, true].into_iter().enumerate() {
            let started = Instant::now();
            classify(reg, &pack, &settings(votes), &Cancel::new()).unwrap();
            times[i].push(started.elapsed().as_secs_f64());
        }
    }
    let best = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
    let (_, written) = matrix(reg, &Filter::default());
    eprintln!(
        "{} stacks: without votes {:.3} s, with {:.3} s (best of 3), {} votes, {:.1} per stack",
        manifest.counts.stacks,
        best(&times[0]),
        best(&times[1]),
        written.votes,
        written.votes as f64 / written.stacks.max(1) as f64
    );
}
