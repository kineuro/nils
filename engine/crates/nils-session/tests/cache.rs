// SPDX-License-Identifier: AGPL-3.0-only

//! The session cache (`docs/specs/wave4b-the-ask.md`, §7 and §15, slice 2),
//! on the synthetic registry, on both backends.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::sync::{Barrier, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use nils_dicom::synth::TempDir;
use nils_registry::day::Day;
use nils_registry::home::{Home, InitOptions};
use nils_registry::schema::{Type, table};
use nils_registry::session::{self, Anchor, Naming, Scheme};
use nils_registry::{Backend, Insert, Param, Registry, Row, Store, Subkeys, lock, merge};
use nils_session::{
    Anchors, MOVED_KIND, Rebuilt, ensure, ensure_for, labels_by_study, sessions_of,
};

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_session_test";

struct Lab {
    name: &'static str,
    registry: Registry,
    _dir: TempDir,
    _guard: Option<MutexGuard<'static, ()>>,
}

impl Drop for Lab {
    fn drop(&mut self) {
        if self._guard.is_some() {
            self.registry
                .store()
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
                ))
                .ok();
        }
    }
}

fn lab(name: &'static str, backend: Backend, dsn: Option<String>) -> Lab {
    let dir = TempDir::new("session-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-session-test-key").unwrap();
    let mut registry = home
        .init(&InitOptions {
            backend,
            dsn,
            schema: (backend == Backend::Postgres).then(|| SCHEMA.to_string()),
            scheme: nils_registry::Scheme::Blake2b32,
            key: "k".to_string(),
            display_length: 12,
            session_scheme: None,
        })
        .unwrap();
    nils_synth::build(
        &mut registry,
        &nils_synth::Plan {
            seed: 3,
            subjects: 40,
        },
    )
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
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => {
            let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
            let mut l = lab("postgres", Backend::Postgres, Some(dsn));
            l._guard = Some(guard);
            out.push(l);
        }
        _ => eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped"),
    }
    out
}

/// The Postgres lab alone, for what only a backend whose writers run side
/// by side can show; none, and said, where no Postgres is given.
fn postgres_lab() -> Option<Lab> {
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => {
            let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
            let mut l = lab("postgres", Backend::Postgres, Some(dsn));
            l._guard = Some(guard);
            Some(l)
        }
        _ => {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres test is skipped");
            None
        }
    }
}

/// What the resolver says on its own, per subject, for the check that the
/// cache says the same.
fn direct(
    registry: &mut Registry,
    scheme: &Scheme,
) -> BTreeMap<String, Vec<(Day, Option<String>)>> {
    timelines(registry)
        .into_iter()
        .map(|(code, studies)| {
            let anchor = studies.iter().map(|s| s.day).min();
            let sessions = session::sessions(&studies, anchor, scheme)
                .into_iter()
                .map(|s| (s.first, s.label))
                .collect();
            (code, sessions)
        })
        .collect()
}

/// Every subject's dated studies, by code.
fn timelines(registry: &mut Registry) -> BTreeMap<String, Vec<session::Study>> {
    let store = registry.store();
    let d = store.dialect();
    let st = table("study");
    let filled = d.text_of_qualified(Some("st"), st.column("date_filled").unwrap());
    let dated = d.text_of_qualified(Some("st"), st.column("study_date").unwrap());
    let sql = format!(
        "SELECT su.code, st.id, COALESCE({filled}, {dated}) FROM {} st JOIN {} su ON su.id = st.subject_id \
         WHERE COALESCE({filled}, {dated}) IS NOT NULL ORDER BY su.code, 3, st.id",
        store.qualified("study"),
        store.qualified("subject")
    );
    let mut by: BTreeMap<String, Vec<session::Study>> = BTreeMap::new();
    for r in store.query(&sql, &[]).unwrap() {
        by.entry(r.text(0).unwrap().to_string())
            .or_default()
            .push(session::Study::new(
                r.int(1).unwrap(),
                Day::parse(r.text(2).unwrap()).unwrap(),
            ));
    }
    by
}

