// SPDX-License-Identifier: AGPL-3.0-only

//! The session cache (`docs/specs/wave4b-the-ask.md`, §7 and §15, slice 2),
//! on the synthetic registry, on both backends.

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::day::Day;
use nils_registry::home::{Home, InitOptions};
use nils_registry::schema::{Type, table};
use nils_registry::session::{self, Anchor, Naming, Scheme};
use nils_registry::{Backend, Insert, Param, Registry};
use nils_session::{Anchors, MOVED_KIND, ensure, labels_by_study, sessions_of};

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
            scheme: nils_registry::Scheme::DEFAULT,
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

/// What the resolver says on its own, per subject, for the check that the
/// cache says the same.
fn direct(
    registry: &mut Registry,
    scheme: &Scheme,
) -> BTreeMap<String, Vec<(Day, Option<String>)>> {
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
    by.into_iter()
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