#[test]
fn the_cache_says_what_the_resolver_says_and_rebuilds_only_what_moved() {
    for mut l in labs() {
        let scheme = Scheme {
            window_days: 14,
            naming: Naming::Months {
                cadence: vec![0, 12, 24, 36, 48, 60],
                tolerance: 3.0,
            },
            ..Scheme::default()
        };
        let anchors = Anchors::resolve(&mut l.registry, &scheme, BTreeMap::new()).unwrap();
        let first = ensure(&mut l.registry, &scheme, &anchors, None, false).unwrap();
        assert_eq!(first.unchanged, 0, "{}", l.name);
        assert_eq!(first.rebuilt, first.subjects, "{}", l.name);
        assert!(first.sessions > 0, "{}", l.name);
        assert_eq!(first.relabelled, first.sessions, "{}", l.name);

        // the cache and the resolver agree, subject by subject
        let expected = direct(&mut l.registry, &scheme);
        let cached = sessions_of(l.registry.store(), &scheme, None).unwrap();
        let mut got: BTreeMap<String, Vec<(Day, Option<String>)>> = BTreeMap::new();
        for c in &cached {
            got.entry(c.code.clone())
                .or_default()
                .push((c.first, c.label.clone()));
        }
        assert_eq!(got, expected, "{}", l.name);
        assert!(cached.iter().all(|c| !c.studies.is_empty()), "{}", l.name);
        assert!(cached.iter().all(|c| c.id > 0), "{}", l.name);

        // every dated study has a session, and its label is the cached one
        let by_study = labels_by_study(l.registry.store(), &scheme).unwrap();
        let dated: i64 = {
            let store = l.registry.store();
            let sql = format!(
                "SELECT COUNT(*) FROM {} WHERE COALESCE(date_filled, study_date) IS NOT NULL",
                store.qualified("study")
            );
            store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
        };
        assert_eq!(by_study.len() as i64, dated, "{}", l.name);
        for c in &cached {
            for st in &c.studies {
                let l2 = &by_study[st];
                assert_eq!(
                    (l2.session_id, l2.first, l2.name()),
                    (c.id, c.first, c.name())
                );
            }
        }

        // nothing changed: nothing is rebuilt
        let again = ensure(&mut l.registry, &scheme, &anchors, None, false).unwrap();
        assert_eq!(again.rebuilt, 0, "{}", l.name);
        assert_eq!(again.unchanged, again.subjects, "{}", l.name);
        assert_eq!(again.moved + again.vanished + again.items, 0, "{}", l.name);

        // a second scheme with the same window shares identity and adds labels
        let by_date = Scheme {
            window_days: 14,
            ..Scheme::default()
        };
        let third = ensure(&mut l.registry, &by_date, &Anchors::default(), None, false).unwrap();
        assert_eq!(third.rebuilt, 0, "{}", l.name);
        assert_eq!(third.relabelled, third.sessions, "{}", l.name);
        let dated_labels = sessions_of(l.registry.store(), &by_date, None).unwrap();
        assert_eq!(dated_labels.len(), cached.len(), "{}", l.name);
        assert!(dated_labels.iter().zip(&cached).all(|(a, b)| a.id == b.id));
        assert!(dated_labels.iter().all(|c| c.label.is_some()));
        assert!(
            dated_labels
                .iter()
                .zip(&cached)
                .any(|(a, b)| a.label != b.label),
            "{}",
            l.name
        );
    }
}

#[test]
fn a_moved_span_keeps_its_id_rekeys_its_pick_and_raises_an_item() {
    for mut l in labs() {
        let scheme = Scheme {
            window_days: 14,
            ..Scheme::default()
        };
        let anchors = Anchors::default();
        ensure(&mut l.registry, &scheme, &anchors, None, false).unwrap();
        let before = sessions_of(l.registry.store(), &scheme, None).unwrap();
        // a session with two studies: move its first study three days
        // earlier, so the session opens on a new day and keeps its studies
        let target = before
            .iter()
            .find(|c| c.studies.len() >= 2)
            .expect("a two-study session")
            .clone();
        let first_study = *target.studies.first().unwrap();
        let day = target.first;
        let earlier = Day::from_days(day.to_days() - 3);
        let iso = |d: Day| format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day());
        // a pick keyed on the old day, under this scheme
        l.registry
            .store()
            .insert(
                &Insert::new(
                    table("pick"),
                    &[
                        "model",
                        "role",
                        "subject_id",
                        "session_day",
                        "scheme",
                        "scheme_digest",
                        "reference",
                        "pack",
                        "pack_version",
                        "actor",
                        "author_kind",
                        "decided_at",
                    ],
                ),
                &[vec![
                    Param::from("m"),
                    Param::from("t1w"),
                    Param::Int(target.subject_id),
                    Param::from(iso(day)),
                    Param::from("window=14d,date"),
                    Param::from(scheme.digest()),
                    Param::from("registry"),
                    Param::from("mri"),
                    Param::from("synthetic"),
                    Param::from("test"),
                    Param::from("agent"),
                    Param::from("2026-09-07T00:00:00Z"),
                ]],
            )
            .unwrap();
        {
            let store = l.registry.store();
            let d = store.dialect();
            store
                .execute(
                    &format!(
                        "UPDATE {} SET date_filled = {}, study_date = {} WHERE id = {}",
                        store.qualified("study"),
                        d.param(1, Type::Date),
                        d.param(2, Type::Date),
                        d.param(3, Type::Int)
                    ),
                    &[
                        Param::from(iso(earlier)),
                        Param::from(iso(earlier)),
                        Param::Int(first_study),
                    ],
                )
                .unwrap();
        }
        let done = ensure(&mut l.registry, &scheme, &anchors, None, false).unwrap();
        assert_eq!(done.rebuilt, 1, "{}", l.name);
        assert_eq!(done.moved, 1, "{}", l.name);
        assert_eq!(done.picks_rekeyed, 1, "{}", l.name);
        assert_eq!(done.items, 1, "{}", l.name);
        let after = sessions_of(l.registry.store(), &scheme, Some(&target.code)).unwrap();
        let same = after
            .iter()
            .find(|c| c.id == target.id)
            .expect("the surrogate survives");
        assert_eq!(same.first, earlier, "{}", l.name);
        assert_eq!(same.studies.len(), target.studies.len(), "{}", l.name);
        // the pick followed the day, and the item says so
        let store = l.registry.store();
        let d = store.dialect();
        let day_col = d.text_of_qualified(Some("p"), table("pick").column("session_day").unwrap());
        let picks = store
            .query(
                &format!(
                    "SELECT {day_col}, p.withdrawn_at FROM {} p WHERE p.subject_id = {}",
                    store.qualified("pick"),
                    d.param(1, Type::Int)
                ),
                &[Param::Int(target.subject_id)],
            )
            .unwrap();
        assert_eq!(picks.len(), 1);
        assert_eq!(
            Day::parse(picks[0].text(0).unwrap()).unwrap(),
            same.first,
            "{}",
            l.name
        );
        assert!(picks[0].opt_text(1).unwrap().is_none());
        let items = store
            .query(
                &format!(
                    "SELECT kind, scope, evidence FROM {} WHERE kind = {}",
                    store.qualified("review_item"),
                    d.param(1, Type::Text)
                ),
                &[Param::from(MOVED_KIND)],
            )
            .unwrap();
        assert_eq!(items.len(), 1, "{}", l.name);
        let evidence: serde_json::Value = serde_json::from_str(items[0].text(2).unwrap()).unwrap();
        assert_eq!(evidence["picks_rekeyed"], 1);
        assert_eq!(evidence["from_first"], iso(day));

        // the whole session gone: its pick is withdrawn, never deleted
        {
            let store = l.registry.store();
            let d = store.dialect();
            for st in &same.studies {
                store
                    .execute(
                        &format!(
                            "UPDATE {} SET date_filled = NULL, study_date = NULL WHERE id = {}",
                            store.qualified("study"),
                            d.param(1, Type::Int)
                        ),
                        &[Param::Int(*st)],
                    )
                    .unwrap();
            }
        }
        let gone = ensure(&mut l.registry, &scheme, &anchors, None, false).unwrap();
        assert_eq!(gone.vanished, 1, "{}", l.name);
        assert_eq!(gone.picks_withdrawn, 1, "{}", l.name);
        let store = l.registry.store();
        let withdrawn = store
            .query(
                &format!(
                    "SELECT COUNT(*) FROM {} WHERE subject_id = {} AND withdrawn_at IS NOT NULL",
                    store.qualified("pick"),
                    store.dialect().param(1, Type::Int)
                ),
                &[Param::Int(target.subject_id)],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(withdrawn, 1, "{}", l.name);
        let left = sessions_of(store, &scheme, Some(&target.code)).unwrap();
        assert!(left.iter().all(|c| c.id != target.id), "{}", l.name);
    }
}

#[test]
fn an_event_anchored_scheme_labels_the_same_rows_for_every_reader() {
    for mut l in labs() {
        let scheme = Scheme {
            window_days: 14,
            anchor: Anchor::Event,
            event: Some("Diagnosis".to_string()),
            naming: Naming::Months {
                cadence: vec![0, 12, 24, 36, 48, 60, 72, 84, 96, 108, 120],
                tolerance: 6.0,
            },
            ..Scheme::default()
        };
        let anchors = Anchors::resolve(&mut l.registry, &scheme, BTreeMap::new()).unwrap();
        assert!(!anchors.events.is_empty(), "{}", l.name);
        ensure(&mut l.registry, &scheme, &anchors, None, false).unwrap();
        let listed = sessions_of(l.registry.store(), &scheme, None).unwrap();
        let by_study = labels_by_study(l.registry.store(), &scheme).unwrap();
        // what `nils session list` prints and what the release and the picker
        // read are one set of rows
        let mut seen: HashMap<i64, String> = HashMap::new();
        for c in &listed {
            for st in &c.studies {
                seen.insert(*st, c.name());
            }
        }
        assert_eq!(seen.len(), by_study.len(), "{}", l.name);
        for (st, l2) in &by_study {
            assert_eq!(seen[st], l2.name(), "{}", l.name);
        }
        // and the labels are the event's months, not the first session's
        assert!(
            listed.iter().any(|c| c.label.as_deref() == Some("M00")),
            "{}",
            l.name
        );
    }
}

/// Record 35 finding 7: the two shapes identity ends in that no session can
/// hold. A study row whose series were all filed under another study became
/// a session with nothing in it, carrying the same labels as a real one; a
/// subject that owns series and no study got no session at all and appeared
/// in no ask, with nothing anywhere saying so.
#[test]
fn a_study_with_nothing_in_it_makes_no_session_and_is_said() {
    for mut l in labs() {
        let name = l.name;
        let scheme = Scheme {
            window_days: 14,
            ..Scheme::default()
        };
        let anchors = Anchors::default();
        let before = ensure(&mut l.registry, &scheme, &anchors, None, false).unwrap();
        assert_eq!(before.empty_studies, 0, "{name}");
        assert_eq!(before.without_a_study, 0, "{name}");

        // a subject of the synthetic registry keeps a study row with no
        // series, as the digest leaves one when its series name another
        // study
        let subject = {
            let store = l.registry.store();
            let sql = format!(
                "SELECT id, code FROM {} ORDER BY id",
                store.qualified("subject")
            );
            let r = store.query(&sql, &[]).unwrap();
            (r[0].int(0).unwrap(), r[0].text(1).unwrap().to_string())
        };
        let study = l
            .registry
            .store()
            .insert(
                &Insert::new(
                    table("study"),
                    &[
                        "study_instance_uid",
                        "subject_id",
                        "study_date",
                        "first_batch_id",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from("1.2.826.0.1.3680043.10.999.1"),
                    Param::Int(subject.0),
                    Param::from("2024-03-04"),
                    Param::Int(1),
                ]],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();

        let done = ensure(&mut l.registry, &scheme, &anchors, None, true).unwrap();
        assert_eq!(done.empty_studies, 1, "{name}");
        assert_eq!(done.without_a_study, 0, "{name}");

        // no session was made from it
        let sessions = sessions_of(l.registry.store(), &scheme, None).unwrap();
        assert!(
            sessions.iter().all(|c| !c.studies.contains(&study)),
            "{name}: a session made from nothing"
        );
        assert!(
            sessions.iter().all(|c| !c.studies.is_empty()),
            "{name}: every session holds a study"
        );

        // and the row is said, once, with what it is
        let items = open_items(&mut l.registry, "identity.empty_study");
        assert_eq!(items.len(), 1, "{name}: {items:?}");
        assert_eq!(items[0].0, "study", "{name}");
        assert_eq!(items[0].1["study_id"], study, "{name}");
        assert_eq!(items[0].1["code"], subject.1, "{name}");
        assert_eq!(items[0].2["series"], 0, "{name}");

        // a second rebuild says it again and does not ask twice
        ensure(&mut l.registry, &scheme, &anchors, None, true).unwrap();
        assert_eq!(
            open_items(&mut l.registry, "identity.empty_study").len(),
            1,
            "{name}"
        );

        // and when the row is whole again the question closes itself
        l.registry
            .store()
            .insert(
                &Insert::new(
                    table("series"),
                    &[
                        "series_instance_uid",
                        "study_id",
                        "subject_id",
                        "modality",
                        "n_instances",
                        "n_stacks",
                        "first_batch_id",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from("1.2.826.0.1.3680043.10.999.1.1"),
                    Param::Int(study),
                    Param::Int(subject.0),
                    Param::from("MR"),
                    Param::Int(4),
                    Param::Int(1),
                    Param::Int(1),
                ]],
            )
            .unwrap();
        let after = ensure(&mut l.registry, &scheme, &anchors, None, true).unwrap();
        assert_eq!(after.empty_studies, 0, "{name}");
        assert!(
            open_items(&mut l.registry, "identity.empty_study").is_empty(),
            "{name}"
        );
        let sessions = sessions_of(l.registry.store(), &scheme, None).unwrap();
        assert!(
            sessions.iter().any(|c| c.studies.contains(&study)),
            "{name}: a study that holds something is on the timeline"
        );
    }
}

/// The other half: a subject that owns series and no study of its own.
#[test]
fn a_subject_with_series_and_no_study_is_named_rather_than_invisible() {
    for mut l in labs() {
        let name = l.name;
        let scheme = Scheme {
            window_days: 14,
            ..Scheme::default()
        };
        let anchors = Anchors::default();
        ensure(&mut l.registry, &scheme, &anchors, None, false).unwrap();

        // a series whose files named a patient their study does not
        let (study, code) = {
            let store = l.registry.store();
            let sql = format!(
                "SELECT st.id, su.code FROM {} st JOIN {} su ON su.id = st.subject_id ORDER BY st.id",
                store.qualified("study"),
                store.qualified("subject")
            );
            let r = store.query(&sql, &[]).unwrap();
            (r[0].int(0).unwrap(), r[0].text(1).unwrap().to_string())
        };
        let other = l
            .registry
            .store()
            .insert(
                &Insert::new(table("subject"), &["code", "created_at"]).returning(&["id"]),
                &[vec![
                    Param::from("sub-orphan"),
                    Param::from("2026-09-19T00:00:00Z"),
                ]],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        l.registry
            .store()
            .insert(
                &Insert::new(
                    table("series"),
                    &[
                        "series_instance_uid",
                        "study_id",
                        "subject_id",
                        "modality",
                        "n_instances",
                        "n_stacks",
                        "first_batch_id",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from("1.2.826.0.1.3680043.10.999.2.1"),
                    Param::Int(study),
                    Param::Int(other),
                    Param::from("MR"),
                    Param::Int(476),
                    Param::Int(15),
                    Param::Int(1),
                ]],
            )
            .unwrap();

        let done = ensure(&mut l.registry, &scheme, &anchors, None, true).unwrap();
        assert_eq!(done.without_a_study, 1, "{name}");
        // it still has no session, because it is on no timeline, and now
        // something says so
        let sessions = sessions_of(l.registry.store(), &scheme, None).unwrap();
        assert!(
            sessions.iter().all(|c| c.subject_id != other),
            "{name}: a subject with no study has no session"
        );
        assert_ne!(code, "sub-orphan", "{name}");
        let items = open_items(&mut l.registry, "identity.no_study");
        assert_eq!(items.len(), 1, "{name}: {items:?}");
        assert_eq!(items[0].0, "subject", "{name}");
        assert_eq!(items[0].1["subject_id"], other, "{name}");
        assert_eq!(items[0].2["series"], 1, "{name}");
        assert_eq!(items[0].2["instances"], 476, "{name}");
        // asked once, however often a rebuild runs
        ensure(&mut l.registry, &scheme, &anchors, None, true).unwrap();
        assert_eq!(
            open_items(&mut l.registry, "identity.no_study").len(),
            1,
            "{name}"
        );
    }
}

/// The open items of a kind: scope, reference and evidence.
fn open_items(
    registry: &mut Registry,
    kind: &str,
) -> Vec<(String, serde_json::Value, serde_json::Value)> {
    let store = registry.store();
    let d = store.dialect();
    let t = table("review_item");
    let sql = format!(
        "SELECT scope, {}, {} FROM {} WHERE kind = {} AND status = 'open' ORDER BY id",
        d.text_of(t.column("ref").unwrap()),
        d.text_of(t.column("evidence").unwrap()),
        store.qualified("review_item"),
        d.param(1, Type::Text)
    );
    store
        .query(&sql, &[Param::from(kind)])
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.text(0).unwrap().to_string(),
                serde_json::from_str(r.opt_text(1).unwrap().unwrap_or("{}")).unwrap(),
                serde_json::from_str(r.opt_text(2).unwrap().unwrap_or("{}")).unwrap(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------- reads at once

/// How many connections bring the cache up to date at once, and how often.
const AT_ONCE: usize = 6;
const ROUNDS: usize = 25;
const COLD_ROUNDS: usize = 8;

/// The scheme the reads below read under: a window and month labels, so
/// that identity and labels both have rows to collide on.
fn months_scheme() -> Scheme {
    Scheme {
        window_days: 14,
        naming: Naming::Months {
            cadence: vec![0, 12, 24, 36, 48, 60],
            tolerance: 3.0,
        },
        ..Scheme::default()
    }
}

/// Every subject's id, as a door that reads a dataset's visits names them.
fn subject_ids(registry: &mut Registry) -> BTreeSet<i64> {
    let store = registry.store();
    let sql = format!("SELECT id FROM {}", store.qualified("subject"));
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| r.int(0).unwrap())
        .collect()
}

/// More connections to the lab's registry, one per reader, as the server
/// keeps one per handler thread.
fn connections(l: &Lab, n: usize) -> Vec<Registry> {
    (0..n)
        .map(|_| Home::new(l._dir.path()).open().unwrap())
        .collect()
}

/// `each` on every registry at once, their starts lined up; what each run
/// answered, a failure as its text.
fn at_once(
    registries: &mut [Registry],
    each: &(dyn Fn(&mut Registry) -> Result<Rebuilt, nils_session::Error> + Sync),
) -> Vec<Result<Rebuilt, String>> {
    let start = Barrier::new(registries.len());
    std::thread::scope(|s| {
        let runs: Vec<_> = registries
            .iter_mut()
            .map(|r| {
                let start = &start;
                s.spawn(move || {
                    start.wait();
                    each(r).map_err(|e| e.to_string())
                })
            })
            .collect();
        runs.into_iter()
            .map(|h| h.join().expect("a reader panicked"))
            .collect()
    })
}

/// The failures among some runs, each said once with how often.
fn failures(runs: &[Result<Rebuilt, String>]) -> String {
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for e in runs.iter().filter_map(|r| r.as_ref().err()) {
        *by.entry(e.as_str()).or_default() += 1;
    }
    by.iter()
        .map(|(e, n)| format!("{n} times: {e}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The rows of a table by id, each with what says it was written: on
/// Postgres the transaction that wrote its version (`xmin`), which an update
/// in place changes as a new row does; on SQLite nothing, where [`changes`]
/// counts every row written instead.
fn versions(registry: &mut Registry, t: &str) -> Vec<(i64, String)> {
    let store = registry.store();
    let version = match store.backend() {
        Backend::Postgres => "xmin::text",
        Backend::Sqlite => "''",
    };
    let sql = format!(
        "SELECT id, {version} FROM {} ORDER BY id",
        store.qualified(t)
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| (r.int(0).unwrap(), r.text(1).unwrap().to_string()))
        .collect()
}

/// The tables a build of the cache writes, each with its rows' versions.
fn written(registry: &mut Registry) -> BTreeMap<&'static str, Vec<(i64, String)>> {
    [
        "session_cache",
        "session_cache_study",
        "session_label",
        "review_item",
    ]
    .into_iter()
    .map(|t| (t, versions(registry, t)))
    .collect()
}

/// Every row SQLite's connection has written since it opened; none on
/// Postgres.
fn changes(registry: &mut Registry) -> Option<u64> {
    match registry.store() {
        Store::Sqlite(c) => Some(c.total_changes()),
        Store::Postgres { .. } => None,
    }
}

/// The label rows under a scheme by session: the row's id, then what it
/// says.
fn label_rows(registry: &mut Registry, scheme: &Scheme) -> BTreeMap<i64, Row> {
    let store = registry.store();
    let sql = format!(
        "SELECT session_id, id, label, months, nominal, offset_months, flagged, reason \
         FROM {} WHERE scheme_digest = {}",
        store.qualified("session_label"),
        store.dialect().param(1, Type::Text)
    );
    store
        .query(&sql, &[Param::from(scheme.digest())])
        .unwrap()
        .into_iter()
        .map(|r| (r.int(0).unwrap(), r))
        .collect()
}

/// 2026-10-14, a registry on Postgres: the Main scans page sends its
/// requests at once, and each brings the session cache up to date for the
/// dataset's subjects before it reads their visits. Each deleted every label
/// row of those subjects and wrote it again, so under read committed the
/// second of two, whose delete could not see the rows the first wrote
/// meanwhile, failed on the labels' unique key, and the page with it.
/// Several readers at once, many times over: none fails, and none writes a
/// row, since nothing changed.
#[test]
fn readers_of_one_cache_at_once_never_collide() {
    let Some(mut l) = postgres_lab() else {
        return;
    };
    let scheme = months_scheme();
    let anchors = Anchors::resolve(&mut l.registry, &scheme, BTreeMap::new()).unwrap();
    let subjects = subject_ids(&mut l.registry);
    let first = ensure_for(&mut l.registry, &scheme, &anchors, &subjects).unwrap();
    assert_eq!(first.rebuilt, subjects.len());
    let labels = label_rows(&mut l.registry, &scheme);
    assert_eq!(labels.len(), first.sessions);
    let before = written(&mut l.registry);

    let mut readers = connections(&l, AT_ONCE);
    let read = |r: &mut Registry| ensure_for(r, &scheme, &anchors, &subjects);
    let mut runs = Vec::new();
    for _ in 0..ROUNDS {
        runs.extend(at_once(&mut readers, &read));
    }
    let failed = runs.iter().filter(|r| r.is_err()).count();
    assert_eq!(
        failed,
        0,
        "{failed} of {} reads failed: {}",
        runs.len(),
        failures(&runs)
    );
    for done in runs.iter().flatten() {
        assert_eq!(
            (done.unchanged, done.rebuilt, done.relabelled),
            (subjects.len(), 0, 0)
        );
    }
    // the cache is the one the first build made, row for row
    assert_eq!(label_rows(&mut l.registry, &scheme), labels);
    assert_eq!(written(&mut l.registry), before);
}

/// The same where the readers find the cache cold at once: the identity rows
/// had the same exposure, each reader making the sessions another had just
/// made. The first builds them, and the rest find them built.
#[test]
fn readers_that_find_the_cache_cold_at_once_build_it_once() {
    let Some(mut l) = postgres_lab() else {
        return;
    };
    let scheme = months_scheme();
    let anchors = Anchors::resolve(&mut l.registry, &scheme, BTreeMap::new()).unwrap();
    let subjects = subject_ids(&mut l.registry);
    let expected = direct(&mut l.registry, &scheme);
    let mut readers = connections(&l, AT_ONCE);
    let read = |r: &mut Registry| ensure_for(r, &scheme, &anchors, &subjects);
    for round in 0..COLD_ROUNDS {
        // nothing built under the window
        {
            let store = l.registry.store();
            for t in ["session_label", "session_cache_study", "session_cache"] {
                store
                    .execute(&format!("DELETE FROM {}", store.qualified(t)), &[])
                    .unwrap();
            }
        }
        let runs = at_once(&mut readers, &read);
        let failed = runs.iter().filter(|r| r.is_err()).count();
        assert_eq!(
            failed,
            0,
            "round {round}: {failed} of {} reads failed: {}",
            runs.len(),
            failures(&runs)
        );
        // each subject built once, by whichever reader came first, and no
        // span said to have moved
        let done: Vec<&Rebuilt> = runs.iter().flatten().collect();
        assert_eq!(
            done.iter().map(|d| d.rebuilt).sum::<usize>(),
            subjects.len(),
            "round {round}"
        );
        assert!(
            done.iter().all(|d| d.moved + d.vanished + d.items == 0),
            "round {round}"
        );
        // and the cache says what the resolver says
        let cached = sessions_of(l.registry.store(), &scheme, None).unwrap();
        let mut got: BTreeMap<String, Vec<(Day, Option<String>)>> = BTreeMap::new();
        for c in &cached {
            got.entry(c.code.clone())
                .or_default()
                .push((c.first, c.label.clone()));
        }
        assert_eq!(got, expected, "round {round}");
    }
}

/// And where they find a span moved at once: it moves once and is said
/// once, where the readers after the first failed on the labels' key.
#[test]
fn readers_that_find_a_span_moved_at_once_say_it_once() {
    let Some(mut l) = postgres_lab() else {
        return;
    };
    let scheme = months_scheme();
    let anchors = Anchors::resolve(&mut l.registry, &scheme, BTreeMap::new()).unwrap();
    let subjects = subject_ids(&mut l.registry);
    ensure_for(&mut l.registry, &scheme, &anchors, &subjects).unwrap();
    // a subject's first visit, of two studies or more, that a day more
    // still holds whole: its first study a day earlier moves the visit and
    // nothing else
    let cached = sessions_of(l.registry.store(), &scheme, None).unwrap();
    let target = cached
        .iter()
        .filter(|c| c.studies.len() >= 2 && c.last.to_days() - c.first.to_days() <= 13)
        .find(|c| {
            !cached
                .iter()
                .any(|o| o.subject_id == c.subject_id && o.first < c.first)
        })
        .expect("a first visit of two studies")
        .clone();
    let first_study = target.studies[0];
    let mut readers = connections(&l, AT_ONCE);
    let read = |r: &mut Registry| ensure_for(r, &scheme, &anchors, &subjects);
    for round in 0..COLD_ROUNDS {
        // a day earlier, then back, then earlier again
        let day = if round % 2 == 0 {
            Day::from_days(target.first.to_days() - 1)
        } else {
            target.first
        };
        {
            let store = l.registry.store();
            let d = store.dialect();
            let iso = format!("{:04}-{:02}-{:02}", day.year(), day.month(), day.day());
            store
                .execute(
                    &format!(
                        "UPDATE {} SET date_filled = {}, study_date = {} WHERE id = {}",
                        store.qualified("study"),
                        d.param(1, Type::Date),
                        d.param(2, Type::Date),
                        d.param(3, Type::Int)
                    ),
                    &[
                        Param::from(iso.as_str()),
                        Param::from(iso.as_str()),
                        Param::Int(first_study),
                    ],
                )
                .unwrap();
        }
        let runs = at_once(&mut readers, &read);
        let failed = runs.iter().filter(|r| r.is_err()).count();
        assert_eq!(
            failed,
            0,
            "round {round}: {failed} of {} reads failed: {}",
            runs.len(),
            failures(&runs)
        );
        let done: Vec<Rebuilt> = runs.into_iter().flatten().collect();
        let sum = |f: fn(&Rebuilt) -> usize| done.iter().map(f).sum::<usize>();
        assert_eq!(
            (sum(|d| d.rebuilt), sum(|d| d.moved), sum(|d| d.items)),
            (1, 1, 1),
            "round {round}"
        );
        let after = sessions_of(l.registry.store(), &scheme, Some(&target.code)).unwrap();
        let same = after
            .iter()
            .find(|c| c.id == target.id)
            .expect("the surrogate survives");
        assert_eq!(
            (same.first, &same.studies),
            (day, &target.studies),
            "round {round}"
        );
    }
    // one item for each move
    let store = l.registry.store();
    let sql = format!(
        "SELECT COUNT(*) FROM {} WHERE kind = {}",
        store.qualified("review_item"),
        store.dialect().param(1, Type::Text)
    );
    let items = store.query(&sql, &[Param::from(MOVED_KIND)]).unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(items, COLD_ROUNDS as i64);
}

/// A read of timelines that did not change writes nothing: not a label, not
/// an identity row, not the review item an earlier build raised.
#[test]
fn an_unchanged_cache_brought_up_to_date_writes_nothing() {
    for mut l in labs() {
        let name = l.name;
        let scheme = months_scheme();
        let anchors = Anchors::resolve(&mut l.registry, &scheme, BTreeMap::new()).unwrap();
        // a study row with no series, so that an open review item is there
        // to be left alone too
        let subject = *subject_ids(&mut l.registry).first().unwrap();
        l.registry
            .store()
            .insert(
                &Insert::new(
                    table("study"),
                    &[
                        "study_instance_uid",
                        "subject_id",
                        "study_date",
                        "first_batch_id",
                    ],
                ),
                &[vec![
                    Param::from("1.2.826.0.1.3680043.10.999.3"),
                    Param::Int(subject),
                    Param::from("2024-03-04"),
                    Param::Int(1),
                ]],
            )
            .unwrap();
        let subjects = subject_ids(&mut l.registry);
        let first = ensure_for(&mut l.registry, &scheme, &anchors, &subjects).unwrap();
        assert_eq!(first.relabelled, first.sessions, "{name}");
        assert_eq!(first.empty_studies, 1, "{name}");
        let before = written(&mut l.registry);
        let counted = changes(&mut l.registry);

        let again = ensure_for(&mut l.registry, &scheme, &anchors, &subjects).unwrap();
        assert_eq!(
            (again.unchanged, again.rebuilt, again.relabelled),
            (subjects.len(), 0, 0),
            "{name}"
        );
        assert_eq!(again.empty_studies, 1, "{name}");
        assert_eq!(written(&mut l.registry), before, "{name}");
        assert_eq!(changes(&mut l.registry), counted, "{name}: rows written");

        // and the same over every subject, as a release or a list reads
        let all = ensure(&mut l.registry, &scheme, &anchors, None, false).unwrap();
        assert_eq!((all.rebuilt, all.relabelled), (0, 0), "{name}");
        assert_eq!(written(&mut l.registry), before, "{name}");
        assert_eq!(changes(&mut l.registry), counted, "{name}: rows written");
    }
}

/// Labels that changed while the timeline did not, as when a month zero is
/// given anew, are updated where they stand: the same rows with new words,
/// and only the rows whose words changed.
#[test]
fn a_changed_label_is_updated_in_place_and_nothing_else_is_written() {
    for mut l in labs() {
        let name = l.name;
        let scheme = Scheme {
            anchor: Anchor::Explicit,
            ..months_scheme()
        };
        // month zero is each subject's first visit
        let timelines = timelines(&mut l.registry);
        let explicit: BTreeMap<String, Day> = timelines
            .iter()
            .map(|(code, studies)| (code.clone(), studies.iter().map(|s| s.day).min().unwrap()))
            .collect();
        let anchors = Anchors {
            explicit: explicit.clone(),
            events: BTreeMap::new(),
        };
        let subjects = subject_ids(&mut l.registry);
        ensure_for(&mut l.registry, &scheme, &anchors, &subjects).unwrap();
        let before = label_rows(&mut l.registry, &scheme);
        let versions_before = versions(&mut l.registry, "session_label");
        let counted = changes(&mut l.registry);

        // one subject's month zero a year earlier: each of its visits is a
        // year further from it
        let code = timelines
            .iter()
            .find(|(_, studies)| studies.len() >= 3)
            .map(|(code, _)| code.clone())
            .expect("a subject with several studies");
        let zero = Day::from_days(explicit[&code].to_days() - 365);
        let mut moved = explicit.clone();
        moved.insert(code.clone(), zero);
        let anchors = Anchors {
            explicit: moved,
            events: BTreeMap::new(),
        };
        let done = ensure_for(&mut l.registry, &scheme, &anchors, &subjects).unwrap();
        let theirs = sessions_of(l.registry.store(), &scheme, Some(&code)).unwrap();
        assert_eq!(done.rebuilt, 0, "{name}");
        assert_eq!(done.relabelled, theirs.len(), "{name}");

        // what the resolver says under the new month zero
        let want: Vec<(Day, Option<String>, Option<i32>)> =
            session::sessions(&timelines[&code], Some(zero), &scheme)
                .into_iter()
                .map(|s| (s.first, s.label, s.months))
                .collect();
        let got: Vec<(Day, Option<String>, Option<i32>)> = theirs
            .iter()
            .map(|c| (c.first, c.label.clone(), c.months))
            .collect();
        assert_eq!(got, want, "{name}");

        // the same rows, the subject's alone with new words
        let after = label_rows(&mut l.registry, &scheme);
        assert_eq!(
            after.keys().collect::<Vec<_>>(),
            before.keys().collect::<Vec<_>>(),
            "{name}"
        );
        let ids: BTreeSet<i64> = theirs.iter().map(|c| c.id).collect();
        let mut rows_of_theirs = BTreeSet::new();
        for (session, row) in &after {
            let was = &before[session];
            assert_eq!(row.int(1).unwrap(), was.int(1).unwrap(), "{name}: one row");
            if ids.contains(session) {
                assert_ne!(row, was, "{name}: the subject's label changed");
                rows_of_theirs.insert(row.int(1).unwrap());
            } else {
                assert_eq!(row, was, "{name}");
            }
        }
        // and only those rows were written
        let versions_after = versions(&mut l.registry, "session_label");
        let rewritten: BTreeSet<i64> = versions_after
            .iter()
            .zip(&versions_before)
            .filter(|(a, b)| a != b)
            .map(|(a, _)| a.0)
            .collect();
        match changes(&mut l.registry) {
            Some(now) => assert_eq!(
                now - counted.unwrap(),
                ids.len() as u64,
                "{name}: rows written"
            ),
            None => assert_eq!(rewritten, rows_of_theirs, "{name}"),
        }
    }
}

/// Label rows whose session is gone.
fn orphan_labels(registry: &mut Registry) -> i64 {
    let store = registry.store();
    let sql = format!(
        "SELECT COUNT(*) FROM {} l WHERE NOT EXISTS (SELECT 1 FROM {} c WHERE c.id = l.session_id)",
        store.qualified("session_label"),
        store.qualified("session_cache")
    );
    store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
}

/// A merge of two subjects drops their sessions for the next build to make
/// again, and a build that went on beside it could write a label of a
/// session the merge had just dropped (2026-10-14). The merge takes the
/// cache's lock: it waits for a build that holds it, and what the build
/// wrote goes with the sessions. On Postgres, where the two run side by
/// side.
#[test]
fn a_merge_waits_for_a_build_and_leaves_no_label_of_a_dropped_session() {
    let Some(mut l) = postgres_lab() else {
        return;
    };
    let scheme = months_scheme();
    let anchors = Anchors::resolve(&mut l.registry, &scheme, BTreeMap::new()).unwrap();
    let subjects = subject_ids(&mut l.registry);
    ensure_for(&mut l.registry, &scheme, &anchors, &subjects).unwrap();
    let cached = sessions_of(l.registry.store(), &scheme, None).unwrap();
    let mut ids = subjects.iter().copied();
    let (canonical, alias) = (ids.next().unwrap(), ids.next().unwrap());
    let studies = |cached: &[nils_session::Cached], s: i64| -> usize {
        cached
            .iter()
            .filter(|c| c.subject_id == s)
            .map(|c| c.studies.len())
            .sum()
    };
    let both = studies(&cached, canonical) + studies(&cached, alias);
    let session = cached
        .iter()
        .find(|c| c.subject_id == canonical)
        .expect("a session of the canonical subject")
        .id;
    assert_eq!(orphan_labels(&mut l.registry), 0);

    // a build under way, its transaction holding the lock
    let mut build = connections(&l, 1).pop().unwrap();
    build.store().begin().unwrap();
    lock::take_for_transaction(build.store(), lock::SESSION_CACHE).unwrap();

    let mut merger = connections(&l, 1).pop().unwrap();
    let mut linkage = merger.open_linkage().unwrap();
    let keys = Subkeys::derive(&merger.pseudonym_key().unwrap());
    let started = Instant::now();
    let (waited, merged) = std::thread::scope(|s| {
        let merging = s.spawn(|| {
            let merged = merge::merge(
                merger.store(),
                &mut linkage,
                &keys,
                &merge::Ask {
                    canonical,
                    alias,
                    why: "one person",
                    actor: "test",
                    job_id: None,
                    place_id: None,
                },
            );
            (started.elapsed(), merged)
        });
        std::thread::sleep(Duration::from_millis(300));
        // the build labels the canonical subject's session under another
        // scheme, and ends
        let store = build.store();
        store
            .insert(
                &Insert::new(
                    table("session_label"),
                    &["session_id", "scheme_digest", "flagged"],
                ),
                &[vec![
                    Param::Int(session),
                    Param::from("another scheme"),
                    Param::Int(0),
                ]],
            )
            .unwrap();
        store.commit().unwrap();
        merging.join().unwrap()
    });
    merged.unwrap();
    assert_eq!(
        orphan_labels(&mut l.registry),
        0,
        "a label of a dropped session"
    );
    assert!(
        waited >= Duration::from_millis(300),
        "the merge went on beside the build, done after {waited:?}"
    );

    // the next build makes the merged subject's sessions again, every study
    // of both on them
    ensure_for(&mut l.registry, &scheme, &anchors, &subjects).unwrap();
    let cached = sessions_of(l.registry.store(), &scheme, None).unwrap();
    assert_eq!(studies(&cached, canonical), both);
    assert_eq!(studies(&cached, alias), 0);
    assert_eq!(orphan_labels(&mut l.registry), 0);
}
